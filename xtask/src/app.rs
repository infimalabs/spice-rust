//! Native, lane-local compilation of the browser app.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::release::ReleaseEnvironment;

/// The target the browser app is compiled for.
pub(crate) const WASM_TARGET: &str = "wasm32-unknown-unknown";

const REMAPPED_CARGO_HOME: &str = "/cargo-home";
const GENERATED_MODULE_REFERENCES: usize = 2;

/// Compile the browser app into the one stage this worktree's resident serves.
///
/// The stage is ignored by Git and rebuilt by each lane. It is removed first,
/// so a failed build cannot be mistaken for a complete prior generation.
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
        "[app] {} is ready for `spice up` in this worktree",
        stage.display()
    )
    .map_err(AppBuildError::Output)
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
            "canvas-app.html carried {references} app.js references, expected \
             {GENERATED_MODULE_REFERENCES}"
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
        }
    }
}

impl std::error::Error for AppBuildError {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

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
        let root =
            std::env::temp_dir().join(format!("spice-xtask-lane-app-{}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale fixture");
        }
        fs::create_dir_all(&root).expect("create fixture");
        fs::write(root.join("app.js"), b"module").expect("write generated module");
        fs::write(root.join("app_bg.wasm"), b"wasm").expect("write wasm");
        fs::write(root.join("style.css"), b"style").expect("write stylesheet");
        fs::write(
            root.join("canvas-app.html"),
            b"<script src=\"/app.js\"></script><link href=\"/app.js\">",
        )
        .expect("write shell");

        finalize_stage(&root).expect("generated stage finalizes");

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
        assert!(
            fs::read_to_string(root.join("canvas-app.html"))
                .expect("read shell")
                .contains("/app.module")
        );
        fs::remove_dir_all(root).expect("remove fixture");
    }
}
