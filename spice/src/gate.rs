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
    FocusedArmReport, GateId, HygieneConfiguration, KillRun, RepositoryExecutionEvent, RimCapture,
    Selector, StateRunScope, VerifierCompilerContract, VerifierCompilerEvidence,
    admit_verifier_compiler_receipt, encode_verifier_compiler_receipt,
};
use wire::Digest;

use crate::repository::{Repository, StateError};

const GIT: &str = "/usr/bin/git";
const PROCESS_PATH: &str = concat!("PA", "TH");
const CARGO_BUILD_JOBS: &str = "CARGO_BUILD_JOBS";

mod finding_source;

pub use finding_source::{
    GateFindingAnchor, capture_gate_finding_anchor, require_gate_finding_source_forward,
    validate_gate_finding_source,
};

/// One staged battery capture retained across the full verifier's actions.
///
/// Keeping this capture alive is what makes every compiler token and the later
/// battery execution name one immutable selected source rather than two
/// independently materialized readings of the index.
pub struct PreparedBattery<'trace> {
    capture: BatteryCapture<'trace>,
    verifier_environment: Vec<(OsString, OsString)>,
    cargo_jobs: Option<usize>,
    verifier_receipt_admitted: bool,
}

/// One retained staged capture selected for one canonical arm.
///
/// The ordinary change gate uses the always-on staging arm as its one source
/// capture. That gives test selection, policy selection, and the first focused
/// result one immutable root and digest without acquiring sibling families.
pub struct PreparedArm<'trace> {
    capture: BatteryCapture<'trace>,
    gate_id: GateId,
}

impl PreparedArm<'_> {
    /// Identity of the immutable selected source retained by this preparation.
    #[must_use]
    pub const fn source(&self) -> &Digest {
        self.capture.source()
    }

    /// Private selected-tree root retained for this preparation's lifetime.
    #[must_use]
    pub fn root(&self) -> &Path {
        self.capture.root()
    }

    /// Exact changed paths captured at the command rim for this source.
    #[must_use]
    pub fn changes(&self) -> &[String] {
        self.capture.changes()
    }

    /// Every regular path retained by the immutable selected tree.
    #[must_use]
    pub fn paths(&self) -> &[String] {
        self.capture.paths()
    }

    /// Prove the live selected source has not moved since this preparation.
    ///
    /// # Errors
    ///
    /// Returns the exact staged index, `HEAD`, or repository movement.
    pub fn verify_source(&self) -> Result<(), RunError> {
        self.capture
            .verify_source()
            .map_err(|error| RunError::refusal("gate::source-moved", error))
    }

    /// Execute the one canonical focused arm this capture selected.
    ///
    /// # Errors
    ///
    /// Returns the canonical focused-arm refusal without acquiring siblings.
    pub fn execute(&mut self) -> Result<FocusedArmReport, RunError> {
        self.capture
            .execute_arm_report(self.gate_id)
            .map_err(|error| RunError::refusal("gate::battery-refused", error))
    }

    /// Shared state transaction for additional focused captures in this run.
    #[must_use]
    pub fn state_run(&self) -> ::gate::BatteryStateRun {
        self.capture.state_run()
    }

    /// Publish all focused observations after the enclosing run accepts.
    pub fn accept_state(&self, scope: StateRunScope) -> Result<(), RunError> {
        self.capture
            .accept_state(scope)
            .map(|_| ())
            .map_err(|error| RunError::refusal("gate::state-publication-refused", error))
    }

    /// Discard all focused observations after the enclosing run rejects.
    pub fn reject_state(&self) -> Result<(), RunError> {
        self.capture
            .reject_state()
            .map_err(|error| RunError::fault("gate::state-rejection-unavailable", error))
    }
}

