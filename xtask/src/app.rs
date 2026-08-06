//! Compiling the browser app and owning the bytes committed as its stage.
//!
//! A committed build artifact makes one question load-bearing that an ordinary
//! build leaves implicit: which environment produced it. Reproducing the stage
//! is only possible for the environment whose compiler metadata went into it,
//! so [`stage_app`] admits exactly that environment and refuses every other,
//! while [`check_app`] gives each host the part of the answer it can settle.
//! What those bytes were built *from* is a separate claim, kept in
//! [`crate::stage_sources`].

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::release::ReleaseEnvironment;
use crate::stage_sources::{
    STAGE_INSTRUCTIONS, STAGE_SOURCE_LISTING, compare_stage_sources, record_stage_sources,
};
use crate::toolchain;

/// The target the browser app is compiled for.
///
/// Every other step builds for the host and only for the host, so the far side
/// of each `cfg(not(target_arch = "wasm32"))` is code no host step ever
/// compiles. The app is compiled from this workspace and embedded in the binary
/// that owns the socket, which makes that far side shipped code rather than a
/// portability courtesy.
pub(crate) const WASM_TARGET: &str = "wasm32-unknown-unknown";

/// The path the app build's Cargo home is remapped to.
///
/// The build is told to read crates from wherever the rim resolved, and told to
/// record that it read them from here. Only the second half can be a constant:
/// the recorded spelling has to be the same on every host or the artifact stops
/// being reproducible, which is the whole reason the remapping exists.
const REMAPPED_CARGO_HOME: &str = "/cargo-home";

/// The host whose builds own the committed app stage.
///
/// Cargo folds `rustc --version --verbose`, which names the host triple beside
/// the compiler release, into every crate's `-Cmetadata`. The same sources
/// therefore emit different wasm-bindgen disambiguator symbols and a
/// differently hashed `canvas-app.html` on every other host, so byte equality
/// against a foreign-triple rebuild is not a stricter check but an unreachable
/// one. One environment has to own these bytes; it is the sealed Linux image,
/// whose `rustc` reports `x86_64-unknown-linux-gnu`, because that is the
/// environment the release battery can reproduce with networking disabled. The
/// release that same report names is the other half of this identity, and one
/// invocation yields both -- [`toolchain::observed`] asks, and the string it
/// hands back is what this constant is compared against.
const STAGING_HOST: &str = "x86_64-unknown-linux-gnu";

/// Whether a host tuple owns the app stage.
///
/// This is the tuple half of what decides ownership, and it admits one tuple or
/// none; [`toolchain::pinned_release`] is the release half, and a host passing
/// this one still refuses under any other release. Both halves are read off one
/// `rustc --version --verbose` report, so this compares the tuple the compiler
/// says it emits for -- the tuple whose `-Cmetadata` the staged bytes actually
/// carry -- rather than the one this xtask was compiled for. Those are not the
/// same question, and only the first one is answered by the compiler that will
/// run. Because it takes a value, every lane can offer it the musl and gnux32
/// neighbours no lane compiles for, and the owning tuple as well.
fn owns_the_stage(host: &str) -> bool {
    host == STAGING_HOST
}

/// What the `rustc` this environment would build with reports about itself.
///
/// Both callers that decide anything on the compiler's identity come through
/// here, so neither can end up reading a different `rustc` than the one whose
/// bytes are at stake -- including when that one is named by a compiler
/// override or by the `build.rustc` key of a Cargo config file instead of by
/// `PATH`, which [`ReleaseEnvironment::rustc`] settles in Cargo's own order.
/// The directory handed to it is the app build's own working directory, so the
/// config files walked are the ones that build reads rather than the ones some
/// other directory would have offered.
///
/// An environment naming no `rustc` at all is refused here rather than treated
/// as an unknown host: the question is what the compiler is, and there is no
/// compiler to ask. A Cargo config that exists and cannot be read carries its
/// own refusal up, because a build whose compiler is undecided is not a build
/// whose compiler is the one on `PATH`.
fn observed_toolchain(
    root: &Path,
    environment: &ReleaseEnvironment,
) -> Result<toolchain::ObservedToolchain, AppBuildError> {
    let rustc = environment
        .rustc(&root.join("app"))
        .map_err(|error| AppBuildError::UnreadableToolchainReport(error.to_string()))?
        .ok_or_else(|| {
            AppBuildError::UnreadableToolchainReport(
                "no rustc is named by RUSTC, by CARGO_BUILD_RUSTC, by build.rustc in any cargo \
                 config this build reads, or on PATH, so the toolchain a build here would \
                 compile under is unknown"
                    .to_owned(),
            )
        })?;
    toolchain::observed(&rustc, root)
}

const APP_FILES: [&str; 4] = ["app.module", "app_bg.wasm", "canvas-app.html", "style.css"];
const RELEASE_STAGE_DIRECTORIES: [&str; 2] = ["objects", "targets"];
const GENERATED_MODULE_REFERENCES: usize = 2;

/// Compile the browser app with the pinned Trunk adapter and replace the
/// committed stage with exactly those generated assets.
///
/// Only the staging environment may write the stage: a rebuild from anywhere
/// else replaces bytes the release battery reproduces with bytes it cannot. The
/// identity of what was compiled is recorded beside the stage in the same
/// breath, because this is the one run that knows it first-hand.
///
/// # Errors
///
/// Returns an unreadable toolchain report, an unpinned release, a foreign
/// staging host, a missing Cargo home, Trunk execution failure, a refused
/// generated stage, a build input the repository does not track or cannot be
/// asked about, or an unreadable build input.
pub fn stage_app(
    environment: &ReleaseEnvironment,
    output: &mut impl Write,
) -> Result<(), AppBuildError> {
    let root = workspace_root()?;
    admit_staging_environment(&root, environment)?;
    let stage = root.join("dist/stage");
    run_trunk(&root, environment, &stage, output)?;
    record_stage_sources(&root)
}

