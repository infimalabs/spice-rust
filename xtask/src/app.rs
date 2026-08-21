//! Native, worktree-local compilation of the browser app.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::release::ReleaseEnvironment;
use wire::{Digest, Domain};

mod source;

const REMAPPED_CARGO_HOME: &str = "/cargo-home";
const APP_SOURCE_IDENTITY_ENV: &str = "SPICE_APP_SOURCE_IDENTITY"; // env-policy: allow
const GENERATED_MODULE_REFERENCES: usize = 2;
const GENERATED_REFERENCES: [(&str, usize); 5] = [
    ("/app.module", 2),
    ("/app_bg.wasm", 2),
    ("/style.css", 1),
    ("/mosaic.css", 1),
    ("/responsive.css", 1),
];
static NEXT_BUILD: AtomicU64 = AtomicU64::new(0);

/// Run `cargo xtask app` to compile the browser app into the one stage
/// this worktree's resident serves.
///
/// The stage is ignored by Git and rebuilt by each worktree. Compilation and
/// finalization happen outside the served path; one atomic pointer switch
/// publishes the complete immutable generation, so failure preserves the
/// previously active generation.
///
/// # Errors
///
/// Returns an unreadable workspace root, a stage that cannot be removed, a
/// missing Cargo home, Trunk execution failure, or a refused generated stage.
pub fn app(environment: &ReleaseEnvironment, output: &mut impl Write) -> Result<(), AppBuildError> {
    let root = workspace_root()?;
    app_from(&root, &root, environment, output)
}

/// Refuse the retired cross-worktree verifier app publication path.
///
/// The function remains as a compatibility boundary for the former hidden
/// command spelling; it performs no source capture, compilation, or write.
///
/// # Errors
///
/// Always returns [`AppBuildError::VerifierPublicationRetired`].
pub fn app_to(
    _environment: &ReleaseEnvironment,
    _publish_root: &Path,
    _output: &mut impl Write,
) -> Result<(), AppBuildError> {
    Err(AppBuildError::VerifierPublicationRetired)
}

fn app_from(
    root: &Path,
    publish_root: &Path,
    environment: &ReleaseEnvironment,
    output: &mut impl Write,
) -> Result<(), AppBuildError> {
    let sources = source::SourceSnapshot::capture(root)?;
    let source_identity = sources.identity().clone();
    app_build_at(
        root,
        publish_root,
        &source_identity,
        output,
        |stage, output| {
            run_trunk(
                root,
                publish_root,
                environment,
                stage,
                &source_identity,
                output,
            )
        },
        || sources.verify_unchanged(root),
    )
}

#[cfg(test)]
fn app_at<W: Write>(
    root: &Path,
    source_identity: &Digest,
    output: &mut W,
    build: impl FnOnce(&Path, &mut W) -> Result<(), AppBuildError>,
    verify_sources: impl FnOnce() -> Result<(), AppBuildError>,
) -> Result<(), AppBuildError> {
    app_build_at(root, root, source_identity, output, build, verify_sources)
}