impl PreparedBattery<'_> {
    /// Identity of the immutable selected source retained by this preparation.
    #[must_use]
    pub const fn source(&self) -> &Digest {
        self.capture.source()
    }

    /// Private selected-tree root retained for this preparation's lifetime.
    #[must_use]
    pub fn root(&self) -> &Path {
        self.capture.root()
    }

    /// Exact changed paths captured at the command rim for this source.
    #[must_use]
    pub fn changes(&self) -> &[String] {
        self.capture.changes()
    }

    /// Execute the existing canonical mutation profile over the retained source.
    ///
    /// # Errors
    ///
    /// Returns the canonical mutation refusal; no second selection is captured.
    pub fn execute_kill(&self) -> Result<KillRun, RunError> {
        self.capture
            .execute_kill()
            .map_err(|error| RunError::refusal("gate::kill-refused", error))
    }

    /// Project the selected file-shape and routine-complexity arms early.
    ///
    /// The complete battery later reuses the same capture and cached metrics;
    /// this seam exists only to refuse structural defects before expensive
    /// external verifier suites begin.
    pub fn execute_structural_preflight(&mut self) -> Result<Vec<FocusedArmReport>, RunError> {
        self.capture
            .execute_structural_preflight()
            .map_err(|error| RunError::refusal("gate::structural-preflight-refused", error))
    }

    /// Capture an exact compiler contract for one reusable arm.
    ///
    /// # Errors
    ///
    /// Refuses a foreign arm, an unavailable/drifted declared tool, or a Cargo
    /// job cap that differs from the first contract minted by this capture.
    pub fn verifier_compiler_contract(
        &mut self,
        gate_id: GateId,
        cargo_jobs: usize,
    ) -> Result<VerifierCompilerContract, RunError> {
        bind_cargo_jobs(
            &mut self.verifier_environment,
            &mut self.cargo_jobs,
            cargo_jobs,
        )?;
        self.capture
            .verifier_compiler_contract(gate_id, self.verifier_environment.clone())
            .map_err(|error| RunError::refusal("gate::verifier-evidence-refused", error))
    }

    /// Capture an exact compiler contract when its reusable arm is applicable.
    ///
    /// # Errors
    ///
    /// Refuses a foreign arm, an unavailable/drifted declared tool, or a Cargo
    /// job cap that differs from the first contract minted by this capture.
    pub fn applicable_verifier_compiler_contract(
        &mut self,
        gate_id: GateId,
        cargo_jobs: usize,
    ) -> Result<Option<VerifierCompilerContract>, RunError> {
        bind_cargo_jobs(
            &mut self.verifier_environment,
            &mut self.cargo_jobs,
            cargo_jobs,
        )?;
        self.capture
            .applicable_verifier_compiler_contract(gate_id, self.verifier_environment.clone())
            .map_err(|error| RunError::refusal("gate::verifier-evidence-refused", error))
    }

    /// Admit a cross-process compiler receipt against this retained capture.
    ///
    /// # Errors
    ///
    /// Refuses missing, stale, partial, mismatched, or untrusted evidence.
    pub fn admit_verifier_compiler_receipt(
        &mut self,
        receipt: &[u8],
        cargo_jobs: usize,
        authority: &[u8],
    ) -> Result<Vec<VerifierCompilerEvidence>, RunError> {
        let mut contracts = Vec::new();
        for gate_id in [GateId::RustTypecheck, GateId::CanvasWebTypecheck] {
            if let Some(contract) =
                self.applicable_verifier_compiler_contract(gate_id, cargo_jobs)?
            {
                contracts.push(contract);
            }
        }
        let evidence = admit_verifier_compiler_receipt(receipt, &contracts, authority)
            .map_err(|error| RunError::refusal("gate::verifier-receipt-refused", error))?;
        self.verifier_receipt_admitted = true;
        Ok(evidence)
    }

    /// Execute this retained capture with any completed compiler evidence.
    ///
    /// # Errors
    ///
    /// Returns the canonical battery refusal.
    pub fn execute(
        &mut self,
        evidence: &[VerifierCompilerEvidence],
    ) -> Result<BatteryReport, RunError> {
        let result = if self.verifier_receipt_admitted {
            self.capture.execute_with_required_verifier_evidence(
                evidence,
                self.verifier_environment.clone(),
            )
        } else {
            self.capture
                .execute_with_verifier_evidence(evidence, self.verifier_environment.clone())
        };
        result.map_err(|error| RunError::refusal("gate::battery-refused", error))
    }

    /// Publish the exhaustive state transaction after the enclosing verifier accepts.
    pub fn accept_state(&self) -> Result<(), RunError> {
        self.capture
            .accept_state(StateRunScope::Exhaustive)
            .map(|_| ())
            .map_err(|error| RunError::refusal("gate::state-publication-refused", error))
    }

    /// Discard the exhaustive state transaction after refusal or interruption.
    pub fn reject_state(&self) -> Result<(), RunError> {
        self.capture
            .reject_state()
            .map_err(|error| RunError::fault("gate::state-rejection-unavailable", error))
    }
}