/// Whether this build may write the stage: the release first, then the tuple.
///
/// Both dimensions Cargo folds into `-Cmetadata` are asked of one report, so
/// the release and the tuple cannot come from two different compilers. That
/// also puts both doors in reach of every lane: each is decided on a string the
/// compiler handed over, so a test anywhere can drive either one with a report
/// of its choosing, rather than a door only the owning host can knock on and
/// only the owning host can prove is still there. The release is asked first
/// because it is the half an operator can act on without changing machines.
fn admit_staging_environment(
    root: &Path,
    environment: &ReleaseEnvironment,
) -> Result<(), AppBuildError> {
    let observed = observed_toolchain(root, environment)?;
    if observed.release() != toolchain::pinned_release() {
        return Err(AppBuildError::ForeignToolchainRelease(
            observed.release().to_owned(),
        ));
    }
    if !owns_the_stage(observed.host()) {
        return Err(AppBuildError::ForeignStagingHost(
            observed.host().to_owned(),
        ));
    }
    Ok(())
}

/// Compile the browser app into the worktree's development stage.
///
/// The committed stage is reproducible bytes owned by one environment, so a
/// lane that changed the app cannot see its own change without a round trip
/// through the sealed image. This writes a second stage instead, at
/// [`spice::canvas::LOCAL_STAGE`], which the resident serves in place of the
/// embedded bytes whenever it exists; it is never committed and never
/// compared. Every host may run it, because nothing downstream reproduces
/// what it emits.
///
/// The directory is removed first rather than built over: [`finalize_stage`]
/// refuses a stage that already carries `app.module`, so a second run would
/// otherwise fail on the artifact its own first run left behind.
///
/// # Errors
///
/// Returns an unreadable workspace root, a development stage that cannot be
/// removed, a missing Cargo home, Trunk execution failure, or a refused
/// generated stage.
pub fn local_app(
    environment: &ReleaseEnvironment,
    output: &mut impl Write,
) -> Result<(), AppBuildError> {
    let root = workspace_root()?;
    let stage = root.join(spice::canvas::LOCAL_STAGE);
    if stage.exists() {
        fs::remove_dir_all(&stage).map_err(|source| AppBuildError::Io {
            path: stage.clone(),
            source,
        })?;
    }
    run_trunk(&root, environment, &stage, output)?;
    writeln!(
        output,
        "[app] {} is now served by the next `spice up` in this worktree; \
         delete it to go back to the embedded app",
        stage.display()
    )
    .map_err(AppBuildError::Output)
}

/// Rebuild the browser app outside the committed stage and compare it back.
///
/// Every host first asks whether the stage was built from the sources now in
/// the tree, which is answered from recorded input identity and needs no
/// rebuild, then rebuilds and compares the staged inventory. Only the staging
/// host goes on to compare bytes, because only its compiler metadata can
/// reproduce them -- and it is the same `rustc` report that says which host
/// this is, so the tuple this names is the one that compiled the rebuild it
/// then declines to compare.
///
/// # Errors
///
/// Returns build inputs the recorded stage identity no longer describes, a
/// tracked set that cannot be read, a toolchain report that cannot be read, a
/// missing Cargo home, Trunk execution failure, an inventory that differs from
/// the rebuild, or, on the staging host, the first staged asset whose bytes
/// differ.
pub fn check_app(
    environment: &ReleaseEnvironment,
    output: &mut impl Write,
) -> Result<(), AppBuildError> {
    let root = workspace_root()?;
    compare_stage_sources(&root)?;
    let host = observed_toolchain(&root, environment)?.host().to_owned();
    let temporary = environment
        .build_root(&root)
        .join(format!("xtask-canvas-check-{}", std::process::id()));
    if temporary.exists() {
        fs::remove_dir_all(&temporary).map_err(|source| AppBuildError::Io {
            path: temporary.clone(),
            source,
        })?;
    }
    let result = run_trunk(&root, environment, &temporary, output).and_then(|()| {
        let stage = root.join("dist/stage");
        compare_stage_inventory(&stage, &temporary)?;
        if owns_the_stage(&host) {
            return compare_stage_bytes(&stage, &temporary);
        }
        writeln!(
            output,
            "[app] {host} is not {STAGING_HOST}; compared the staged inventory \
             without asserting bytes no host but {STAGING_HOST} can reproduce"
        )
        .map_err(AppBuildError::Output)
    });
    let cleanup = fs::remove_dir_all(&temporary).map_err(|source| AppBuildError::Io {
        path: temporary,
        source,
    });
    result.and(cleanup)
}

fn run_trunk(
    root: &Path,
    environment: &ReleaseEnvironment,
    stage: &Path,
    output: &mut impl Write,
) -> Result<(), AppBuildError> {
    let build_root = environment.build_root(root);
    let cargo_home = environment
        .cargo_home()
        .ok_or(AppBuildError::MissingCargoHome)?;
    prepare_app_build(&build_root)?;
    writeln!(output, "[app] trunk 0.21.14 -> {}", stage.display())
        .map_err(AppBuildError::Output)?;
    let rustflags = remapped_rustflags(root, &cargo_home)?;
    let status = trunk_command(root, &build_root, &cargo_home, stage, rustflags)
        .status()
        .map_err(AppBuildError::Launch)?;
    if !status.success() {
        return Err(AppBuildError::Exit(status.code()));
    }
    finalize_stage(stage)
}

fn app_build_directory(build_root: &Path) -> PathBuf {
    build_root.join("xtask-app-build")
}

fn prepare_app_build(build_root: &Path) -> Result<(), AppBuildError> {
    let build = app_build_directory(build_root);
    fs::create_dir_all(&build).map_err(|source| AppBuildError::Io {
        path: build,
        source,
    })
}

