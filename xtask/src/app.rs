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
use crate::stage_sources::{STAGE_SOURCE_LISTING, compare_stage_sources, record_stage_sources};
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
/// release that same report names is the other half of this identity, and
/// [`toolchain`] owns it.
const STAGING_HOST: &str = "x86_64-unknown-linux-gnu";

/// Whether a host tuple owns the app stage.
///
/// This is the tuple half of what decides ownership, and it admits one tuple or
/// none; [`toolchain::pinned_release`] is the release half, and a host passing
/// this one still refuses under any other release. The decision reads a spelled
/// tuple rather than a `cfg!` predicate over the same four fields, because a
/// predicate answers only for the target its binary was compiled for: dropping
/// a field from one fails nothing anywhere this fleet runs, since a foreign
/// host already disagrees in every field and the owning host still agrees in
/// every remaining one. Given a value, a test can offer this the musl
/// neighbour no lane compiles for.
fn owns_the_stage(host: &str) -> bool {
    host == STAGING_HOST
}

/// Whether this program was compiled for the tuple [`STAGING_HOST`] names.
fn compiled_for_staging_host() -> bool {
    owns_the_stage(&host_spelling())
}

/// This host, spelled in the four fields ownership is decided on.
///
/// A triple's vendor and environment fields are observable after all --
/// `std::env::consts` simply stops at arch and OS, and stopping there spells a
/// musl host and the image's gnu host identically while their `-Cmetadata`
/// differs. This is the sole reading of those fields, and [`owns_the_stage`]
/// decides on what it returns, so the tuple a refusal names is the tuple that
/// was compared. A field this cannot spell exactly becomes `other`, which no
/// owning tuple matches, so an unrecognised host refuses rather than resembling
/// the image.
fn host_spelling() -> String {
    let vendor = if cfg!(target_vendor = "apple") {
        "apple"
    } else if cfg!(target_vendor = "unknown") {
        "unknown"
    } else {
        "other"
    };
    let environment = if cfg!(target_env = "gnu") {
        "gnu"
    } else if cfg!(target_env = "musl") {
        "musl"
    } else if cfg!(target_env = "") {
        "none"
    } else {
        "other"
    };
    format!(
        "{}-{vendor}-{}-{environment}",
        std::env::consts::ARCH,
        std::env::consts::OS,
    )
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
/// Returns an unreadable or unpinned toolchain release, a foreign staging host,
/// a missing Cargo home, Trunk execution failure, a refused generated stage, or
/// an unreadable build input.
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
/// Both dimensions Cargo folds into `-Cmetadata` are asked here, and the
/// release is asked first deliberately. The tuple is a fact about this binary,
/// settled when it was compiled, so on every host but the owning one it answers
/// before anything else can be observed -- and a door that only the owning host
/// can reach is a door no test anywhere else can prove is still there. Asking
/// the environment first puts the release refusal in reach of every host, and a
/// foreign host still refuses on the tuple immediately after.
fn admit_staging_environment(
    root: &Path,
    environment: &ReleaseEnvironment,
) -> Result<(), AppBuildError> {
    let rustc = environment.rustc().ok_or_else(|| {
        AppBuildError::UnreadableToolchainRelease(
            "PATH names no rustc, so the release a build here would compile under is unknown"
                .to_owned(),
        )
    })?;
    let observed = toolchain::observed_release(&rustc, root)?;
    if observed != toolchain::pinned_release() {
        return Err(AppBuildError::ForeignToolchainRelease(observed));
    }
    if !compiled_for_staging_host() {
        return Err(AppBuildError::ForeignStagingHost(host_spelling()));
    }
    Ok(())
}

/// Rebuild the browser app outside the committed stage and compare it back.
///
/// Every host first asks whether the stage was built from the sources now in
/// the tree, which is answered from recorded input identity and needs no
/// rebuild, then rebuilds and compares the staged inventory. Only the staging
/// host goes on to compare bytes, because only its compiler metadata can
/// reproduce them.
///
/// # Errors
///
/// Returns build inputs the recorded stage identity no longer describes, a
/// missing Cargo home, Trunk execution failure, an inventory that differs from
/// the rebuild, or, on the staging host, the first staged asset whose bytes
/// differ.
pub fn check_app(
    environment: &ReleaseEnvironment,
    output: &mut impl Write,
) -> Result<(), AppBuildError> {
    let root = workspace_root()?;
    compare_stage_sources(&root)?;
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
        if compiled_for_staging_host() {
            return compare_stage_bytes(&stage, &temporary);
        }
        let host = host_spelling();
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
        // Cargo exports `CARGO` as the absolute path of the binary running
        // this command, and Trunk builds with that binary rather than the one
        // the environment puts on `PATH`. In the sealed image those are not
        // the same program: `PATH` names a wrapper that supplies the vendored
        // source replacement, so an inherited `CARGO` reaches past it to a
        // Cargo with no offline sources and the build fails resolving crates
        // that are sitting right there. Dropped for the same reason the
        // wrappers below are: the app build chooses its own toolchain.
        .env_remove("CARGO")
        .env("CARGO_BUILD_BUILD_DIR", build)
        .env("CARGO_ENCODED_RUSTFLAGS", rustflags)
        .env("CARGO_HOME", cargo_home)
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", target)
        .env("NO_COLOR", "true")
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
    UnreadableToolchainRelease(String),
    ForeignToolchainRelease(String),
    ForeignStagingHost(String),
    GeneratedStage(String),
    Inventory {
        expected: Vec<String>,
        observed: Vec<String>,
    },
    Drift(String),
    SourceInput(PathBuf),
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
            Self::UnreadableToolchainRelease(reason) => write!(
                formatter,
                "the toolchain release this build would compile under could not be read: \
                 {reason}"
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
                "a staged app build input is neither a regular file nor a directory: {}",
                path.display()
            ),
            Self::StaleStage(paths) => write!(
                formatter,
                "the committed app stage was not built from the sources now in the tree: \
                 {} changed since {STAGE_SOURCE_LISTING} recorded them; restage with \
                 cargo xtask stage-app in image {}, which is the one environment allowed \
                 to write the stage",
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
                // Removed, not emptied: Trunk falls back to `PATH` only when
                // `CARGO` is absent, and an empty value would name a program
                // that does not exist.
                ("CARGO".to_owned(), None),
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

    /// Staging reads the release, and reads it through the door it ships.
    ///
    /// The tuple half of the identity is settled for this binary -- a test can
    /// hand [`owns_the_stage`] any tuple, but none can make this process the
    /// owning host -- so this arranges the half that can be reached from here:
    /// a `PATH` whose `rustc` reports a release this workspace does not pin.
    /// Nothing else in `stage_app` can stand in for the refusal, which is the
    /// point -- delete the call and a foreign host answers with the tuple
    /// instead, while the owning host goes on to build, so the wiring is what
    /// fails here rather than only the rule.
    #[cfg(unix)]
    #[test]
    fn staging_refuses_a_toolchain_release_the_workspace_does_not_pin() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = std::env::temp_dir().join(format!(
            "spice-xtask-app-foreign-release-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale fixture");
        }
        fs::create_dir_all(&root).expect("create foreign toolchain fixture");
        let rustc = root.join("rustc");
        fs::write(
            &rustc,
            "#!/bin/sh\nprintf 'rustc 9.9.9\\nrelease: 9.9.9\\n\
             host: x86_64-unknown-linux-gnu\\n'\n",
        )
        .expect("write a rustc reporting an unpinned release");
        fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755))
            .expect("make the fixture rustc executable");
        let environment = ReleaseEnvironment::new(
            Some(root.clone().into_os_string()),
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
    /// they are missing one. It names the image too, because an operator on an
    /// x86_64 Linux box is missing an image and not an architecture, and the
    /// digest is the only thing they can act on.
    #[test]
    fn staging_from_a_foreign_host_refuses_and_names_both_environments() {
        let refusal =
            AppBuildError::ForeignStagingHost("aarch64-apple-macos-none".to_owned()).to_string();

        assert!(
            refusal.contains("aarch64-apple-macos-none") && refusal.contains(STAGING_HOST),
            "the refusal named neither the observed host nor the owning one: {refusal}",
        );
        assert!(
            refusal.contains(proof::DEFAULT_IMAGE),
            "the refusal named no image to stage from: {refusal}",
        );
    }

    /// An x86_64 Linux host that is not the image spells its arch and OS
    /// exactly as the image does, so ownership cannot be decided on those two
    /// fields. Each tuple below differs from the owning one in exactly one
    /// field, so every field is load-bearing: drop any of them from
    /// [`STAGING_HOST`] and the neighbour that field alone separates is
    /// admitted here. None of these hosts is one this fleet compiles for, which
    /// is the point -- the decision takes a value, so a macOS lane can offer it
    /// the musl tuple that would otherwise be refused only in theory.
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
        ] {
            assert!(
                !owns_the_stage(foreign),
                "{foreign} differs from {STAGING_HOST} in one field and was admitted anyway, so \
                 that field is not part of the decision",
            );
        }
    }

    /// The decision is only as wide as the tuple it is handed. A spelling that
    /// stopped early would offer a musl host the same four-field string the
    /// image spells, so the count is held here rather than left to the fields
    /// this lane happens to be compiled for -- on macOS every field disagrees
    /// already, and a dropped one would not change the answer.
    #[test]
    fn this_host_is_spelled_in_every_field_the_decision_reads() {
        let spelling = host_spelling();

        assert_eq!(
            spelling.split('-').count(),
            STAGING_HOST.split('-').count(),
            "this host is spelled {spelling}, in fewer fields than {STAGING_HOST}, so a host \
             differing from the owning one only in a dropped field would spell it exactly",
        );
    }
}