fn bind_cargo_jobs(
    environment: &mut Vec<(OsString, OsString)>,
    bound: &mut Option<usize>,
    requested: usize,
) -> Result<(), RunError> {
    let requested = requested.max(1);
    match *bound {
        Some(current) if current != requested => Err(RunError::refusal(
            "gate::verifier-budget-refused",
            format!(
                concat!(
                    "full verifier Cargo job cap changed from {} to {} ",
                    "after capture"
                ),
                current, requested
            ),
        )),
        Some(_) => Ok(()),
        None => {
            environment.retain(|(name, _)| name != CARGO_BUILD_JOBS);
            environment.push((
                OsString::from(CARGO_BUILD_JOBS),
                OsString::from(requested.to_string()),
            ));
            environment.sort();
            *bound = Some(requested);
            Ok(())
        }
    }
}

/// Capture the selected source once for a full verifier run.
///
/// # Errors
///
/// Returns the same rim and capture failures as [`run`].
pub fn prepare(selector: Selector) -> Result<PreparedBattery<'static>, RunError> {
    let verifier_environment = verifier_environment();
    capture(selector, None, None).map(|capture| PreparedBattery {
        capture,
        verifier_environment,
        cargo_jobs: None,
        verifier_receipt_admitted: false,
    })
}

/// Capture a traced complete battery into an existing state transaction.
pub fn prepare_in_state_traced<'trace>(
    selector: Selector,
    state_run: ::gate::BatteryStateRun,
    trace: &'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send),
) -> Result<PreparedBattery<'trace>, RunError> {
    let verifier_environment = verifier_environment();
    capture_inner(selector, None, Some(trace), Some(state_run)).map(|capture| PreparedBattery {
        capture,
        verifier_environment,
        cargo_jobs: None,
        verifier_receipt_admitted: false,
    })
}

/// Capture one selected source for exactly one canonical battery arm.
///
/// # Errors
///
/// Returns the same rim and selected-arm capture failures as [`run_arm_report`].
pub fn prepare_arm(selector: Selector, gate_id: GateId) -> Result<PreparedArm<'static>, RunError> {
    capture(selector, Some(gate_id), None).map(|capture| PreparedArm { capture, gate_id })
}

/// Capture one focused arm into an existing enclosing state transaction.
pub fn prepare_arm_in_state(
    selector: Selector,
    gate_id: GateId,
    state_run: ::gate::BatteryStateRun,
) -> Result<PreparedArm<'static>, RunError> {
    capture_in_state(selector, gate_id, state_run).map(|capture| PreparedArm { capture, gate_id })
}

/// Compile every reusable verifier stage and encode its source-bound receipt.
///
/// This operation is intentionally separate from the bounded commit worker.
/// The later consumer admits the receipt against a fresh capture and never
/// falls back to compiler execution when the artifact is absent or stale.
///
/// # Errors
///
/// Returns capture, compiler, completeness, or receipt-encoding failures.
pub fn verifier_compiler_receipt(
    selector: Selector,
    cargo_jobs: usize,
    authority: &[u8],
) -> Result<Vec<u8>, RunError> {
    let mut battery = prepare(selector)?;
    let mut evidence = Vec::new();
    for gate_id in [GateId::RustTypecheck, GateId::CanvasWebTypecheck] {
        let Some(contract) = battery.applicable_verifier_compiler_contract(gate_id, cargo_jobs)?
        else {
            continue;
        };
        let compiler = contract
            .prepare_stable()
            .map_err(|error| RunError::fault("gate::verifier-source-unavailable", error))?;
        let mut processes = Vec::new();
        for stage in compiler.stages() {
            let run = compiler
                .run(*stage)
                .map_err(|error| RunError::fault("gate::verifier-compiler-unavailable", error))?;
            if !run.success() {
                return Err(RunError::refusal(
                    "gate::verifier-compiler-refused",
                    verifier_compiler_failure(*stage, &run),
                ));
            }
            processes.push(run.into_evidence().ok_or_else(|| {
                RunError::fault(
                    "gate::verifier-compiler-evidence-missing",
                    format!("successful {stage:?} run minted no compiler evidence"),
                )
            })?);
        }
        evidence.push(
            compiler
                .complete(&processes)
                .map_err(|error| RunError::refusal("gate::verifier-evidence-refused", error))?,
        );
    }
    encode_verifier_compiler_receipt(&evidence, authority)
        .map_err(|error| RunError::fault("gate::verifier-receipt-unavailable", error))
}

