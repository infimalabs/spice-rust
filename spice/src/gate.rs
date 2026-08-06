//! The one native command rim for a canonical gate battery.
//!
//! Every public gate entrypoint calls this module instead of assembling a
//! contract, invoking another command, or retaining a predecessor runtime.
//! Ambient process state is captured once here and handed to `gate` as typed
//! observations; acquisition and decisions remain owned by the gate crate.

use std::env;
use std::ffi::OsString;
use std::fmt;
use std::path::Path;
use std::process::Command;

use ::gate::{
    ActorCapture, BatteryArmResult, BatteryCapture, BatteryReceipt, BatteryReport, ClaimCapture,
    GateId, HygieneConfiguration, RepositoryExecutionEvent, RimCapture, Selector,
};

use crate::repository::{Repository, StateError};

const GIT: &str = "/usr/bin/git";
const PROCESS_PATH: &str = concat!("PA", "TH");
/// Execute the canonical battery over one selected source.
///
/// # Errors
///
/// Returns the exact rim acquisition or battery refusal. No alternate command
/// or predecessor implementation is attempted.
pub fn run(selector: Selector) -> Result<BatteryReport, RunError> {
    let mut capture = capture(selector, None, None)?;
    capture
        .execute()
        .map_err(|error| RunError::refusal("gate::battery-refused", error))
}

/// Execute one named canonical gate arm over one selected source.
///
/// # Errors
///
/// Returns the same acquisition and arm refusal as [`run`], without executing
/// sibling battery arms.
pub fn run_arm(selector: Selector, gate_id: GateId) -> Result<BatteryArmResult, RunError> {
    let mut capture = capture(selector, Some(gate_id), None)?;
    capture
        .execute_arm(gate_id)
        .map_err(|error| RunError::refusal("gate::battery-refused", error))
}

/// Execute the canonical battery and mint its current receipt.
///
/// # Errors
///
/// Returns the same acquisition and completeness refusals as [`run`].
pub fn receipt(selector: Selector) -> Result<BatteryReceipt, RunError> {
    let mut capture = capture(selector, None, None)?;
    capture
        .execute_receipt()
        .map_err(|error| RunError::refusal("gate::battery-refused", error))
}

/// Execute as [`run`] while exposing the gate-owned invocation trace.
///
/// This is an observation seam for tests and diagnostics. It cannot replace
/// an arm, alter its evidence, or grant execution authority.
///
/// # Errors
///
/// Returns the same acquisition and completeness refusals as [`run`].
pub fn run_traced(
    selector: Selector,
    trace: &mut dyn FnMut(RepositoryExecutionEvent),
) -> Result<BatteryReport, RunError> {
    let mut capture = capture(selector, None, Some(trace))?;
    capture
        .execute()
        .map_err(|error| RunError::refusal("gate::battery-refused", error))
}

fn capture<'trace>(
    selector: Selector,
    selected: Option<GateId>,
    trace: Option<&'trace mut dyn FnMut(RepositoryExecutionEvent)>,
) -> Result<BatteryCapture<'trace>, RunError> {
    let directory = env::current_dir()
        .map_err(|error| RunError::fault("gate::working-directory-unavailable", error))?;
    let repository = Repository::discover(&directory)
        .map_err(|error| RunError::fault("gate::repository-unresolved", error))?;
    repository.admit_state().map_err(|error| match error {
        StateError::Filesystem { .. } => RunError::fault("gate::state-unavailable", error),
        _ => RunError::refusal("gate::state-refused", error),
    })?;
    let actor = acting_actor(repository.worktree_root())?;
    let changes = selected_changes(repository.worktree_root(), &selector)?;
    let rim = RimCapture::new(directory, search_path(), changes)
        .map_err(|error| RunError::refusal("gate::rim-refused", error))?;
    let actor = ActorCapture::Acting {
        actor,
        // BatteryCapture reconciles this observation against the exact state
        // authority it loads. The daemonless command rim holds no clock or
        // mutation lease, so it asserts no still-active peer claim.
        bound_claims: Vec::new(),
    };
    let claim = ClaimCapture::Idle;
    let hygiene = HygieneConfiguration::current();
    let root = repository.worktree_root();
    match (selected, trace) {
        (Some(gate_id), Some(trace)) => BatteryCapture::capture_arm_traced(
            root, &selector, claim, actor, rim, hygiene, gate_id, trace,
        ),
        (Some(gate_id), None) => {
            BatteryCapture::capture_arm(root, &selector, claim, actor, rim, hygiene, gate_id)
        }
        (None, Some(trace)) => {
            BatteryCapture::capture_traced(root, &selector, claim, actor, rim, hygiene, trace)
        }
        (None, None) => BatteryCapture::capture(root, &selector, claim, actor, rim, hygiene),
    }
    .map_err(|error| {
        let detail = error.detail();
        let reason = if detail.starts_with("cannot load captured battery contract:") {
            "gate::contract-refused"
        } else if detail.starts_with("cannot capture battery source:") {
            "gate::selection-refused"
        } else {
            "gate::capture-refused"
        };
        RunError::refusal(reason, detail)
    })
}

