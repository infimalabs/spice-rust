//! One capture of everything a battery run consumes and everything it runs.
//!
//! [`RepositoryExecutor`] already materializes one selection, loads the tracked
//! contract from it, and resolves the source identity every arm is labelled
//! with. The authorities the battery reads alongside those arms come from the
//! same three things, so they are taken here rather than assembled beside a
//! second materialization of the same selection.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use wire::{Digest, Domain};

use super::repository_executor::{RepositoryExecutor, VerifierContractAcquisition};
use super::{
    BatteryArmResult, BatteryError, BatteryInputs, BatteryReceipt, BatteryReport, BatteryScope,
    BatteryStateRun, BatteryToolEvidence, ConfigurationEvidence, RepositoryExecutionEvent,
    StateRunScope, run_battery,
};
use crate::core::{ActorCapture, ClaimCapture};
use crate::hygiene::HygieneConfiguration;
use crate::input::{BASELINE_PATH, Contract, POLICY_PATH};
use crate::manifest::OriginLayer;
use crate::policy::Policy as RulePolicy;
use crate::state::{ActorInputs, ActorResolution, BoundDecision, StateLedger, StateReceipt};
use crate::toolchain::RimCapture;
use crate::toolchain::{VerifierCompilerContract, VerifierCompilerEvidence};
use crate::tree::Selector;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InputMode {
    Strict,
    Generic,
}

/// Release whose grammar understood the tracked documents read here.
const CURRENT_RELEASE: &str = "current";

/// Every authority one battery run reads, owned by the capture that read them.
///
/// Held apart from the executor that runs against them because
/// [`run_battery`] borrows the authorities for as long as it borrows the
/// executor mutably. Keeping them in two fields of one capture is what lets a
/// caller hand over both without ever holding one without the other.
///
/// Nothing outside this module reads an authority back: what a run resolved is
/// published by the report's own context, digested and complete, rather than
/// handed out twice in two shapes.
struct BatteryAuthorities {
    source: Digest,
    actor: ActorResolution,
    scope: BatteryScope,
    configuration: Vec<ConfigurationEvidence>,
    policy: RulePolicy,
    state: StateLedger,
    bounds: Vec<BoundDecision>,
    tools: Vec<BatteryToolEvidence>,
}

fn authorities_match_after_state(
    prior: &super::BatteryContext,
    current: &super::BatteryContext,
    durable: &Digest,
    state: &StateReceipt,
) -> bool {
    state.accepted()
        && state.after() == durable
        && prior.source() == current.source()
        && prior.actor() == current.actor()
        && prior.scope() == current.scope()
        && prior.configuration() == current.configuration()
        && prior.policy() == current.policy()
        && prior.bounds() == current.bounds()
        && prior.latches() == current.latches()
        && prior.tools() == current.tools()
}

impl BatteryAuthorities {
    /// Borrow the complete authority set as one validated input contract.
    fn inputs(&self) -> Result<BatteryInputs<'_>, BatteryError> {
        BatteryInputs::new(
            &self.source,
            &self.actor,
            &self.scope,
            &self.configuration,
            &self.policy,
            &self.state,
            &self.bounds,
            &self.tools,
        )
    }
}

/// One selection turned into both the authorities and the executor.
///
/// The two halves are produced from a single materialization: a capture that
/// took the authorities from one selection and ran the arms against another
/// would publish a report whose context describes a tree its results never saw.
pub struct BatteryCapture<'trace> {
    authorities: BatteryAuthorities,
    executor: RepositoryExecutor<'trace>,
    selected: Option<crate::GateId>,
    state_run: BatteryStateRun,
}

