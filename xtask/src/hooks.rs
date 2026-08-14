//! Explicitly activate the installed common Git hooks for every worktree.

use std::fmt;
use std::fs;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const HOOKS: [&str; 3] = ["pre-commit", "commit-msg", "reference-transaction"];

/// The installed product path every linked worktree's hook resolves.
const ENTRY_POINT_PATH: &str = ".spice-install/bin/spice";

/// Point the repository and every linked worktree at the installed hook set.
///
/// This is deliberately separate from digest-bound installation: the install
/// plan publishes the hooks and product, while this explicit activation changes
/// only Git configuration after proving those installed assets exist.
///
/// # Errors
///
/// Returns repository discovery, installed-asset, Git, or output failures.
pub fn activate_hooks(root: &Path, output: &mut impl Write) -> Result<(), HookActivationError> {
    let repository = spice::repository::Repository::discover(root)
        .map_err(|error| HookActivationError::Repository(error.to_string()))?;
    let hooks = repository.common_dir().join(".spice-install/git-hooks");
    for hook in HOOKS {
        let path = hooks.join(hook);
        if !is_executable(&path)? {
            return Err(HookActivationError::HookNotExecutable(path));
        }
    }
    let entry_point = repository.common_dir().join(ENTRY_POINT_PATH);
    if !is_executable(&entry_point)? {
        return Err(HookActivationError::HookNotExecutable(entry_point));
    }
    let hooks_text = hooks
        .to_str()
        .ok_or_else(|| HookActivationError::UnnameablePath(hooks.clone()))?;
    let overrides = preflight_worktree_hooks(&repository, hooks_text)?;

    for (worktree, prior) in overrides {
        git(
            &worktree,
            &["config", "--worktree", "--unset-all", "core.hooksPath"],
            "clear worktree hook override",
        )?;
        writeln!(
            output,
            "cleared worktree hook override at {} (was {prior})",
            worktree.display()
        )
        .map_err(HookActivationError::Output)?;
    }
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

/// Refuse before the first write when a worktree selects foreign hooks.
///
/// **Same-worktree coexistence is unsupported for hooks.** `core.hooksPath` is
/// single-valued per scope, so one worktree gets this port's guarantees or the
/// Python predecessor's and never both. The stronger claim -- unsupported per
/// *repository* -- is refused by the evidence: `extensions.worktreeConfig` makes
/// per-worktree selection real, and lanes do run on it.
///
/// The other direction, one hook path dispatching to both, is rejected because
/// `reference-transaction` runs inside the ref transaction while the
/// repository-global `packed-refs` lock is held; it therefore execs a prebuilt
/// binary and never a build system. A dispatcher would pay a second interpreter
/// start under that lock on every ref update, in all lanes at once. The two
/// gates also disagree on rules, so running both means the stricter one governs
/// silently.
///
/// So this walks every worktree and reads its *effective* `core.hooksPath`
/// before anything is published. Two values configured across scopes refuse even
/// where each alone would pass. It once cleared every worktree unconditionally;
/// stopping here is what keeps it from stripping the predecessor's hooks.
fn preflight_worktree_hooks(
    repository: &spice::repository::Repository,
    hooks_text: &str,
) -> Result<Vec<(PathBuf, String)>, HookActivationError> {
    let listed = git(
        repository.worktree_root(),
        &["worktree", "list", "--porcelain"],
        "list Git worktrees",
    )?;
    let listed = utf8("git worktree list --porcelain", &listed.stdout)?;
    let mut overrides = Vec::new();
    for worktree in listed
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
    {
        let configured = optional_git_config(
            &worktree,
            &["config", "--get-all", "core.hooksPath"],
            "read effective worktree hooks",
            None,
        )?;
        if let Some(values) = configured
            && !is_one_tolerated_hook_selection(&values, hooks_text)
        {
            return Err(HookActivationError::ForeignHookPath {
                worktree,
                observed: values.join(", "),
            });
        }
        if let Some(values) = optional_git_config(
            &worktree,
            &["config", "--worktree", "--get-all", "core.hooksPath"],
            "read worktree hook override",
            Some("extension worktreeConfig is enabled"),
        )? {
            overrides.push((worktree, values.join("\n")));
        }
    }
    Ok(overrides)
}

/// Whether all effective configuration layers repeat one admitted selection.
///
/// Git reports the same value once per layer that declares it. Repeating one
/// tolerated selection does not add another hook path, while even two different
/// tolerated values would leave activation guessing which layer is authoritative.
fn is_one_tolerated_hook_selection(values: &[String], hooks_text: &str) -> bool {
    let Some(first) = values.first() else {
        return false;
    };
    (first == ".githooks" || first == hooks_text) && values.iter().all(|value| value == first)
}

fn optional_git_config(
    root: &Path,
    arguments: &[&str],
    operation: &'static str,
    absent_diagnostic: Option<&str>,
) -> Result<Option<Vec<String>>, HookActivationError> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .map_err(|source| HookActivationError::Launch { operation, source })?;
    if output.status.success() {
        let values = utf8(operation, &output.stdout)?
            .lines()
            .map(str::to_owned)
            .collect();
        return Ok(Some(values));
    }
    let diagnostic = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.status.code() == Some(1)
        || absent_diagnostic.is_some_and(|absent| diagnostic.contains(absent))
    {
        return Ok(None);
    }
    Err(HookActivationError::Exit {
        operation,
        code: output.status.code(),
        diagnostic,
    })
}

