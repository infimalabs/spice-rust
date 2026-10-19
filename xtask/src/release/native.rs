//! Target-specific Cargo invocation for one linked appliance executable.

use std::ffi::OsString;
use std::path::Path;

use wire::manifest::ReleaseTarget;

/// Ask Cargo for the exact linked binary carrying `canvas`, and retain line
/// tables only for the macOS build whose dSYM consumes them. Keeping the
/// profile override on this target-specific command leaves the Linux
/// executable bytes unchanged. `cargo rustc` hands the link argument to the
/// final executable alone, so no dependency rebuilds when the archive changes.
pub(super) fn cargo_arguments(
    target: ReleaseTarget,
    target_dir: &Path,
    canvas: &crate::app::StagedCanvas,
) -> Result<Vec<OsString>, crate::app::AppBuildError> {
    let object = match target {
        ReleaseTarget::Aarch64AppleDarwin => dist::ObjectTarget::MacAarch64,
        ReleaseTarget::X86_64UnknownLinuxMusl => dist::ObjectTarget::LinuxX86_64,
        ReleaseTarget::X86_64PcWindowsMsvc => dist::ObjectTarget::WindowsX86_64,
    };
    let binary = "spice";
    let mut arguments = vec![
        OsString::from("rustc"),
        OsString::from("--locked"),
        OsString::from("--release"),
        OsString::from("-p"),
        OsString::from("spice"),
        OsString::from("--bin"),
        OsString::from(binary),
    ];
    arguments.extend([
        OsString::from("--target"),
        OsString::from(target.as_str()),
        OsString::from("--target-dir"),
    ]);
    arguments.push(target_dir.as_os_str().to_owned());
    arguments.push(OsString::from("--message-format=json-render-diagnostics"));
    if target == ReleaseTarget::Aarch64AppleDarwin {
        arguments.extend([
            OsString::from("--config"),
            OsString::from("profile.release.debug=\"line-tables-only\""),
        ]);
    }
    arguments.extend(canvas.link_arguments(target_dir, object)?);
    Ok(arguments)
}
