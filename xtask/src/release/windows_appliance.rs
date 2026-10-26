use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

use sill::Environment;
use wire::manifest::ReleaseTarget;

use super::{
    CommandSpec, Executor as _, ReleaseEnvironment, SystemExecutor, TargetBuildError, build_error,
    cargo_executable, find_program, native, read_regular, source, successful, workspace_root,
};

/// Cross-build the complete Windows appliance with the current staged canvas,
/// without staging a release payload or receipt. Until hosted Windows staging
/// is usable, this local build is the Windows release route: a checkout equal
/// to HEAD (release output aside) reports the release product version, and any
/// other checkout reports the `-dev` version so no release version vouches for
/// uncommitted source.
pub fn build_windows_appliance(
    release: ReleaseEnvironment,
    ambient: Environment,
    output: &mut impl Write,
) -> Result<PathBuf, TargetBuildError> {
    let target = ReleaseTarget::X86_64PcWindowsMsvc;
    let root = workspace_root()?;
    let path = release
        .path
        .as_deref()
        .ok_or(TargetBuildError::MissingProgram("PATH"))?;
    let native_executor = SystemExecutor::ambient(&root, ambient);
    let build = match source::verify_checkout(&native_executor, &find_program(path, "git")?) {
        Ok(()) => dist::Build::Staged,
        Err(TargetBuildError::SourceDirty(reason)) => {
            writeln!(
                output,
                "[windows-appliance] {reason}; building the -dev version"
            )
            .map_err(TargetBuildError::Output)?;
            dist::Build::Dev
        }
        Err(error) => return Err(error),
    };
    let version =
        spice::version::source_version(&root, build).map_err(TargetBuildError::SourceVersion)?;
    crate::app(&release, output).map_err(TargetBuildError::Application)?;
    let canvas = crate::app::StagedCanvas::read(&root).map_err(TargetBuildError::Application)?;
    let target_dir = root.join("target");
    let arguments = native::cargo_arguments(target, &target_dir, &version, &canvas)
        .map_err(TargetBuildError::Application)?;
    let spice = find_program(path, "spice")?;
    let mut wrapped_arguments = vec![
        OsString::from("dev"),
        OsString::from("cargo"),
        OsString::from("--"),
    ];
    wrapped_arguments.extend(arguments);
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
