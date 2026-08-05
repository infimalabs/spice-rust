//! Production execution of the compiled battery from one selected tree.

use std::path::{Path, PathBuf};

use wire::Digest;

use super::{
    ArmEvidence, ArmExecution, ArmFamily, BatteryContext, BatteryError, BatteryExecutor,
    BatteryInputs, CompiledArm, inline_evidence,
};
use crate::GateId;
use crate::core::{ActorCapture, ClaimCapture, CoreSource, acquire_core, evaluate_core};
use crate::hygiene::{HygieneConfiguration, HygieneSource, acquire_hygiene};
use crate::input::Contract;
use crate::metrics::{BaselineCapture, MetricSource, acquire_metrics};
use crate::toolchain::{RimCapture, ToolchainSource, acquire_toolchain, measure_toolchain};
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
    /// The executor invoked one complete family acquisition.
    FamilyInvoked { family: ArmFamily },
}

enum FamilyExecution {
    Complete(Vec<ArmEvidence>),
    Refused(String),
}

/// One capture-owning production implementation of [`BatteryExecutor`].
///
/// A family is acquired when its first arm is requested. Its complete result
/// set or refusal is cached, so the remaining canonical positions only project
/// family-owned evidence and cannot run the family again.
///
/// The core family is the one exception, and it is acquired in full at capture.
/// Two of its arms observe the live repository the tree was materialized from,
/// so they can only be taken in the same moment the source is captured: an
/// acquisition deferred to first request would read an index, a `MERGE_HEAD`,
/// or a worktree that moved after the capture the report names. Its complete
/// result or refusal is retained from that one capture and handed over
/// unchanged when the first core arm is requested.
///
/// The committed magic-number baselines are read in that same moment and for
/// the same reason, even though the arm that reads them is not a core arm. They
/// are the live repository's `HEAD`, and `HEAD` moves: a read deferred to first
/// request would compare part of the selection against one commit and the rest
/// against another, and the report would name neither.
pub struct RepositoryExecutor<'trace> {
    tree: SelectedTree,
    contract: Contract,
    source: Digest,
    core: Result<Vec<ArmEvidence>, String>,
    baselines: BaselineCapture,
    rim: RimCapture,
    hygiene: HygieneConfiguration,
    context: Option<BatteryContext>,
    families: [Option<FamilyExecution>; FAMILY_COUNT],
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
    pub(super) fn capture(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        actor: ActorCapture,
        rim: RimCapture,
        hygiene: HygieneConfiguration,
        trace: Option<&'trace mut dyn FnMut(RepositoryExecutionEvent)>,
    ) -> Result<Self, BatteryError> {
        let repository: PathBuf = repository.into();
        let tree = selector.materialize(&repository).map_err(|error| {
            BatteryError::new(format!("cannot capture battery source: {error}"))
        })?;
        let contract = Contract::load(tree.root()).map_err(|error| {
            BatteryError::new(format!("cannot load captured battery contract: {error}"))
        })?;
        // One capture, taken once: the digest every arm is labelled with, the
        // whole core family, and the committed baselines the magic-number delta
        // is measured against are all read here, and nothing below this block
        // can reach the live repository again.
        let (source, core, baselines) = {
            let captured =
                CoreSource::capture(&tree, &contract, claim, actor).map_err(|error| {
                    BatteryError::new(format!("cannot identify battery source: {error}"))
                })?;
            let core = core_family(&captured, &contract);
            let baselines = BaselineCapture::capture(
                &repository,
                contract.rule_policy().document().file_classes().magic(),
            );
            (captured.digest().clone(), core, baselines)
        };
        Ok(Self {
            tree,
            contract,
            source,
            core,
            baselines,
            rim,
            hygiene,
            context: None,
            families: std::array::from_fn(|_| None),
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

    fn acquire_family(&self, family: ArmFamily) -> FamilyExecution {
        let result = match family {
            // Not acquired here: this hands over the result the capture already
            // took, so the first core arm cannot start a second observation.
            ArmFamily::Core => self.core.clone(),
            ArmFamily::Toolchain => self.acquire_toolchain_family(),
            ArmFamily::Metrics => self.acquire_metrics_family(),
            ArmFamily::Hygiene => self.acquire_hygiene_family(),
        };
        match result {
            Ok(evidence) => FamilyExecution::Complete(evidence),
            Err(reason) => FamilyExecution::Refused(reason),
        }
    }

    fn acquire_toolchain_family(&self) -> Result<Vec<ArmEvidence>, String> {
        let source = ToolchainSource::capture(&self.tree, self.rim.clone());
        let report = measure_toolchain(acquire_toolchain(&source, &self.contract))
            .map_err(|error| error.to_string())?;
        Ok(report
            .arms
            .into_iter()
            .map(|report| ArmEvidence::Toolchain { report })
            .collect())
    }

    /// Acquire all four structural arms, each carrying its own typed state.
    ///
    /// A family-wide refusal is reserved for a source that cannot be walked or
    /// evidence that violates its declaration. An arm this build cannot measure
    /// is not one of those: it acquires as evidence naming the input it lacks,
    /// so the three that stay unreachable cannot suppress the one that measures.
    fn acquire_metrics_family(&self) -> Result<Vec<ArmEvidence>, String> {
        let source = MetricSource::capture(&self.tree, &self.contract, &self.baselines)
            .map_err(|error| error.to_string())?;
        Ok(acquire_metrics(&source)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|report| ArmEvidence::Metrics { report })
            .collect())
    }

    fn acquire_hygiene_family(&self) -> Result<Vec<ArmEvidence>, String> {
        let source = HygieneSource::capture(&self.tree, &self.contract)
            .map_err(|error| error.to_string())?;
        let report = acquire_hygiene(&source, &self.hygiene).map_err(|error| error.to_string())?;
        Ok(report
            .arms
            .into_iter()
            .map(|report| ArmEvidence::Hygiene { report })
            .collect())
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

/// Acquire and judge the complete core family from one captured source.
///
/// Taken while the capture is still in hand rather than from anything the
/// executor retains afterwards, because the source is the only thing that ever
/// held the two Git observations. A refusal is carried as the family's result
/// instead of failing the capture: an incomplete family is evidence the battery
/// projects, not a reason no battery can run.
fn core_family(source: &CoreSource<'_>, contract: &Contract) -> Result<Vec<ArmEvidence>, String> {
    let inputs = acquire_core(source, contract).map_err(|error| error.to_string())?;
    Ok(evaluate_core(&inputs)
        .arms()
        .iter()
        .cloned()
        .map(|report| ArmEvidence::Core { report })
        .collect())
}

const fn family_index(family: ArmFamily) -> usize {
    match family {
        ArmFamily::Core => 0,
        ArmFamily::Toolchain => 1,
        ArmFamily::Metrics => 2,
        ArmFamily::Hygiene => 3,
    }
}
