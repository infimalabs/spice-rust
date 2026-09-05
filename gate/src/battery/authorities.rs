//! One capture of everything a battery run consumes and everything it runs.
//!
//! [`RepositorySelection`] materializes one selection and loads its tracked
//! contract before the state, source identity, and repository observations are
//! bound. The authorities the battery reads alongside its arms come from that
//! same source capture, so they are retained here rather than assembled beside
//! a second materialization of the selection.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use wire::{Digest, Domain};

use super::repository_executor::{RepositoryExecutor, RepositorySelection, RepositorySource};
use super::{
    BatteryArmResult, BatteryError, BatteryInputs, BatteryReport, BatteryScope, BatteryStateRun,
    BatteryToolEvidence, ConfigurationEvidence, RepositoryExecutionEvent, StateRunScope,
    run_battery,
};
use crate::core::{ActorCapture, ClaimCapture};
use crate::hygiene::HygieneConfiguration;
use crate::input::{Contract, POLICY_PATH};
use crate::manifest::OriginLayer;
use crate::policy::Policy as RulePolicy;
use crate::state::{ActorInputs, ActorResolution, BoundDecision, StateLedger, StateReceipt};
use crate::toolchain::RimCapture;
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

/// Run-wide authority that every focused arm derives from without recapturing bytes.
#[derive(Clone)]
struct BatterySourceCapture {
    repository: RepositorySource,
    actor: ActorCapture,
    resolved_actor: ActorResolution,
    scope: BatteryScope,
    state: StateLedger,
    rim: RimCapture,
    hygiene: HygieneConfiguration,
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
    source_capture: BatterySourceCapture,
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

    /// Prove the live selector source still names this capture.
    ///
    /// # Errors
    ///
    /// Returns the exact selected-tree movement diagnostic.
    pub fn verify_source(&self) -> Result<(), BatteryError> {
        self.executor.verify_source()
    }

    /// Preserve index movement as a signal for a reservation-scoped semantic recapture.
    pub fn verify_refreshable_source(
        &self,
    ) -> Result<crate::tree::RefreshableSourceState, BatteryError> {
        self.executor.verify_refreshable_source()
    }

    /// Every regular path retained by this exact selected source.
    #[must_use]
    pub fn paths(&self) -> &[String] {
        self.executor.paths()
    }

    /// Canonical staged entries and their exact rename lineage, when staged.
    #[must_use]
    pub fn staged_authority(&self) -> Option<&crate::tree::StagedAuthority> {
        self.executor.staged_authority()
    }

    /// Compile one action closure from this capture's retained tree.
    pub fn compile_action_input(
        &self,
        selection: &crate::ChangeSelectionReport,
        gate_id: crate::GateId,
        inventory: crate::ActionInputInventory,
    ) -> Result<crate::ActionInputClosure, BatteryError> {
        crate::compile_action_input(self.executor.tree(), selection, gate_id, inventory)
            .map_err(|error| BatteryError::new(error.to_string()))
    }

    /// Compile one explicit sub-action closure from this capture's retained tree.
    pub fn compile_explicit_action_input(
        &self,
        paths: Vec<String>,
        non_files: Vec<crate::ActionNonFileInput>,
    ) -> Result<crate::ActionInputManifest, BatteryError> {
        crate::compile_explicit_action_input(self.executor.tree(), paths, non_files)
            .map_err(|error| BatteryError::new(error.to_string()))
    }

    /// Command-rim compiler environment bound before focused execution.
    #[must_use]
    pub fn compiler_environment(&self) -> &[(OsString, OsString)] {
        self.executor.compiler_environment()
    }

    /// Materialize the exact committed base retained by this staged capture.
    ///
    /// # Errors
    ///
    /// Returns repository, object, path, or filesystem failures from the
    /// retained selected-tree authority.
    pub fn materialize_staged_base(
        &self,
    ) -> Result<Option<crate::tree::SelectedTree>, BatteryError> {
        self.executor.materialize_staged_base()
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
        let repository_source = RepositorySelection::capture(&repository, selector, input_mode)?;
        let tree = repository_source.tree();
        let (state, bound_actor) =
            state_run.bind(tree.paths(), tree.renames(), tree.publishes_state(), &actor)?;
        let repository_source =
            repository_source.bind(claim, bound_actor.clone(), state.latches(), &rim)?;
        let source_capture = BatterySourceCapture {
            repository: repository_source,
            actor: bound_actor,
            state,
            resolved_actor,
            scope,
            rim,
            hygiene,
        };
        Self::capture_from_source(source_capture, selected, trace, state_run, selection)
    }

