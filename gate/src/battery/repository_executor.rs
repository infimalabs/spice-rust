//! Production execution of the compiled battery from one selected tree.

use std::path::{Path, PathBuf};

use wire::Digest;

use super::{
    ArmEvidence, ArmExecution, ArmFamily, BatteryContext, BatteryError, BatteryExecutor,
    BatteryInputs, CompiledArm, inline_evidence,
};
use crate::GateId;
use crate::core::{
    ActorCapture, CORE_GATE_IDS, ClaimCapture, CoreSource, acquire_core, acquire_core_arm,
    evaluate_core, evaluate_core_arm,
};
use crate::hygiene::{
    HYGIENE_ARMS, HygieneConfiguration, HygieneSource, acquire_hygiene, acquire_hygiene_arm,
};
use crate::input::Contract;
use crate::metrics::{
    BaselineCapture, MetricSource, STRUCTURAL_ARMS, acquire_metric_arm_with_bounds,
    acquire_metrics_with_bounds,
};
use crate::state::{BoundDecision, LatchEntry};
use crate::toolchain::{
    RimCapture, TASTE_EXTENSIONS, TOOLCHAIN_ARMS, ToolchainSource, acquire_toolchain,
    acquire_toolchain_arm, environment_baseline_suffixes, measure_toolchain, measure_toolchain_arm,
};
use crate::tree::{SelectedTree, Selector};

const FAMILY_COUNT: usize = 4;

/// Observable executor boundary used by integration tests and callers that
/// need to account for orchestration without replacing family acquisition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepositoryExecutionEvent {
    /// `run_battery` invoked one canonical arm position.
    ArmInvoked {
        position: usize,
        gate_id: GateId,
        family: ArmFamily,
    },
    /// The executor crossed one family acquisition boundary.
    FamilyInvoked { family: ArmFamily },
    /// One family began the named arm's actual analysis.
    AnalysisInvoked { gate_id: GateId, family: ArmFamily },
}

enum FamilyExecution {
    Complete(Vec<ArmEvidence>),
    Refused(String),
}

/// One capture-owning production implementation of [`BatteryExecutor`].
///
/// A family is projected when its first arm is requested. A complete capture
/// caches the family's complete result set or refusal; a selected capture
/// caches only the named result. Either way, no requested analysis runs twice.
///
/// The core and metrics families are the two exceptions, and are acquired at
/// capture. A complete run acquires each complete family; a selected run
/// acquires only the named arm. Two core arms observe the live repository the
/// tree was materialized from, so they can only be taken in the same moment the
/// source is captured: an acquisition deferred to first request would read an
/// index, a `MERGE_HEAD`, or a worktree that moved after the capture the report
/// names.
/// Metrics are acquired immediately after that observation because their
/// file-shape and routine-complexity rows decide elastic bounds which must be
/// present in the immutable run context before any arm executes. Each complete
/// result or refusal is retained and handed over unchanged when its first arm
/// is requested.
///
/// The committed comparison and merge-authorship baselines are read in that
/// same moment and for the same reason, even though some arms that read them
/// are not core arms. They are the live repository's `HEAD` and pending merge
/// parents, and those move: a read deferred to first request would compare part
/// of the selection against one ancestry and the rest against another, and the
/// report would name neither.
///
/// The acting actor and the latch inventory are handed directly from the core
/// capture to the structural acquisition. That keeps both families deciding
/// against one ledger reading; neither is retained for a later observation.
pub struct RepositoryExecutor<'trace> {
    tree: SelectedTree,
    contract: Contract,
    source: Digest,
    core: Option<Result<Vec<ArmEvidence>, String>>,
    metrics: Option<Result<Vec<ArmEvidence>, String>>,
    baselines: BaselineCapture,
    bounds: Vec<BoundDecision>,
    rim: RimCapture,
    hygiene: HygieneConfiguration,
    context: Option<BatteryContext>,
    families: [Option<FamilyExecution>; FAMILY_COUNT],
    selected: Option<GateId>,
    trace: Option<&'trace mut dyn FnMut(RepositoryExecutionEvent)>,
}

