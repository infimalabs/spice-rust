//! Production execution of the compiled battery from one selected tree.

use std::path::{Path, PathBuf};

use wire::Digest;

use super::{
    ArmDisposition, ArmEvidence, ArmExecution, ArmFamily, BatteryContext, BatteryError,
    BatteryExecutor, BatteryInputs, BatteryManifest, BatteryManifestArm, CompiledArm,
    inline_evidence,
};
use crate::GateId;
use crate::core::{ActorCapture, ClaimCapture, CoreSource, acquire_core, evaluate_core};
use crate::hygiene::{HygieneConfiguration, HygieneSource, acquire_hygiene};
use crate::input::Contract;
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
pub struct RepositoryExecutor<'trace> {
    tree: SelectedTree,
    contract: Contract,
    source: Digest,
    core: Result<Vec<ArmEvidence>, String>,
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
        // One capture, taken once: the digest every arm is labelled with and the
        // whole core family are read from the same `CoreSource`, and nothing
        // below this block can reach the live repository again.
        let (source, core) = {
            let captured =
                CoreSource::capture(&tree, &contract, claim, actor).map_err(|error| {
                    BatteryError::new(format!("cannot identify battery source: {error}"))
                })?;
            let core = core_family(&captured, &contract);
            (captured.digest().clone(), core)
        };
        Ok(Self {
            tree,
            contract,
            source,
            core,
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
            ArmFamily::Metrics => Err(metrics_unavailable()),
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

/// Why no metrics arm runs here, and which predecessor steps that leaves open.
///
/// The manifest gives every row one `compiled` disposition, which reads as
/// covered, so a refusal naming only the absent entrypoint would leave whoever
/// is deciding whether the predecessor gate can be retired to work the cost out
/// by hand. The step names are read back out of the canonical manifest rather
/// than restated here, so a row that changes family carries this list with it.
fn metrics_unavailable() -> String {
    let manifest = BatteryManifest::canonical()
        .expect("the canonical manifest decodes before any arm is executed");
    let unanswered: Vec<&str> = manifest
        .arms()
        .iter()
        .filter(|arm| declared_family(arm) == ArmFamily::Metrics)
        .filter_map(BatteryManifestArm::predecessor)
        .collect();
    format!(
        "metrics family acquisition is unavailable: this build has no complete \
         selected-scope structural acquisition entrypoint, so the predecessor steps \
         it answers for ({}) are measured by nothing here",
        unanswered.join(", ")
    )
}

/// The family one manifest row's single compiled disposition declares.
fn declared_family(arm: &BatteryManifestArm) -> ArmFamily {
    let ArmDisposition::Compiled { family, .. } = arm.disposition();
    *family
}

const fn family_index(family: ArmFamily) -> usize {
    match family {
        ArmFamily::Core => 0,
        ArmFamily::Toolchain => 1,
        ArmFamily::Metrics => 2,
        ArmFamily::Hygiene => 3,
    }
}