/// The Trunk invocation, and the inherited environment it edits.
///
/// Trunk inherits this process's environment, so every variable Cargo reads is
/// carried into the app build whether or not this function mentions it. Eight
/// entries are named here and the assertion below pins all eight. Two of them
/// are edits to what was inherited rather than values supplied fresh, and the
/// reason is recorded beside them, because a variable that is simply absent
/// from this list is indistinguishable from one somebody meant to handle:
/// `RUSTC_WRAPPER` and `RUSTC_WORKSPACE_WRAPPER` are emptied.
///
/// `CARGO`, `RUSTC`, and `CARGO_BUILD_RUSTC` are deliberately not among them.
/// The sealed image's source replacement sits at `/.cargo/config.toml`, so the
/// Cargo that launched xtask and the one on `PATH` read the same offline source
/// authority; Trunk may retain the exact parent Cargo instead of exchanging it
/// for an ambient executable. The other two name the compiler this build will
/// run, and removing them here would make the build disagree with the report
/// [`admit_staging_environment`] admitted it on; they are passed through and
/// measured instead, by [`ReleaseEnvironment::rustc`], which reads them in
/// Cargo's own order. The working directory set below is the same one that
/// resolver walks for a `build.rustc` a config file names, so the compiler
/// admitted and the compiler Cargo picks are settled from one hierarchy rather
/// than two.
fn trunk_command(
    root: &Path,
    build_root: &Path,
    cargo_home: &OsStr,
    stage: &Path,
    rustflags: String,
) -> Command {
    let target = build_root.join("xtask-app");
    let build = app_build_directory(build_root);
    let mut command = Command::new("trunk");
    command
        .current_dir(root.join("app"))
        .env("CARGO_BUILD_BUILD_DIR", build)
        .env("CARGO_ENCODED_RUSTFLAGS", rustflags)
        .env("CARGO_HOME", cargo_home)
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", target)
        .env("NO_COLOR", "true")
        // Emptied rather than removed: an empty wrapper is how Cargo is told
        // there is none, while removing them would let an inherited sccache or
        // distcc stand between the build and the compiler whose report was
        // admitted, and fold its own cache decisions into bytes that must be
        // reproducible from the sources alone.
        .env("RUSTC_WORKSPACE_WRAPPER", "")
        .env("RUSTC_WRAPPER", "")
        .args([
            "build",
            "index.html",
            "--release",
            "--locked",
            "--filehash=false",
            "--html-output",
            "canvas-app.html",
            "--dist",
        ])
        .arg(stage);
    command
}

fn finalize_stage(stage: &Path) -> Result<(), AppBuildError> {
    let generated = stage.join("app.js");
    let module = stage.join("app.module");
    if module.exists() {
        return Err(AppBuildError::GeneratedStage(
            "app.module existed before generated app.js was renamed".to_owned(),
        ));
    }
    fs::rename(&generated, &module).map_err(|source| AppBuildError::Io {
        path: generated,
        source,
    })?;

    let html_path = stage.join("canvas-app.html");
    let html = fs::read_to_string(&html_path).map_err(|source| AppBuildError::Io {
        path: html_path.clone(),
        source,
    })?;
    let references = html.matches("/app.js").count();
    if references != GENERATED_MODULE_REFERENCES {
        return Err(AppBuildError::GeneratedStage(format!(
            "canvas-app.html carried {references} app.js references, \
             expected {GENERATED_MODULE_REFERENCES}"
        )));
    }
    fs::write(&html_path, html.replace("/app.js", "/app.module")).map_err(|source| {
        AppBuildError::Io {
            path: html_path,
            source,
        }
    })
}

fn remapped_rustflags(root: &Path, cargo_home: &OsStr) -> Result<String, AppBuildError> {
    let root = root
        .to_str()
        .ok_or_else(|| AppBuildError::NonUtf8(root.to_path_buf()))?;
    let cargo_home = cargo_home
        .to_str()
        .ok_or_else(|| AppBuildError::NonUtf8(PathBuf::from(cargo_home)))?;
    Ok([
        format!("--remap-path-prefix={root}=/workspace"),
        format!("--remap-path-prefix={cargo_home}={REMAPPED_CARGO_HOME}"),
    ]
    .join("\u{1f}"))
}