    fn capture_from_source(
        source_capture: BatterySourceCapture,
        selected: Option<crate::GateId>,
        trace: Option<&'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send)>,
        state_run: BatteryStateRun,
        selection: Option<&crate::ChangeSelectionReport>,
    ) -> Result<Self, BatteryError> {
        source_capture.repository.verify_source()?;
        let executor = RepositoryExecutor::capture_from_source(
            &source_capture.repository,
            source_capture.actor.clone(),
            source_capture.rim.clone(),
            source_capture.hygiene.clone(),
            &state_run,
            &source_capture.state,
            selected,
            selection,
            trace,
        )?;
        let contract = executor.contract();
        let authorities = BatteryAuthorities {
            source: executor.source().clone(),
            actor: source_capture.resolved_actor.clone(),
            scope: source_capture.scope.clone(),
            configuration: tracked_evidence(contract)?,
            policy: contract.rule_policy().clone(),
            // The repository-document, structural file-shape, and structural
            // routine-complexity arms measured every selected path or routine
            // against a per-actor elastic limit. These are the exact decisions
            // those capture-time acquisitions judged their rows against, all
            // decided from the same ledger published above; nothing is
            // re-resolved merely to describe the run.
            bounds: executor.bounds().to_vec(),
            state: source_capture.state.clone(),
            tools: tool_evidence(contract)?,
        };
        Ok(Self {
            source_capture,
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

    /// Run the canonical battery with the compiler environment captured at the command rim.
    pub fn execute_with_environment(
        &mut self,
        environment: Vec<(OsString, OsString)>,
    ) -> Result<BatteryReport, BatteryError> {
        if self.selected.is_some() {
            return Err(BatteryError::new(
                "selected capture cannot execute a complete battery",
            ));
        }
        self.executor.set_compiler_environment(environment)?;
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

    /// Run one canonical arm with its caller-captured operational compiler environment.
    ///
    /// # Errors
    ///
    /// Returns the same selected-arm and execution failures as [`Self::execute_arm_report`],
    /// plus replacement after the toolchain family has completed.
    pub fn execute_arm_report_with_environment(
        &mut self,
        gate_id: crate::GateId,
        environment: Vec<(OsString, OsString)>,
    ) -> Result<super::FocusedArmReport, BatteryError> {
        self.executor.set_compiler_environment(environment)?;
        self.execute_arm_report(gate_id)
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

    /// Derive one reached focused arm from this capture's immutable source authority.
    ///
    /// The live selector seal is verified before derivation. The returned arm
    /// receives a distinct executor and arm-specific evidence while sharing the
    /// selected tree, loaded contract, core observations, ancestry baselines,
    /// actor, command rim, hygiene configuration, and enclosing state ledger.
    pub fn derive_arm_in_state_reached(
        &self,
        selection: &crate::ChangeSelectionReport,
        gate_id: crate::GateId,
    ) -> Result<BatteryCapture<'static>, BatteryError> {
        BatteryCapture::<'static>::capture_from_source(
            self.source_capture.clone(),
            Some(gate_id),
            None,
            self.state_run.clone(),
            Some(selection),
        )
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

    /// Objects retained from the validated reach plan for a focused recapture.
    ///
    /// Standalone selected arms and complete batteries have no authored reach
    /// plan and therefore return `None`.
    #[must_use]
    pub fn execution_objects(&self) -> Option<&[crate::ChangeReachObject]> {
        self.executor.execution_objects()
    }

    /// Stable source and outputs completed by a successful focused native Rust arm.
    #[must_use]
    pub fn reusable_rust_workspace(&self) -> Option<&crate::RustAnalysisWorkspace> {
        self.executor.reusable_rust_workspace()
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