impl<'trace> RepositoryExecutor<'trace> {
    /// Materialize and retain one repository selection for a complete battery.
    ///
    /// `rim` carries the directory, executable search path, and changed paths
    /// the caller observed at the binary rim. All three are supplied rather than
    /// derived here: an executor that read the repository back would measure a
    /// set the caller never saw, and an executor that defaulted to none would
    /// scan nothing and call every tree clean.
    ///
    /// `latches` is the durable breach inventory the core family decides its
    /// elastic bounds against: a latched path is held to its base limit rather
    /// than the varied one. The ledger it comes from is read by the caller,
    /// which publishes that same ledger as the run's state authority, so the
    /// bounds this capture decides and the state its report names are one read.
    ///
    /// Every arm and family invocation is emitted to `trace` when one is given,
    /// which observes execution without gaining any authority over it.
    ///
    /// Reached only through [`BatteryCapture`], which is what keeps an executor
    /// from being run against inputs no capture ever read.
    ///
    /// # Errors
    ///
    /// Refuses selection, tracked-contract, or source-identity failure before
    /// an executor can be used. A core family that refuses to acquire is not
    /// one of these: the capture succeeded, and the refusal it produced is the
    /// evidence every core position projects.
    ///
    /// [`BatteryCapture`]: super::BatteryCapture
    // Every argument is an observation of the live repository that no selected
    // tree can supply, so there is nothing here to group into a narrower type
    // that would not just be this list under one name.
    #[expect(clippy::too_many_arguments)]
    pub(super) fn capture(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
        latches: &[LatchEntry],
        selected: Option<GateId>,
        mut trace: Option<&'trace mut dyn FnMut(RepositoryExecutionEvent)>,
    ) -> Result<Self, BatteryError> {
        let repository: PathBuf = repository.into();
        let tree = selector.materialize(&repository).map_err(|error| {
            BatteryError::new(format!("cannot capture battery source: {error}"))
        })?;
        let contract = Contract::load(tree.root()).map_err(|error| {
            BatteryError::new(format!("cannot load captured battery contract: {error}"))
        })?;
        // One capture, taken once: the digest every arm is labelled with, the
        // requested core acquisition and the bounds it decided, and the
        // committed baselines every delta or merge-authorship decision reads
        // are all read here, and nothing below this block can reach the live
        // repository again. Suffix classes cover arms with class-bound jurisdictions;
        // exact rim changes cover every-change arms such as local-paths, whose
        // inputs may be extensionless. Both are retained together because a
        // second capture could name a different commit than this report does.
        let (source, core, mut bounds, baselines) = {
            let captured = match selected {
                Some(gate_id) => CoreSource::capture_arm(
                    &tree,
                    &contract,
                    claim,
                    actor.clone(),
                    latches,
                    gate_id,
                ),
                None => CoreSource::capture(&tree, &contract, claim, actor.clone(), latches),
            }
            .map_err(|error| {
                BatteryError::new(format!("cannot identify battery source: {error}"))
            })?;
            let (core, bounds) = match selected {
                None => {
                    record_capture_analyses(&mut trace, ArmFamily::Core, CORE_GATE_IDS);
                    let (evidence, bounds) = core_family(&captured, &contract);
                    (Some(evidence), bounds)
                }
                Some(gate_id) if CORE_GATE_IDS.contains(&gate_id) => {
                    record_capture_analyses(&mut trace, ArmFamily::Core, [gate_id]);
                    let (evidence, bounds) = core_arm(&captured, &contract, gate_id);
                    (Some(evidence), bounds)
                }
                Some(_) => (None, Vec::new()),
            };
            let classes = contract.rule_policy().document().file_classes();
            let mut selected = classes.magic().to_vec();
            selected.extend(classes.file_shape().iter().cloned());
            selected.extend(classes.complexity().iter().cloned());
            selected.extend(environment_baseline_suffixes());
            selected.extend(
                TASTE_EXTENSIONS
                    .iter()
                    .map(|extension| format!(".{extension}")),
            );
            selected.sort();
            selected.dedup();
            let baselines =
                BaselineCapture::capture_with_paths(&repository, &selected, rim.changes());
            (captured.digest().clone(), core, bounds, baselines)
        };
        let (metrics, metric_bounds) = match selected {
            None => {
                record_capture_analyses(
                    &mut trace,
                    ArmFamily::Metrics,
                    STRUCTURAL_ARMS.map(|arm| arm.id()),
                );
                let (evidence, bounds) =
                    metrics_family(&tree, &contract, &baselines, &actor, latches);
                (Some(evidence), bounds)
            }
            Some(gate_id) if STRUCTURAL_ARMS.iter().any(|arm| arm.id() == gate_id) => {
                record_capture_analyses(&mut trace, ArmFamily::Metrics, [gate_id]);
                let (evidence, bounds) =
                    metrics_arm(&tree, &contract, &baselines, &actor, latches, gate_id);
                (Some(evidence), bounds)
            }
            Some(_) => (None, Vec::new()),
        };
        bounds.extend(metric_bounds);
        Ok(Self {
            tree,
            contract,
            source,
            core,
            metrics,
            baselines,
            bounds,
            rim,
            hygiene,
            context: None,
            families: std::array::from_fn(|_| None),
            selected,
            trace,
        })
    }