fn compare_stage_inventory(expected: &Path, observed: &Path) -> Result<(), AppBuildError> {
    let expected_names = stage_names(expected)?;
    let observed_names = stage_names(observed)?;
    let expected_app_names = expected_names
        .iter()
        .filter(|name| APP_FILES.contains(&name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let mut expected_release_shape_is_valid = true;
    for name in expected_names
        .iter()
        .filter(|name| !APP_FILES.contains(&name.as_str()))
    {
        if !RELEASE_STAGE_DIRECTORIES.contains(&name.as_str()) {
            expected_release_shape_is_valid = false;
            continue;
        }
        let path = expected.join(name);
        let metadata =
            fs::symlink_metadata(&path).map_err(|source| AppBuildError::Io { path, source })?;
        expected_release_shape_is_valid &= metadata.file_type().is_dir();
    }
    if expected_app_names != APP_FILES
        || observed_names != APP_FILES
        || !expected_release_shape_is_valid
    {
        return Err(AppBuildError::Inventory {
            expected: expected_names,
            observed: observed_names,
        });
    }
    Ok(())
}

fn compare_stage_bytes(expected: &Path, observed: &Path) -> Result<(), AppBuildError> {
    for name in APP_FILES {
        let expected_path = expected.join(name);
        let observed_path = observed.join(name);
        let expected_bytes = fs::read(&expected_path).map_err(|source| AppBuildError::Io {
            path: expected_path,
            source,
        })?;
        let observed_bytes = fs::read(&observed_path).map_err(|source| AppBuildError::Io {
            path: observed_path,
            source,
        })?;
        if expected_bytes != observed_bytes {
            return Err(AppBuildError::Drift(name.to_owned()));
        }
    }
    Ok(())
}

fn stage_names(stage: &Path) -> Result<Vec<String>, AppBuildError> {
    let mut names = fs::read_dir(stage)
        .map_err(|source| AppBuildError::Io {
            path: stage.to_path_buf(),
            source,
        })?
        .map(|entry| {
            entry
                .map_err(|source| AppBuildError::Io {
                    path: stage.to_path_buf(),
                    source,
                })
                .and_then(|entry| {
                    entry
                        .file_name()
                        .into_string()
                        .map_err(|_| AppBuildError::NonUtf8(entry.path()))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    Ok(names)
}

fn workspace_root() -> Result<PathBuf, AppBuildError> {
    std::env::current_dir().map_err(|source| AppBuildError::Io {
        path: PathBuf::from("."),
        source,
    })
}

#[derive(Debug)]
pub enum AppBuildError {
    Output(io::Error),
    Launch(io::Error),
    Exit(Option<i32>),
    Io {
        path: PathBuf,
        source: io::Error,
    },
    NonUtf8(PathBuf),
    MissingCargoHome,
    UnreadableToolchainReport(String),
    ForeignToolchainRelease(String),
    ForeignStagingHost(String),
    GeneratedStage(String),
    Inventory {
        expected: Vec<String>,
        observed: Vec<String>,
    },
    Drift(String),
    SourceInput(PathBuf),
    UnreadableSourceIndex(String),
    UntrackedSourceInput(String),
    StaleStage(Vec<String>),
}

impl fmt::Display for AppBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Output(error) => write!(formatter, "app build output failed: {error}"),
            Self::Launch(error) => write!(formatter, "trunk did not launch: {error}"),
            Self::Exit(Some(code)) => write!(formatter, "trunk failed with exit {code}"),
            Self::Exit(None) => formatter.write_str("trunk was terminated by a signal"),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::NonUtf8(path) => write!(formatter, "stage path is not UTF-8: {}", path.display()),
            Self::MissingCargoHome => formatter.write_str(
                "the app build needs CARGO_HOME or HOME to locate the crates it compiles",
            ),
            Self::UnreadableToolchainReport(reason) => write!(
                formatter,
                "the toolchain this build would compile under could not be read, so neither \
                 the release nor the host it emits for is known: {reason}"
            ),
            Self::ForeignToolchainRelease(observed) => write!(
                formatter,
                "the app stage is owned by rustc {} builds in image {} and this toolchain \
                 reports {observed}: Cargo folds the compiler release into every -Cmetadata, \
                 so staging here would replace bytes the pinned release reproduces with bytes \
                 no other release can",
                toolchain::pinned_release(),
                proof::DEFAULT_IMAGE
            ),
            Self::ForeignStagingHost(host) => write!(
                formatter,
                "the app stage is owned by {STAGING_HOST} builds in image {} and this host \
                 is {host}: staging here would replace bytes the sealed image reproduces \
                 with bytes no other environment can",
                proof::DEFAULT_IMAGE
            ),
            Self::GeneratedStage(reason) => {
                write!(formatter, "generated app stage refused: {reason}")
            }
            Self::Inventory { expected, observed } => write!(
                formatter,
                "staged app inventory differs: committed={expected:?}, rebuilt={observed:?}"
            ),
            Self::Drift(path) => write!(formatter, "staged app drifted at {path}"),
            Self::SourceInput(path) => write!(
                formatter,
                "a staged app build input is tracked but is not a regular file: {}",
                path.display()
            ),
            Self::UnreadableSourceIndex(reason) => write!(
                formatter,
                "the tracked build inputs could not be read, so what the stage was built \
                 from cannot be stated: {reason}; mounting a linked worktree is the usual \
                 cause, because its repository sits outside the mount, and \
                 {STAGE_INSTRUCTIONS} says what to mount instead"
            ),
            Self::UntrackedSourceInput(input) => write!(
                formatter,
                "the staged app build input {input} names no tracked file, so the recorded \
                 stage identity would leave out sources the stage was built from"
            ),
            Self::StaleStage(paths) => write!(
                formatter,
                "the committed app stage was not built from the sources now in the tree: \
                 {} changed since {STAGE_SOURCE_LISTING} recorded them; restage with \
                 cargo xtask stage-app in image {}, which is the one environment allowed \
                 to write the stage, from a checkout whose repository is under the mount \
                 as {STAGE_INSTRUCTIONS} describes, because that run refuses a linked \
                 worktree",
                paths.join(", "),
                proof::DEFAULT_IMAGE
            ),
        }
    }
}

impl std::error::Error for AppBuildError {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// The build root here is deliberately outside the workspace, which is how
    /// the sealed image presents it: sources on a read-only mount, writes on a
    /// separate scratch. Every writable name the command carries must land on
    /// the scratch, while the directory it reads sources from must not move.
    #[test]
    fn app_build_command_owns_cache_and_wrapper_inputs() {
        let root = Path::new("/workspace");
        let build_root = Path::new("/cargo-target");
        let stage = Path::new("/stage");
        let command = trunk_command(
            root,
            build_root,
            OsStr::new("/cargo-home"),
            stage,
            "remapped-flags".to_owned(),
        );
        let environment = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            environment,
            BTreeMap::from([
                (
                    "CARGO_BUILD_BUILD_DIR".to_owned(),
                    Some("/cargo-target/xtask-app-build".to_owned()),
                ),
                (
                    "CARGO_ENCODED_RUSTFLAGS".to_owned(),
                    Some("remapped-flags".to_owned()),
                ),
                ("CARGO_HOME".to_owned(), Some("/cargo-home".to_owned())),
                ("CARGO_INCREMENTAL".to_owned(), Some("0".to_owned())),
                (
                    "CARGO_TARGET_DIR".to_owned(),
                    Some("/cargo-target/xtask-app".to_owned()),
                ),
                ("NO_COLOR".to_owned(), Some("true".to_owned())),
                ("RUSTC_WORKSPACE_WRAPPER".to_owned(), Some(String::new())),
                ("RUSTC_WRAPPER".to_owned(), Some(String::new())),
            ])
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/workspace/app")));
        assert_eq!(command.get_program(), "trunk");
        assert_eq!(
            command
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            [
                "build",
                "index.html",
                "--release",
                "--locked",
                "--filehash=false",
                "--html-output",
                "canvas-app.html",
                "--dist",
                "/stage",
            ]
        );
    }

    #[test]
    fn app_build_preparation_creates_the_dedicated_build_directory() {
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-app-build-preparation-{}",
            std::process::id(),
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale app-build fixture");
        }

        prepare_app_build(&root).expect("prepare the app-build directory");

        let build = app_build_directory(&root);
        assert!(
            build.is_dir(),
            "the dedicated Cargo build directory is absent"
        );
        fs::remove_dir_all(root).expect("remove app-build fixture");
    }

    /// A `rustc` in `directory` that answers with one chosen report.
    ///
    /// The report is handed over verbatim, so a lane can offer the decision a
    /// release it does not pin or a host it cannot compile for -- including the
    /// owning host, which is the case a lane that is not the owning host could
    /// otherwise never reach.
    #[cfg(unix)]
    fn rustc_script(directory: &Path, report: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;

        fs::create_dir_all(directory).expect("create the fixture toolchain directory");
        let rustc = directory.join("rustc");
        fs::write(
            &rustc,
            format!("#!/bin/sh\ncat <<'REPORT'\n{}\nREPORT\n", report.trim_end()),
        )
        .expect("write a rustc answering with the chosen report");
        fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755))
            .expect("make the fixture rustc executable");
        rustc
    }

    /// A `PATH` whose `rustc` answers with one chosen report, and nothing else.
    #[cfg(unix)]
    fn rustc_reporting(root: &Path, report: &str) -> ReleaseEnvironment {
        rustc_script(root, report);
        ReleaseEnvironment::new(
            Some(root.to_path_buf().into_os_string()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
    }

    /// Which spelling names the compiler a build would run instead of `PATH`'s.
    ///
    /// Cargo reads both, and reads `RUSTC` first. Naming them here rather than
    /// taking a string keeps a fixture from asking about a spelling that does
    /// not exist and passing because nothing was overridden at all.
    #[cfg(unix)]
    enum CompilerOverride {
        Rustc,
        CargoBuildRustc,
    }

    /// A pinned `rustc` on `PATH`, and a foreign one named by an override.
    ///
    /// The `PATH` compiler reports the pinned release and the owning tuple, so
    /// it clears both doors on its own: with the override unread, admission
    /// passes and staging goes on to build. Every refusal reachable from here
    /// is therefore a refusal that read the override, and the only release it
    /// can name is the one only the override names.
    #[cfg(unix)]
    fn rustc_overridden_by(root: &Path, spelling: &CompilerOverride) -> ReleaseEnvironment {
        let on_path = root.join("path");
        rustc_script(
            &on_path,
            &format!(
                "rustc {release}\nrelease: {release}\nhost: {STAGING_HOST}\n",
                release = toolchain::pinned_release(),
            ),
        );
        let foreign = rustc_script(
            &root.join("overridden"),
            "rustc 9.9.9\nrelease: 9.9.9\nhost: x86_64-unknown-linux-gnu\n",
        )
        .into_os_string();
        let (rustc, build_rustc) = match spelling {
            CompilerOverride::Rustc => (Some(foreign), None),
            CompilerOverride::CargoBuildRustc => (None, Some(foreign)),
        };
        ReleaseEnvironment::new(
            Some(on_path.into_os_string()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            rustc,
            build_rustc,
        )
    }

    /// Staging refuses the compiler an override names, for either spelling.
    ///
    /// `PATH` is not where Cargo starts looking. `RUSTC` replaces the compiler
    /// outright and `CARGO_BUILD_RUSTC` does the same when `RUSTC` is absent,
    /// so a `PATH` walk on its own admits the release of a compiler that will
    /// never run and then stages bytes under one nothing pinned. Both spellings
    /// are driven, because a guard that reads one of them leaves the other
    /// exactly as open as it was.
    #[cfg(unix)]
    fn staging_refuses_the_compiler_an_override_names(label: &str, spelling: &CompilerOverride) {
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-app-overridden-{label}-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale fixture");
        }
        let environment = rustc_overridden_by(&root, spelling);

        let mut output: Vec<u8> = Vec::new();
        let error = stage_app(&environment, &mut output)
            .expect_err("a compiler this workspace does not pin may not write the stage");

        assert!(
            matches!(
                &error,
                AppBuildError::ForeignToolchainRelease(observed) if observed == "9.9.9"
            ),
            "staging measured the compiler on PATH rather than the one {label} names: {error}",
        );
        let refusal = error.to_string();
        for expected in [toolchain::pinned_release(), "9.9.9"] {
            assert!(
                refusal.contains(expected),
                "the refusal never names {expected}: {refusal}",
            );
        }
        assert!(
            output.is_empty(),
            "staging began before the release was admitted"
        );
        fs::remove_dir_all(root).expect("remove overridden toolchain fixture");
    }

    #[cfg(unix)]
    #[test]
    fn staging_refuses_the_compiler_rustc_names() {
        staging_refuses_the_compiler_an_override_names("RUSTC", &CompilerOverride::Rustc);
    }

    #[cfg(unix)]
    #[test]
    fn staging_refuses_the_compiler_cargo_build_rustc_names() {
        staging_refuses_the_compiler_an_override_names(
            "CARGO_BUILD_RUSTC",
            &CompilerOverride::CargoBuildRustc,
        );
    }

    /// Staging refuses the compiler a Cargo config file names, not only the
    /// two environment spellings.
    ///
    /// The Cargo home config is the one file on the app build's own resolution
    /// path a test may write: `run_trunk` hands Trunk the Cargo home this
    /// environment names and the resolver asks that same one, while the other
    /// two -- the workspace's tracked `.cargo/config.toml` and the config the
    /// sealed image lays at the filesystem root -- are files no fixture may
    /// edit. It is the same door regardless: `build.rustc` is one key and the
    /// walk that finds it is one walk.
    ///
    /// The compiler on `PATH` reports the pinned release and the owning tuple,
    /// so it clears both admission doors by itself. Any refusal reachable from
    /// here is therefore one that read the config, and 9.9.9 is a release only
    /// the config names -- with the file unread this call goes on to build.
    #[cfg(unix)]
    #[test]
    fn staging_refuses_the_compiler_a_cargo_config_names() {
        let root =
            std::env::temp_dir().join(format!("spice-xtask-app-configured-{}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale fixture");
        }
        rustc_script(
            &root.join("path"),
            &format!(
                "rustc {release}\nrelease: {release}\nhost: {STAGING_HOST}\n",
                release = toolchain::pinned_release(),
            ),
        );
        let foreign = rustc_script(
            &root.join("configured"),
            "rustc 9.9.9\nrelease: 9.9.9\nhost: x86_64-unknown-linux-gnu\n",
        );
        let cargo_home = root.join("cargo-home");
        fs::create_dir_all(&cargo_home).expect("fixture cargo home");
        fs::write(
            cargo_home.join("config.toml"),
            format!("[build]\nrustc = \"{}\"\n", foreign.display()),
        )
        .expect("fixture cargo config");
        let environment = ReleaseEnvironment::new(
            Some(root.join("path").into_os_string()),
            None,
            Some(cargo_home.into_os_string()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );

        let mut output: Vec<u8> = Vec::new();
        let error = stage_app(&environment, &mut output)
            .expect_err("a compiler this workspace does not pin may not write the stage");

        assert!(
            matches!(
                &error,
                AppBuildError::ForeignToolchainRelease(observed) if observed == "9.9.9"
            ),
            "staging measured the compiler on PATH rather than the one build.rustc names: {error}",
        );
        assert!(
            output.is_empty(),
            "staging began before the release was admitted"
        );
        fs::remove_dir_all(root).expect("remove configured toolchain fixture");
    }

    /// Toolchain observation roots Cargo config discovery at the app build.
    ///
    /// The configured compiler sits under `app/.cargo`, where a walk starting
    /// at the workspace root cannot see it, and it disagrees with a live
    /// compiler on `PATH`. The observed release therefore pins both the
    /// directory handed to [`ReleaseEnvironment::rustc`] and the fact that the
    /// config layer is asked before `PATH`.
    #[cfg(unix)]
    #[test]
    fn toolchain_observation_reads_the_app_builds_own_cargo_config() {
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-app-local-config-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale fixture");
        }
        let on_path = root.join("path");
        rustc_script(
            &on_path,
            &format!(
                "rustc {release}\nrelease: {release}\nhost: {STAGING_HOST}\n",
                release = toolchain::pinned_release(),
            ),
        );
        let configured = rustc_script(
            &root.join("configured"),
            "rustc 9.9.9\nrelease: 9.9.9\nhost: x86_64-unknown-linux-gnu\n",
        );
        let cargo = root.join("app/.cargo");
        fs::create_dir_all(&cargo).expect("fixture app Cargo directory");
        fs::write(
            cargo.join("config.toml"),
            format!("[build]\nrustc = \"{}\"\n", configured.display()),
        )
        .expect("fixture app Cargo config");
        let environment = ReleaseEnvironment::new(
            Some(on_path.into_os_string()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );

        let observed = observed_toolchain(&root, &environment)
            .expect("the compiler selected for the app build reports its identity");

        assert_eq!(
            (observed.release(), observed.host()),
            ("9.9.9", "x86_64-unknown-linux-gnu"),
            "toolchain observation did not read build.rustc from app/.cargo ahead of PATH",
        );
        fs::remove_dir_all(root).expect("remove app-local config fixture");
    }

    /// `RUSTC` is read ahead of `CARGO_BUILD_RUSTC`, which is Cargo's own order.
    ///
    /// Measured against the installed Cargo: with both set, the compiler
    /// `RUSTC` names is the one invoked. The two fixtures above cannot see that
    /// order, because with only one spelling ever set both orders answer
    /// identically -- an order is only decidable where the two answers differ.
    /// So they differ here, and the one `CARGO_BUILD_RUSTC` names clears both
    /// doors: asking it first would stage under a compiler that never runs.
    #[cfg(unix)]
    #[test]
    fn staging_reads_rustc_ahead_of_cargo_build_rustc() {
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-app-override-order-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale fixture");
        }
        let pinned = format!(
            "rustc {release}\nrelease: {release}\nhost: {STAGING_HOST}\n",
            release = toolchain::pinned_release(),
        );
        let on_path = root.join("path");
        rustc_script(&on_path, &pinned);
        let foreign = rustc_script(
            &root.join("rustc"),
            "rustc 9.9.9\nrelease: 9.9.9\nhost: x86_64-unknown-linux-gnu\n",
        );
        let admitted = rustc_script(&root.join("build-rustc"), &pinned);
        let environment = ReleaseEnvironment::new(
            Some(on_path.into_os_string()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(foreign.into_os_string()),
            Some(admitted.into_os_string()),
        );

        let mut output: Vec<u8> = Vec::new();
        let error = stage_app(&environment, &mut output)
            .expect_err("the compiler RUSTC names is the one that would run");

        assert!(
            matches!(
                &error,
                AppBuildError::ForeignToolchainRelease(observed) if observed == "9.9.9"
            ),
            "staging measured CARGO_BUILD_RUSTC while RUSTC outranks it: {error}",
        );
        assert!(
            output.is_empty(),
            "staging began before the release was admitted"
        );
        fs::remove_dir_all(root).expect("remove override order fixture");
    }

    /// Staging reads the release, and reads it through the door it ships.
    ///
    /// The host this reports is the owning one, so the release is the only
    /// thing left to refuse on and nothing else in `stage_app` can stand in for
    /// the refusal. That is the point -- delete the call and this fixture, whose
    /// report names the tuple that owns the stage, goes on to build and write
    /// it under a release nothing pinned, so the wiring is what fails here
    /// rather than only the rule.
    #[cfg(unix)]
    #[test]
    fn staging_refuses_a_toolchain_release_the_workspace_does_not_pin() {
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-app-foreign-release-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale fixture");
        }
        let environment = rustc_reporting(
            &root,
            "rustc 9.9.9\nrelease: 9.9.9\nhost: x86_64-unknown-linux-gnu\n",
        );

        let mut output: Vec<u8> = Vec::new();
        let error = stage_app(&environment, &mut output)
            .expect_err("a release this workspace does not pin may not write the stage");

        assert!(
            matches!(
                &error,
                AppBuildError::ForeignToolchainRelease(observed) if observed == "9.9.9"
            ),
            "staging answered with something other than the release refusal: {error}",
        );
        let refusal = error.to_string();
        for expected in [toolchain::pinned_release(), "9.9.9"] {
            assert!(
                refusal.contains(expected),
                "the refusal never names {expected}: {refusal}",
            );
        }
        assert!(
            output.is_empty(),
            "staging began before the release was admitted"
        );
        fs::remove_dir_all(root).expect("remove foreign toolchain fixture");
    }

    #[test]
    fn app_comparison_admits_only_release_owned_stage_directories() {
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-app-stage-inventory-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale fixture");
        }
        let expected = root.join("expected");
        let observed = root.join("observed");
        fs::create_dir_all(&expected).expect("create expected stage");
        fs::create_dir_all(&observed).expect("create observed stage");
        for name in APP_FILES {
            fs::write(expected.join(name), name).expect("write expected app file");
            fs::write(observed.join(name), name).expect("write observed app file");
        }
        for name in RELEASE_STAGE_DIRECTORIES {
            fs::create_dir(expected.join(name)).expect("create release stage directory");
        }

        compare_stage_inventory(&expected, &observed)
            .expect("release directories are separate ownership");

        fs::write(expected.join("unowned"), b"bytes").expect("write unowned stage entry");
        assert!(matches!(
            compare_stage_inventory(&expected, &observed),
            Err(AppBuildError::Inventory { .. })
        ));
        fs::remove_file(expected.join("unowned")).expect("remove unowned stage entry");
        fs::remove_dir(expected.join("objects")).expect("remove release stage directory");
        fs::write(expected.join("objects"), b"not a directory")
            .expect("replace release directory with file");
        assert!(matches!(
            compare_stage_inventory(&expected, &observed),
            Err(AppBuildError::Inventory { .. })
        ));

        fs::remove_dir_all(root).expect("remove fixture");
    }

    /// The two comparisons have to answer separately, because every host runs
    /// the first and only the staging host runs the second. A byte difference
    /// must therefore be invisible to the inventory question and fatal to the
    /// byte question; if the inventory check still noticed bytes, a foreign
    /// host would fail on exactly the difference it is excused from having.
    #[test]
    fn a_byte_difference_is_invisible_to_the_inventory_and_fatal_to_the_bytes() {
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-app-stage-bytes-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale fixture");
        }
        let expected = root.join("expected");
        let observed = root.join("observed");
        fs::create_dir_all(&expected).expect("create expected stage");
        fs::create_dir_all(&observed).expect("create observed stage");
        for name in APP_FILES {
            fs::write(expected.join(name), name).expect("write expected app file");
            fs::write(observed.join(name), name).expect("write observed app file");
        }

        compare_stage_bytes(&expected, &observed).expect("identical stages agree byte for byte");

        // One case per staged asset, so a comparison that only ever read the
        // first entry would be caught rather than pass on the rest.
        for name in APP_FILES {
            fs::write(observed.join(name), "foreign-metadata").expect("rewrite observed app file");

            compare_stage_inventory(&expected, &observed)
                .expect("differing bytes must not change the inventory answer");
            assert!(
                matches!(
                    compare_stage_bytes(&expected, &observed),
                    Err(AppBuildError::Drift(drifted)) if drifted == name
                ),
                "{name} drifted without being named as the drifted asset",
            );

            fs::write(observed.join(name), name).expect("restore observed app file");
        }

        fs::remove_dir_all(root).expect("remove fixture");
    }

    /// The refusal has to name both hosts, because the operator's next move
    /// depends on which environment they are missing, not on the fact that
    /// they are missing one. The observed one is spelled the way the compiler
    /// spells it -- a macOS lane's `rustc` reports `aarch64-apple-darwin`, not
    /// the tuple that lane was compiled for -- so what the refusal names is the
    /// string that was compared. It names the image too, because an operator on
    /// an x86_64 Linux box is missing an image and not an architecture, and the
    /// digest is the only thing they can act on.
    #[test]
    fn staging_from_a_foreign_host_refuses_and_names_both_environments() {
        let refusal =
            AppBuildError::ForeignStagingHost("aarch64-apple-darwin".to_owned()).to_string();

        assert!(
            refusal.contains("aarch64-apple-darwin") && refusal.contains(STAGING_HOST),
            "the refusal named neither the observed host nor the owning one: {refusal}",
        );
        assert!(
            refusal.contains(proof::DEFAULT_IMAGE),
            "the refusal named no image to stage from: {refusal}",
        );
    }

    /// An x86_64 Linux host that is not the image spells its arch and OS
    /// exactly as the image does, so ownership cannot be decided on those two
    /// fields. Each tuple below is separated from the owning one by a single
    /// field -- one that differs, one that is missing, one that is appended --
    /// so every field is load-bearing: drop any of them from [`STAGING_HOST`],
    /// or compare less of the string than all of it, and the neighbour that
    /// field alone separates is admitted here. None of these hosts is one this
    /// fleet compiles for, which is the point -- the decision takes a value, so
    /// a macOS lane can offer it the musl tuple that would otherwise be refused
    /// only in theory.
    #[test]
    fn only_the_owning_tuple_may_write_the_stage() {
        assert!(
            owns_the_stage("x86_64-unknown-linux-gnu"),
            "the owning tuple was refused, so no host at all could write the stage",
        );

        for foreign in [
            "aarch64-unknown-linux-gnu",
            "x86_64-apple-linux-gnu",
            "x86_64-unknown-macos-gnu",
            "x86_64-unknown-linux-musl",
            "x86_64-unknown-linux",
            "x86_64-unknown-linux-gnux32",
        ] {
            assert!(
                !owns_the_stage(foreign),
                "{foreign} is separated from {STAGING_HOST} by one field and was admitted \
                 anyway, so that field is not part of the decision",
            );
        }
    }

    /// The whole ownership decision, driven with tuples no lane compiles for.
    ///
    /// The tuple is a string the compiler hands over, so this asks the decision
    /// itself rather than a helper that spells this host. Every report names
    /// the pinned release, leaving the host as the only thing left to decide
    /// on, and the owning tuple is among them: that is the case no `cfg!`
    /// design could put in front of a macOS lane, where ownership could be
    /// refused but never granted, so the arm that admits a build was
    /// unreachable everywhere it could be tested. The others are the
    /// neighbours a comparison of less than the whole string would admit --
    /// musl differs in the environment field alone, `gnux32` only by an
    /// appended ABI, and the fourth stops one field short.
    #[cfg(unix)]
    #[test]
    fn the_ownership_decision_reads_the_host_its_compiler_reports() {
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-app-observed-host-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale fixture");
        }

        for (host, owns) in [
            ("x86_64-unknown-linux-gnu", true),
            ("x86_64-unknown-linux-musl", false),
            ("x86_64-unknown-linux-gnux32", false),
            ("x86_64-unknown-linux", false),
            ("aarch64-apple-darwin", false),
        ] {
            let environment = rustc_reporting(
                &root,
                &format!("release: {}\nhost: {host}\n", toolchain::pinned_release()),
            );

            let admitted = admit_staging_environment(&root, &environment);

            if owns {
                assert!(
                    admitted.is_ok(),
                    "a compiler reporting {host} under the pinned release was refused, so no \
                     environment at all could write the stage: {:?}",
                    admitted.err(),
                );
                continue;
            }
            let error =
                admitted.expect_err("a host that is not the owning tuple may not write the stage");
            assert!(
                matches!(&error, AppBuildError::ForeignStagingHost(observed) if observed == host),
                "{host} was refused as something other than a foreign staging host: {error}",
            );
        }

        fs::remove_dir_all(root).expect("remove fixture");
    }

    /// A compiler that names no host is refused, not filled in.
    ///
    /// The release is reported and matches, so the missing host line is all
    /// that is left to decide on. Supplying one from anywhere else -- what this
    /// binary was compiled for, most temptingly -- would let a compiler that
    /// will not say what it emits for write the bytes the stage is identified
    /// by, which is the substitution this whole decision exists to refuse.
    #[cfg(unix)]
    #[test]
    fn a_report_naming_no_host_refuses_rather_than_supplying_one() {
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-app-hostless-report-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale fixture");
        }
        let environment = rustc_reporting(
            &root,
            &format!("rustc {0}\nrelease: {0}\n", toolchain::pinned_release()),
        );

        let error = admit_staging_environment(&root, &environment)
            .expect_err("a compiler that names no host may not write the stage");

        assert!(
            matches!(
                &error,
                AppBuildError::UnreadableToolchainReport(reason) if reason.contains("named no host")
            ),
            "a report without a host line answered with something other than a refusal naming \
             the missing field: {error}",
        );
        fs::remove_dir_all(root).expect("remove fixture");
    }

    /// An environment naming no compiler at all is refused before anything is built.
    ///
    /// Both halves of the identity come from a compiler, so when there is no
    /// compiler to ask there is no question left to answer. This is the one
    /// refusal the report parser never sees, and without it an environment that
    /// names none would reach the build with neither half decided.
    ///
    /// There are four source spellings a compiler can be named by now, so the
    /// refusal is held to naming all four: an operator told only about `PATH`
    /// would go looking in the one place that was not the reason.
    #[test]
    fn an_environment_naming_no_compiler_is_refused() {
        let environment = ReleaseEnvironment::new(
            None, None, None, None, None, None, None, None, None, None, None,
        );

        let error = admit_staging_environment(Path::new("."), &environment)
            .expect_err("an environment with no rustc may not write the stage");

        assert!(
            matches!(
                &error,
                AppBuildError::UnreadableToolchainReport(reason) if reason.contains("no rustc")
            ),
            "an absent compiler answered with something other than an unreadable report: {error}",
        );
        let refusal = error.to_string();
        for named in ["RUSTC", "CARGO_BUILD_RUSTC", "build.rustc", "PATH"] {
            assert!(
                refusal.contains(named),
                "the refusal never names {named} as a place a compiler could have come from: \
                 {refusal}",
            );
        }
    }
}
