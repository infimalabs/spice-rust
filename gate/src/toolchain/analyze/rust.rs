//! The native Rust check and its stable selected-source compilation cache.
//!
//! The canvas arm in [`super`] already compiles Rust, so the invocation and the
//! diagnostic parse are shared with it rather than restated here. What is not
//! shared is the reason this arm exists, the target set that serves it, and the
//! `target` pair it owns; those are the whole of this file.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(target_family = "unix")]
use std::os::unix::fs::PermissionsExt as _;

use super::{
    Analysis, CARGO, CARGO_BUILD_DIRECTORY, CARGO_TARGET_DIRECTORY, COMPILED_HOME_DIRECTORY,
    CompileInvocation, CompilerResources, HOME_DIRECTORY, RUSTC_WORKSPACE_WRAPPER, RUSTC_WRAPPER,
    run_compile_analysis_with_wait, tool_search_path,
};
use crate::measure::Bounds;
use crate::toolchain::{
    ArmOutcome, ArmTiming, FindingDetail, RustAnalysisWorkspace, ToolchainDeclaration,
};
use sill::{Acquisition, Environment, Possession, WaitPolicy};

/// Compile changed owners and the exact native surface reached in dependents.
///
/// The canvas arm beside this one already compiles Rust, but only for
/// `wasm32-unknown-unknown` and only the default targets, so nothing in the
/// battery ever built a `#[cfg(test)]` body. A rename that lands while a peer's
/// branch was cut before it leaves stale *references* behind, never
/// declarations, and references live in bodies: the vocabulary gate cannot see
/// them and the merge is textually clean. The compiler names each one with its
/// file and line, which is what this arm publishes.
pub(in crate::toolchain) fn rust_typecheck(
    declaration: ToolchainDeclaration,
    root: &Path,
    repository: &Path,
    changes: &[String],
    bounds: &Bounds,
    resources: CompilerResources<'_>,
) -> Analysis {
    rust_typecheck_with_workspace(
        declaration,
        root,
        changes,
        bounds,
        resources,
        || RustWorkspace::create(root, repository),
        None,
    )
}

fn rust_typecheck_with_workspace(
    declaration: ToolchainDeclaration,
    root: &Path,
    changes: &[String],
    bounds: &Bounds,
    resources: CompilerResources<'_>,
    create_workspace: impl FnOnce() -> Result<RustWorkspace, String>,
    wait: Option<WaitPolicy>,
) -> Analysis {
    if !root.join("Cargo.toml").is_file() {
        return Analysis::not_applicable(declaration);
    }
    let Some(cargo) = resources.program(CARGO) else {
        return Analysis::terminal(
            ArmOutcome::ToolUnavailable,
            format!("{} declares no {CARGO} program", declaration.id.as_str()),
        );
    };
    let workspace = match create_workspace() {
        Ok(workspace) => workspace,
        Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
    };
    let analysis = match rust_environment(cargo, &workspace, resources.environment()) {
        Err(reason) => Analysis::terminal(ArmOutcome::ParseFailure, reason),
        Ok(environment) => {
            let reach = crate::workspace_tests::workspace_package_reach_at(
                &workspace.source,
                changes,
                &environment,
                OsStr::new(cargo),
            );
            match reach.map(|reach| reach.cargo_check_arguments()) {
                Ok(invocations) if !invocations.is_empty() => {
                    let mut elapsed = Duration::ZERO;
                    let mut observed = false;
                    let mut refusal = None;
                    for owned in invocations {
                        let arguments = owned.iter().map(String::as_str).collect::<Vec<_>>();
                        let invocation = CompileInvocation {
                            name: "cargo native check",
                            program: cargo,
                            arguments: &arguments,
                            cwd: &workspace.source,
                        };
                        match run_compile_analysis_with_wait(
                            declaration.id,
                            &workspace.source,
                            invocation,
                            bounds,
                            &environment,
                            |code, message| FindingDetail::RustTypeOutput { code, message },
                            wait.clone(),
                        ) {
                            Ok(duration) => {
                                observed = true;
                                elapsed = elapsed.saturating_add(duration);
                            }
                            Err(failure) => {
                                let (analysis, duration) = failure.into_parts();
                                observed |= duration.is_some();
                                elapsed = duration
                                    .map_or(elapsed, |duration| elapsed.saturating_add(duration));
                                refusal = Some(analysis);
                                break;
                            }
                        }
                    }
                    let analysis = refusal.unwrap_or_else(|| {
                        Analysis::terminal(
                            ArmOutcome::Passed,
                            format!(
                                "{} completed its declared native analysis",
                                declaration.id.as_str()
                            ),
                        )
                    });
                    if observed {
                        analysis.timed(timing(elapsed, bounds))
                    } else {
                        analysis
                    }
                }
                Ok(_) => Analysis::not_applicable(declaration),
                Err(error) => Analysis::terminal(ArmOutcome::ParseFailure, error.to_string()),
            }
        }
    };
    match workspace.release() {
        None => analysis,
        Some(reason) => Analysis::terminal(ArmOutcome::ParseFailure, reason),
    }
}

