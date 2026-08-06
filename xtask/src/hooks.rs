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

/// Which guard the published entry point carries, written beside it.
///
/// The published copy is a whole binary, so comparing it byte for byte answers
/// a question nobody asked: it differs after any edit anywhere in xtask's
/// dependency graph, which is nearly every commit, and a signal that fires on
/// every commit is one nobody reads. This records the one thing whose staleness
/// changes what Git enforces.
const GUARD_STAMP_PATH: &str = ".spice-install/bin/xtask.guard";

/// The guard the running build carries, as a digest of the source that decides
/// it.
///
/// This is deliberately narrower than the binary and deliberately wider than
/// nothing. It moves when `reference_transaction.rs` moves, which is exactly
/// when the admissions and refusals a published copy enforces stop matching the
/// tree. It does not move when something the guard merely depends on moves, so
/// a dependency that changes the guard's behaviour without touching its own
/// source is outside what this notices; the alternative that would catch that
/// case is the whole-binary comparison rejected above, which catches everything
/// and is therefore read by no one.
const GUARD_DIGEST: u64 = digest(include_str!("reference_transaction.rs").as_bytes());

/// FNV-1a, in a `const fn` so the digest is fixed at build time rather than
/// read from a file the published copy could disagree with.
const fn digest(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    hash
}

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
/// Nothing rebuilds at hook time, which is the whole point of publishing, so
/// the copy is exactly as current as the last activation left it. Activation
/// therefore cannot be the only thing that notices a stale copy, because it is
/// run once at cutover: it stamps [`GUARD_STAMP_PATH`] with the guard it is
/// publishing, and [`audit_entry_point`] is what reads that stamp afterwards.
///
/// A copy is current only if its bytes and its mode are both right. Byte
/// equality alone would let a published file whose executable bit was cleared
/// elsewhere be reported as current forever, while the hook that execs it
/// aborts every reference transaction in every worktree: the documented repair
/// would be the tool that just said nothing was wrong.
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
    if fs::read(&destination).is_ok_and(|published| published == bytes)
        && is_executable(&destination)?
    {
        stamp_guard(repository)?;
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
    stamp_guard(repository)?;
    writeln!(
        output,
        "published hook entry point {} from {}",
        destination.display(),
        entry_point.display()
    )
    .map_err(HookActivationError::Output)
}

/// Record which guard the copy just published carries.
///
/// Written on both publication paths, including the one that changed nothing:
/// equal bytes mean the same build, so the stamp it would write is the stamp
/// already there, and writing it anyway is what makes a missing stamp mean
/// "published before this was recorded" rather than "published without it".
fn stamp_guard(repository: &spice::repository::Repository) -> Result<(), HookActivationError> {
    let path = repository.common_dir().join(GUARD_STAMP_PATH);
    fs::write(&path, format!("{GUARD_DIGEST:016x}\n"))
        .map_err(|source| HookActivationError::Publish { path, source })
}

/// Refuse when the guard Git enforces is not the guard in this tree.
///
/// This is the half of currency that activation cannot provide. Publication
/// happens once, at cutover, so after that the copy eleven worktrees exec is
/// frozen: a landed change to `reference_transaction.rs` would otherwise sit
/// unenforced behind a green battery in every lane, with nothing anywhere
/// saying so. Reading the stamp costs one file read and needs no build, which
/// is why the check can live in front of the battery rather than inside it.
///
/// It refuses rather than re-publishing, and that is the decision rather than
/// an omission. Re-publishing here would make the guard eleven worktrees
/// enforce a function of which lane last ran the battery, and the lanes sit at
/// different commits, so the enforced policy would change under everyone
/// without an operator ever choosing it. Refusing costs one documented command
/// and leaves the choice of which build becomes authoritative where the cutover
/// already put it.
///
/// Nothing is owed while the Rust hooks are not live. Until `core.hooksPath`
/// names the installed set, no published copy is executed by anything and a
/// missing or stale one enforces nothing, so this reports that and admits.
///
/// # Errors
///
/// Returns repository discovery, Git, or output failures, and refuses with
/// [`HookActivationError::EntryPointStale`] when the published guard is absent
/// or differs from this build's.
pub fn audit_entry_point(root: &Path, output: &mut impl Write) -> Result<(), HookActivationError> {
    let repository = spice::repository::Repository::discover(root)
        .map_err(|error| HookActivationError::Repository(error.to_string()))?;
    let hooks = repository.common_dir().join(".spice-install/git-hooks");
    let configured = git(
        repository.worktree_root(),
        &["config", "--get", "core.hooksPath"],
        "read the active hooks path",
    )
    .ok()
    .filter(|listed| listed.status.success())
    .map(|listed| String::from_utf8_lossy(&listed.stdout).trim().to_owned())
    .unwrap_or_default();
    if configured != hooks.to_string_lossy() {
        return writeln!(
            output,
            "reference-transaction guard not activated; core.hooksPath is {configured:?}"
        )
        .map_err(HookActivationError::Output);
    }
    let path = repository.common_dir().join(GUARD_STAMP_PATH);
    let published = fs::read_to_string(&path)
        .ok()
        .and_then(|stamp| u64::from_str_radix(stamp.trim(), 16).ok());
    if published != Some(GUARD_DIGEST) {
        return Err(HookActivationError::EntryPointStale { path, published });
    }
    writeln!(
        output,
        "reference-transaction guard published at {GUARD_DIGEST:016x} matches this build"
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
    EntryPointStale {
        path: PathBuf,
        published: Option<u64>,
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
            Self::EntryPointStale { path, published } => {
                let published = published.map_or_else(
                    || "no stamp".to_owned(),
                    |published| format!("{published:016x}"),
                );
                write!(
                    formatter,
                    "the reference-transaction guard Git enforces is not this build's: \
                     {} records {published} and this build carries {GUARD_DIGEST:016x}; \
                     run `cargo xtask activate-hooks` to publish this build's guard",
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
            | Self::EntryPointStale { .. }
            | Self::UnnameablePath(_)
            | Self::Exit { .. }
            | Self::NonUtf8 { .. }
            | Self::ConfigurationDrift { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::digest;

    /// The stamp is a detector only if editing the guard moves it, so the edits
    /// it must not be blind to are pinned here rather than assumed from the
    /// algorithm's name.
    ///
    /// A fold without position -- an XOR, a sum -- agrees with itself across a
    /// transposition, and a fix that reorders two admissions would then publish
    /// as current forever. The second case is the same argument for length: a
    /// guard whose only change is a deleted trailing clause must still move.
    #[test]
    fn the_stamp_moves_for_edits_the_guard_can_receive() {
        assert_ne!(digest(b"refuse then admit"), digest(b"admit then refuse"));
        assert_ne!(digest(b"admit unless upstream"), digest(b"admit unless"));
        assert_ne!(digest(b""), digest(b"\0"));
    }
}