impl BatteryCapture<'_> {
    /// Snapshot the complete current authority context without executing an arm.
    pub fn context(&self) -> Result<super::BatteryContext, BatteryError> {
        self.authorities.inputs()?.snapshot()
    }

    /// Re-admit one retained complete receipt against this exact fresh capture.
    ///
    /// Receipt decoding proves the retained document is canonical and complete;
    /// the comparisons here additionally bind it to every authority resolved by
    /// this capture, including the current baseline document.  This does not
    /// execute an arm or mutate the shared state transaction.
    pub fn admit_receipt(&self, document: &[u8]) -> Result<BatteryReceipt, BatteryError> {
        self.admit_receipt_inner(document, None)
    }

    /// Re-admit one retained complete receipt after its exact state transition.
    ///
    /// The complete verifier reads the ledger before it runs, then publishes
    /// the accepted ledger after every arm admits. A subsequent capture binds
    /// the transition's exact `after` identity to the durable ledger it loaded,
    /// while comparing freshly normalized latch and bound projections to the
    /// receipt. This permits irrelevant expiry normalization without permitting
    /// a persisted transition, gate-relevant state, or independent authority to
    /// drift.
    pub fn admit_receipt_after_state(
        &self,
        document: &[u8],
        state: &StateReceipt,
    ) -> Result<BatteryReceipt, BatteryError> {
        self.admit_receipt_inner(document, Some(state))
    }

    /// Re-admit a retained receipt for per-arm semantic comparison.
    ///
    /// The old source and state identities are deliberately not compared here;
    /// callers must compare independently derived per-arm source and state
    /// authorities before reusing any result. Every source-independent global
    /// authority and the current baseline remain exact. Tool identities are
    /// intentionally left to the consumer's per-arm declared-tool comparison.
    pub fn admit_receipt_for_semantic_comparison(
        &self,
        document: &[u8],
    ) -> Result<BatteryReceipt, BatteryError> {
        if self.selected.is_some() {
            return Err(BatteryError::new(
                "semantic battery receipt admission requires a complete capture",
            ));
        }
        let receipt = BatteryReceipt::verify(document)
            .map_err(|error| BatteryError::new(format!("battery receipt refused: {error}")))?;
        if !receipt.run().admits() {
            return Err(BatteryError::new(
                "semantic battery receipt does not carry an admitted complete run",
            ));
        }
        let current = self.authorities.inputs()?.snapshot()?;
        let prior = receipt.run().context();
        if prior.actor() != current.actor()
            || prior.scope() != current.scope()
            || prior.configuration() != current.configuration()
            || prior.policy() != current.policy()
        {
            return Err(BatteryError::new(
                "semantic battery receipt does not match current non-source authorities",
            ));
        }
        let baseline = self.executor.contract().baseline_digest().ok_or_else(|| {
            BatteryError::new(format!(
                "semantic battery receipt requires lower-is-better state at {BASELINE_PATH:?}"
            ))
        })?;
        if receipt.baseline() != &baseline {
            return Err(BatteryError::new(
                "semantic battery receipt does not match the current baseline authority",
            ));
        }
        Ok(receipt)
    }

    fn admit_receipt_inner(
        &self,
        document: &[u8],
        state: Option<&StateReceipt>,
    ) -> Result<BatteryReceipt, BatteryError> {
        if self.selected.is_some() {
            return Err(BatteryError::new(
                "battery receipt admission requires a complete capture",
            ));
        }
        let receipt = BatteryReceipt::verify(document)
            .map_err(|error| BatteryError::new(format!("battery receipt refused: {error}")))?;
        if !receipt.run().admits() {
            return Err(BatteryError::new(
                "battery receipt does not carry an admitted complete run",
            ));
        }
        let current = self.authorities.inputs()?.snapshot()?;
        let prior = receipt.run().context();
        let authorities_match = match state {
            Some(state) => {
                let durable = self.state_run.durable_ledger()?.digest();
                authorities_match_after_state(prior, &current, &durable, state)
            }
            None => prior == &current,
        };
        if !authorities_match {
            return Err(BatteryError::new(
                "battery receipt does not match the current captured authorities",
            ));
        }
        let baseline = self.executor.contract().baseline_digest().ok_or_else(|| {
            BatteryError::new(format!(
                "battery receipt requires lower-is-better state at {BASELINE_PATH:?}"
            ))
        })?;
        if receipt.baseline() != &baseline {
            return Err(BatteryError::new(
                "battery receipt does not match the current baseline authority",
            ));
        }
        Ok(receipt)
    }

    /// Prove the live selector source still names this capture.
    ///
    /// # Errors
    ///
    /// Returns the exact selected-tree movement diagnostic.
    pub fn verify_source(&self) -> Result<(), BatteryError> {
        self.executor.verify_source()
    }

    /// Every regular path retained by this exact selected source.
    #[must_use]
    pub fn paths(&self) -> &[String] {
        self.executor.paths()
    }

    /// Tracked generated-path selectors captured from this selected tree.
    #[must_use]
    pub fn generated_paths(&self) -> &[String] {
        self.executor.contract().shape().generated_paths()
    }

    /// Capture one repository into a complete, runnable battery.
    ///
    /// `claim`, `actor`, `rim`, and `hygiene` are the caller's observations of
    /// the live worktree, which no selected tree can supply. Everything else is
    /// read here: the policy, configuration, and tool evidence come from the
    /// contract this capture loaded, the scope from the selector it
    /// materialized, and the ledger from the repository it was taken from.
    ///
    /// # Errors
    ///
    /// Refuses selection, contract, source-identity, ledger, or authority
    /// failure before any arm can run. An absent actor is one of these: a
    /// battery report names the identity it was run by, and no other input can
    /// stand in for one.
    pub fn capture(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
    ) -> Result<Self, BatteryError> {
        Self::capture_inner(
            repository,
            selector,
            claim,
            actor,
            rim,
            hygiene,
            InputMode::Strict,
            None,
            None,
            None,
        )
    }

    /// Capture one repository with the installed generic input policy.
    ///
    /// Missing repository policy and lower-is-better state remain typed
    /// absences. Present documents are parsed strictly, and selected family
    /// arms retain sole authority to discover the tools they actually use.
    pub fn capture_generic(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
    ) -> Result<Self, BatteryError> {
        Self::capture_inner(
            repository,
            selector,
            claim,
            actor,
            rim,
            hygiene,
            InputMode::Generic,
            None,
            None,
            None,
        )
    }

    /// Capture one repository for exactly one canonical arm.
    ///
    /// # Errors
    ///
    /// Refuses the same pre-execution failures as [`Self::capture`].
    pub fn capture_arm(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
        gate_id: crate::GateId,
    ) -> Result<Self, BatteryError> {
        Self::capture_inner(
            repository,
            selector,
            claim,
            actor,
            rim,
            hygiene,
            InputMode::Strict,
            Some(gate_id),
            None,
            None,
        )
    }

    /// Capture exactly one canonical arm with generic installed inputs.
    pub fn capture_generic_arm(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
        gate_id: crate::GateId,
    ) -> Result<Self, BatteryError> {
        Self::capture_inner(
            repository,
            selector,
            claim,
            actor,
            rim,
            hygiene,
            InputMode::Generic,
            Some(gate_id),
            None,
            None,
        )
    }

    /// Capture one arm into an existing enclosing state transaction.
    #[expect(
        clippy::too_many_arguments,
        reason = "the shared state run accompanies six independent capture authorities"
    )]
    pub fn capture_arm_in_state(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
        gate_id: crate::GateId,
        state_run: BatteryStateRun,
    ) -> Result<Self, BatteryError> {
        Self::capture_inner(
            repository,
            selector,
            claim,
            actor,
            rim,
            hygiene,
            InputMode::Strict,
            Some(gate_id),
            None,
            Some(state_run),
        )
    }

    /// Capture one generic installed arm into an enclosing state transaction.
    #[expect(
        clippy::too_many_arguments,
        reason = "the shared state run accompanies six independent capture authorities"
    )]
    pub fn capture_generic_arm_in_state(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
        gate_id: crate::GateId,
        state_run: BatteryStateRun,
    ) -> Result<Self, BatteryError> {
        Self::capture_inner(
            repository,
            selector,
            claim,
            actor,
            rim,
            hygiene,
            InputMode::Generic,
            Some(gate_id),
            None,
            Some(state_run),
        )
    }

    /// Capture one strict selected arm with its validated authored reach in an
    /// existing state transaction.
    #[expect(
        clippy::too_many_arguments,
        reason = "the reach and shared state accompany six independent capture authorities"
    )]
    pub fn capture_arm_in_state_reached(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
        selection: &crate::ChangeSelectionReport,
        gate_id: crate::GateId,
        state_run: BatteryStateRun,
    ) -> Result<Self, BatteryError> {
        Self::capture_inner_with_reach(
            repository,
            selector,
            claim,
            actor,
            rim,
            hygiene,
            InputMode::Strict,
            Some(gate_id),
            None,
            Some(state_run),
            Some(selection),
        )
    }

    /// Capture one generic selected arm with its validated authored reach in
    /// an existing state transaction.
    #[expect(
        clippy::too_many_arguments,
        reason = "the reach and shared state accompany six independent capture authorities"
    )]
    pub fn capture_generic_arm_in_state_reached(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
        selection: &crate::ChangeSelectionReport,
        gate_id: crate::GateId,
        state_run: BatteryStateRun,
    ) -> Result<Self, BatteryError> {
        Self::capture_inner_with_reach(
            repository,
            selector,
            claim,
            actor,
            rim,
            hygiene,
            InputMode::Generic,
            Some(gate_id),
            None,
            Some(state_run),
            Some(selection),
        )
    }
}

