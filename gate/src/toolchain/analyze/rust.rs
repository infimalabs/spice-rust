//! The native Rust check, and the build directories it must not share.
//!
//! The canvas arm in [`super`] already compiles Rust, so the invocation and the
//! diagnostic parse are shared with it rather than restated here. What is not
//! shared is the reason this arm exists, the target set that serves it, and the
//! `target` pair it owns; those are the whole of this file.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{
    Analysis, CARGO, CARGO_BUILD_DIRECTORY, CARGO_TARGET_DIRECTORY, COMPILED_HOME_DIRECTORY,
    CompileInvocation, HOME_DIRECTORY, run_compile_analysis_with_wait, tool_search_path,
};
use crate::measure::Bounds;
use crate::toolchain::{ArmOutcome, ArmTiming, FindingDetail, ToolchainDeclaration};
use sill::{Acquisition, Environment, LockFile, LockName, Possession, WaitPolicy};

/// The native analysis, kept in its declaration order.
///
/// `--all-targets` is the whole point of the arm rather than a thoroughness
/// flourish: a plain `cargo check` builds no test target, and test bodies are
/// exactly where a rename's stale references survive a clean textual merge.
pub(super) const CARGO_CHECK_ARGUMENTS: &[&str] =
    &["check", "--locked", "--workspace", "--all-targets"];

/// Compile the selected workspace natively, including every test target.
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
    bounds: &Bounds,
    programs: &BTreeMap<&str, &str>,
) -> Analysis {
    rust_typecheck_with_wait(declaration, root, bounds, programs, None)
}

