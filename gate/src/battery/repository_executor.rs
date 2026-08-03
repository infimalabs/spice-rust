//! Production execution of the compiled battery from one selected tree.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use wire::Digest;

use super::{
    ArmEvidence, ArmExecution, ArmFamily, BatteryContext, BatteryError, BatteryExecutor,
    BatteryInputs, CompiledArm, inline_evidence,
};
use crate::GateId;
use crate::core::{ClaimCapture, CoreSource, acquire_core, evaluate_core};
use crate::hygiene::{HygieneConfiguration, HygieneSource, acquire_hygiene};
use crate::input::Contract;
use crate::toolchain::{ToolchainSource, acquire_toolchain, measure_toolchain};
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
pub struct RepositoryExecutor<'trace> {
    repository: PathBuf,
    tree: SelectedTree,
    contract: Contract,
    source: Digest,
    claim: ClaimCapture,
    search_path: Option<OsString>,
    hygiene: HygieneConfiguration,
    context: Option<BatteryContext>,
    families: [Option<FamilyExecution>; FAMILY_COUNT],
    trace: Option<&'trace mut dyn FnMut(RepositoryExecutionEvent)>,
}

impl RepositoryExecutor<'_> {
    /// Materialize and retain one repository selection for a complete battery.
    ///
    /// # Errors
    ///
    /// Refuses selection, tracked-contract, or source-identity failure before
    /// an executor can be used.
    pub fn capture(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        search_path: Option<OsString>,
        hygiene: HygieneConfiguration,
    ) -> Result<Self, BatteryError> {
        Self::capture_inner(repository, selector, claim, search_path, hygiene, None)
    }
}

impl<'trace> RepositoryExecutor<'trace> {
    /// Capture one repository and emit every arm and family invocation to
    /// `trace` without granting it authority over execution.
    ///
    /// # Errors
    ///
    /// Refuses the same pre-execution failures as [`Self::capture`].
    pub fn capture_traced(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        search_path: Option<OsString>,
        hygiene: HygieneConfiguration,
        trace: &'trace mut dyn FnMut(RepositoryExecutionEvent),
    ) -> Result<Self, BatteryError> {
        Self::capture_inner(
            repository,
            selector,
            claim,
            search_path,
            hygiene,
            Some(trace),
        )
    }

    fn capture_inner(
        repository: impl Into<PathBuf>,
        selector: &Selector,
        claim: ClaimCapture,
        search_path: Option<OsString>,
        hygiene: HygieneConfiguration,
        trace: Option<&'trace mut dyn FnMut(RepositoryExecutionEvent)>,
    ) -> Result<Self, BatteryError> {
        let repository = repository.into();
        let tree = selector.materialize(&repository).map_err(|error| {
            BatteryError::new(format!("cannot capture battery source: {error}"))
        })?;
        let contract = Contract::load(tree.root()).map_err(|error| {
            BatteryError::new(format!("cannot load captured battery contract: {error}"))
        })?;
        let source = CoreSource::capture(&repository, &tree, &contract, claim.clone())
            .map_err(|error| BatteryError::new(format!("cannot identify battery source: {error}")))?
            .digest()
            .clone();
        Ok(Self {
            repository,
            tree,
            contract,
            source,
            claim,
            search_path,
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
            ArmFamily::Core => self.acquire_core_family(),
            ArmFamily::Toolchain => self.acquire_toolchain_family(),
            ArmFamily::Metrics => Err(concat!(
                "metrics family acquisition is unavailable: this build has no complete ",
                "selected-scope structural acquisition entrypoint"
            )
            .to_owned()),
            ArmFamily::Hygiene => self.acquire_hygiene_family(),
        };
        match result {
            Ok(evidence) => FamilyExecution::Complete(evidence),
            Err(reason) => FamilyExecution::Refused(reason),
        }
    }

    fn acquire_core_family(&self) -> Result<Vec<ArmEvidence>, String> {
        let source = CoreSource::capture(
            &self.repository,
            &self.tree,
            &self.contract,
            self.claim.clone(),
        )
        .map_err(|error| error.to_string())?;
        let inputs = acquire_core(&source, &self.contract).map_err(|error| error.to_string())?;
        Ok(evaluate_core(&inputs)
            .arms()
            .iter()
            .cloned()
            .map(|report| ArmEvidence::Core { report })
            .collect())
    }

    fn acquire_toolchain_family(&self) -> Result<Vec<ArmEvidence>, String> {
        let source = ToolchainSource::capture(&self.tree, self.search_path.clone());
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

const fn family_index(family: ArmFamily) -> usize {
    match family {
        ArmFamily::Core => 0,
        ArmFamily::Toolchain => 1,
        ArmFamily::Metrics => 2,
        ArmFamily::Hygiene => 3,
    }
}