    /// Identity of the selected bytes this executor owns.
    #[must_use]
    pub const fn source(&self) -> &Digest {
        &self.source
    }

    /// Private selected-tree root retained for the executor's lifetime.
    #[must_use]
    pub fn root(&self) -> &Path {
        self.tree.root()
    }

    /// The tracked contract this capture loaded from the selected tree.
    pub(super) const fn contract(&self) -> &Contract {
        &self.contract
    }

    /// Every elastic bound the capture's core and structural acquisitions decided.
    ///
    /// These are the decisions the arms were already judged against, not a
    /// second resolution taken for the report. A family or row that could not
    /// decide a bound reports none for that position.
    pub(super) fn bounds(&self) -> &[BoundDecision] {
        &self.bounds
    }

    fn bind_inputs(&mut self, inputs: &BatteryInputs<'_>) -> Result<(), BatteryError> {
        if inputs.source() != &self.source {
            return Err(BatteryError::new(format!(
                "battery inputs name source {}, but the executor owns {}",
                inputs.source(),
                self.source
            )));
        }
        let context = inputs.snapshot()?;
        match &self.context {
            Some(bound) if bound != &context => Err(BatteryError::new(
                "repository executor cannot cross battery input contexts",
            )),
            Some(_) => Ok(()),
            None => {
                self.context = Some(context);
                Ok(())
            }
        }
    }

    fn record(&mut self, event: RepositoryExecutionEvent) {
        if let Some(trace) = &mut self.trace {
            trace(event);
        }
    }

    fn ensure_family(&mut self, family: ArmFamily) {
        let index = family_index(family);
        if self.families[index].is_some() {
            return;
        }
        self.record(RepositoryExecutionEvent::FamilyInvoked { family });
        let execution = self.acquire_family(family);
        self.families[index] = Some(execution);
    }

    fn acquire_family(&mut self, family: ArmFamily) -> FamilyExecution {
        let result = match family {
            // Not acquired here: this hands over the result the capture already
            // took, so the first core arm cannot start a second observation.
            ArmFamily::Core => self.core.clone().unwrap_or_else(|| {
                Err("selected capture did not acquire the core family".to_owned())
            }),
            ArmFamily::Toolchain => self.acquire_toolchain_family(),
            // Like core, this hands over the one result captured before the run
            // context froze its exact bound decisions.
            ArmFamily::Metrics => self.metrics.clone().unwrap_or_else(|| {
                Err("selected capture did not acquire the metrics family".to_owned())
            }),
            ArmFamily::Hygiene => self.acquire_hygiene_family(),
        };
        match result {
            Ok(evidence) => FamilyExecution::Complete(evidence),
            Err(reason) => FamilyExecution::Refused(reason),
        }
    }

