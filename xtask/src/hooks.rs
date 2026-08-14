//! Explicitly activate the installed common Git hooks for every worktree.

use std::fmt;
use std::fs;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The installed product path every linked worktree's hook resolves.
const ENTRY_POINT_PATH: &str = ".spice-install/bin/spice";

/// Ask the installed appliance to preview and apply its own current plan.
///
/// This developer convenience owns no hook preflight or mutation. The installed
/// `spice install` plan is the sole authority for both assets and Git selection.
///
/// # Errors
///
/// Returns repository discovery, installed-asset, Git, or output failures.
pub fn activate_hooks(root: &Path, output: &mut impl Write) -> Result<(), HookActivationError> {
    let repository = spice::repository::Repository::discover(root)
        .map_err(|error| HookActivationError::Repository(error.to_string()))?;
    let entry_point = repository.common_dir().join(ENTRY_POINT_PATH);
    if !is_executable(&entry_point)? {
        return Err(HookActivationError::HookNotExecutable(entry_point));
    }
    let preview = delegated_install(
        &entry_point,
        repository.worktree_root(),
        &[],
        "preview installed hook activation",
    )?;
    let preview: spice::install::InstallPreview =
        wire::decode(&preview.stdout).map_err(|error| HookActivationError::InvalidReply {
            operation: "preview installed hook activation",
            detail: error.to_string(),
        })?;
    let apply = format!("--apply={}", preview.digest());
    let receipt = delegated_install(
        &entry_point,
        repository.worktree_root(),
        &[apply.as_str()],
        "apply installed hook activation",
    )?;
    let receipt: spice::install::InstallReceipt =
        wire::decode(&receipt.stdout).map_err(|error| HookActivationError::InvalidReply {
            operation: "apply installed hook activation",
            detail: error.to_string(),
        })?;
    if receipt.status() != spice::install::InstallStatus::Complete
        || receipt.digest() != preview.digest()
        || receipt.plan() != preview.plan()
    {
        return Err(HookActivationError::InvalidReply {
            operation: "apply installed hook activation",
            detail: format!(
                "installer returned status {:?}, digest {}, and a plan matching preview={}",
                receipt.status(),
                receipt.digest(),
                receipt.plan() == preview.plan(),
            ),
        });
    }
    writeln!(
        output,
        "activated Spice hooks through installed plan {}",
        receipt.digest(),
    )
    .map_err(HookActivationError::Output)
}