fn timing(elapsed: Duration, bounds: &Bounds) -> ArmTiming {
    ArmTiming {
        elapsed_ms: milliseconds(elapsed),
        observation_bound_ms: observation_bound_ms(bounds),
    }
}

fn observation_bound_ms(bounds: &Bounds) -> u64 {
    let deadlines = bounds.deadlines();
    milliseconds(
        deadlines
            .runtime()
            .saturating_add(deadlines.output_drain())
            .saturating_add(deadlines.reap())
            .saturating_add(bounds.grace()),
    )
}

fn milliseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Persistent Cargo output for native checks of one live repository worktree.
///
/// The selected tree remains the private immutable input and receives no
/// generated bytes. Cargo's output is shared at a fixed gate-owned path below
/// the live repository's excluded `target/` directory. Keeping native analysis
/// outside Cargo's default target lets ordinary selected tests execute in
/// parallel without competing for one target lock, while the stable source and
/// output paths retain Cargo fingerprints between gate runs. Linked worktrees
/// resolve different repository paths and different targets.
///
/// Build-script output remains in a stable gate-owned subdirectory. That keeps
/// a machine-level `build.build-dir` containing a selected workspace-path hash
/// from minting another cold directory for every nonce-named selected tree.
/// Canvas uses its own target and build pair because its wasm/release artifacts
/// do not overlap the native launcher and all-target check.
struct RustWorkspace {
    source: PathBuf,
    cargo_target: PathBuf,
    cargo_build: PathBuf,
    possession: Option<Possession>,
}

const RUST_SOURCE_DIRECTORY: &str = "gate-native-source";
const RUST_SOURCE_LABEL: &str = "rust analysis source cache";
const RUST_SOURCE_LOCK: &str = "rust-analysis-source-cache";
const RUST_TARGET_DIRECTORY: &str = "spice-gate-native";
pub(in crate::toolchain) const RUST_BUILD_DIRECTORY: &str = "spice-gate-native-stable-build-v1";

impl RustWorkspace {
    fn create(selected: &Path, repository: &Path) -> Result<Self, String> {
        let workspace = rust_analysis_workspace(repository)?;
        Self::create_at(selected, workspace.source(), workspace.cargo_target())
    }

    fn create_at(selected: &Path, source: &Path, target: &Path) -> Result<Self, String> {
        let Some(parent) = source.parent() else {
            return Err(format!(
                "{RUST_SOURCE_LABEL} {} has no parent directory",
                source.display()
            ));
        };
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "{RUST_SOURCE_LABEL} parent {} could not be created: {error}",
                parent.display()
            )
        })?;
        let lock = crate::held_root::held_root_lock(source, RUST_SOURCE_LOCK, RUST_SOURCE_LABEL)?;
        let possession = match lock.try_acquire() {
            Ok(Acquisition::Acquired(possession)) => possession,
            Ok(Acquisition::Contended) => {
                return Err(format!(
                    "{RUST_SOURCE_LABEL} {} is in use by another canonical gate",
                    source.display()
                ));
            }
            Err(error) => {
                return Err(format!(
                    "{RUST_SOURCE_LABEL} {} could not be locked: {error}",
                    source.display()
                ));
            }
        };
        synchronize_source(selected, source).map_err(|error| {
            format!(
                "{RUST_SOURCE_LABEL} {} could not mirror {}: {error}",
                source.display(),
                selected.display()
            )
        })?;
        let cargo_target = target.to_path_buf();
        let cargo_build = target.join(RUST_BUILD_DIRECTORY);
        fs::create_dir_all(&cargo_target)
            .and_then(|()| fs::create_dir_all(&cargo_build))
            .map_err(|error| {
                format!(
                    "rust analysis output directories for {} could not be created below {}: \
                     {error}",
                    selected.display(),
                    target.display()
                )
            })?;
        Ok(Self {
            source: source.to_path_buf(),
            cargo_target,
            cargo_build,
            possession: Some(possession),
        })
    }

    /// Leave the persistent bytes removable after the compiler releases them.
    ///
    /// Permission changes do not move file mtimes, so Cargo can still reuse the
    /// verified content on the next run. Keeping the possession until after the
    /// walk prevents a second same-worktree gate from observing a half-unsealed
    /// source cache.
    fn release(mut self) -> Option<String> {
        let result = make_cache_writable(&self.source).err().map(|error| {
            format!(
                "{RUST_SOURCE_LABEL} {} could not be made removable after analysis: {error}",
                self.source.display()
            )
        });
        drop(self.possession.take());
        result
    }
}