impl<'trace> BatteryCapture<'trace> {
    /// Capture as [`Self::capture`] and emit every arm and family invocation to
    /// `trace` without granting it authority over execution.
    ///
    /// # Errors
    ///
    /// Refuses the same pre-execution failures as [`Self::capture`].
    pub fn capture_traced(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
        trace: &'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send),
    ) -> Result<Self, BatteryError> {
        Self::capture_inner(
            repository,
            selector,
            claim,
            actor,
            rim,
            hygiene,
            InputMode::Strict,
            None,
            Some(trace),
            None,
        )
    }

    /// Capture a generic installed battery and expose its execution events.
    pub fn capture_generic_traced(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
        trace: &'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send),
    ) -> Result<Self, BatteryError> {
        Self::capture_inner(
            repository,
            selector,
            claim,
            actor,
            rim,
            hygiene,
            InputMode::Generic,
            None,
            Some(trace),
            None,
        )
    }

    /// Capture a complete battery into an existing state transaction while
    /// exposing production execution events.
    #[expect(
        clippy::too_many_arguments,
        reason = "the observer and shared state accompany six independent capture authorities"
    )]
    pub fn capture_in_state_traced(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
        state_run: BatteryStateRun,
        trace: &'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send),
    ) -> Result<Self, BatteryError> {
        Self::capture_inner(
            repository,
            selector,
            claim,
            actor,
            rim,
            hygiene,
            InputMode::Strict,
            None,
            Some(trace),
            Some(state_run),
        )
    }

    /// Capture a traced generic installed battery in one state transaction.
    #[expect(
        clippy::too_many_arguments,
        reason = "the observer and shared state accompany six independent capture authorities"
    )]
    pub fn capture_generic_in_state_traced(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
        state_run: BatteryStateRun,
        trace: &'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send),
    ) -> Result<Self, BatteryError> {
        Self::capture_inner(
            repository,
            selector,
            claim,
            actor,
            rim,
            hygiene,
            InputMode::Generic,
            None,
            Some(trace),
            Some(state_run),
        )
    }

    /// Capture one canonical arm while exposing production analysis events.
    ///
    /// # Errors
    ///
    /// Refuses the same pre-execution failures as [`Self::capture_arm`].
    #[expect(
        clippy::too_many_arguments,
        reason = "the selected identity and observer accompany six independent capture authorities"
    )]
    pub fn capture_arm_traced(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
        gate_id: crate::GateId,
        trace: &'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send),
    ) -> Result<Self, BatteryError> {
        Self::capture_inner(
            repository,
            selector,
            claim,
            actor,
            rim,
            hygiene,
            InputMode::Strict,
            Some(gate_id),
            Some(trace),
            None,
        )
    }

    /// Capture one generic installed arm while exposing analysis events.
    #[expect(
        clippy::too_many_arguments,
        reason = "the selected identity and observer accompany six independent capture authorities"
    )]
    pub fn capture_generic_arm_traced(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
        gate_id: crate::GateId,
        trace: &'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send),
    ) -> Result<Self, BatteryError> {
        Self::capture_inner(
            repository,
            selector,
            claim,
            actor,
            rim,
            hygiene,
            InputMode::Generic,
            Some(gate_id),
            Some(trace),
            None,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the execution mode accompanies six independent capture authorities"
    )]
    fn capture_inner(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
        input_mode: InputMode,
        selected: Option<crate::GateId>,
        trace: Option<&'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send)>,
        state_run: Option<BatteryStateRun>,
    ) -> Result<Self, BatteryError> {
        Self::capture_inner_with_reach(
            repository, selector, claim, actor, rim, hygiene, input_mode, selected, trace,
            state_run, None,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the execution mode and optional reach accompany six independent capture authorities"
    )]
    fn capture_inner_with_reach(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
        input_mode: InputMode,
        selected: Option<crate::GateId>,
        trace: Option<&'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send)>,
        state_run: Option<BatteryStateRun>,
        selection: Option<&crate::ChangeSelectionReport>,
    ) -> Result<Self, BatteryError> {
        let repository: PathBuf = repository.into();
        // Read before the capture consumes the observation, and both before the
        // selection is materialized: an authority that cannot be produced is a
        // refusal, and refusing it here costs no private tree.
        let resolved_actor = run_actor(&actor)?;
        let scope = selected_scope(selector, &repository)?;
        let state_run = match state_run {
            Some(state_run) => state_run,
            None => BatteryStateRun::capture(&repository)?,
        };
        let executor = RepositoryExecutor::capture(
            &repository,
            selector,
            claim,
            actor,
            rim,
            hygiene,
            &state_run,
            input_mode,
            selected,
            selection,
            trace,
        )?;
        let state = state_run.ledger()?;
        let contract = executor.contract();
        let authorities = BatteryAuthorities {
            source: executor.source().clone(),
            actor: resolved_actor,
            scope,
            configuration: tracked_evidence(contract)?,
            policy: contract.rule_policy().clone(),
            // The repository-document, structural file-shape, and structural
            // routine-complexity arms measured every selected path or routine
            // against a per-actor elastic limit. These are the exact decisions
            // those capture-time acquisitions judged their rows against, all
            // decided from the same ledger published above; nothing is
            // re-resolved merely to describe the run.
            bounds: executor.bounds().to_vec(),
            state,
            tools: tool_evidence(contract)?,
        };
        Ok(Self {
            authorities,
            executor,
            selected,
            state_run,
        })
    }

    /// Run the canonical battery against this capture.
    ///
    /// # Errors
    ///
    /// Returns the manifest, context, or completeness refusal that prevented
    /// one total result vector. An arm that fails is a result, not an error.
    pub fn execute(&mut self) -> Result<BatteryReport, BatteryError> {
        if let Some(gate_id) = self.selected {
            return Err(BatteryError::new(format!(
                "selected capture for {gate_id} cannot execute a complete battery"
            )));
        }
        let inputs = self.authorities.inputs()?;
        run_battery(&inputs, &mut self.executor)
    }

    /// Capture one exact compiler contract for the enclosing full verifier.
    ///
    /// This does not run a gate arm.  It binds the verifier process it returns
    /// to this capture's selected source and currently identified tools.
    ///
    /// # Errors
    ///
    /// Refuses selected-arm captures, foreign arms, or unusable tool evidence.
    pub fn verifier_compiler_contract(
        &self,
        gate_id: crate::GateId,
        environment: Vec<(OsString, OsString)>,
    ) -> Result<VerifierCompilerContract, BatteryError> {
        self.executor
            .verifier_compiler_contract(gate_id, environment)
    }

    /// Capture a compiler contract only when the selected tree makes its arm applicable.
    ///
    /// # Errors
    ///
    /// Refuses selected-arm captures, foreign arms, or unusable tool evidence.
    pub fn applicable_verifier_compiler_contract(
        &self,
        gate_id: crate::GateId,
        environment: Vec<(OsString, OsString)>,
    ) -> Result<Option<VerifierCompilerContract>, BatteryError> {
        match self
            .executor
            .verifier_compiler_contract_acquisition(gate_id, environment)?
        {
            VerifierContractAcquisition::Applicable(contract) => Ok(Some(contract)),
            VerifierContractAcquisition::NotApplicable(_) => Ok(None),
        }
    }

    /// Run the complete battery with successful full-verifier compiler proof.
    ///
    /// Invalid evidence never admits an arm: the toolchain family ignores it
    /// and executes that arm's ordinary standalone analysis.
    ///
    /// # Errors
    ///
    /// Returns the same completeness failures as [`Self::execute`], plus use
    /// of evidence on a selected capture or after its toolchain family starts.
    pub fn execute_with_verifier_evidence(
        &mut self,
        evidence: &[VerifierCompilerEvidence],
        environment: Vec<(OsString, OsString)>,
    ) -> Result<BatteryReport, BatteryError> {
        self.execute_with_verifier_evidence_mode(evidence, environment, false)
    }

    /// Run the complete battery with compiler proof that must remain valid.
    ///
    /// Unlike the general full-verifier seam, rejected evidence refuses the
    /// compiler arms instead of falling back to compiler execution.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::execute_with_verifier_evidence`].
    pub fn execute_with_required_verifier_evidence(
        &mut self,
        evidence: &[VerifierCompilerEvidence],
        environment: Vec<(OsString, OsString)>,
    ) -> Result<BatteryReport, BatteryError> {
        self.execute_with_verifier_evidence_mode(evidence, environment, true)
    }

    fn execute_with_verifier_evidence_mode(
        &mut self,
        evidence: &[VerifierCompilerEvidence],
        environment: Vec<(OsString, OsString)>,
        required: bool,
    ) -> Result<BatteryReport, BatteryError> {
        if self.selected.is_some() {
            return Err(BatteryError::new(
                "selected capture cannot execute a complete battery",
            ));
        }
        self.executor
            .set_verifier_evidence(evidence, environment, required)?;
        let inputs = self.authorities.inputs()?;
        run_battery(&inputs, &mut self.executor)
    }

    /// Run exactly one canonical arm against this capture.
    ///
    /// # Errors
    ///
    /// Returns the same manifest, input, and executor refusals as a complete
    /// run, scoped to the selected arm.
    pub fn execute_arm(
        &mut self,
        gate_id: crate::GateId,
    ) -> Result<BatteryArmResult, BatteryError> {
        self.execute_arm_report(gate_id)
            .map(super::FocusedArmReport::into_result)
    }

    /// Run exactly one canonical arm and retain its verifiable input context.
    ///
    /// # Errors
    ///
    /// Returns the same manifest, input, and executor refusals as
    /// [`Self::execute_arm`].
    pub fn execute_arm_report(
        &mut self,
        gate_id: crate::GateId,
    ) -> Result<super::FocusedArmReport, BatteryError> {
        if self.selected != Some(gate_id) {
            return Err(BatteryError::new(format!(
                "arm execution for {gate_id} requires a capture selected for that arm"
            )));
        }
        let inputs = self.authorities.inputs()?;
        super::run_focused_arm(&inputs, &mut self.executor, gate_id)
    }

    /// Project the two structural arms captured for this complete battery.
    ///
    /// Their metrics were acquired when the immutable selected source was
    /// materialized. Projecting them here therefore performs no second source
    /// capture or analysis, and a later complete execution reuses the same
    /// retained family evidence.
    ///
    /// # Errors
    ///
    /// Refuses selected-arm captures or invalid canonical inputs.
    pub fn execute_structural_preflight(
        &mut self,
    ) -> Result<Vec<super::FocusedArmReport>, BatteryError> {
        if self.selected.is_some() {
            return Err(BatteryError::new(
                "structural preflight requires a complete battery capture",
            ));
        }
        let inputs = self.authorities.inputs()?;
        [crate::GateId::FileShape, crate::GateId::Complexity]
            .into_iter()
            .map(|gate_id| super::run_focused_arm(&inputs, &mut self.executor, gate_id))
            .collect()
    }

    /// Run once and bind the complete vector to the captured baseline.
    ///
    /// The baseline comes from the same selected contract the executor owns;
    /// callers cannot mint a receipt against a second contract or reconstruct
    /// the floor identity beside the run.
    ///
    /// # Errors
    ///
    /// Returns the same completeness refusal as [`Self::execute`].
    pub fn execute_receipt(&mut self) -> Result<BatteryReceipt, BatteryError> {
        let baseline = self.executor.contract().baseline_digest().ok_or_else(|| {
            BatteryError::new(format!(
                "battery receipt requires lower-is-better state at {BASELINE_PATH:?}"
            ))
        })?;
        self.execute()
            .map(|report| BatteryReceipt::mint(report, &baseline))
    }

    /// Bind a completed report from this capture to its selected baseline.
    ///
    /// This is the full-verifier seam: compiler evidence is installed and the
    /// battery executes through its existing path first, then the resulting
    /// complete report is minted without acquiring a second contract.
    pub fn mint_receipt(&self, report: BatteryReport) -> Result<BatteryReceipt, BatteryError> {
        let current = self.authorities.inputs()?.snapshot()?;
        if report.context() != &current {
            return Err(BatteryError::new(
                "battery report does not match the capture minting its receipt",
            ));
        }
        let baseline = self.executor.contract().baseline_digest().ok_or_else(|| {
            BatteryError::new(format!(
                "battery receipt requires lower-is-better state at {BASELINE_PATH:?}"
            ))
        })?;
        Ok(BatteryReceipt::mint(report, &baseline))
    }

    /// Identity of the selected bytes this capture owns.
    #[must_use]
    pub const fn source(&self) -> &Digest {
        &self.authorities.source
    }

    /// Private selected-tree root retained for the capture's lifetime.
    #[must_use]
    pub fn root(&self) -> &Path {
        self.executor.root()
    }

    /// Exact changed paths observed before this selected source was captured.
    #[must_use]
    pub fn changes(&self) -> &[String] {
        self.executor.changes()
    }

    /// Execute the existing canonical mutation profile over this same source.
    ///
    /// # Errors
    ///
    /// Returns the first mutation selection, tool, inventory, baseline, or
    /// integrity refusal. No second selected tree or mutation runner is used.
    pub fn execute_kill(&self) -> Result<crate::KillRun, crate::KillError> {
        self.executor.execute_kill()
    }

    /// Shared durable-state transaction this capture contributed to.
    #[must_use]
    pub fn state_run(&self) -> BatteryStateRun {
        self.state_run.clone()
    }

    /// Publish this enclosing run's accumulated state after total acceptance.
    pub fn accept_state(&self, scope: StateRunScope) -> Result<StateReceipt, BatteryError> {
        self.state_run.accept(scope)
    }

    /// Reject this enclosing run without publishing any accumulated state.
    pub fn reject_state(&self) -> Result<(), BatteryError> {
        self.state_run.reject()
    }
}

