use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

use sill::Environment;
use wire::manifest::ReleaseTarget;

use super::{
    CommandSpec, Executor as _, ReleaseEnvironment, SystemExecutor, TargetBuildError, build_error,
    cargo_executable, find_program, native, read_regular, successful, workspace_root,
};

/// Cross-build the complete development Windows appliance with the current
/// staged canvas, without staging a release payload or receipt. It reports the
/// source checkout's `-dev` product version.
pub fn build_windows_appliance(
    release: ReleaseEnvironment,
    ambient: Environment,
    output: &mut impl Write,
) -> Result<PathBuf, TargetBuildError> {
    let target = ReleaseTarget::X86_64PcWindowsMsvc;
    let root = workspace_root()?;
    let version = spice::version::source_version(&root, dist::Build::Dev)
        .map_err(TargetBuildError::SourceVersion)?;
    crate::app(&release, output).map_err(TargetBuildError::Application)?;
    let canvas = crate::app::StagedCanvas::read(&root).map_err(TargetBuildError::Application)?;
    let target_dir = root.join("target");
    let arguments = native::cargo_arguments(target, &target_dir, &version, &canvas)
        .map_err(TargetBuildError::Application)?;
    let spice = find_program(
        release
            .path
            .as_deref()
            .ok_or(TargetBuildError::MissingProgram("PATH"))?,
        "spice",
    )?;
    let mut wrapped_arguments = vec![
        OsString::from("dev"),
        OsString::from("cargo"),
        OsString::from("--"),
    ];
    wrapped_arguments.extend(arguments);
    let native_executor = SystemExecutor::ambient(&root, ambient);
    let native = successful(native_executor.execute(CommandSpec::new(
        "native-build",
        &spice,
        wrapped_arguments,
    ))?)?;
    let executable = cargo_executable(&native.stdout).map_err(TargetBuildError::Build)?;
    let bytes = read_regular(&executable)?;
    canvas
        .canvas()
        .verify_linked(&bytes)
        .map_err(build_error)
        .map_err(TargetBuildError::Build)?;
    writeln!(
        output,
        "[windows-appliance] executable={} version={version} canvas={}",
        executable.display(),
        canvas.canvas().generation()
    )
    .map_err(TargetBuildError::Output)?;
    Ok(executable)
}