pub(in crate::toolchain) fn rust_analysis_workspace(
    repository: &Path,
) -> Result<RustAnalysisWorkspace, String> {
    let (worktree_git, _) = crate::tree::git_directories(repository).map_err(|error| {
        format!(
            "rust analysis cache could not resolve the Git directory for {}: {error}",
            repository.display()
        )
    })?;
    let source = worktree_git.join("spice").join(RUST_SOURCE_DIRECTORY);
    let cargo_target = repository.join("target").join(RUST_TARGET_DIRECTORY);
    let cargo_build = cargo_target.join(RUST_BUILD_DIRECTORY);
    Ok(RustAnalysisWorkspace {
        source,
        cargo_target,
        cargo_build,
    })
}

impl Drop for RustWorkspace {
    fn drop(&mut self) {
        if self.possession.is_some() {
            let _ = make_cache_writable(&self.source);
            drop(self.possession.take());
        }
    }
}

/// Make one stable compiler source exactly equal to the immutable selection.
///
/// Unchanged files are deliberately left in place, preserving their mtimes and
/// Cargo fingerprints. Changed or absent bytes are repaired while this caller
/// holds the cache lock. A complete second comparison precedes sealing, so a
/// killed partial synchronization is reusable only after the next run repairs
/// and verifies it.
pub(in crate::toolchain) fn synchronize_source(selected: &Path, cached: &Path) -> io::Result<()> {
    match fs::symlink_metadata(cached) {
        Ok(metadata) if metadata.is_dir() => make_cache_writable(cached)?,
        Ok(_) => return Err(unsafe_cache_entry(cached)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(cached)?,
        Err(error) => return Err(error),
    }
    sync_directory(selected, cached)?;
    verify_directory_copy(selected, cached)?;
    seal_directory_copy(selected, cached)
}

pub(in crate::toolchain) fn make_cache_writable(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        set_owner_writable(path, &metadata)?;
        for entry in sorted_entries(path)? {
            make_cache_writable(&entry.path())?;
        }
        return Ok(());
    }
    if metadata.is_file() {
        return set_owner_writable(path, &metadata);
    }
    Err(unsafe_cache_entry(path))
}

fn set_owner_writable(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    #[cfg(target_family = "unix")]
    {
        let mode = if metadata.is_dir() { 0o700 } else { 0o600 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(target_family = "unix"))]
    {
        let mut permissions = metadata.permissions();
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)
    }
}

fn sync_directory(selected: &Path, cached: &Path) -> io::Result<()> {
    let selected_entries = sorted_entries(selected)?;
    let names: BTreeSet<_> = selected_entries
        .iter()
        .map(fs::DirEntry::file_name)
        .collect();
    for stale in sorted_entries(cached)? {
        if !names.contains(&stale.file_name()) {
            remove_cache_entry(&stale.path())?;
        }
    }
    for entry in selected_entries {
        let source = entry.path();
        let destination = cached.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source)?;
        if metadata.is_dir() {
            ensure_cache_directory(&destination)?;
            sync_directory(&source, &destination)?;
        } else if metadata.is_file() {
            sync_file(&source, &destination)?;
        } else {
            return Err(unsafe_cache_entry(&source));
        }
    }
    Ok(())
}

fn ensure_cache_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => {
            remove_cache_entry(path)?;
            fs::create_dir(path)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(path),
        Err(error) => Err(error),
    }
}