fn delegated_install(
    executable: &Path,
    worktree: &Path,
    arguments: &[&str],
    operation: &'static str,
) -> Result<Output, HookActivationError> {
    let output = Command::new(executable)
        .arg("install")
        .args(arguments)
        .current_dir(worktree)
        .output()
        .map_err(|source| HookActivationError::InstalledLaunch { operation, source })?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(HookActivationError::InstalledExit {
            operation,
            code: output.status.code(),
            diagnostic: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Audit that an activated hook set can reach the installed product.
///
/// Product currency belongs to the digest-bound installer. This check owns
/// only the activation invariant: an active hook resolves one executable Spice
/// target and never an xtask copy or source-build fallback.
///
/// # Errors
///
/// Returns repository discovery, installed-target, Git, or output failures.
pub fn audit_entry_point(root: &Path, output: &mut impl Write) -> Result<(), HookActivationError> {
    let repository = spice::repository::Repository::discover(root)
        .map_err(|error| HookActivationError::Repository(error.to_string()))?;
    let hooks = repository.common_dir().join(".spice-install/git-hooks");
    let configured = configured_hooks_path(git(
        repository.worktree_root(),
        &["config", "--get", "core.hooksPath"],
        "read the active hooks path",
    ))?
    .unwrap_or_default();
    if configured != hooks.to_string_lossy() {
        return writeln!(
            output,
            "reference-transaction target not activated; core.hooksPath is {configured:?}"
        )
        .map_err(HookActivationError::Output);
    }
    let path = repository.common_dir().join(ENTRY_POINT_PATH);
    if !is_executable(&path)? {
        return Err(HookActivationError::HookNotExecutable(path));
    }
    writeln!(
        output,
        "reference-transaction target is installed and executable at {}",
        path.display()
    )
    .map_err(HookActivationError::Output)
}

/// Treat only Git's documented absent-value status as an inactive hook path.
fn configured_hooks_path(
    configured: Result<Output, HookActivationError>,
) -> Result<Option<String>, HookActivationError> {
    match configured {
        Ok(configured) => Ok(Some(
            utf8("git config --get core.hooksPath", &configured.stdout)?
                .trim()
                .to_owned(),
        )),
        Err(HookActivationError::Exit { code: Some(1), .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

fn git(
    root: &Path,
    arguments: &[&str],
    operation: &'static str,
) -> Result<Output, HookActivationError> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .map_err(|source| HookActivationError::Launch { operation, source })?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(HookActivationError::Exit {
            operation,
            code: output.status.code(),
            diagnostic: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn utf8<'a>(operation: &'static str, bytes: &'a [u8]) -> Result<&'a str, HookActivationError> {
    std::str::from_utf8(bytes).map_err(|_| HookActivationError::NonUtf8 { operation })
}

fn is_executable(path: &Path) -> Result<bool, HookActivationError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(HookActivationError::Inspect {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    #[cfg(unix)]
    return Ok(metadata.is_file() && metadata.permissions().mode() & 0o111 != 0);
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Ok(false)
    }
}

/// Explicit hook-activation failure.
#[derive(Debug)]
pub enum HookActivationError {
    Repository(String),
    HookNotExecutable(PathBuf),
    Inspect {
        path: PathBuf,
        source: io::Error,
    },
    Launch {
        operation: &'static str,
        source: io::Error,
    },
    InstalledLaunch {
        operation: &'static str,
        source: io::Error,
    },
    Exit {
        operation: &'static str,
        code: Option<i32>,
        diagnostic: String,
    },
    InstalledExit {
        operation: &'static str,
        code: Option<i32>,
        diagnostic: String,
    },
    NonUtf8 {
        operation: &'static str,
    },
    InvalidReply {
        operation: &'static str,
        detail: String,
    },
    Output(io::Error),
}

impl fmt::Display for HookActivationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(detail) => write!(formatter, "discover hook repository: {detail}"),
            Self::HookNotExecutable(path) => write!(
                formatter,
                "hook activation failed: {} is not executable",
                path.display()
            ),
            Self::Inspect { path, source } => {
                write!(
                    formatter,
                    "inspect installed hook {}: {source}",
                    path.display()
                )
            }
            Self::Launch { operation, source } => {
                write!(formatter, "failed to launch Git to {operation}: {source}")
            }
            Self::InstalledLaunch { operation, source } => {
                write!(
                    formatter,
                    "failed to launch installed Spice to {operation}: {source}"
                )
            }
            Self::Exit {
                operation,
                code: Some(code),
                diagnostic,
            } => write!(
                formatter,
                "Git failed to {operation} with exit {code}: {}",
                diagnostic.trim_ascii_end()
            ),
            Self::Exit {
                operation,
                code: None,
                diagnostic,
            } => write!(
                formatter,
                "Git terminated by signal while trying to {operation}: {}",
                diagnostic.trim_ascii_end()
            ),
            Self::InstalledExit {
                operation,
                code: Some(code),
                diagnostic,
            } => write!(
                formatter,
                "installed Spice failed to {operation} with exit {code}: {}",
                diagnostic.trim_ascii_end()
            ),
            Self::InstalledExit {
                operation,
                code: None,
                diagnostic,
            } => write!(
                formatter,
                "installed Spice terminated by signal while trying to {operation}: {}",
                diagnostic.trim_ascii_end()
            ),
            Self::NonUtf8 { operation } => write!(formatter, "{operation} emitted non-UTF-8"),
            Self::InvalidReply { operation, detail } => {
                write!(
                    formatter,
                    "installed Spice returned an invalid reply while trying to {operation}: {detail}"
                )
            }
            Self::Output(source) => write!(formatter, "hook activation output failed: {source}"),
        }
    }
}

impl std::error::Error for HookActivationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Inspect { source, .. }
            | Self::Launch { source, .. }
            | Self::InstalledLaunch { source, .. }
            | Self::Output(source) => Some(source),
            Self::Repository(_)
            | Self::HookNotExecutable(_)
            | Self::Exit { .. }
            | Self::InstalledExit { .. }
            | Self::NonUtf8 { .. } => None,
            Self::InvalidReply { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::{HookActivationError, configured_hooks_path};

    #[test]
    fn only_an_unset_hook_path_is_an_inactive_configuration() {
        let unset = configured_hooks_path(Err(HookActivationError::Exit {
            operation: "read the active hooks path",
            code: Some(1),
            diagnostic: String::new(),
        }))
        .expect("Git status 1 means the value is unset");
        assert_eq!(unset, None);

        let launch = configured_hooks_path(Err(HookActivationError::Launch {
            operation: "read the active hooks path",
            source: io::Error::other("fixture Git launch failure"),
        }))
        .expect_err("a Git launch failure is not an inactive repository");
        assert!(matches!(launch, HookActivationError::Launch { .. }));

        let unexpected = configured_hooks_path(Err(HookActivationError::Exit {
            operation: "read the active hooks path",
            code: Some(2),
            diagnostic: "fixture Git config failure".to_owned(),
        }))
        .expect_err("an unexpected Git exit is not an inactive repository");
        assert!(matches!(
            unexpected,
            HookActivationError::Exit { code: Some(2), .. }
        ));
    }
}
