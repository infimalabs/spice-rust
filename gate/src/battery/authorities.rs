//! One capture of everything a battery run consumes and everything it runs.
//!
//! [`RepositoryExecutor`] already materializes one selection, loads the tracked
//! contract from it, and resolves the source identity every arm is labelled
//! with. The authorities the battery reads alongside those arms come from the
//! same three things, so they are taken here rather than assembled beside a
//! second materialization of the same selection.

use std::path::{Path, PathBuf};

use wire::{Digest, Domain};

use super::repository_executor::RepositoryExecutor;
use super::{
    BatteryArmResult, BatteryError, BatteryInputs, BatteryReceipt, BatteryReport, BatteryScope,
    BatteryToolEvidence, ConfigurationEvidence, RepositoryExecutionEvent, run_battery,
    run_battery_arm,
};
use crate::core::{ActorCapture, ClaimCapture};
use crate::hygiene::HygieneConfiguration;
use crate::input::{Contract, POLICY_PATH};
use crate::manifest::OriginLayer;
use crate::policy::Policy as RulePolicy;
use crate::state::{
    ActorInputs, ActorResolution, BoundDecision, CurrentState, StateLedger, current_state_path,
    predecessor_state_sources,
};
use crate::toolchain::RimCapture;
use crate::tree::{Selector, git_directories};

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
}

impl BatteryCapture<'_> {
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
        Self::capture_inner(repository, selector, claim, actor, rim, hygiene, None)
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
        trace: &'trace mut dyn FnMut(RepositoryExecutionEvent),
    ) -> Result<Self, BatteryError> {
        Self::capture_inner(
            repository,
            selector,
            claim,
            actor,
            rim,
            hygiene,
            Some(trace),
        )
    }

    fn capture_inner(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
        trace: Option<&'trace mut dyn FnMut(RepositoryExecutionEvent)>,
    ) -> Result<Self, BatteryError> {
        let repository: PathBuf = repository.into();
        // Read before the capture consumes the observation, and both before the
        // selection is materialized: an authority that cannot be produced is a
        // refusal, and refusing it here costs no private tree.
        let resolved_actor = run_actor(&actor)?;
        let scope = selected_scope(selector, &repository)?;
        let state = live_ledger(&repository)?;
        reconcile_claims(&state, &actor)?;
        let executor = RepositoryExecutor::capture(
            &repository,
            selector,
            claim,
            actor,
            rim,
            hygiene,
            state.latches(),
            trace,
        )?;
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
        })
    }

    /// Run the canonical battery against this capture.
    ///
    /// # Errors
    ///
    /// Returns the manifest, context, or completeness refusal that prevented
    /// one total result vector. An arm that fails is a result, not an error.
    pub fn execute(&mut self) -> Result<BatteryReport, BatteryError> {
        let inputs = self.authorities.inputs()?;
        self.executor.select(None);
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
        let inputs = self.authorities.inputs()?;
        self.executor.select(Some(gate_id));
        run_battery_arm(&inputs, &mut self.executor, gate_id)
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
        let baseline = self.executor.contract().baseline().digest();
        self.execute()
            .map(|report| BatteryReceipt::mint(report, &baseline))
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

/// Load the durable ledger the live repository holds.
///
/// Read from the repository rather than the selected tree because the ledger is
/// not tracked: it lives below Git's common directory so every linked worktree
/// sees one authority, and a selection would never contain it.
fn live_ledger(repository: &Path) -> Result<StateLedger, BatteryError> {
    let (worktree_git, common_git) = git_directories(repository).map_err(|error| {
        BatteryError::new(format!("cannot locate battery state authority: {error}"))
    })?;
    let sources = predecessor_state_sources(&worktree_git, &common_git);
    let state = CurrentState::load(current_state_path(&common_git), &sources)
        .map_err(|error| BatteryError::new(format!("cannot load battery state: {error}")))?;
    Ok(state.before().clone())
}

/// Refuse a capture whose two readings of one ledger cannot be the same read.
///
/// This capture reads the ledger itself and publishes it as the run's state
/// authority, while the repository-document arm answers `peer_claim` from the
/// bound claims the caller hands over. The caller supplies those because expiry
/// needs a clock and the gate holds none, so it reads this same ledger and
/// keeps what its run clock still calls active. A clock only drops claims, so
/// every bound claim is still one the ledger holds verbatim; one that is not
/// came from another repository or another moment, and the report would then
/// publish a state authority that contradicts a measurement resolved against
/// it. Refusing is the reconciliation: nothing here can tell which reading was
/// the stale one, and inventing an order between them would pick for the caller.
fn reconcile_claims(state: &StateLedger, actor: &ActorCapture) -> Result<(), BatteryError> {
    let ActorCapture::Acting { bound_claims, .. } = actor else {
        return Ok(());
    };
    for claim in bound_claims {
        if !state.claims().contains(claim) {
            return Err(BatteryError::new(format!(
                "battery bound claim on {:?} held by {:?} is not one the state authority holds: \
                 the run clock only drops claims, so these are two different readings",
                claim.path(),
                claim.actor()
            )));
        }
    }
    Ok(())
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