/// Resolve the one actor a battery run is attributed to.
///
/// Seeded through [`ActorInputs`], which owns the documented resolution order,
/// rather than through a second order written here. `require_actor` refuses the
/// provenance-only members of that order, so the candidate path below is never
/// the answer: it is present because the resolver takes one, and the tracked
/// contract is the only path a run-wide resolution could name.
fn run_actor(actor: &ActorCapture) -> Result<ActorResolution, BatteryError> {
    let candidate = ActorInputs::new(POLICY_PATH)
        .map_err(|error| BatteryError::new(format!("battery actor provenance: {error}")))?;
    let Some(active) = actor.active_actor() else {
        return Err(BatteryError::new(
            "battery capture requires an acting actor: a battery report names who ran it",
        ));
    };
    candidate
        .with_active_actor(active)
        .require_actor()
        .map_err(|error| BatteryError::new(format!("battery actor: {error}")))
}

/// Name the selection every arm reads and the repository it was taken from.
fn selected_scope(selector: &Selector, repository: &Path) -> Result<BatteryScope, BatteryError> {
    let (name, provenance) = match selector {
        Selector::Staged => (
            "staged-index",
            format!("the index of {}", repository.display()),
        ),
        Selector::Revision(revision) => (
            "revision",
            format!("{revision} in {}", repository.display()),
        ),
    };
    BatteryScope::new(name, provenance)
}

