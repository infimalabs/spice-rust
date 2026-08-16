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
    ActorCapture, BatteryArmResult, BatteryCapture, BatteryReceipt, BatteryReport,
    ChangeSelectionReport, ClaimCapture, FocusedArmReport, GateId, HygieneConfiguration, KillRun,
    RepositoryExecutionEvent, RimCapture, Selector, StateRunScope, VerifierCompilerContract,
    VerifierCompilerEvidence, admit_verifier_compiler_receipt, encode_verifier_compiler_receipt,
};
use wire::Digest;

use crate::repository::{Repository, StateError};

const GIT: &str = "/usr/bin/git";
const PROCESS_PATH: &str = concat!("PA", "TH");
const CARGO_BUILD_JOBS: &str = "CARGO_BUILD_JOBS";

#[derive(Clone, Copy)]
enum InputMode {
    Generic,
    Strict,
}

mod capture;
mod environment;
mod finding_source;
mod full_verifier_evidence;

use environment::verifier_environment;
#[cfg(test)]
use environment::{SHELL_HOOK_PREFIX, SHELL_LAST_COMMAND, verifier_environment_from};

pub use finding_source::{
    GateFindingAnchor, capture_gate_finding_anchor, require_gate_finding_source_forward,
    validate_gate_finding_source,
};
pub use full_verifier_evidence::{
    AdmittedFullVerifierEvidence, AdmittedSemanticVerifierEvidence, FullVerifierEvidence,
    SemanticFullVerifierAdmission, admit_full_verifier_evidence,
    admit_semantic_full_verifier_evidence,
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

    /// Tracked generated-path selectors captured from this selected tree.
    #[must_use]
    pub fn generated_paths(&self) -> &[String] {
        self.capture.generated_paths()
    }

    /// Complete immutable authority context captured for this focused arm.
    pub fn context(&self) -> Result<::gate::BatteryContext, RunError> {
        self.capture
            .context()
            .map_err(|error| RunError::refusal("gate::context-refused", error))
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

    /// Every regular path retained by the immutable selected tree.
    #[must_use]
    pub fn paths(&self) -> &[String] {
        self.capture.paths()
    }

    /// Tracked generated-path selectors captured from this selected tree.
    #[must_use]
    pub fn generated_paths(&self) -> &[String] {
        self.capture.generated_paths()
    }

    /// Complete immutable authority context captured for this battery.
    pub fn context(&self) -> Result<::gate::BatteryContext, RunError> {
        self.capture
            .context()
            .map_err(|error| RunError::refusal("gate::context-refused", error))
    }

    /// Source-independent compiler authorities for semantic receipt comparison.
    pub fn semantic_compiler_authorities(
        &mut self,
        cargo_jobs: usize,
    ) -> Result<Vec<(GateId, Digest)>, RunError> {
        let mut authorities = Vec::new();
        for gate_id in [GateId::RustTypecheck, GateId::CanvasWebTypecheck] {
            if let Some(contract) =
                self.applicable_verifier_compiler_contract(gate_id, cargo_jobs)?
            {
                authorities.push((gate_id, contract.semantic_authority()));
            }
        }
        Ok(authorities)
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

    /// Re-admit one complete battery receipt against this fresh strict capture.
    pub fn admit_battery_receipt(&self, receipt: &[u8]) -> Result<BatteryReceipt, RunError> {
        self.capture
            .admit_receipt(receipt)
            .map_err(|error| RunError::refusal("gate::battery-receipt-refused", error))
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

    /// Execute the complete battery and bind its report to this capture.
    pub fn execute_receipt(
        &mut self,
        evidence: &[VerifierCompilerEvidence],
    ) -> Result<BatteryReceipt, RunError> {
        let report = self.execute(evidence)?;
        self.capture
            .mint_receipt(report)
            .map_err(|error| RunError::fault("gate::battery-receipt-unavailable", error))
    }

    /// Rebind stable-source compiler proof to this retained battery capture and
    /// execute with required evidence.
    ///
    /// The stable private mirror exists only to preserve Cargo fingerprints;
    /// the receipt boundary reconstructs opaque process tokens against this
    /// capture's exact source, repository, environment, tools, and stages. A
    /// missing or mismatched proof refuses instead of falling back to compilers
    /// whose stable source locks are still held by the verifier.
    ///
    /// # Errors
    ///
    /// Refuses incomplete, stale, mismatched, noncanonical, or untrusted
    /// compiler evidence and all ordinary battery receipt failures.
    pub fn execute_prepared_compiler_receipt(
        &mut self,
        evidence: &[VerifierCompilerEvidence],
        cargo_jobs: usize,
    ) -> Result<BatteryReceipt, RunError> {
        let authority = full_verifier_evidence::prepared_compiler_authority(self.source());
        let receipt = encode_verifier_compiler_receipt(evidence, &authority)
            .map_err(|error| RunError::fault("gate::verifier-receipt-unavailable", error))?;
        let evidence = self.admit_verifier_compiler_receipt(&receipt, cargo_jobs, &authority)?;
        self.execute_receipt(&evidence)
    }

    /// Publish the exhaustive state transaction after the enclosing verifier accepts.
    pub fn accept_state(&self) -> Result<::gate::StateReceipt, RunError> {
        self.capture
            .accept_state(StateRunScope::Exhaustive)
            .map_err(|error| RunError::refusal("gate::state-publication-refused", error))
    }

    /// Re-admit a complete battery receipt across its exact accepted state transition.
    pub fn admit_battery_receipt_after_state(
        &self,
        receipt: &[u8],
        state: &::gate::StateReceipt,
    ) -> Result<BatteryReceipt, RunError> {
        self.capture
            .admit_receipt_after_state(receipt, state)
            .map_err(|error| RunError::refusal("gate::battery-receipt-refused", error))
    }

    /// Re-admit a receipt's global authorities before per-arm semantic comparison.
    pub fn admit_battery_receipt_for_semantic_comparison(
        &self,
        receipt: &[u8],
    ) -> Result<BatteryReceipt, RunError> {
        self.capture
            .admit_receipt_for_semantic_comparison(receipt)
            .map_err(|error| RunError::refusal("gate::battery-receipt-refused", error))
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
    prepare_with_mode(selector, InputMode::Generic)
}

/// Capture the current source repository's strict self-hosting contract.
///
/// Unlike installed generic acquisition, this requires both tracked input
/// documents and identifies every compiled legacy parser tool before returning.
pub fn prepare_strict(selector: Selector) -> Result<PreparedBattery<'static>, RunError> {
    prepare_with_mode(selector, InputMode::Strict)
}

fn prepare_with_mode(
    selector: Selector,
    input_mode: InputMode,
) -> Result<PreparedBattery<'static>, RunError> {
    let verifier_environment = verifier_environment();
    capture(selector, input_mode, None, None).map(|capture| PreparedBattery {
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
    capture_inner(
        selector,
        InputMode::Generic,
        None,
        Some(trace),
        Some(state_run),
    )
    .map(|capture| PreparedBattery {
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
    prepare_arm_with_mode(selector, gate_id, InputMode::Generic)
}

/// Capture one current-source arm under the strict self-hosting contract.
pub fn prepare_arm_strict(
    selector: Selector,
    gate_id: GateId,
) -> Result<PreparedArm<'static>, RunError> {
    prepare_arm_with_mode(selector, gate_id, InputMode::Strict)
}

fn prepare_arm_with_mode(
    selector: Selector,
    gate_id: GateId,
    input_mode: InputMode,
) -> Result<PreparedArm<'static>, RunError> {
    capture(selector, input_mode, Some(gate_id), None)
        .map(|capture| PreparedArm { capture, gate_id })
}

/// Capture one focused arm into an existing enclosing state transaction.
pub fn prepare_arm_in_state(
    selector: Selector,
    gate_id: GateId,
    state_run: ::gate::BatteryStateRun,
) -> Result<PreparedArm<'static>, RunError> {
    prepare_arm_in_state_with_mode(selector, gate_id, state_run, InputMode::Generic)
}

/// Capture one current-source arm into an existing state transaction.
pub fn prepare_arm_in_state_strict(
    selector: Selector,
    gate_id: GateId,
    state_run: ::gate::BatteryStateRun,
) -> Result<PreparedArm<'static>, RunError> {
    prepare_arm_in_state_with_mode(selector, gate_id, state_run, InputMode::Strict)
}

/// Capture one generic installed arm with its validated authored reach in an
/// existing state transaction.
pub fn prepare_arm_in_state_reached(
    selector: Selector,
    selection: &ChangeSelectionReport,
    gate_id: GateId,
    state_run: ::gate::BatteryStateRun,
) -> Result<PreparedArm<'static>, RunError> {
    capture_reached_in_state(selector, InputMode::Generic, selection, gate_id, state_run)
        .map(|capture| PreparedArm { capture, gate_id })
}

/// Capture one strict current-source arm with its validated authored reach in
/// an existing state transaction.
pub fn prepare_arm_in_state_reached_strict(
    selector: Selector,
    selection: &ChangeSelectionReport,
    gate_id: GateId,
    state_run: ::gate::BatteryStateRun,
) -> Result<PreparedArm<'static>, RunError> {
    capture_reached_in_state(selector, InputMode::Strict, selection, gate_id, state_run)
        .map(|capture| PreparedArm { capture, gate_id })
}