fn app_build_at<W: Write>(
    source_root: &Path,
    publish_root: &Path,
    source_identity: &Digest,
    output: &mut W,
    build: impl FnOnce(&Path, &mut W) -> Result<(), AppBuildError>,
    verify_sources: impl FnOnce() -> Result<(), AppBuildError>,
) -> Result<(), AppBuildError> {
    let stage = publish_root.join(spice::canvas::APP_STAGE);
    let temporary = publish_root.join("dist").join(format!(
        ".stage-app-build-{}-{}",
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
        writeln!(output, "{}", gate::VERIFIER_APP_COMPILER_READY_RECORD)
            .map_err(AppBuildError::Output)?;
        output.flush().map_err(AppBuildError::Output)?;
        verify_sources()?;
        let identity = finalize_stage(&temporary, source_identity)?;
        publish_generation(&stage, &temporary, &identity)?;
        writeln!(
            output,
            "[app] {} generation {identity} from selected sources {} ({source_identity}) is ready \
             for `spice up` in this worktree",
            stage.display(),
            source_root.display(),
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
    if stage.exists() && !stage.join(spice::canvas::APP_STAGE_CURRENT).is_file() {
        fs::remove_dir_all(stage).map_err(|source| AppBuildError::Io {
            path: stage.to_path_buf(),
            source,
        })?;
    }
    let generations = stage.join(spice::canvas::APP_STAGE_GENERATIONS);
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

    let pointer = stage.join(spice::canvas::APP_STAGE_CURRENT);
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
    cache_identity_root: &Path,
    environment: &ReleaseEnvironment,
    stage: &Path,
    source_identity: &Digest,
    output: &mut impl Write,
) -> Result<(), AppBuildError> {
    let build_root = environment.build_root(root);
    let worktree_identity = worktree_identity(cache_identity_root)?;
    let cargo_home = environment
        .cargo_home()
        .ok_or(AppBuildError::MissingCargoHome)?;
    prepare_app_build(&build_root, &worktree_identity)?;
    writeln!(output, "[app] trunk 0.21.14 -> {}", stage.display())
        .map_err(AppBuildError::Output)?;
    let rustflags = remapped_rustflags(root, &cargo_home)?;
    let status = trunk_command(
        root,
        &build_root,
        &cargo_home,
        stage,
        rustflags,
        &worktree_identity,
        source_identity,
    )
    .status()
    .map_err(AppBuildError::Launch)?;
    if !status.success() {
        return Err(AppBuildError::Exit(status.code()));
    }
    Ok(())
}

fn app_build_namespace(worktree_identity: &Digest) -> String {
    format!("xtask-app-{worktree_identity}")
}

fn app_build_directory(build_root: &Path, worktree_identity: &Digest) -> PathBuf {
    build_root.join(format!("{}-build", app_build_namespace(worktree_identity)))
}

fn app_target_directory(build_root: &Path, worktree_identity: &Digest) -> PathBuf {
    build_root.join(app_build_namespace(worktree_identity))
}

fn prepare_app_build(build_root: &Path, worktree_identity: &Digest) -> Result<(), AppBuildError> {
    let build = app_build_directory(build_root, worktree_identity);
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
    worktree_identity: &Digest,
    source_identity: &Digest,
) -> Command {
    let target = app_target_directory(build_root, worktree_identity);
    let build = app_build_directory(build_root, worktree_identity);
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
        .env(APP_SOURCE_IDENTITY_ENV, source_identity.as_str())
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

fn finalize_stage(stage: &Path, source_identity: &Digest) -> Result<Digest, AppBuildError> {
    validate_wasm_source(stage, source_identity)?;
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

fn validate_wasm_source(stage: &Path, source_identity: &Digest) -> Result<(), AppBuildError> {
    let wasm = read_regular(&stage.join("app_bg.wasm"))?;
    if wasm
        .windows(source_identity.as_str().len())
        .any(|window| window == source_identity.as_str().as_bytes())
    {
        return Ok(());
    }
    Err(AppBuildError::GeneratedStage(format!(
        "app_bg.wasm does not carry source identity {source_identity}; refusing a complete but \
         foreign generation"
    )))
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

fn worktree_identity(root: &Path) -> Result<Digest, AppBuildError> {
    let canonical = fs::canonicalize(root).map_err(|source| AppBuildError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let spelling = canonical
        .to_str()
        .ok_or_else(|| AppBuildError::NonUtf8(canonical.clone()))?;
    Ok(Digest::compute(Domain::DistInventory, spelling.as_bytes()))
}

fn workspace_root() -> Result<PathBuf, AppBuildError> {
    std::env::current_dir().map_err(|source| AppBuildError::Io {
        path: PathBuf::from("."),
        source,
    })
}

#[derive(Debug)]
pub enum AppBuildError {
    VerifierPublicationRetired,
    Output(io::Error),
    Launch(io::Error),
    Exit(Option<i32>),
    Io { path: PathBuf, source: io::Error },
    NonUtf8(PathBuf),
    MissingCargoHome,
    SourceIdentity(String),
    GeneratedStage(String),
    Atomic(atomic::Error),
}

impl fmt::Display for AppBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VerifierPublicationRetired => formatter.write_str(
                "verifier app publication is retired; use `cargo xtask app` in the target worktree",
            ),
            Self::Output(error) => write!(formatter, "app build output failed: {error}"),
            Self::Launch(error) => write!(formatter, "trunk did not launch: {error}"),
            Self::Exit(Some(code)) => write!(formatter, "trunk failed with exit {code}"),
            Self::Exit(None) => formatter.write_str("trunk was terminated by a signal"),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::NonUtf8(path) => write!(formatter, "stage path is not UTF-8: {}", path.display()),
            Self::MissingCargoHome => formatter.write_str(
                "the app build needs CARGO_HOME or HOME to locate the crates it compiles",
            ),
            Self::SourceIdentity(reason) => {
                write!(formatter, "app source identity refused: {reason}")
            }
            Self::GeneratedStage(reason) => {
                write!(formatter, "generated app stage refused: {reason}")
            }
            Self::Atomic(error) => write!(formatter, "app generation publication failed: {error}"),
        }
    }
}

impl std::error::Error for AppBuildError {}

#[cfg(test)]
mod tests;