/// Digest every value the tracked configuration document resolved.
///
/// One evidence per leaf, keyed by its exact field path, so a document that
/// declares nothing produces nothing and a document that declares the same
/// field twice cannot produce two winners. Tables are descended rather than
/// digested: a table is the path to a value, not a value.
fn tracked_evidence(contract: &Contract) -> Result<Vec<ConfigurationEvidence>, BatteryError> {
    let mut evidence = Vec::new();
    let mut field = Vec::new();
    resolve_value(contract.tracked_configuration(), &mut field, &mut evidence)?;
    Ok(evidence)
}

fn resolve_value(
    value: &toml::Value,
    field: &mut Vec<String>,
    into: &mut Vec<ConfigurationEvidence>,
) -> Result<(), BatteryError> {
    if let toml::Value::Table(table) = value {
        for (key, nested) in table {
            field.push(key.clone());
            resolve_value(nested, field, into)?;
            field.pop();
        }
        return Ok(());
    }
    let encoded = wire::encode(value).map_err(|error| {
        BatteryError::new(format!(
            "cannot encode tracked configuration {}: {error}",
            field.join(".")
        ))
    })?;
    into.push(ConfigurationEvidence::new(
        field.join("."),
        Digest::compute(Domain::GateConfiguration, &encoded),
        CURRENT_RELEASE,
        OriginLayer::Repository,
        POLICY_PATH,
        field.clone(),
    )?);
    Ok(())
}

