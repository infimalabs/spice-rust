//! Bounded public folds of private installation authority.

use serde::{Deserialize, Serialize};
use wire::Digest;

use super::{Asset, GeneratedImage, InstallError, InstallPlan, Location, SymbolProvenance};

/// Kind of durable action represented by one ordered public stage.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallStageKind {
    FilePublication,
    Retirement,
    HookActivation,
}

/// One byte-free file-publication stage in canonical execution order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallOperationSummary {
    index: usize,
    kind: InstallStageKind,
    asset: Asset,
    location: Location,
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous: Option<GeneratedImage>,
    generated: GeneratedImage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    superseded: Option<GeneratedImage>,
    introduced_parents: Vec<String>,
}

impl InstallOperationSummary {
    /// Zero-based position among all durable stages.
    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }

    /// Stable action discriminator for this stage.
    #[must_use]
    pub const fn kind(&self) -> InstallStageKind {
        self.kind
    }

    /// Kind of asset this stage publishes.
    #[must_use]
    pub const fn asset(&self) -> Asset {
        self.asset
    }

    /// Root against which [`Self::path`] resolves.
    #[must_use]
    pub const fn location(&self) -> Location {
        self.location
    }

    /// Repository-relative destination spelling.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Mode and digest restored by unapply, absent for an introduced file.
    #[must_use]
    pub const fn previous(&self) -> Option<&GeneratedImage> {
        self.previous.as_ref()
    }

    /// Mode and digest published by this stage.
    #[must_use]
    pub const fn generated(&self) -> &GeneratedImage {
        &self.generated
    }

    /// Mode and digest replaced by a refresh.
    #[must_use]
    pub const fn superseded(&self) -> Option<&GeneratedImage> {
        self.superseded.as_ref()
    }

    /// Parent directories introduced by this stage, outermost first.
    #[must_use]
    pub fn introduced_parents(&self) -> &[String] {
        &self.introduced_parents
    }
}

/// One byte-free legacy retirement stage in canonical execution order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRetirementSummary {
    index: usize,
    kind: InstallStageKind,
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous: Option<GeneratedImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    observed: Option<GeneratedImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    superseded: Option<GeneratedImage>,
}

impl InstallRetirementSummary {
    /// Zero-based position among all durable stages.
    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }

    /// Stable action discriminator for this stage.
    #[must_use]
    pub const fn kind(&self) -> InstallStageKind {
        self.kind
    }

    /// Repository-common relative path removed by this stage.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Mode and digest restored by unapply, when one existed.
    #[must_use]
    pub const fn previous(&self) -> Option<&GeneratedImage> {
        self.previous.as_ref()
    }

    /// Mode and digest observed before retirement.
    #[must_use]
    pub const fn observed(&self) -> Option<&GeneratedImage> {
        self.observed.as_ref()
    }

    /// Mode and digest replaced by a refresh.
    #[must_use]
    pub const fn superseded(&self) -> Option<&GeneratedImage> {
        self.superseded.as_ref()
    }
}

/// Ordered byte-free metadata for repository-wide Git hook activation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HookActivationSummary {
    index: usize,
    kind: InstallStageKind,
    hooks_path: String,
    worktrees: Vec<String>,
    mutation_count: usize,
}

impl HookActivationSummary {
    /// Zero-based position among all durable stages.
    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }

    /// Stable action discriminator for this stage.
    #[must_use]
    pub const fn kind(&self) -> InstallStageKind {
        self.kind
    }

    /// Absolute installed hook directory selected for every worktree.
    #[must_use]
    pub fn hooks_path(&self) -> &str {
        &self.hooks_path
    }

    /// Canonically ordered linked worktrees covered by activation.
    #[must_use]
    pub fn worktrees(&self) -> &[String] {
        &self.worktrees
    }

    /// Number of exact Git configuration mutations in the private plan.
    #[must_use]
    pub const fn mutation_count(&self) -> usize {
        self.mutation_count
    }
}

/// Byte-free public description of one canonical private install plan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallPlanSummary {
    version: u16,
    operation_count: usize,
    operations: Vec<InstallOperationSummary>,
    retirements: Vec<InstallRetirementSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hook_activation: Option<HookActivationSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    symbols: Option<SymbolProvenance>,
}