fn rust_typecheck_with_wait(
    declaration: ToolchainDeclaration,
    root: &Path,
    bounds: &Bounds,
    programs: &BTreeMap<&str, &str>,
    wait: Option<WaitPolicy>,
) -> Analysis {
    if !root.join("Cargo.toml").is_file() {
        return Analysis::not_applicable(declaration);
    }
    let Some(cargo) = programs.get(CARGO) else {
        return Analysis::terminal(
            ArmOutcome::ToolUnavailable,
            format!("{} declares no {CARGO} program", declaration.id.as_str()),
        );
    };
    let workspace = match RustWorkspace::create(root) {
        Ok(workspace) => workspace,
        Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
    };
    let analysis = match rust_environment(cargo, &workspace) {
        Err(reason) => Analysis::terminal(ArmOutcome::ParseFailure, reason),
        Ok(environment) => {
            let invocation = CompileInvocation {
                name: "cargo native check",
                program: cargo,
                arguments: CARGO_CHECK_ARGUMENTS,
                cwd: root,
            };
            let result = run_compile_analysis_with_wait(
                declaration.id,
                root,
                invocation,
                bounds,
                &environment,
                |code, message| FindingDetail::RustTypeOutput { code, message },
                wait,
            );
            match result {
                Err(failure) => {
                    let (analysis, elapsed) = failure.into_parts();
                    match elapsed {
                        Some(elapsed) => analysis.timed(timing(elapsed, bounds)),
                        None => analysis,
                    }
                }
                Ok(elapsed) => Analysis::terminal(
                    ArmOutcome::Passed,
                    format!(
                        "{} completed its declared native analysis",
                        declaration.id.as_str()
                    ),
                )
                .timed(timing(elapsed, bounds)),
            }
        }
    };
    // A build root that could not be torn down is reported rather than
    // discarded, so a later vanished or poisoned root is attributable here.
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

/// Build directories the native check owns, kept off the lane's own `target`.
///
/// Cargo takes an exclusive lock on a build directory. Sharing the lane's
/// `target` would make this arm block on, and be blocked by, any `cargo` the
/// occupant is already running, so the arm gets its own pair exactly as the
/// canvas arm does.
///
/// # Removal is possible only under the root's lock
///
/// Both build directories live *inside* `root`, and `root` is a fixed sibling
/// of the selected tree carrying no per-invocation token, so two arms analysing
/// one tree resolve the same path. Tearing that root down unconditionally would
/// delete a directory another arm is still compiling into, and the victim would
/// report a rustc error naming a missing artefact rather than the removal. It
/// is also the shape that survives a killed run: whatever the previous arm left
/// behind is indistinguishable from a root in active use by looking at it.
///
/// The mechanism is therefore an advisory lock at `<root>.lock`, held for the
/// life of the workspace, and the invariant is that **this arm removes a build
/// root only while it possesses that lock**. Contention is not a removal that
/// gets refused; it is a removal that never becomes reachable, because a
/// contended arm returns before it owns anything to tear down. Possession also
/// makes reclaiming a stale root safe: a root whose lock is free is one no live
/// build holds, which is exactly the run that was killed before its teardown.
///
/// [`remove_build_root`] takes a [`Possession`] rather than a path, so the
/// invariant is carried by the signature and a removal cannot be spelled
/// without the lock in hand. `flock` grants per-open-file-description, so a
/// second acquisition contends even from inside this process; that is what lets
/// a fixture hold a build root and drive the decision directly rather than
/// having to lose a race to observe it.
struct RustWorkspace {
    root: PathBuf,
    cargo_target: PathBuf,
    cargo_build: PathBuf,
    possession: Option<Possession>,
}

/// What one attempt to remove a build root did, so no attempt is discarded.
#[derive(Debug, Eq, PartialEq)]
enum BuildRootRemoval {
    /// The root is gone, whether this attempt unlinked it or found it absent.
    Removed,
    /// The root still exists and the reason is carried rather than dropped.
    Failed(String),
}

impl BuildRootRemoval {
    fn reason(&self, root: &Path) -> Option<String> {
        match self {
            Self::Removed => None,
            Self::Failed(error) => Some(format!(
                "rust analysis build root {} could not be removed: {error}",
                root.display()
            )),
        }
    }
}

/// Remove a build root whose lock this caller already possesses.
///
/// Taking [`Possession`] rather than a path alone is the guard: there is no way
/// to spell a removal without first holding the lock, so a root a live build
/// holds is unreachable here rather than merely unlucky. Absence counts as
/// removed; every other refusal is carried back out.
fn remove_build_root(root: &Path, _possession: &Possession) -> BuildRootRemoval {
    match fs::remove_dir_all(root) {
        Ok(()) => BuildRootRemoval::Removed,
        Err(error) if error.kind() == io::ErrorKind::NotFound => BuildRootRemoval::Removed,
        Err(error) => BuildRootRemoval::Failed(error.to_string()),
    }
}

/// The lock guarding one build root, kept beside the root rather than inside
/// it so removing the root cannot remove the evidence that it was held.
fn build_root_lock(root: &Path) -> Result<LockFile, String> {
    let Some(name) = root.file_name() else {
        return Err(format!(
            "rust analysis workspace {} has no name to lock",
            root.display()
        ));
    };
    let mut lock_name = name.to_os_string();
    lock_name.push(".lock");
    let path = root.with_file_name(lock_name);
    let name = LockName::new("rust-analysis-build-root")
        .map_err(|error| format!("rust analysis build-root lock name was refused: {error}"))?;
    Ok(LockFile::new(name, path))
}

/// Where one selected tree's build root lives.
///
/// A sibling of the tree rather than a child, so the analysis never writes
/// inside the tree it is measuring.
fn build_root_path(selected: &Path) -> Result<PathBuf, String> {
    let Some(name) = selected.file_name() else {
        return Err(format!(
            "rust selected tree {selected:?} has no name for an analysis workspace"
        ));
    };
    let mut workspace_name = name.to_os_string();
    workspace_name.push(".spice-rust-analysis");
    Ok(selected.with_file_name(workspace_name))
}

impl RustWorkspace {
    fn create(selected: &Path) -> Result<Self, String> {
        let root = build_root_path(selected)?;
        let lock = build_root_lock(&root)?;
        let possession = match lock.try_acquire() {
            Ok(Acquisition::Acquired(possession)) => possession,
            Ok(Acquisition::Contended) => {
                return Err(format!(
                    "rust analysis build root {} is in use by another analysis of this tree",
                    root.display()
                ));
            }
            Err(error) => {
                return Err(format!(
                    "rust analysis build root {} could not be locked: {error}",
                    root.display()
                ));
            }
        };
        // Possession proves no live build holds this root, so anything already
        // here was left by a run that died before its own teardown.
        if let Some(reason) = remove_build_root(&root, &possession).reason(&root) {
            return Err(reason);
        }
        fs::create_dir(&root).map_err(|error| {
            format!(
                "rust analysis workspace {} could not be created: {error}",
                root.display()
            )
        })?;
        let cargo_target = root.join("cargo-target");
        let cargo_build = root.join("cargo-build");
        if let Err(error) =
            fs::create_dir(&cargo_target).and_then(|()| fs::create_dir(&cargo_build))
        {
            let created = format!(
                "rust analysis output directories for {} could not be created: {error}",
                selected.display()
            );
            let removal = remove_build_root(&root, &possession);
            return Err(match removal.reason(&root) {
                Some(reason) => format!("{created}; {reason}"),
                None => created,
            });
        }
        Ok(Self {
            root,
            cargo_target,
            cargo_build,
            possession: Some(possession),
        })
    }

    /// Tear the build root down and hand back anything that went wrong.
    ///
    /// Teardown is explicit because `Drop` cannot report, and an unreported
    /// removal failure is the half of this arm's history that made a vanished
    /// build root undiagnosable from the remover's side.
    fn release(mut self) -> Option<String> {
        self.possession
            .take()
            .and_then(|possession| remove_build_root(&self.root, &possession).reason(&self.root))
    }
}

impl Drop for RustWorkspace {
    /// Last resort for an unwind, where no caller remains to receive a reason.
    ///
    /// `release` takes the possession, so a workspace torn down through it
    /// arrives here already emptied and this does nothing.
    fn drop(&mut self) {
        if let Some(possession) = self.possession.take() {
            let _ = remove_build_root(&self.root, &possession);
        }
    }
}

fn rust_environment(cargo: &str, workspace: &RustWorkspace) -> Result<Environment, String> {
    let path = tool_search_path("rust-typecheck", &[cargo, "/usr/bin/cc", "/bin/sh"])?;
    Environment::new([
        ("PATH", path),
        (HOME_DIRECTORY, COMPILED_HOME_DIRECTORY.into()),
        (
            CARGO_TARGET_DIRECTORY,
            workspace.cargo_target.as_os_str().to_owned(),
        ),
        (
            CARGO_BUILD_DIRECTORY,
            workspace.cargo_build.as_os_str().to_owned(),
        ),
    ])
    .map_err(|error| format!("native typecheck tool environment was refused: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    use crate::toolchain::{ArmFinding, GateId, declaration};
    use sill::{Cancellation, CancellationToken, Deadlines, RestartDisposition, WaitPolicy};

    static NEXT_RUST_FIXTURE: AtomicU64 = AtomicU64::new(0);

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
    /// `a_local_binding_is_not_a_coined_name`, which asserts that a `let driver`
    /// binding coins nothing while `driver` is a retired spelling; judging every
    /// path expression turns that law red, and judging only qualified paths
    /// still misses a bare call exactly like this one.
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

    /// Plant a build root holding one artefact, as a live build would leave it.
    fn plant_build_root(selected: &Path) -> (PathBuf, PathBuf) {
        let root = build_root_path(selected).expect("the fixture tree names a build root");
        fs::create_dir_all(root.join("cargo-target")).expect("plant a build root");
        let artefact = root.join("cargo-target/planted.rmeta");
        fs::write(&artefact, b"artefact").expect("plant a build artefact");
        (root, artefact)
    }

    /// A build root another build is compiling into is never removed.
    ///
    /// The holder is a second acquisition of the same lock. `flock` grants per
    /// open file description, so this contends exactly as a separate process
    /// would, which is what makes the decision drivable instead of raced.
    #[test]
    fn a_build_root_a_live_build_holds_is_refused_and_survives_intact() {
        let fixture = RustFixture::new("held-build-root");
        let (root, artefact) = plant_build_root(&fixture.root);
        let lock = build_root_lock(&root).expect("the build root names a lock");
        let Ok(Acquisition::Acquired(held)) = lock.try_acquire() else {
            panic!("a freshly planted build root must be lockable");
        };

        let refused = match RustWorkspace::create(&fixture.root) {
            Err(reason) => reason,
            Ok(_) => panic!("a build root a live build holds must be refused"),
        };

        assert!(
            refused.contains("is in use by another analysis of this tree"),
            "the refusal names why it refused: {refused}"
        );
        assert!(
            refused.contains(&root.display().to_string()),
            "the refusal names the build root: {refused}"
        );
        assert_eq!(
            fs::read(&artefact).expect("the held build's artefact survives"),
            b"artefact",
            "the held build root was removed out from under its build"
        );
        drop(held);
        fs::remove_dir_all(&root).expect("remove the held fixture root");
        fs::remove_file(lock.path()).expect("remove the held fixture lock");
    }

    /// The guard cannot pass by refusing every removal.
    ///
    /// A root no build holds is what a killed run leaves behind. Refusing it
    /// too would poison every later analysis of the tree, so it is reclaimed.
    #[test]
    fn a_build_root_no_live_build_holds_is_reclaimed_rather_than_refused() {
        let fixture = RustFixture::new("stale-build-root");
        let (root, artefact) = plant_build_root(&fixture.root);

        let workspace =
            RustWorkspace::create(&fixture.root).expect("a build root no build holds is reclaimed");

        assert!(
            !artefact.exists(),
            "the stale artefact outlived the run that abandoned it"
        );
        assert!(
            workspace.cargo_target.is_dir(),
            "the reclaimed root was rebuilt for this analysis"
        );
        let lock_path = build_root_lock(&root)
            .expect("the build root names a lock")
            .path()
            .to_path_buf();
        assert_eq!(workspace.release(), None, "teardown reports no reason");
        assert!(!root.exists(), "teardown removed the build root");
        fs::remove_file(&lock_path).expect("remove the reclaimed fixture lock");
    }

    /// A workspace the real compiler is run over.
    struct RustFixture {
        root: PathBuf,
    }

    impl RustFixture {
        fn new(label: &str) -> Self {
            let sequence = NEXT_RUST_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "gate-rust-{label}-{}-{sequence}",
                std::process::id()
            ));
            if root.exists() {
                fs::remove_dir_all(&root).expect("remove stale native fixture");
            }
            fs::create_dir_all(root.join("src")).expect("create native fixture tree");
            fs::write(root.join("Cargo.toml"), FIXTURE_MANIFEST).expect("write fixture manifest");
            fs::write(root.join("Cargo.lock"), FIXTURE_LOCK).expect("write fixture lock");
            Self { root }
        }

        fn source(&self, contents: &str) {
            fs::write(self.root.join("src/lib.rs"), contents).expect("write fixture source");
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
            let programs = BTreeMap::from([(CARGO, cargo)]);
            rust_typecheck_with_wait(
                declaration(GateId::RustTypecheck).expect("native arm is declared"),
                &self.root,
                bounds,
                &programs,
                wait,
            )
        }

        fn sleeping_cargo(&self, label: &str) -> PathBuf {
            let path = self.root.join(label);
            fs::write(&path, "#!/bin/sh\n/bin/sleep 5\n").expect("write sleeping Cargo");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .expect("make sleeping Cargo executable");
            path
        }
    }

    impl Drop for RustFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).expect("remove native fixture");
        }
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
            [format!("{CARGO} {}", CARGO_CHECK_ARGUMENTS.join(" "))],
            declared
        );
    }

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
        assert!(timing.elapsed_ms > 0);
        assert!(timing.elapsed_ms <= timing.observation_bound_ms);
    }
}