/// Audit that the activated hooks and receipt own this exact built product.
///
/// The verifier builds `current_binary` before calling this function. The audit
/// remains side-effect free: it checks the selected hook path, validates every
/// receipt-owned asset, and compares the installed binary with those freshly
/// built bytes. The hook hot path does none of this work.
///
/// # Errors
///
/// Returns repository discovery, installed-target, Git, or output failures.
pub fn audit_entry_point(
    root: &Path,
    current_binary: &Path,
    output: &mut impl Write,
) -> Result<(), HookActivationError> {
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
        return Err(HookActivationError::ConfigurationDrift {
            expected: hooks,
            observed: configured,
        });
    }
    let audit = spice::install::audit_appliance(&repository, current_binary)
        .map_err(HookActivationError::ApplianceInspect)?;
    if !audit.is_current() {
        return Err(HookActivationError::Appliance {
            state: audit.state(),
            detail: audit.detail().to_owned(),
            repair: audit.repair().map(str::to_owned),
        });
    }
    writeln!(
        output,
        "installed Spice appliance is current: {}",
        audit.detail()
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
    UnnameablePath(PathBuf),
    ForeignHookPath {
        worktree: PathBuf,
        observed: String,
    },
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
    Appliance {
        state: spice::install::ApplianceState,
        detail: String,
        repair: Option<String>,
    },
    ApplianceInspect(spice::install::InstallError),
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
            Self::UnnameablePath(path) => {
                write!(
                    formatter,
                    "installed hook path is not UTF-8: {}",
                    path.display()
                )
            }
            Self::ForeignHookPath { worktree, observed } => write!(
                formatter,
                "refusing to replace foreign core.hooksPath {observed:?} in {}; \
                 keep Rust and predecessor hooks in separate worktrees",
                worktree.display()
            ),
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
                "hook activation failed: expected {}, found {observed}",
                expected.display()
            ),
            Self::Appliance {
                state,
                detail,
                repair,
            } => write!(
                formatter,
                "installed Spice appliance is {}: {detail}; preview repair with `{}` and apply \
                 its printed digest",
                state.as_str(),
                repair
                    .as_deref()
                    .unwrap_or("inspect the installed receipt before retrying"),
            ),
            Self::ApplianceInspect(error) => {
                write!(formatter, "inspect installed Spice appliance: {error}")
            }
            Self::Output(source) => write!(formatter, "hook activation output failed: {source}"),
        }
    }
}

impl std::error::Error for HookActivationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Inspect { source, .. } | Self::Launch { source, .. } | Self::Output(source) => {
                Some(source)
            }
            Self::ApplianceInspect(error) => Some(error),
            Self::Repository(_)
            | Self::HookNotExecutable(_)
            | Self::UnnameablePath(_)
            | Self::ForeignHookPath { .. }
            | Self::Exit { .. }
            | Self::NonUtf8 { .. }
            | Self::ConfigurationDrift { .. }
            | Self::Appliance { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::{HookActivationError, configured_hooks_path};

    #[test]
    fn appliance_refusal_names_state_evidence_and_exact_repair() {
        let error = HookActivationError::Appliance {
            state: spice::install::ApplianceState::HookTampered,
            detail: "fixture receipt-bound hook differs".to_owned(),
            repair: Some("target/debug/spice install".to_owned()),
        };

        assert_eq!(
            error.to_string(),
            concat!(
                "installed Spice appliance is hook-tampered: ",
                "fixture receipt-bound hook differs; preview repair with ",
                "`target/debug/spice install` and apply its printed digest"
            )
        );
    }

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
