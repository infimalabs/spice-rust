//! Canvas web analysis for the selected tree and its owned build roots.

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

use sill::Environment;

use super::build_root::{CanvasSource, CanvasWorkspace};
use super::*;

/// Run the declared canvas checks in order and stop at the first non-clean one.
///
/// The analysis is run against a cell its workspace reports a failed removal
/// into, and that report outranks whatever the analysis concluded. A workspace
/// left on disk refuses every later canvas analysis of the same tree, so an arm
/// that passed while leaking one would report a clean tree and then be unable to
/// measure it again; the refusal here is the same refusal, delivered on the run
/// that caused it instead of the next one.
pub(in crate::toolchain) fn canvas_web_typecheck(
    declaration: ToolchainDeclaration,
    root: &Path,
    repository: &Path,
    bounds: &Bounds,
    programs: &BTreeMap<&str, &str>,
) -> Analysis {
    let removal = Cell::new(None);
    let analysis = canvas_analysis(declaration, root, repository, bounds, programs, &removal);
    match removal.take() {
        Some(report) => Analysis::terminal(ArmOutcome::ParseFailure, report),
        None => analysis,
    }
}

fn canvas_analysis(
    declaration: ToolchainDeclaration,
    root: &Path,
    repository: &Path,
    bounds: &Bounds,
    programs: &BTreeMap<&str, &str>,
    removal: &Cell<Option<String>>,
) -> Analysis {
    if !root.join("Cargo.toml").is_file() || !root.join("app/index.html").is_file() {
        return Analysis::not_applicable(declaration);
    }
    let Some(cargo) = programs.get(CARGO) else {
        return Analysis::terminal(
            ArmOutcome::ToolUnavailable,
            format!("{} declares no {CARGO} program", declaration.id.as_str()),
        );
    };
    let Some(trunk) = programs.get(TRUNK) else {
        return Analysis::terminal(
            ArmOutcome::ToolUnavailable,
            format!("{} declares no {TRUNK} program", declaration.id.as_str()),
        );
    };
    // The mirror must outlive both compiler invocations; only the generated
    // trunk distribution belongs to the transient per-run workspace below.
    let source = match CanvasSource::create(root, repository, removal) {
        Ok(source) => source,
        Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
    };
    let workspace = match CanvasWorkspace::create(root, removal) {
        Ok(workspace) => workspace,
        Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
    };
    let outputs = match CanvasOutputs::create(&workspace, root, repository) {
        Ok(outputs) => outputs,
        Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
    };
    let environment = match canvas_environment(cargo, trunk, &outputs) {
        Ok(environment) => environment,
        Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
    };

    let source = source.path();
    let app = source.join("app");
    let invocations = [
        CompileInvocation {
            name: "cargo wasm32 check",
            program: cargo,
            arguments: CARGO_WASM_ARGUMENTS,
            cwd: source,
        },
        CompileInvocation {
            name: "trunk canvas build",
            program: trunk,
            arguments: TRUNK_BUILD_ARGUMENTS,
            cwd: app.as_path(),
        },
    ];
    for invocation in invocations {
        if let Err(failure) = run_compile_analysis(
            declaration.id,
            source,
            invocation,
            bounds,
            &environment,
            |code, message| FindingDetail::CanvasWebOutput { code, message },
        ) {
            return failure.analysis;
        }
    }
    Analysis::terminal(
        ArmOutcome::Passed,
        format!(
            "{} completed both declared canvas analyses",
            declaration.id.as_str()
        ),
    )
}

fn canvas_environment(
    cargo: &str,
    trunk: &str,
    outputs: &CanvasOutputs,
) -> Result<Environment, String> {
    let path = tool_search_path("canvas", &[cargo, trunk, "/usr/bin/cc", "/bin/sh"])?;
    Environment::new([
        ("PATH", path),
        (HOME_DIRECTORY, COMPILED_HOME_DIRECTORY.into()),
        (
            CARGO_TARGET_DIRECTORY,
            outputs.cargo_target.as_os_str().to_owned(),
        ),
        (
            CARGO_BUILD_DIRECTORY,
            outputs.cargo_build.as_os_str().to_owned(),
        ),
        (RUSTC_WRAPPER, std::ffi::OsString::new()),
        (RUSTC_WORKSPACE_WRAPPER, std::ffi::OsString::new()),
        (
            TRUNK_DISTRIBUTION_DIRECTORY,
            outputs.trunk_dist.as_os_str().to_owned(),
        ),
    ])
    .map_err(|error| format!("canvas tool environment was refused: {error}"))
}

/// Writable output roots for tools examining one immutable selected tree.
///
/// The Cargo pair is deliberately not inside [`CanvasWorkspace`]: it is a
/// fixed repository path that every canvas analysis of this repository shares,
/// so the compiled artifacts survive the run that produced them. `input.rs`
/// records what that warmth is worth -- five cold samples of the `trunk build`
/// this arm runs took 15.0 to 24.9 seconds against the 2.6 to 2.9 they take
/// warm -- and a root minted per invocation would pay the cold price every time
/// while stranding the previous one.
///
/// Sharing it is safe because Cargo already arbitrates it. Two of these
/// analyses run at once against one repository were measured taking the same
/// pair: the second reported `Blocking waiting for file lock on build
/// directory` and then completed, both exiting zero. What must not be shared is
/// the generated output, and that is the part living under the owned root.
struct CanvasOutputs {
    cargo_target: PathBuf,
    cargo_build: PathBuf,
    trunk_dist: PathBuf,
}

impl CanvasOutputs {
    fn create(
        workspace: &CanvasWorkspace<'_>,
        selected: &Path,
        repository: &Path,
    ) -> Result<Self, String> {
        let (cargo_target, cargo_build) = canvas_output_roots(repository);
        let trunk_dist = workspace.path().join("trunk-dist");
        if let Err(error) = fs::create_dir_all(&cargo_target)
            .and_then(|()| fs::create_dir_all(&cargo_build))
            .and_then(|()| fs::create_dir(&trunk_dist))
        {
            return Err(format!(
                "canvas analysis output directories for {} could not be created: {error}",
                selected.display()
            ));
        }
        Ok(Self {
            cargo_target,
            cargo_build,
            trunk_dist,
        })
    }
}

pub(in crate::toolchain) fn canvas_output_roots(repository: &Path) -> (PathBuf, PathBuf) {
    (
        repository.join("target/xtask-app"),
        repository.join("target/xtask-app-build"),
    )
}
