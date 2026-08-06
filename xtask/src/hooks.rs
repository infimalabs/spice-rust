//! Explicitly activate the installed common Git hooks for every worktree.

use std::fmt;
use std::fs;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const HOOKS: [&str; 3] = ["pre-commit", "commit-msg", "reference-transaction"];

/// The one name a hook can spell for an executable Cargo keeps somewhere else.
///
/// This sits beside the installed hooks under the Git common directory, so all
/// eleven linked worktrees resolve it to the same file. That is correct rather
/// than merely convenient: `reference_transaction::apply` reads only the
/// repository at the current directory, through Git, and carries no lane
/// identity and no per-worktree configuration, so one executable serves every
/// worktree and no per-lane dispatch is owed.
const ENTRY_POINT_PATH: &str = ".spice-install/bin/xtask";

/// Point the repository and every linked worktree at the installed hook set.
///
/// This is deliberately separate from digest-bound installation: the install
/// plan publishes bytes, while this explicit cutover changes Git configuration
/// and publishes the executable those bytes name.
///
/// `entry_point` is the executable to publish. It is a parameter rather than
/// `current_exe` read in here so that the provenance of the published file is
/// decided by the caller: the command passes the running `xtask`, and a test
/// can drive activation with the binary Cargo built for it rather than
/// publishing the test harness itself.
///
/// # Errors
///
/// Returns repository discovery, installed-hook, publication, Git, or output
/// failures.
pub fn activate_hooks(
    root: &Path,
    entry_point: &Path,
    output: &mut impl Write,
) -> Result<(), HookActivationError> {
    let repository = spice::repository::Repository::discover(root)
        .map_err(|error| HookActivationError::Repository(error.to_string()))?;
    let hooks = repository.common_dir().join(".spice-install/git-hooks");
    for hook in HOOKS {
        let path = hooks.join(hook);
        if !is_executable(&path)? {
            return Err(HookActivationError::HookNotExecutable(path));
        }
    }
    // Published before the configuration flips, so the hooks Git is about to
    // start running never observe a moment where the file they exec is absent.
    publish_entry_point(&repository, entry_point, output)?;

    let listed = git(
        repository.worktree_root(),
        &["worktree", "list", "--porcelain"],
        "list Git worktrees",
    )?;
    let listed = utf8("git worktree list --porcelain", &listed.stdout)?;
    for worktree in listed
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
    {
        let prior = Command::new("git")
            .args([
                "-C",
                worktree,
                "config",
                "--worktree",
                "--get-all",
                "core.hooksPath",
            ])
            .output()
            .map_err(|source| HookActivationError::Launch {
                operation: "read worktree hook override",
                source,
            })?;
        if prior.status.success() {
            let prior = utf8(
                "git config --worktree --get-all core.hooksPath",
                &prior.stdout,
            )?;
            git(
                Path::new(worktree),
                &["config", "--worktree", "--unset-all", "core.hooksPath"],
                "clear worktree hook override",
            )?;
            writeln!(
                output,
                "cleared worktree hook override at {worktree} (was {})",
                prior.trim_ascii_end()
            )
            .map_err(HookActivationError::Output)?;
        } else if prior.status.code() != Some(1)
            && !String::from_utf8_lossy(&prior.stderr)
                .contains("extension worktreeConfig is enabled")
        {
            return Err(HookActivationError::Exit {
                operation: "read worktree hook override",
                code: prior.status.code(),
                diagnostic: String::from_utf8_lossy(&prior.stderr).into_owned(),
            });
        }
    }

    let hooks_text = hooks
        .to_str()
        .ok_or_else(|| HookActivationError::UnnameablePath(hooks.clone()))?;
    git(
        repository.worktree_root(),
        &["config", "--local", "core.hooksPath", hooks_text],
        "activate common Git hooks",
    )?;
    let configured = git(
        repository.worktree_root(),
        &["config", "--local", "--get", "core.hooksPath"],
        "verify common Git hooks",
    )?;
    let configured = utf8(
        "git config --local --get core.hooksPath",
        &configured.stdout,
    )?
    .trim_ascii_end();
    if configured != hooks_text {
        return Err(HookActivationError::ConfigurationDrift {
            expected: hooks,
            observed: configured.to_owned(),
        });
    }
    writeln!(output, "activated Spice hooks from {hooks_text}").map_err(HookActivationError::Output)
}

