//! Native, lane-local compilation of the browser app.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::release::ReleaseEnvironment;
use wire::{Digest, Domain};

/// The target the browser app is compiled for.
pub(crate) const WASM_TARGET: &str = "wasm32-unknown-unknown";

const REMAPPED_CARGO_HOME: &str = "/cargo-home";
const GENERATED_MODULE_REFERENCES: usize = 2;
const GENERATED_REFERENCES: [(&str, usize); 3] =
    [("/app.module", 2), ("/app_bg.wasm", 2), ("/style.css", 1)];
static NEXT_BUILD: AtomicU64 = AtomicU64::new(0);

/// Run `cargo xtask local-app` to compile the browser app into the one stage
/// this worktree's resident serves.
///
/// The stage is ignored by Git and rebuilt by each lane. Compilation and
/// finalization happen outside the served path; one atomic pointer switch
/// publishes the complete immutable generation, so failure preserves the
/// previously active generation.
///
/// # Errors
///
/// Returns an unreadable workspace root, a stage that cannot be removed, a
/// missing Cargo home, Trunk execution failure, or a refused generated stage.
pub fn local_app(
    environment: &ReleaseEnvironment,
    output: &mut impl Write,
) -> Result<(), AppBuildError> {
    let root = workspace_root()?;
    local_app_at(&root, output, |stage, output| {
        run_trunk(&root, environment, stage, output)
    })
}

fn local_app_at<W: Write>(
    root: &Path,
    output: &mut W,
    build: impl FnOnce(&Path, &mut W) -> Result<(), AppBuildError>,
) -> Result<(), AppBuildError> {
    let stage = root.join(spice::canvas::LOCAL_STAGE);
    let temporary = root.join("dist").join(format!(
        ".stage-local-build-{}-{}",
        std::process::id(),
        NEXT_BUILD.fetch_add(1, Ordering::Relaxed)
    ));
    if temporary.exists() {
        fs::remove_dir_all(&temporary).map_err(|source| AppBuildError::Io {
            path: temporary.clone(),
            source,
        })?;
    }
    let result = (|| {
        build(&temporary, output)?;
        let identity = finalize_stage(&temporary)?;
        publish_generation(&stage, &temporary, &identity)?;
        writeln!(
            output,
            "[app] {} generation {identity} is ready for `spice up` in this worktree",
            stage.display()
        )
        .map_err(AppBuildError::Output)
    })();
    let cleanup = if temporary.exists() {
        fs::remove_dir_all(&temporary).map_err(|source| AppBuildError::Io {
            path: temporary,
            source,
        })
    } else {
        Ok(())
    };
    result.and(cleanup)
}

fn publish_generation(
    stage: &Path,
    temporary: &Path,
    identity: &Digest,
) -> Result<(), AppBuildError> {
    if stage.exists() && !stage.join(spice::canvas::LOCAL_STAGE_CURRENT).is_file() {
        fs::remove_dir_all(stage).map_err(|source| AppBuildError::Io {
            path: stage.to_path_buf(),
            source,
        })?;
    }
    let generations = stage.join(spice::canvas::LOCAL_STAGE_GENERATIONS);
    fs::create_dir_all(&generations).map_err(|source| AppBuildError::Io {
        path: generations.clone(),
        source,
    })?;
    let generation = generations.join(identity.as_str());
    if generation.exists() {
        if !same_generation(temporary, &generation)? {
            return Err(AppBuildError::GeneratedStage(format!(
                "generation {identity} already exists with different bytes"
            )));
        }
        fs::remove_dir_all(temporary).map_err(|source| AppBuildError::Io {
            path: temporary.to_path_buf(),
            source,
        })?;
    } else {
        fs::rename(temporary, &generation).map_err(|source| AppBuildError::Io {
            path: temporary.to_path_buf(),
            source,
        })?;
    }

    let pointer = stage.join(spice::canvas::LOCAL_STAGE_CURRENT);
    let destination = atomic::Destination::new(&pointer).map_err(AppBuildError::Atomic)?;
    let mut staged = destination.stage().map_err(AppBuildError::Atomic)?;
    writeln!(staged, "{identity}").map_err(|source| AppBuildError::Io {
        path: pointer,
        source,
    })?;
    staged.commit().map_err(AppBuildError::Atomic)
}