fn sync_file(source: &Path, destination: &Path) -> io::Result<()> {
    let unchanged = match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.is_file() => same_file(source, destination)?,
        Ok(_) => {
            remove_cache_entry(destination)?;
            false
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    if !unchanged {
        fs::copy(source, destination)?;
    }
    Ok(())
}

fn same_file(left: &Path, right: &Path) -> io::Result<bool> {
    let left_metadata = fs::metadata(left)?;
    let right_metadata = fs::metadata(right)?;
    if left_metadata.len() != right_metadata.len() {
        return Ok(false);
    }
    Ok(fs::read(left)? == fs::read(right)?)
}

fn remove_cache_entry(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else if metadata.is_file() {
        fs::remove_file(path)
    } else {
        Err(unsafe_cache_entry(path))
    }
}

fn verify_directory_copy(selected: &Path, cached: &Path) -> io::Result<()> {
    let selected_entries = sorted_entries(selected)?;
    let cached_entries = sorted_entries(cached)?;
    let selected_names: Vec<_> = selected_entries
        .iter()
        .map(fs::DirEntry::file_name)
        .collect();
    let cached_names: Vec<_> = cached_entries.iter().map(fs::DirEntry::file_name).collect();
    if selected_names != cached_names {
        return Err(io::Error::other("cached directory entries differ"));
    }
    for entry in selected_entries {
        let source = entry.path();
        let destination = cached.join(entry.file_name());
        let source_metadata = fs::symlink_metadata(&source)?;
        let cached_metadata = fs::symlink_metadata(&destination)?;
        if source_metadata.is_dir() && cached_metadata.is_dir() {
            verify_directory_copy(&source, &destination)?;
            continue;
        }
        if !source_metadata.is_file()
            || !cached_metadata.is_file()
            || !same_file(&source, &destination)?
        {
            return Err(io::Error::other(format!(
                "cached entry {} differs from its selected source",
                destination.display()
            )));
        }
    }
    Ok(())
}

fn seal_directory_copy(selected: &Path, cached: &Path) -> io::Result<()> {
    for entry in sorted_entries(selected)? {
        let source = entry.path();
        let destination = cached.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source)?;
        if metadata.is_dir() {
            seal_directory_copy(&source, &destination)?;
        }
        fs::set_permissions(&destination, metadata.permissions())?;
    }
    fs::set_permissions(cached, fs::symlink_metadata(selected)?.permissions())
}

fn sorted_entries(path: &Path) -> io::Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

fn unsafe_cache_entry(path: &Path) -> io::Error {
    io::Error::other(format!(
        "{} is not a regular file or directory",
        path.display()
    ))
}