fn prepare_arm_in_state_with_mode(
    selector: Selector,
    gate_id: GateId,
    state_run: ::gate::BatteryStateRun,
    input_mode: InputMode,
) -> Result<PreparedArm<'static>, RunError> {
    capture_in_state(selector, input_mode, gate_id, state_run)
        .map(|capture| PreparedArm { capture, gate_id })
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
    let mut battery = prepare_strict(selector)?;
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
    let mut prepared = prepare_traced_with_mode(selector, trace, InputMode::Strict)?;
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
    prepare_traced_with_mode(selector, trace, InputMode::Generic)
}

fn prepare_traced_with_mode<'trace>(
    selector: Selector,
    trace: &'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send),
    input_mode: InputMode,
) -> Result<PreparedBattery<'trace>, RunError> {
    let verifier_environment = verifier_environment();
    capture(selector, input_mode, None, Some(trace)).map(|capture| PreparedBattery {
        capture,
        verifier_environment,
        cargo_jobs: None,
        verifier_receipt_admitted: false,
    })
}

/// Execute the canonical battery over one selected source.
///
/// # Errors
///
/// Returns the exact rim acquisition or battery refusal. No alternate command
/// or predecessor implementation is attempted.
pub fn run(selector: Selector) -> Result<BatteryReport, RunError> {
    run_with_mode(selector, InputMode::Generic)
}