#[expect(
    clippy::disallowed_methods,
    reason = "the canonical gate command rim owns executable-path acquisition"
)]
fn search_path() -> Option<OsString> {
    env::var_os(PROCESS_PATH)
}

fn acting_actor(repository: &Path) -> Result<String, RunError> {
    let output = git(repository, &["config", "--get", "user.email"])?;
    let actor = std::str::from_utf8(&output)
        .map_err(|error| RunError::refusal("gate::actor-refused", error))?
        .trim();
    if actor.is_empty() || actor.contains(['\n', '\r']) {
        return Err(RunError::refusal(
            "gate::actor-refused",
            "Git user.email must name one nonempty single-line acting identity",
        ));
    }
    Ok(actor.to_owned())
}

fn selected_changes(repository: &Path, selector: &Selector) -> Result<Vec<String>, RunError> {
    let arguments: Vec<OsString> = match selector {
        Selector::Staged => [
            "diff",
            "--cached",
            "--name-only",
            "-z",
            "--diff-filter=ACMR",
        ]
        .into_iter()
        .map(OsString::from)
        .collect(),
        Selector::Revision(revision) => {
            if revision.starts_with('-') {
                return Err(RunError::refusal(
                    "gate::revision-refused",
                    format!("revision cannot be parsed as a Git option: {revision:?}"),
                ));
            }
            [
                "diff-tree",
                "--root",
                "--no-commit-id",
                "--name-only",
                "-r",
                "-z",
            ]
            .into_iter()
            .map(OsString::from)
            .chain(std::iter::once(OsString::from(revision)))
            .collect()
        }
    };
    let output = git_os(repository, &arguments).map_err(|error| match selector {
        Selector::Revision(_) if !error.is_fault() => {
            RunError::refusal("gate::selection-refused", error.detail())
        }
        _ => error,
    })?;
    let mut changes = output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            std::str::from_utf8(path)
                .map(str::to_owned)
                .map_err(|error| RunError::refusal("gate::changes-refused", error))
        })
        .collect::<Result<Vec<_>, _>>()?;
    changes.sort();
    changes.dedup();
    Ok(changes)
}

fn git(repository: &Path, arguments: &[&str]) -> Result<Vec<u8>, RunError> {
    let arguments = arguments.iter().map(OsString::from).collect::<Vec<_>>();
    git_os(repository, &arguments)
}

fn git_os(repository: &Path, arguments: &[OsString]) -> Result<Vec<u8>, RunError> {
    let output = Command::new(GIT)
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
        .map_err(|error| RunError::fault("gate::git-unavailable", error))?;
    if !output.status.success() {
        return Err(RunError::refusal(
            "gate::git-refused",
            format!(
                "{} {:?} exited {:?}: {}",
                GIT,
                arguments,
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(output.stdout)
}

/// One typed command-rim failure.
#[derive(Debug)]
pub struct RunError {
    reason: &'static str,
    detail: String,
    fault: bool,
}

impl RunError {
    fn refusal(reason: &'static str, detail: impl fmt::Display) -> Self {
        Self {
            reason,
            detail: detail.to_string(),
            fault: false,
        }
    }

    fn fault(reason: &'static str, detail: impl fmt::Display) -> Self {
        Self {
            reason,
            detail: detail.to_string(),
            fault: true,
        }
    }

    /// Stable command-surface refusal identity.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        self.reason
    }

    /// Complete underlying acquisition evidence.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// Whether this is an environment fault rather than an input refusal.
    #[must_use]
    pub const fn is_fault(&self) -> bool {
        self.fault
    }
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.reason, self.detail)
    }
}

impl std::error::Error for RunError {}