    fn acquire_toolchain_family(&mut self) -> Result<Vec<ArmEvidence>, String> {
        let analyses = self.selected.map_or_else(
            || {
                TOOLCHAIN_ARMS
                    .iter()
                    .map(|arm| arm.id())
                    .collect::<Vec<_>>()
            },
            |gate_id| vec![gate_id],
        );
        self.record_analyses(ArmFamily::Toolchain, analyses);
        let source =
            ToolchainSource::capture_with_baselines(&self.tree, self.rim.clone(), &self.baselines);
        if let Some(gate_id) = self.selected {
            let observation = acquire_toolchain_arm(&source, &self.contract, gate_id)
                .map_err(|error| error.to_string())?;
            let report = measure_toolchain_arm(observation).map_err(|error| error.to_string())?;
            return Ok(vec![ArmEvidence::Toolchain { report }]);
        }
        let report = measure_toolchain(acquire_toolchain(&source, &self.contract))
            .map_err(|error| error.to_string())?;
        Ok(report
            .arms
            .into_iter()
            .map(|report| ArmEvidence::Toolchain { report })
            .collect())
    }

    fn acquire_hygiene_family(&mut self) -> Result<Vec<ArmEvidence>, String> {
        let analyses = self.selected.map_or_else(
            || HYGIENE_ARMS.iter().map(|arm| arm.id()).collect::<Vec<_>>(),
            |gate_id| vec![gate_id],
        );
        self.record_analyses(ArmFamily::Hygiene, analyses);
        let source = HygieneSource::capture(&self.tree, &self.contract)
            .map_err(|error| error.to_string())?;
        if let Some(gate_id) = self.selected {
            let report = acquire_hygiene_arm(&source, &self.hygiene, gate_id)
                .map_err(|error| error.to_string())?;
            return Ok(vec![ArmEvidence::Hygiene { report }]);
        }
        let report = acquire_hygiene(&source, &self.hygiene).map_err(|error| error.to_string())?;
        Ok(report
            .arms
            .into_iter()
            .map(|report| ArmEvidence::Hygiene { report })
            .collect())
    }

    fn record_analyses(&mut self, family: ArmFamily, gate_ids: impl IntoIterator<Item = GateId>) {
        for gate_id in gate_ids {
            self.record(RepositoryExecutionEvent::AnalysisInvoked { gate_id, family });
        }
    }

    fn project(&self, arm: &CompiledArm) -> Result<ArmEvidence, BatteryError> {
        let family = self.families[family_index(arm.family())]
            .as_ref()
            .expect("family is acquired before projection");
        match family {
            FamilyExecution::Refused(reason) => Err(BatteryError::new(format!(
                "{:?} family refused: {}",
                arm.family(),
                inline_evidence(reason)
            ))),
            FamilyExecution::Complete(evidence) => evidence
                .iter()
                .find(|item| item.gate_id() == arm.gate_id())
                .cloned()
                .ok_or_else(|| {
                    BatteryError::new(format!(
                        "{:?} family omitted compiled gate {}",
                        arm.family(),
                        arm.gate_id()
                    ))
                }),
        }
    }
}

impl BatteryExecutor for RepositoryExecutor<'_> {
    fn execute(
        &mut self,
        arm: &CompiledArm,
        inputs: &BatteryInputs<'_>,
    ) -> Result<ArmExecution, BatteryError> {
        self.record(RepositoryExecutionEvent::ArmInvoked {
            position: arm.position(),
            gate_id: arm.gate_id(),
            family: arm.family(),
        });
        self.bind_inputs(inputs)?;
        self.ensure_family(arm.family());
        let evidence = self.project(arm)?;
        ArmExecution::new(
            evidence.state()?,
            evidence.reason()?,
            evidence.provenance()?,
            evidence,
        )
    }
}