/// Execute a traced battery using only a fresh, exact compiler receipt.
///
/// Receipt admission occurs before battery execution. Missing, stale, partial,
/// or mismatched evidence is a refusal and never invokes a compiler here.
///
/// # Errors
///
/// Returns capture, receipt-admission, or canonical battery failures.
pub fn run_traced_with_verifier_receipt(
    selector: Selector,
    receipt: &[u8],
    cargo_jobs: usize,
    authority: &[u8],
    trace: &mut (dyn FnMut(RepositoryExecutionEvent) + Send),
) -> Result<BatteryReport, RunError> {
    let mut prepared = prepare_traced(selector, trace)?;
    let evidence = prepared.admit_verifier_compiler_receipt(receipt, cargo_jobs, authority)?;
    let report = prepared.execute(&evidence)?;
    if report.admits() {
        prepared.accept_state()?;
    } else {
        prepared.reject_state()?;
    }
    Ok(report)
}

fn verifier_compiler_failure(
    stage: ::gate::VerifierCompilerStage,
    run: &::gate::VerifierCompilerRun,
) -> String {
    let completion = if run.cancelled() {
        "cancelled"
    } else if run.timed_out() {
        "timed-out"
    } else {
        "exited"
    };
    verifier_compiler_failure_detail(stage, completion, run.code(), run.stdout(), run.stderr())
}

fn verifier_compiler_failure_detail(
    stage: ::gate::VerifierCompilerStage,
    completion: &str,
    code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
) -> String {
    format!(
        "stage={stage:?}; completion={completion}; code={code:?}; stdout={}; stderr={}",
        bounded_compiler_output(stdout),
        bounded_compiler_output(stderr),
    )
}

fn bounded_compiler_output(bytes: &[u8]) -> String {
    const LIMIT: usize = 16 * 1024;
    let start = bytes.len().saturating_sub(LIMIT);
    String::from_utf8_lossy(&bytes[start..]).trim().to_owned()
}

/// Capture the selected source once while exposing execution progress.
///
/// The returned preparation retains the trace for its lifetime, so test
/// selection and the later battery execution can share one immutable source
/// without losing the supervisor's exact family/arm cursor.
///
/// # Errors
///
/// Returns the same rim and capture failures as [`prepare`].
pub fn prepare_traced<'trace>(
    selector: Selector,
    trace: &'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send),
) -> Result<PreparedBattery<'trace>, RunError> {
    let verifier_environment = verifier_environment();
    capture(selector, None, Some(trace)).map(|capture| PreparedBattery {
        capture,
        verifier_environment,
        cargo_jobs: None,
        verifier_receipt_admitted: false,
    })
}

#[expect(
    clippy::disallowed_methods,
    reason = "the canonical gate command rim owns full-verifier environment acquisition"
)]
fn verifier_environment() -> Vec<(OsString, OsString)> {
    let mut environment = env::vars_os().collect::<Vec<_>>();
    environment.sort();
    environment
}

/// Execute the canonical battery over one selected source.
///
/// # Errors
///
/// Returns the exact rim acquisition or battery refusal. No alternate command
/// or predecessor implementation is attempted.
pub fn run(selector: Selector) -> Result<BatteryReport, RunError> {
    let mut prepared = prepare(selector)?;
    let report = prepared.execute(&[])?;
    if report.admits() {
        prepared.accept_state()?;
    } else {
        prepared.reject_state()?;
    }
    Ok(report)
}