impl InstallPlanSummary {
    pub(super) fn from_plan(plan: &InstallPlan) -> Self {
        let operations = plan
            .operations
            .iter()
            .enumerate()
            .map(|(index, operation)| InstallOperationSummary {
                index,
                kind: InstallStageKind::FilePublication,
                asset: operation.asset,
                location: operation.location,
                path: operation.path.clone(),
                previous: operation.previous.clone(),
                generated: operation.generated.clone(),
                superseded: operation.superseded.clone(),
                introduced_parents: operation.introduced_parents.clone(),
            })
            .collect();
        let retirement_offset = plan.operations.len();
        let retirements = plan
            .retirements
            .iter()
            .enumerate()
            .map(|(offset, retirement)| InstallRetirementSummary {
                index: retirement_offset + offset,
                kind: InstallStageKind::Retirement,
                path: retirement.path.clone(),
                previous: retirement.previous.clone(),
                observed: retirement.observed.clone(),
                superseded: retirement.superseded.clone(),
            })
            .collect();
        let hook_activation =
            plan.hook_activation
                .as_ref()
                .map(|activation| HookActivationSummary {
                    index: retirement_offset + plan.retirements.len(),
                    kind: InstallStageKind::HookActivation,
                    hooks_path: activation.hooks_path().to_owned(),
                    worktrees: activation
                        .observations()
                        .iter()
                        .map(|observation| observation.worktree().to_owned())
                        .collect(),
                    mutation_count: activation.mutations().len(),
                });
        Self {
            version: plan.version,
            operation_count: plan.operation_count(),
            operations,
            retirements,
            hook_activation,
            symbols: plan.symbols.clone(),
        }
    }

    /// Private canonical plan schema version represented by this summary.
    #[must_use]
    pub const fn version(&self) -> u16 {
        self.version
    }

    /// Number of durable publication, retirement, and activation stages.
    #[must_use]
    pub const fn operation_count(&self) -> usize {
        self.operation_count
    }

    /// File publications in their canonical execution order.
    #[must_use]
    pub fn operations(&self) -> &[InstallOperationSummary] {
        &self.operations
    }

    /// Legacy retirements immediately following file publication.
    #[must_use]
    pub fn retirements(&self) -> &[InstallRetirementSummary] {
        &self.retirements
    }

    /// Final repository-wide hook activation stage, when configured.
    #[must_use]
    pub const fn hook_activation(&self) -> Option<&HookActivationSummary> {
        self.hook_activation.as_ref()
    }

    /// Provenance of the executable and retained dSYM, when symbols publish.
    #[must_use]
    pub const fn symbols(&self) -> Option<&SymbolProvenance> {
        self.symbols.as_ref()
    }
}

/// One side-effect-free install preview and the exact identity admitted by apply.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallPreview {
    digest: Digest,
    plan: InstallPlanSummary,
}

impl InstallPreview {
    /// Bind a validated canonical plan to its dedicated-domain identity.
    pub fn from_plan(plan: InstallPlan) -> Result<Self, InstallError> {
        plan.validate()?;
        let digest = plan.digest()?;
        let plan = InstallPlanSummary::from_plan(&plan);
        Ok(Self { digest, plan })
    }

    /// Exact canonical plan identity accepted by `install --apply`.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }

    /// Bounded public metadata for the private canonical installation plan.
    #[must_use]
    pub const fn plan(&self) -> &InstallPlanSummary {
        &self.plan
    }
}

/// Durable progress state for apply and reversal.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallStatus {
    Applying,
    Complete,
    Unapplying,
    Uninstalled,
}

/// Current strict installation receipt projected without private reversal bytes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallReceipt {
    pub(super) plan: InstallPlanSummary,
    pub(super) digest: Digest,
    pub(super) status: InstallStatus,
    pub(super) completed: usize,
}

impl InstallReceipt {
    /// Bounded public metadata for the private plan retained by the record.
    #[must_use]
    pub const fn plan(&self) -> &InstallPlanSummary {
        &self.plan
    }

    /// Plan identity admitted by apply.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }

    /// Durable progress state.
    #[must_use]
    pub const fn status(&self) -> InstallStatus {
        self.status
    }

    /// Operations durably acknowledged in the current direction.
    #[must_use]
    pub const fn completed(&self) -> usize {
        self.completed
    }
}