/// Acquire the complete structural family and retain every bound it decided.
///
/// This happens before [`BatteryInputs`] freezes the run context. The evidence
/// is still projected only when the canonical metrics positions execute, but
/// both that evidence and these decisions come from this single acquisition.
fn metrics_family(
    tree: &SelectedTree,
    contract: &Contract,
    baselines: &BaselineCapture,
    actor: &ActorCapture,
    latches: &[LatchEntry],
) -> (Result<Vec<ArmEvidence>, String>, Vec<BoundDecision>) {
    let source = match MetricSource::capture(tree, contract, baselines, actor, latches) {
        Ok(source) => source,
        Err(error) => return (Err(error.to_string()), Vec::new()),
    };
    let (evidence, bounds) = acquire_metrics_with_bounds(&source).into_parts();
    let arms = evidence.map_err(|error| error.to_string()).map(|reports| {
        reports
            .into_iter()
            .map(|report| ArmEvidence::Metrics { report })
            .collect()
    });
    (arms, bounds)
}

/// Acquire one structural arm and retain only the bounds it decided.
fn metrics_arm(
    tree: &SelectedTree,
    contract: &Contract,
    baselines: &BaselineCapture,
    actor: &ActorCapture,
    latches: &[LatchEntry],
    gate_id: GateId,
) -> (Result<Vec<ArmEvidence>, String>, Vec<BoundDecision>) {
    let source = match MetricSource::capture(tree, contract, baselines, actor, latches) {
        Ok(source) => source,
        Err(error) => return (Err(error.to_string()), Vec::new()),
    };
    let (evidence, bounds) = acquire_metric_arm_with_bounds(&source, gate_id).into_parts();
    let arm = evidence
        .map(|report| vec![ArmEvidence::Metrics { report }])
        .map_err(|error| error.to_string());
    (arm, bounds)
}

/// Acquire and judge the complete core family from one captured source.
///
/// Taken while the capture is still in hand rather than from anything the
/// executor retains afterwards, because the source is the only thing that ever
/// held the two Git observations. A refusal is carried as the family's result
/// instead of failing the capture: an incomplete family is evidence the battery
/// projects, not a reason no battery can run.
///
/// The bounds the acquisition decided are returned beside the arms because this
/// is the only moment they exist: judging consumes the inputs, and re-resolving
/// them from the report afterwards would be a second decision nothing was
/// judged against.
fn core_family(
    source: &CoreSource<'_>,
    contract: &Contract,
) -> (Result<Vec<ArmEvidence>, String>, Vec<BoundDecision>) {
    let inputs = match acquire_core(source, contract) {
        Ok(inputs) => inputs,
        Err(error) => return (Err(error.to_string()), Vec::new()),
    };
    let bounds = inputs.bounds().to_vec();
    let arms = evaluate_core(&inputs)
        .arms()
        .iter()
        .cloned()
        .map(|report| ArmEvidence::Core { report })
        .collect();
    (Ok(arms), bounds)
}

/// Acquire and judge exactly one core arm from the capture made for it.
fn core_arm(
    source: &CoreSource<'_>,
    contract: &Contract,
    gate_id: GateId,
) -> (Result<Vec<ArmEvidence>, String>, Vec<BoundDecision>) {
    let (input, bounds) = match acquire_core_arm(source, contract, gate_id) {
        Ok(acquisition) => acquisition,
        Err(error) => return (Err(error.to_string()), Vec::new()),
    };
    let report = evaluate_core_arm(&input);
    (Ok(vec![ArmEvidence::Core { report }]), bounds)
}

fn record_capture_analyses(
    trace: &mut Option<&mut dyn FnMut(RepositoryExecutionEvent)>,
    family: ArmFamily,
    gate_ids: impl IntoIterator<Item = GateId>,
) {
    if let Some(trace) = trace {
        for gate_id in gate_ids {
            trace(RepositoryExecutionEvent::AnalysisInvoked { gate_id, family });
        }
    }
}

const fn family_index(family: ArmFamily) -> usize {
    match family {
        ArmFamily::Core => 0,
        ArmFamily::Toolchain => 1,
        ArmFamily::Metrics => 2,
        ArmFamily::Hygiene => 3,
    }
}