fn same_generation(left: &Path, right: &Path) -> Result<bool, AppBuildError> {
    for asset in spice::canvas::APP.assets() {
        if read_regular(&left.join(asset.file()))? != read_regular(&right.join(asset.file()))? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn read_regular(path: &Path) -> Result<Vec<u8>, AppBuildError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| AppBuildError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_file() {
        return Err(AppBuildError::GeneratedStage(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    fs::read(path).map_err(|source| AppBuildError::Io {
        path: path.to_path_buf(),
        source,
    })
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
    Ok(())
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

fn finalize_stage(stage: &Path) -> Result<Digest, AppBuildError> {
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
    let generated_modules = html.matches("/app.js").count();
    if generated_modules != GENERATED_MODULE_REFERENCES || html.contains("/app.module") {
        return Err(AppBuildError::GeneratedStage(format!(
            "canvas-app.html carried {generated_modules} app.js references and preexisting \
             app.module={}, expected {GENERATED_MODULE_REFERENCES} app.js references only",
            html.contains("/app.module")
        )));
    }
    let html = html.replace("/app.js", "/app.module");
    for (reference, expected) in GENERATED_REFERENCES {
        let observed = html.matches(reference).count();
        if observed != expected {
            return Err(AppBuildError::GeneratedStage(format!(
                "canvas-app.html carried {observed} {reference} references, expected {expected}"
            )));
        }
    }
    fs::write(&html_path, &html).map_err(|source| AppBuildError::Io {
        path: html_path.clone(),
        source,
    })?;
    validate_inventory(stage)?;
    let identity = generation_digest(stage)?;
    let mut qualified = html;
    for asset in spice::canvas::APP
        .assets()
        .iter()
        .filter(|asset| asset.path() != "/")
    {
        let route = format!(
            "{}/{identity}/{}",
            spice::canvas::APP_GENERATION_ROUTE,
            asset.file()
        );
        qualified = qualified.replace(asset.path(), &route);
    }
    validate_qualified_references(&qualified, &identity)?;
    fs::write(&html_path, qualified).map_err(|source| AppBuildError::Io {
        path: html_path,
        source,
    })?;
    validate_inventory(stage)?;
    Ok(identity)
}

fn validate_qualified_references(html: &str, identity: &Digest) -> Result<(), AppBuildError> {
    for asset in spice::canvas::APP
        .assets()
        .iter()
        .filter(|asset| asset.path() != "/")
    {
        let expected = GENERATED_REFERENCES
            .iter()
            .find_map(|(reference, expected)| (*reference == asset.path()).then_some(*expected))
            .expect("every non-root app asset has one generated reference count");
        let route = format!(
            "{}/{identity}/{}",
            spice::canvas::APP_GENERATION_ROUTE,
            asset.file()
        );
        let qualified = html.matches(&route).count();
        let total = html.matches(asset.path()).count();
        if qualified != expected || total != expected {
            return Err(AppBuildError::GeneratedStage(format!(
                "rewritten canvas-app.html carried {qualified} qualified and {total} total {} \
                 references, expected {expected}",
                asset.path()
            )));
        }
    }
    Ok(())
}

fn validate_inventory(stage: &Path) -> Result<(), AppBuildError> {
    let mut observed = fs::read_dir(stage)
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
                    let path = entry.path();
                    let metadata =
                        fs::symlink_metadata(&path).map_err(|source| AppBuildError::Io {
                            path: path.clone(),
                            source,
                        })?;
                    if !metadata.file_type().is_file() {
                        return Err(AppBuildError::GeneratedStage(format!(
                            "{} is not a regular generation file",
                            path.display()
                        )));
                    }
                    entry.file_name().into_string().map_err(|_| {
                        AppBuildError::GeneratedStage(format!(
                            "{} contains a non-UTF-8 file name",
                            stage.display()
                        ))
                    })
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    observed.sort();
    let mut expected = spice::canvas::APP
        .assets()
        .iter()
        .map(|asset| asset.file().to_owned())
        .collect::<Vec<_>>();
    expected.sort();
    if observed != expected {
        return Err(AppBuildError::GeneratedStage(format!(
            "generated inventory {observed:?}, expected {expected:?}"
        )));
    }
    Ok(())
}

fn generation_digest(stage: &Path) -> Result<Digest, AppBuildError> {
    let mut framed = Vec::new();
    for asset in spice::canvas::APP.assets() {
        let file = asset.file().as_bytes();
        let bytes = read_regular(&stage.join(asset.file()))?;
        framed.extend_from_slice(&(file.len() as u64).to_be_bytes());
        framed.extend_from_slice(file);
        framed.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        framed.extend_from_slice(&bytes);
    }
    Ok(Digest::compute(Domain::DistArtifact, &framed))
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
    Io { path: PathBuf, source: io::Error },
    NonUtf8(PathBuf),
    MissingCargoHome,
    GeneratedStage(String),
    Atomic(atomic::Error),
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
            Self::GeneratedStage(reason) => {
                write!(formatter, "generated app stage refused: {reason}")
            }
            Self::Atomic(error) => write!(formatter, "app generation publication failed: {error}"),
        }
    }
}

impl std::error::Error for AppBuildError {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::mpsc;

    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-{name}-{}-{}",
            std::process::id(),
            NEXT_BUILD.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("create fixture root");
        root
    }

    fn write_raw_generation(stage: &Path, label: &str, valid: bool) {
        fs::create_dir_all(stage).expect("create raw generation");
        fs::write(stage.join("app.js"), format!("module {label}")).expect("write generated module");
        fs::write(stage.join("app_bg.wasm"), format!("wasm {label}")).expect("write wasm");
        fs::write(stage.join("style.css"), format!("style {label}")).expect("write stylesheet");
        let second_wasm = if valid {
            "<link href=\"/app_bg.wasm\">"
        } else {
            ""
        };
        fs::write(
            stage.join("canvas-app.html"),
            format!(
                "<link href=\"/style.css\"><script src=\"/app.js\"></script>\
                 <link href=\"/app.js\"><link href=\"/app_bg.wasm\">{second_wasm}\
                 <main>{label}</main>"
            ),
        )
        .expect("write shell");
    }

    fn publish_fixture(root: &Path, label: &str) {
        local_app_at(root, &mut Vec::new(), |stage, _| {
            write_raw_generation(stage, label, true);
            Ok(())
        })
        .expect("fixture generation publishes");
    }

    #[test]
    fn app_build_command_owns_cache_and_wrapper_inputs() {
        let command = trunk_command(
            Path::new("/workspace"),
            Path::new("/cargo-target"),
            OsStr::new("/cargo-home"),
            Path::new("/stage"),
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
        assert_eq!(environment.len(), 8);
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
    fn finalization_produces_the_closed_lane_stage_inventory() {
        let root = fixture("lane-app-finalize");
        write_raw_generation(&root, "first", true);

        let identity = finalize_stage(&root).expect("generated stage finalizes");

        let mut names = fs::read_dir(&root)
            .expect("read stage")
            .map(|entry| entry.expect("read entry").file_name())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            ["app.module", "app_bg.wasm", "canvas-app.html", "style.css"]
                .map(std::ffi::OsString::from)
        );
        let shell = fs::read_to_string(root.join("canvas-app.html")).expect("read shell");
        for asset in spice::canvas::APP
            .assets()
            .iter()
            .filter(|asset| asset.path() != "/")
        {
            assert!(
                shell.contains(&format!(
                    "{}/{identity}/{}",
                    spice::canvas::APP_GENERATION_ROUTE,
                    asset.file()
                )),
                "the shell did not pin {} to {identity}: {shell}",
                asset.file()
            );
        }
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn publication_switches_only_after_an_event_releases_the_complete_generation() {
        let root = fixture("lane-app-event");
        publish_fixture(&root, "first");
        let source = spice::canvas::AppSource::resolve(&root).expect("first generation resolves");
        let first = source
            .current_generation()
            .expect("first generation remains current");
        let module = spice::canvas::APP
            .asset("/app.module")
            .expect("module belongs to the closed inventory");
        let first_route = first.route(module);
        let (started_tx, started_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);

        std::thread::scope(|scope| {
            let build_root = root.clone();
            let build = scope.spawn(move || {
                local_app_at(&build_root, &mut Vec::new(), |stage, _| {
                    started_tx.send(()).expect("announce build barrier");
                    release_rx.recv().expect("release build barrier");
                    write_raw_generation(stage, "second", true);
                    Ok(())
                })
            });

            started_rx.recv().expect("build reaches its event barrier");
            assert_eq!(
                source
                    .served(&first_route)
                    .expect("first generation remains readable")
                    .expect("first route remains admitted")
                    .into_bytes(),
                b"module first"
            );
            assert_eq!(
                source
                    .current_generation()
                    .expect("the pointer remains on the first generation")
                    .identity(),
                first.identity()
            );
            release_tx.send(()).expect("release the complete build");
            build
                .join()
                .expect("build thread joins")
                .expect("second generation publishes");
        });

        let second = source
            .current_generation()
            .expect("second generation becomes current");
        assert_ne!(second.identity(), first.identity());
        assert_eq!(
            source
                .served(&first_route)
                .expect("first page generation remains readable")
                .expect("first page route remains admitted")
                .into_bytes(),
            b"module first",
            "the active pointer changed an in-flight page's generation-qualified route"
        );
        let shell = source
            .served("/")
            .expect("new shell is readable")
            .expect("root route remains admitted")
            .into_bytes();
        assert!(
            String::from_utf8(shell)
                .expect("fixture shell is UTF-8")
                .contains(second.identity().as_str()),
            "the new root did not pin its dependent routes to the new generation"
        );
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn a_failed_finalization_preserves_the_prior_pointer_or_first_build_absence() {
        let root = fixture("lane-app-failure");
        publish_fixture(&root, "first");
        let source = spice::canvas::AppSource::resolve(&root).expect("first generation resolves");
        let first = source
            .current_generation()
            .expect("first generation is current")
            .identity()
            .clone();

        let error = local_app_at(&root, &mut Vec::new(), |stage, _| {
            write_raw_generation(stage, "broken", false);
            Ok(())
        })
        .expect_err("invalid finalization refuses");
        assert!(error.to_string().contains("app_bg.wasm"));
        assert_eq!(
            source
                .current_generation()
                .expect("failed rebuild preserves current")
                .identity(),
            &first
        );

        let fresh = fixture("lane-app-first-failure");
        local_app_at(&fresh, &mut Vec::new(), |stage, _| {
            write_raw_generation(stage, "broken", false);
            Ok(())
        })
        .expect_err("first invalid finalization refuses");
        assert!(
            !fresh.join(spice::canvas::LOCAL_STAGE).exists(),
            "a failed first build published a visible stage"
        );
        fs::remove_dir_all(root).expect("remove fixture");
        fs::remove_dir_all(fresh).expect("remove fresh fixture");
    }
}