/// Copy the executable the hooks exec to a name a hook can spell.
///
/// Cargo writes its output under a content-keyed directory, so the executable
/// just built sits at a path that moves whenever the sources do and that no
/// hook can name in advance. Activation copies it once to [`ENTRY_POINT_PATH`],
/// which is the only path in this arrangement that survives an edit.
///
/// A stale copy is detected here and nowhere else, by comparing the published
/// bytes against the ones being published. Nothing rebuilds at hook time, which
/// is the whole point of publishing, so the copy is exactly as current as the
/// last activation left it and re-running activation is what reports the
/// difference: an unchanged copy says so, a replaced one names its source.
///
/// The bytes land on a sibling and are renamed into place. A hook in another
/// worktree may be executing the current file, and writing over it would either
/// be refused outright while those bytes run or hand a half-written executable
/// to whoever started next; a rename leaves the running process holding its own
/// inode and swaps the name in one step.
fn publish_entry_point(
    repository: &spice::repository::Repository,
    entry_point: &Path,
    output: &mut impl Write,
) -> Result<(), HookActivationError> {
    let bytes = fs::read(entry_point).map_err(|source| HookActivationError::Publish {
        path: entry_point.to_path_buf(),
        source,
    })?;
    let destination = repository.common_dir().join(ENTRY_POINT_PATH);
    let parent = destination
        .parent()
        .ok_or_else(|| HookActivationError::UnnameablePath(destination.clone()))?;
    fs::create_dir_all(parent).map_err(|source| HookActivationError::Publish {
        path: parent.to_path_buf(),
        source,
    })?;
    if fs::read(&destination).is_ok_and(|published| published == bytes) {
        return writeln!(
            output,
            "hook entry point already current at {}",
            destination.display()
        )
        .map_err(HookActivationError::Output);
    }
    // Named for this process because the worktrees that share the destination
    // share this directory with it, so two activations at once must not stage
    // over one another on the way in.
    let staged = parent.join(format!("xtask.{}.staged", std::process::id()));
    write_executable(&staged, &bytes)?;
    fs::rename(&staged, &destination).map_err(|source| HookActivationError::Publish {
        path: destination.clone(),
        source,
    })?;
    writeln!(
        output,
        "published hook entry point {} from {}",
        destination.display(),
        entry_point.display()
    )
    .map_err(HookActivationError::Output)
}

fn write_executable(path: &Path, bytes: &[u8]) -> Result<(), HookActivationError> {
    fs::write(path, bytes).map_err(|source| HookActivationError::Publish {
        path: path.to_path_buf(),
        source,
    })?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|source| {
        HookActivationError::Publish {
            path: path.to_path_buf(),
            source,
        }
    })?;
    Ok(())
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

/// Explicit hook-cutover failure.
#[derive(Debug)]
pub enum HookActivationError {
    Repository(String),
    HookNotExecutable(PathBuf),
    Inspect {
        path: PathBuf,
        source: io::Error,
    },
    Publish {
        path: PathBuf,
        source: io::Error,
    },
    UnnameablePath(PathBuf),
    Launch {
        operation: &'static str,
        source: io::Error,
    },
    Exit {
        operation: &'static str,
        code: Option<i32>,
        diagnostic: String,
    },
    NonUtf8 {
        operation: &'static str,
    },
    ConfigurationDrift {
        expected: PathBuf,
        observed: String,
    },
    Output(io::Error),
}

impl fmt::Display for HookActivationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(detail) => write!(formatter, "discover hook repository: {detail}"),
            Self::HookNotExecutable(path) => write!(
                formatter,
                "hook installation failed: {} is not executable",
                path.display()
            ),
            Self::Inspect { path, source } => {
                write!(
                    formatter,
                    "inspect installed hook {}: {source}",
                    path.display()
                )
            }
            Self::Publish { path, source } => {
                write!(
                    formatter,
                    "publish hook entry point {}: {source}",
                    path.display()
                )
            }
            Self::UnnameablePath(path) => {
                write!(
                    formatter,
                    "installed hook path is not UTF-8: {}",
                    path.display()
                )
            }
            Self::Launch { operation, source } => {
                write!(formatter, "failed to launch Git to {operation}: {source}")
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
            Self::NonUtf8 { operation } => write!(formatter, "{operation} emitted non-UTF-8"),
            Self::ConfigurationDrift { expected, observed } => write!(
                formatter,
                "hook installation failed: expected {}, found {observed}",
                expected.display()
            ),
            Self::Output(source) => write!(formatter, "hook activation output failed: {source}"),
        }
    }
}

impl std::error::Error for HookActivationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Inspect { source, .. }
            | Self::Publish { source, .. }
            | Self::Launch { source, .. }
            | Self::Output(source) => Some(source),
            Self::Repository(_)
            | Self::HookNotExecutable(_)
            | Self::UnnameablePath(_)
            | Self::Exit { .. }
            | Self::NonUtf8 { .. }
            | Self::ConfigurationDrift { .. } => None,
        }
    }
}
