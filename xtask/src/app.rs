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

/// The target the browser app is compiled for.
pub(crate) const WASM_TARGET: &str = "wasm32-unknown-unknown";

const REMAPPED_CARGO_HOME: &str = "/cargo-home";
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
    let sources = source::SourceSnapshot::capture(&root)?;
    let source_identity = sources.identity().clone();
    app_at(
        &root,
        &source_identity,
        output,
        |stage, output| run_trunk(&root, environment, stage, &source_identity, output),
        || sources.verify_unchanged(&root),
    )
}

fn app_at<W: Write>(
    root: &Path,
    source_identity: &Digest,
    output: &mut W,
    build: impl FnOnce(&Path, &mut W) -> Result<(), AppBuildError>,
    verify_sources: impl FnOnce() -> Result<(), AppBuildError>,
) -> Result<(), AppBuildError> {
    let stage = root.join(spice::canvas::APP_STAGE);
    let temporary = root.join("dist").join(format!(
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
        verify_sources()?;
        let identity = finalize_stage(&temporary, source_identity)?;
        publish_generation(&stage, &temporary, &identity)?;
        writeln!(
            output,
            "[app] {} generation {identity} from sources {source_identity} is ready for `spice up` \
             in this worktree",
            stage.display(),
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
    environment: &ReleaseEnvironment,
    stage: &Path,
    source_identity: &Digest,
    output: &mut impl Write,
) -> Result<(), AppBuildError> {
    let build_root = environment.build_root(root);
    let worktree_identity = worktree_identity(root)?;
    let cargo_home = environment
        .cargo_home()
        .ok_or(AppBuildError::MissingCargoHome)?;
    prepare_app_build(&build_root, &worktree_identity, source_identity)?;
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

fn app_build_namespace(worktree_identity: &Digest, source_identity: &Digest) -> String {
    format!("xtask-app-{worktree_identity}-{source_identity}")
}

fn app_build_directory(
    build_root: &Path,
    worktree_identity: &Digest,
    source_identity: &Digest,
) -> PathBuf {
    build_root.join(format!(
        "{}-build",
        app_build_namespace(worktree_identity, source_identity)
    ))
}

fn app_target_directory(
    build_root: &Path,
    worktree_identity: &Digest,
    source_identity: &Digest,
) -> PathBuf {
    build_root.join(app_build_namespace(worktree_identity, source_identity))
}

fn prepare_app_build(
    build_root: &Path,
    worktree_identity: &Digest,
    source_identity: &Digest,
) -> Result<(), AppBuildError> {
    let build = app_build_directory(build_root, worktree_identity, source_identity);
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
    let target = app_target_directory(build_root, worktree_identity, source_identity);
    let build = app_build_directory(build_root, worktree_identity, source_identity);
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
        .env("SPICE_APP_SOURCE_IDENTITY", source_identity.as_str())
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

    fn fixture_source() -> Digest {
        Digest::compute(Domain::DistInventory, b"fixture app sources")
    }

    fn write_raw_generation(stage: &Path, label: &str, source_identity: &Digest, valid: bool) {
        fs::create_dir_all(stage).expect("create raw generation");
        fs::write(stage.join("app.js"), format!("module {label}")).expect("write generated module");
        fs::write(
            stage.join("app_bg.wasm"),
            format!("wasm {source_identity} {label}"),
        )
        .expect("write wasm");
        fs::write(stage.join("style.css"), format!("style {label}")).expect("write stylesheet");
        fs::write(stage.join("mosaic.css"), format!("mosaic {label}"))
            .expect("write Mosaic stylesheet");
        fs::write(stage.join("responsive.css"), format!("responsive {label}"))
            .expect("write responsive stylesheet");
        let second_wasm = if valid {
            "<link href=\"/app_bg.wasm\">"
        } else {
            ""
        };
        fs::write(
            stage.join("canvas-app.html"),
            format!(
                "<link href=\"/style.css\"><link href=\"/mosaic.css\">\
                 <link href=\"/responsive.css\"><script src=\"/app.js\"></script>\
                 <link href=\"/app.js\"><link href=\"/app_bg.wasm\">{second_wasm}\
                 <main>{label}</main>"
            ),
        )
        .expect("write shell");
    }

    fn publish_fixture(root: &Path, label: &str) {
        let sources = fixture_source();
        app_at(
            root,
            &sources,
            &mut Vec::new(),
            |stage, _| {
                write_raw_generation(stage, label, &sources, true);
                Ok(())
            },
            || Ok(()),
        )
        .expect("fixture generation publishes");
    }

    fn fail_partial_build(stage: &Path) -> Result<(), AppBuildError> {
        fs::create_dir_all(stage).expect("create partial build");
        fs::write(stage.join("canvas-app.html"), b"partial build").expect("write partial build");
        Err(AppBuildError::Exit(Some(42)))
    }

    #[test]
    fn app_build_command_owns_cache_and_wrapper_inputs() {
        let worktree = Digest::compute(Domain::DistInventory, b"fixture worktree");
        let sources = fixture_source();
        let command = trunk_command(
            Path::new("/workspace"),
            Path::new("/cargo-target"),
            OsStr::new("/cargo-home"),
            Path::new("/stage"),
            "remapped-flags".to_owned(),
            &worktree,
            &sources,
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
        assert_eq!(environment.len(), 9);
        assert_eq!(
            environment.get("CARGO_TARGET_DIR"),
            Some(&Some(format!(
                "/cargo-target/xtask-app-{worktree}-{sources}"
            )))
        );
        assert_eq!(
            environment.get("CARGO_BUILD_BUILD_DIR"),
            Some(&Some(format!(
                "/cargo-target/xtask-app-{worktree}-{sources}-build"
            )))
        );
        assert_eq!(
            environment.get("SPICE_APP_SOURCE_IDENTITY"),
            Some(&Some(sources.to_string()))
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

    /// A shared ambient target is only an outer allocation. Each worktree gets
    /// a distinct inner target even when both build the exact same sources.
    #[test]
    fn concurrent_worktrees_use_distinct_children_of_one_outer_target() {
        use std::sync::{Arc, Barrier};

        let outer = fixture("shared-app-target");
        let worktree_a = outer.join("worktree-a");
        let worktree_b = outer.join("worktree-b");
        fs::create_dir(&worktree_a).expect("create first worktree fixture");
        fs::create_dir(&worktree_b).expect("create second worktree fixture");
        let worktrees = [
            worktree_identity(&worktree_a).expect("identify first worktree"),
            worktree_identity(&worktree_b).expect("identify second worktree"),
        ];
        let sources = fixture_source();
        let barrier = Arc::new(Barrier::new(2));
        let workers = worktrees
            .into_iter()
            .zip([b"worktree A wasm".as_slice(), b"worktree B wasm".as_slice()])
            .map(|(worktree, bytes)| {
                let target = app_target_directory(&outer, &worktree, &sources);
                let barrier = Arc::clone(&barrier);
                let bytes = bytes.to_vec();
                std::thread::spawn(move || {
                    fs::create_dir_all(&target).expect("create inner app target");
                    barrier.wait();
                    fs::write(target.join("app_bg.wasm"), &bytes).expect("write worktree artifact");
                    (target, bytes)
                })
            })
            .collect::<Vec<_>>();
        let built = workers
            .into_iter()
            .map(|worker| worker.join().expect("worktree build joins"))
            .collect::<Vec<_>>();

        assert_ne!(
            built[0].0, built[1].0,
            "two worktrees shared an inner target"
        );
        for (target, expected) in &built {
            assert_eq!(
                fs::read(target.join("app_bg.wasm")).expect("read worktree artifact"),
                *expected,
                "one concurrent worktree consumed its peer's artifact"
            );
        }
        fs::remove_dir_all(outer).expect("remove shared-target fixture");
    }

    /// A closed inventory is not provenance. Foreign Wasm paired with this
    /// worktree's shell and stylesheet must fail before a pointer is written.
    #[test]
    fn foreign_wasm_with_right_css_cannot_publish() {
        let root = fixture("foreign-wasm");
        let expected = fixture_source();
        let foreign = Digest::compute(Domain::DistInventory, b"peer worktree sources");

        let error = app_at(
            &root,
            &expected,
            &mut Vec::new(),
            |stage, _| {
                write_raw_generation(stage, "right-css", &foreign, true);
                Ok(())
            },
            || Ok(()),
        )
        .expect_err("foreign Wasm must not publish");

        assert!(
            error
                .to_string()
                .contains("complete but foreign generation"),
            "the contamination refusal did not name the foreign generation: {error}"
        );
        assert!(
            !root.join(spice::canvas::APP_STAGE).exists(),
            "foreign Wasm published a current generation"
        );
        fs::remove_dir_all(root).expect("remove contamination fixture");
    }

    #[test]
    fn finalization_produces_the_closed_worktree_stage_inventory() {
        let root = fixture("worktree-app-finalize");
        let sources = fixture_source();
        write_raw_generation(&root, "first", &sources, true);

        let identity = finalize_stage(&root, &sources).expect("generated stage finalizes");

        let mut names = fs::read_dir(&root)
            .expect("read stage")
            .map(|entry| entry.expect("read entry").file_name())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            [
                "app.module",
                "app_bg.wasm",
                "canvas-app.html",
                "mosaic.css",
                "responsive.css",
                "style.css"
            ]
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
        let root = fixture("worktree-app-event");
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
            let sources = fixture_source();
            let build = scope.spawn(move || {
                app_at(
                    &build_root,
                    &sources,
                    &mut Vec::new(),
                    |stage, _| {
                        started_tx.send(()).expect("announce build barrier");
                        release_rx.recv().expect("release build barrier");
                        write_raw_generation(stage, "second", &sources, true);
                        Ok(())
                    },
                    || Ok(()),
                )
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
        let root = fixture("worktree-app-failure");
        publish_fixture(&root, "first");
        let source = spice::canvas::AppSource::resolve(&root).expect("first generation resolves");
        let first = source
            .current_generation()
            .expect("first generation is current")
            .identity()
            .clone();
        let sources = fixture_source();

        let error = app_at(
            &root,
            &sources,
            &mut Vec::new(),
            |stage, _| {
                write_raw_generation(stage, "broken", &sources, false);
                Ok(())
            },
            || Ok(()),
        )
        .expect_err("invalid finalization refuses");
        assert!(error.to_string().contains("app_bg.wasm"));
        assert_eq!(
            source
                .current_generation()
                .expect("failed rebuild preserves current")
                .identity(),
            &first
        );

        let fresh = fixture("worktree-app-first-failure");
        app_at(
            &fresh,
            &sources,
            &mut Vec::new(),
            |stage, _| {
                write_raw_generation(stage, "broken", &sources, false);
                Ok(())
            },
            || Ok(()),
        )
        .expect_err("first invalid finalization refuses");
        assert!(
            !fresh.join(spice::canvas::APP_STAGE).exists(),
            "a failed first build published a visible stage"
        );
        fs::remove_dir_all(root).expect("remove fixture");
        fs::remove_dir_all(fresh).expect("remove fresh fixture");
    }

    #[test]
    fn a_failed_build_preserves_the_prior_pointer_or_first_build_absence() {
        let root = fixture("worktree-app-build-failure");
        publish_fixture(&root, "first");
        let source = spice::canvas::AppSource::resolve(&root).expect("first generation resolves");
        let first = source
            .current_generation()
            .expect("first generation is current")
            .identity()
            .clone();
        let sources = fixture_source();

        let error = app_at(
            &root,
            &sources,
            &mut Vec::new(),
            |stage, _| fail_partial_build(stage),
            || Ok(()),
        )
        .expect_err("failed rebuild refuses");
        assert!(matches!(error, AppBuildError::Exit(Some(42))));
        assert_eq!(
            source
                .current_generation()
                .expect("failed rebuild preserves current")
                .identity(),
            &first
        );

        let fresh = fixture("worktree-app-first-build-failure");
        let error = app_at(
            &fresh,
            &sources,
            &mut Vec::new(),
            |stage, _| fail_partial_build(stage),
            || Ok(()),
        )
        .expect_err("failed first build refuses");
        assert!(matches!(error, AppBuildError::Exit(Some(42))));
        assert!(
            !fresh.join(spice::canvas::APP_STAGE).exists(),
            "a failed first build published a visible stage"
        );
        assert!(
            fs::read_dir(fresh.join("dist"))
                .expect("read build parent")
                .next()
                .is_none(),
            "a failed first build left a partial temporary"
        );
        fs::remove_dir_all(root).expect("remove fixture");
        fs::remove_dir_all(fresh).expect("remove fresh fixture");
    }
}