fn rust_environment(
    cargo: &str,
    workspace: &RustWorkspace,
    inherited: &[(std::ffi::OsString, std::ffi::OsString)],
) -> Result<Environment, String> {
    let path = tool_search_path("rust-typecheck", &[cargo, "/usr/bin/cc", "/bin/sh"])?;
    let mut entries = inherited
        .iter()
        .cloned()
        .collect::<BTreeMap<std::ffi::OsString, std::ffi::OsString>>();
    entries.insert("PATH".into(), path);
    entries
        .entry(HOME_DIRECTORY.into())
        .or_insert_with(|| COMPILED_HOME_DIRECTORY.into());
    entries.insert(
        CARGO_TARGET_DIRECTORY.into(),
        workspace.cargo_target.as_os_str().to_owned(),
    );
    entries.insert(
        CARGO_BUILD_DIRECTORY.into(),
        workspace.cargo_build.as_os_str().to_owned(),
    );
    entries.insert(RUSTC_WRAPPER.into(), std::ffi::OsString::new());
    entries.insert(RUSTC_WORKSPACE_WRAPPER.into(), std::ffi::OsString::new());
    Environment::new(entries)
        .map_err(|error| format!("native typecheck tool environment was refused: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    use crate::toolchain::{ArmFinding, GateId, declaration, exclusive_scratch_root};
    use sill::{Cancellation, CancellationToken, Deadlines, RestartDisposition, WaitPolicy};

    #[test]
    fn focused_native_check_retains_private_cargo_home_and_job_budget() {
        let workspace = RustWorkspace {
            source: PathBuf::from("/selected"),
            cargo_target: PathBuf::from("/outputs/target"),
            cargo_build: PathBuf::from("/outputs/build"),
            possession: None,
        };
        let inherited = vec![
            (
                std::ffi::OsString::from("CARGO_HOME"),
                std::ffi::OsString::from("/private/cargo-home"),
            ),
            (
                std::ffi::OsString::from("CARGO_BUILD_JOBS"),
                std::ffi::OsString::from("2"),
            ),
        ];

        let environment = rust_environment("/toolchain/bin/cargo", &workspace, &inherited)
            .expect("focused native environment");
        let entries = environment
            .entries()
            .iter()
            .cloned()
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            entries.get(std::ffi::OsStr::new("CARGO_HOME")),
            Some(&std::ffi::OsString::from("/private/cargo-home"))
        );
        assert_eq!(
            entries.get(std::ffi::OsStr::new("CARGO_BUILD_JOBS")),
            Some(&std::ffi::OsString::from("2"))
        );
        assert_eq!(
            entries.get(std::ffi::OsStr::new(CARGO_TARGET_DIRECTORY)),
            Some(&std::ffi::OsString::from("/outputs/target"))
        );
    }

    /// The manifest of a workspace whose only crate is the planted one.
    const FIXTURE_MANIFEST: &str = "\
[workspace]

[package]
name = \"planted\"
version = \"0.0.0\"
edition = \"2024\"

[lib]
path = \"src/lib.rs\"
";

    /// A lock for a crate with no dependencies, written rather than generated.
    ///
    /// `--locked` refuses to create one, so the fixture must ship it. Nothing
    /// is resolved here: a package with no dependency section locks to itself,
    /// which is why this can be a literal and needs no registry.
    const FIXTURE_LOCK: &str = "\
version = 4

[[package]]
name = \"planted\"
version = \"0.0.0\"
";

    /// The planted source: a call, in a test body, to a spelling nothing defines.
    ///
    /// Both halves of the placement are load-bearing. It is a *reference*, not a
    /// declaration, because `gate/tests/retired_names.rs` judges declarations
    /// only: its `scan_item` reads `Item::Fn`'s `sig.ident` and `attrs` and
    /// never descends into `node.block`. Widening that walk is refused from
    /// inside by a law the same suite pins at line 219,
    /// `a_local_binding_is_not_a_coined_name`, which asserts that a local binding
    /// coins nothing even when its spelling is retired; judging every path
    /// expression turns that law red, and judging only qualified paths still
    /// misses a bare call exactly like this one.
    ///
    /// And it sits in a `#[cfg(test)]` body because that is what `--all-targets`
    /// buys. A plain `cargo check` builds no test target, so the canvas arm
    /// beside this one would compile this same crate and report nothing.
    const PLANTED_REFERENCE: &str = "\
pub fn caller() -> u32 {
    7
}

#[cfg(test)]
mod tests {
    #[test]
    fn planted() {
        let _ = dev_app_at();
    }
}
";

    /// The same source with the plant removed and nothing else changed.
    const REMOVED_PLANT: &str = "\
pub fn caller() -> u32 {
    7
}