/// Report every tool the captured contract found and probed.
///
/// The contract probes each declared executable once while it loads, so this
/// projects that one discovery rather than repeating it. A contract that failed
/// to identify a tool never loaded at all, which is why every entry here is an
/// identified one.
fn tool_evidence(contract: &Contract) -> Result<Vec<BatteryToolEvidence>, BatteryError> {
    contract
        .tools()
        .iter()
        .map(|tool| {
            BatteryToolEvidence::identified(
                tool.name(),
                tool.program().display().to_string(),
                tool.version(),
                tool.executable().clone(),
            )
        })
        .collect()
}

#[cfg(test)]
mod transition_tests {
    use serde::Serialize;

    use super::*;

    #[derive(Serialize)]
    struct StateReceiptFixture<'a> {
        version: u64,
        before: &'a Digest,
        after: &'a Digest,
        accepted: bool,
    }

    #[test]
    fn retained_authorities_cross_only_their_exact_accepted_state_transition() {
        let durable_before = digest(b"durable before ledger");
        let judged = digest(b"run-start normalized ledger");
        let after = digest(b"after ledger");
        let normalized = digest(b"normalized after ledger");
        let prior = context(b"source", &judged);
        let current = context(b"source", &normalized);

        assert!(authorities_match_after_state(
            &prior,
            &current,
            &after,
            &state_receipt(&durable_before, &after, true),
        ));
        assert!(!authorities_match_after_state(
            &prior,
            &current,
            &after,
            &state_receipt(&durable_before, &after, false),
        ));
        assert!(!authorities_match_after_state(
            &prior,
            &current,
            &after,
            &state_receipt(&durable_before, &digest(b"foreign after"), true),
        ));
        assert!(!authorities_match_after_state(
            &prior,
            &context(b"changed source", &normalized),
            &after,
            &state_receipt(&durable_before, &after, true),
        ));
        let mut changed_projection =
            serde_json::to_value(&current).expect("serialize current context fixture");
        changed_projection["latches"] = serde_json::json!([{
            "kind": "file-loc",
            "path": "fixture.rs",
        }]);
        let changed_projection = serde_json::from_value(changed_projection)
            .expect("deserialize changed projection fixture");
        assert!(!authorities_match_after_state(
            &prior,
            &changed_projection,
            &after,
            &state_receipt(&durable_before, &after, true),
        ));
    }

    fn context(source: &[u8], state: &Digest) -> super::super::BatteryContext {
        serde_json::from_value(serde_json::json!({
            "source": digest(source),
            "actor": { "source": "active-actor", "value": "fixture-actor" },
            "scope": { "name": "staged-index", "provenance": "fixture index" },
            "configuration": [],
            "policy": digest(b"policy"),
            "state": state,
            "bounds": [],
            "latches": [],
            "tools": [],
        }))
        .expect("deserialize context fixture")
    }

    fn state_receipt(before: &Digest, after: &Digest, accepted: bool) -> StateReceipt {
        let bytes = wire::encode(&StateReceiptFixture {
            version: crate::state::STATE_RECEIPT_VERSION,
            before,
            after,
            accepted,
        })
        .expect("encode state receipt fixture");
        StateReceipt::decode(&bytes).expect("decode state receipt fixture")
    }

    fn digest(bytes: &[u8]) -> Digest {
        Digest::compute(Domain::GateStateReceipt, bytes)
    }
}