/// Execute one named canonical gate arm over one selected source.
///
/// # Errors
///
/// Returns the same acquisition and arm refusal as [`run`], without executing
/// sibling battery arms.
pub fn run_arm(selector: Selector, gate_id: GateId) -> Result<BatteryArmResult, RunError> {
    run_arm_report(selector, gate_id).map(FocusedArmReport::into_result)
}

/// Execute one named canonical gate arm and retain its verifiable input context.
///
/// # Errors
///
/// Returns the same acquisition and arm refusal as [`run_arm`], without
/// executing sibling battery arms.
pub fn run_arm_report(selector: Selector, gate_id: GateId) -> Result<FocusedArmReport, RunError> {
    let mut prepared = prepare_arm(selector, gate_id)?;
    let report = prepared.execute()?;
    if report.admits() {
        prepared.accept_state(StateRunScope::Partial)?;
    } else {
        prepared.reject_state()?;
    }
    Ok(report)
}

/// Execute the canonical battery and mint its current receipt.
///
/// # Errors
///
/// Returns the same acquisition and completeness refusals as [`run`].
pub fn receipt(selector: Selector) -> Result<BatteryReceipt, RunError> {
    let mut prepared = prepare(selector)?;
    let receipt = prepared
        .capture
        .execute_receipt()
        .map_err(|error| RunError::refusal("gate::battery-refused", error))?;
    if receipt.run().admits() {
        prepared.accept_state()?;
    } else {
        prepared.reject_state()?;
    }
    Ok(receipt)
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
    trace: &mut (dyn FnMut(RepositoryExecutionEvent) + Send),
) -> Result<BatteryReport, RunError> {
    let mut prepared = prepare_traced(selector, trace)?;
    let report = prepared.execute(&[])?;
    if report.admits() {
        prepared.accept_state()?;
    } else {
        prepared.reject_state()?;
    }
    Ok(report)
}

fn capture_in_state(
    selector: Selector,
    gate_id: GateId,
    state_run: ::gate::BatteryStateRun,
) -> Result<BatteryCapture<'static>, RunError> {
    capture_inner(selector, Some(gate_id), None, Some(state_run))
}

fn capture<'trace>(
    selector: Selector,
    selected: Option<GateId>,
    trace: Option<&'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send)>,
) -> Result<BatteryCapture<'trace>, RunError> {
    capture_inner(selector, selected, trace, None)
}

fn capture_inner<'trace>(
    selector: Selector,
    selected: Option<GateId>,
    trace: Option<&'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send)>,
    state_run: Option<::gate::BatteryStateRun>,
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
        // BatteryStateRun supplies the fixed clock and active claims from the
        // exact authority every focused capture shares. The command rim makes
        // no competing claim observation of its own.
        bound_claims: Vec::new(),
    };
    let claim = ClaimCapture::Idle;
    let hygiene = HygieneConfiguration::current();
    let root = repository.worktree_root();
    match (selected, trace, state_run) {
        (Some(gate_id), None, Some(state_run)) => BatteryCapture::capture_arm_in_state(
            root, &selector, claim, actor, rim, hygiene, gate_id, state_run,
        ),
        (Some(gate_id), Some(trace), None) => BatteryCapture::capture_arm_traced(
            root, &selector, claim, actor, rim, hygiene, gate_id, trace,
        ),
        (Some(gate_id), None, None) => {
            BatteryCapture::capture_arm(root, &selector, claim, actor, rim, hygiene, gate_id)
        }
        (None, Some(trace), None) => {
            BatteryCapture::capture_traced(root, &selector, claim, actor, rim, hygiene, trace)
        }
        (None, None, None) => BatteryCapture::capture(root, &selector, claim, actor, rim, hygiene),
        (None, Some(trace), Some(state_run)) => BatteryCapture::capture_in_state_traced(
            root, &selector, claim, actor, rim, hygiene, state_run, trace,
        ),
        (None, None, Some(_)) | (Some(_), Some(_), Some(_)) => {
            unreachable!("only complete shared-state captures are traced")
        }
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
            // Report renames as deletion plus addition so both ownership and
            // policy classes survive independently of user Git configuration.
            "--no-renames",
            "--name-only",
            "-z",
            "--diff-filter=ACMRD",
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

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;