#[cfg(test)]
mod tests {
    #[test]
    fn planted() {
        assert_eq!(super::caller(), 7);
    }
}
";

    /// The one-based line the planted call occupies in [`PLANTED_REFERENCE`].
    const PLANTED_LINE: u32 = 9;

    /// The one-based column the planted call's first character occupies.
    const PLANTED_COLUMN: u32 = 17;

    /// Native output stays outside the selected tree and survives one analysis.
    ///
    /// The first workspace plants the witness and is dropped before the second
    /// is created. Finding it again proves this is a cache, not merely a new
    /// scratch root that happens to carry a stable-looking name.
    #[test]
    fn native_output_is_repository_local_and_persists_between_analyses() {
        let fixture = RustFixture::new("persistent-output");
        let first = RustWorkspace::create_at(
            &fixture.root,
            &fixture.cache,
            &fixture
                .repository
                .join("target")
                .join(RUST_TARGET_DIRECTORY),
        )
        .expect("create first native workspace");
        let witness = first.cargo_build.join("dependency.rmeta");
        fs::write(&witness, b"compiled dependency").expect("plant cached artefact");
        assert_eq!(
            first.cargo_target,
            fixture
                .repository
                .join("target")
                .join(RUST_TARGET_DIRECTORY)
        );
        assert_eq!(
            first.cargo_build,
            fixture
                .repository
                .join("target")
                .join(RUST_TARGET_DIRECTORY)
                .join(RUST_BUILD_DIRECTORY)
        );
        assert!(
            !fixture.root.join("target").exists(),
            "the selected tree received no generated output"
        );
        drop(first);

        let second_selected = fixture.base.join("second-selected");
        fs::create_dir(&second_selected).expect("create another nonce-named selected tree");
        let second = RustWorkspace::create_at(
            &second_selected,
            &fixture.cache,
            &fixture
                .repository
                .join("target")
                .join(RUST_TARGET_DIRECTORY),
        )
        .expect("create second native workspace");
        assert_eq!(second.source, fixture.cache);
        assert_eq!(
            second.cargo_target,
            fixture
                .repository
                .join("target")
                .join(RUST_TARGET_DIRECTORY)
        );
        assert_eq!(
            fs::read(second.cargo_build.join("dependency.rmeta"))
                .expect("read retained cached artefact"),
            b"compiled dependency"
        );
    }

    /// The mirror is immutable while Cargo can read it, and one owner exists.
    #[test]
    fn native_source_cache_is_sealed_and_contended_without_a_fallback() {
        let fixture = RustFixture::new("sealed-source");
        fixture.source(REMOVED_PLANT);
        make_readonly(&fixture.root);
        let target = fixture.repository.join("target");
        let first = RustWorkspace::create_at(&fixture.root, &fixture.cache, &target)
            .expect("create the held native workspace");

        assert!(
            fs::metadata(&fixture.cache)
                .expect("read cache permissions")
                .permissions()
                .readonly(),
            "the compiler source root is sealed"
        );
        let refusal = match RustWorkspace::create_at(&fixture.root, &fixture.cache, &target) {
            Err(reason) => reason,
            Ok(_) => panic!("a second gate cannot mutate the held source cache"),
        };
        assert!(
            refusal.contains("is in use by another canonical gate"),
            "contention has one explicit answer: {refusal}"
        );

        drop(first);
        RustWorkspace::create_at(&fixture.root, &fixture.cache, &target)
            .expect("the released cache is reusable");
    }

    /// A workspace the real compiler is run over.
    struct RustFixture {
        base: PathBuf,
        root: PathBuf,
        repository: PathBuf,
        cache: PathBuf,
    }

    impl RustFixture {
        fn new(label: &str) -> Self {
            let base = exclusive_scratch_root(&format!("gate-rust-{label}"));
            let root = base.join("selected");
            let repository = base.join("repository");
            let cache = base.join("native-source");
            fs::create_dir_all(root.join("src")).expect("create native fixture tree");
            fs::create_dir(&repository).expect("create native fixture repository");
            fs::write(root.join("Cargo.toml"), FIXTURE_MANIFEST).expect("write fixture manifest");
            fs::write(root.join("Cargo.lock"), FIXTURE_LOCK).expect("write fixture lock");
            Self {
                base,
                root,
                repository,
                cache,
            }
        }

        fn source(&self, contents: &str) {
            fs::write(self.root.join("src/lib.rs"), contents).expect("write fixture source");
        }

        fn add_reach_packages(&self) {
            for (package, dependencies) in [
                ("consumer", "[dependencies]\nplanted = { path = \"..\" }\n"),
                ("unrelated", ""),
            ] {
                let root = self.root.join(package);
                fs::create_dir_all(root.join("src")).expect("create reach package source");
                fs::write(root.join("src/lib.rs"), "pub fn fixture() {}\n")
                    .expect("write reach package source");
                let manifest = format!(
                    "[package]\nname = \"{package}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n{dependencies}"
                );
                fs::write(root.join("Cargo.toml"), manifest).expect("write reach package manifest");
            }
            let manifest = FIXTURE_MANIFEST.replace(
                "[workspace]",
                "[workspace]\nmembers = [\"consumer\", \"unrelated\"]",
            );
            fs::write(self.root.join("Cargo.toml"), manifest)
                .expect("write reach workspace manifest");
            fs::write(
                self.root.join("Cargo.lock"),
                concat!(
                    "version = 4\n\n",
                    "[[package]]\nname = \"consumer\"\nversion = \"0.0.0\"\n",
                    "dependencies = [\"planted\"]\n\n",
                    "[[package]]\nname = \"planted\"\nversion = \"0.0.0\"\n\n",
                    "[[package]]\nname = \"unrelated\"\nversion = \"0.0.0\"\n",
                ),
            )
            .expect("write reach workspace lock");
        }

        /// Analyze the fixture with the compiler that built these tests.
        ///
        /// `CARGO` names the exact binary Cargo invoked this build with, which
        /// is the same resolution `gate/src/input/declaration.rs` already makes
        /// for the mutation battery. Nothing is stubbed: a stub echoing a canned
        /// diagnostic would prove the parser and never that the compiler finds a
        /// reference the vocabulary gate cannot see.
        fn analyze(&self) -> Analysis {
            let cargo = env!("CARGO");
            self.analyze_with(cargo, &rust_bounds(), None)
        }

        fn analyze_with(&self, cargo: &str, bounds: &Bounds, wait: Option<WaitPolicy>) -> Analysis {
            self.analyze_changes_with(&["src/lib.rs".to_owned()], cargo, bounds, wait)
        }

        fn analyze_changes_with(
            &self,
            changes: &[String],
            cargo: &str,
            bounds: &Bounds,
            wait: Option<WaitPolicy>,
        ) -> Analysis {
            let programs = BTreeMap::from([(CARGO, cargo)]);
            rust_typecheck_with_workspace(
                declaration(GateId::RustTypecheck).expect("native arm is declared"),
                &self.root,
                changes,
                bounds,
                CompilerResources::new(&programs, &[]),
                || {
                    RustWorkspace::create_at(
                        &self.root,
                        &self.cache,
                        &self.repository.join("target"),
                    )
                },
                wait,
            )
        }

        fn sleeping_cargo(&self, label: &str) -> PathBuf {
            let path = self.root.join(label);
            let cargo = env!("CARGO");
            assert!(
                !cargo.contains('\''),
                "fixture Cargo path must be shell-safe"
            );
            fs::write(
                &path,
                format!(
                    concat!(
                        "#!/bin/sh\n",
                        "if [ \"$1\" = metadata ]; then\n",
                        "  exec '{cargo}' \"$@\"\n",
                        "fi\n",
                        "/bin/sleep 5\n",
                    ),
                    cargo = cargo,
                ),
            )
            .expect("write sleeping Cargo");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .expect("make sleeping Cargo executable");
            path
        }

        fn logging_cargo(&self) -> (PathBuf, PathBuf) {
            let path = self.root.join("logging-cargo");
            let log = self.base.join("cargo-arguments.log");
            let cargo = env!("CARGO");
            assert!(
                !cargo.contains('\'') && !log.to_string_lossy().contains('\''),
                "fixture paths must be shell-safe"
            );
            fs::write(
                &path,
                format!(
                    concat!(
                        "#!/bin/sh\n",
                        "if [ \"$1\" = metadata ]; then\n",
                        "  exec '{cargo}' \"$@\"\n",
                        "fi\n",
                        "printf '%s\\n' \"$*\" >> '{log}'\n",
                    ),
                    cargo = cargo,
                    log = log.display(),
                ),
            )
            .expect("write logging Cargo");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .expect("make logging Cargo executable");
            (path, log)
        }
    }

    impl Drop for RustFixture {
        fn drop(&mut self) {
            make_cache_writable(&self.root).expect("make native fixture source removable");
            if self.cache.exists() {
                make_cache_writable(&self.cache).expect("make native fixture cache removable");
            }
            fs::remove_dir_all(&self.base).expect("remove native fixture");
        }
    }

    fn make_readonly(path: &Path) {
        let metadata = fs::symlink_metadata(path).expect("read fixture source metadata");
        if metadata.is_dir() {
            for entry in sorted_entries(path).expect("read fixture source directory") {
                make_readonly(&entry.path());
            }
        }
        let mut permissions = metadata.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(path, permissions).expect("seal fixture source");
    }

    /// Bounds whose runtime cannot elapse, so only the compiler ends the run.
    ///
    /// This fixture asserts nothing about the deadline. A budget that can expire
    /// would only give it a second way to finish, reporting `Timeout` instead of
    /// what the compiler said whenever the lane is loaded enough for a real
    /// build to outrun the number.
    fn rust_bounds() -> Bounds {
        rust_bounds_with_runtime(Duration::MAX)
    }

    fn rust_bounds_with_runtime(runtime: Duration) -> Bounds {
        Bounds::new(
            "native analyzer test",
            Deadlines::new(runtime, Duration::from_secs(1), Duration::from_secs(1))
                .expect("valid native deadlines"),
            Duration::from_millis(25),
            64 * 1024,
        )
        .expect("valid native bounds")
    }

    /// The declared analysis and the compiled one are the same argument vector.
    ///
    /// `--all-targets` is the only reason this arm reaches a body the canvas arm
    /// does not, so a deletion here has to fail somewhere that names it.
    #[test]
    fn native_argument_vector_is_exactly_the_declared_analysis() {
        let declared = declaration(GateId::RustTypecheck)
            .expect("native arm is declared")
            .analysis;

        assert_eq!(
            [format!(
                "{CARGO} check --locked --package <each reached package> <--all-targets or exact reached target flags> <--features exact package-qualified required features>"
            )],
            declared
        );
    }

    mod reach_tests;

    /// The arm refuses a planted body reference, then passes once it is gone.
    ///
    /// This is the non-vacuity proof: the same fixture, the same compiler and
    /// the same arm answer differently for the one line that differs between
    /// [`PLANTED_REFERENCE`] and [`REMOVED_PLANT`], and the refusal carries the
    /// file and the line rather than an opaque exit code.
    #[test]
    fn native_check_names_a_planted_body_reference_and_passes_once_it_is_removed() {
        let fixture = RustFixture::new("planted");

        fixture.source(PLANTED_REFERENCE);
        let refusal = fixture.analyze();

        assert_eq!(refusal.outcome, ArmOutcome::Findings);
        let refusal_timing = refusal
            .timing
            .expect("the compiler refusal reports wall time");
        assert!(refusal_timing.elapsed_ms <= refusal_timing.observation_bound_ms);
        let [finding] = refusal.findings.as_slice() else {
            panic!(
                "the planted reference is one located diagnostic, not {}",
                refusal.findings.len()
            );
        };
        assert_eq!(finding.path.as_deref(), Some("src/lib.rs"));
        assert_eq!(finding.line, Some(PLANTED_LINE));
        assert_eq!(finding.column, Some(PLANTED_COLUMN));
        let FindingDetail::RustTypeOutput { code, message } = &finding.detail else {
            panic!("the native arm publishes its own detail: {finding:?}");
        };
        assert_eq!(code, "E0425");
        assert!(
            message.contains("dev_app_at"),
            "the refusal names the missing spelling: {message}"
        );

        fixture.source(REMOVED_PLANT);
        let cleared = fixture.analyze();

        assert_eq!(cleared.outcome, ArmOutcome::Passed);
        let cleared_timing = cleared
            .timing
            .expect("the successful compiler run reports wall time");
        assert!(cleared_timing.elapsed_ms <= cleared_timing.observation_bound_ms);
        assert_eq!(cleared.findings, Vec::<ArmFinding>::new());
    }

    #[test]
    fn native_check_timeout_retains_bounded_process_time() {
        let fixture = RustFixture::new("timeout");
        fixture.source(REMOVED_PLANT);
        let cargo = fixture.sleeping_cargo("cargo-timeout");
        let bounds = rust_bounds_with_runtime(Duration::from_millis(25));

        let analysis = fixture.analyze_with(
            cargo.to_str().expect("sleeping Cargo path is UTF-8"),
            &bounds,
            None,
        );

        assert_eq!(analysis.outcome, ArmOutcome::Timeout);
        let timing = analysis.timing.expect("a timed-out compiler reports time");
        assert!(timing.elapsed_ms > 0);
        assert!(timing.elapsed_ms <= timing.observation_bound_ms);
    }

    #[test]
    fn native_check_cancellation_is_distinct_and_retains_bounded_process_time() {
        let fixture = RustFixture::new("cancelled");
        fixture.source(REMOVED_PLANT);
        let cargo = fixture.sleeping_cargo("cargo-cancelled");
        let token = CancellationToken::new();
        let cancel = token.clone();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            cancel.cancel();
        });
        let wait = WaitPolicy::new(
            "cancelled native analyzer test",
            Cancellation::Token(token),
            RestartDisposition::Never,
        )
        .expect("valid cancellation policy");

        let analysis = fixture.analyze_with(
            cargo.to_str().expect("sleeping Cargo path is UTF-8"),
            &rust_bounds(),
            Some(wait),
        );
        canceller
            .join()
            .expect("cancellation thread does not panic");

        assert_eq!(analysis.outcome, ArmOutcome::Cancelled);
        let timing = analysis.timing.expect("a cancelled compiler reports time");
        assert!(timing.elapsed_ms <= timing.observation_bound_ms);
    }
}