/// Execute the current source repository's strict self-hosting battery.
pub fn run_strict(selector: Selector) -> Result<BatteryReport, RunError> {
    run_with_mode(selector, InputMode::Strict)
}

fn run_with_mode(selector: Selector, input_mode: InputMode) -> Result<BatteryReport, RunError> {
    let mut prepared = prepare_with_mode(selector, input_mode)?;
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
    run_arm_report_with_mode(selector, gate_id, InputMode::Generic)
}

/// Execute one current-source arm under the strict self-hosting contract.
pub fn run_arm_report_strict(
    selector: Selector,
    gate_id: GateId,
) -> Result<FocusedArmReport, RunError> {
    run_arm_report_with_mode(selector, gate_id, InputMode::Strict)
}

fn run_arm_report_with_mode(
    selector: Selector,
    gate_id: GateId,
    input_mode: InputMode,
) -> Result<FocusedArmReport, RunError> {
    let mut prepared = prepare_arm_with_mode(selector, gate_id, input_mode)?;
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
    input_mode: InputMode,
    gate_id: GateId,
    state_run: ::gate::BatteryStateRun,
) -> Result<BatteryCapture<'static>, RunError> {
    capture_inner(selector, input_mode, Some(gate_id), None, Some(state_run))
}

fn capture_reached_in_state(
    selector: Selector,
    input_mode: InputMode,
    selection: &ChangeSelectionReport,
    gate_id: GateId,
    state_run: ::gate::BatteryStateRun,
) -> Result<BatteryCapture<'static>, RunError> {
    capture::capture_inner_with_reach(
        selector,
        input_mode,
        Some(gate_id),
        None,
        Some(state_run),
        Some(selection),
    )
}

fn capture<'trace>(
    selector: Selector,
    input_mode: InputMode,
    selected: Option<GateId>,
    trace: Option<&'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send)>,
) -> Result<BatteryCapture<'trace>, RunError> {
    capture_inner(selector, input_mode, selected, trace, None)
}

fn capture_inner<'trace>(
    selector: Selector,
    input_mode: InputMode,
    selected: Option<GateId>,
    trace: Option<&'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send)>,
    state_run: Option<::gate::BatteryStateRun>,
) -> Result<BatteryCapture<'trace>, RunError> {
    capture::capture_inner_with_reach(selector, input_mode, selected, trace, state_run, None)
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
