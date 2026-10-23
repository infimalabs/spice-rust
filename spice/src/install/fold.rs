//! Bounded public folds of private installation authority.

use serde::{Deserialize, Serialize};
use std::fmt;

use wire::Digest;

use super::{
    Asset, GeneratedImage, InstallError, InstallPlan, Location, PreparedInstall, SymbolProvenance,
};

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

    fn changes_file(&self) -> bool {
        self.superseded.as_ref().or(self.previous.as_ref()) != Some(&self.generated)
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    introduced_parents: Vec<String>,
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

    /// Parent directories withdrawn after this reversal, outermost first.
    #[must_use]
    pub fn introduced_parents(&self) -> &[String] {
        &self.introduced_parents
    }

    fn removes_file(&self) -> bool {
        self.superseded.is_some() || self.observed.is_some()
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

    /// Number of Git configuration mutations in the private plan that write a
    /// value other than the one in place.
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
                introduced_parents: retirement.introduced_parents.clone(),
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
                    mutation_count: activation
                        .mutations()
                        .iter()
                        .filter(|mutation| mutation.changes())
                        .count(),
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

    /// Restate a complete install as the plan apply would execute over it:
    /// every file already holds what it would publish, nothing remains to
    /// retire, and no hook setting remains to write.
    fn settle(&mut self) {
        for operation in &mut self.operations {
            operation.superseded = Some(operation.generated.clone());
        }
        for retirement in &mut self.retirements {
            retirement.observed = None;
            retirement.superseded = None;
        }
        if let Some(activation) = &mut self.hook_activation {
            activation.mutation_count = 0;
        }
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
    /// Preview the plan apply would execute now, bound to the identity apply
    /// admits. Over a complete install of this same build apply publishes
    /// nothing, so its stages read as already in place.
    pub fn from_prepared(prepared: PreparedInstall) -> Result<Self, InstallError> {
        let converged = prepared.converged();
        let plan = prepared.into_plan();
        plan.validate()?;
        let digest = plan.digest()?;
        let mut plan = InstallPlanSummary::from_plan(&plan);
        if converged {
            plan.settle();
        }
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

    /// Write the person-facing preview, ending with the exact command that
    /// applies this digest from where the preview came from.
    pub fn write_text(
        &self,
        formatter: &mut fmt::Formatter<'_>,
        apply: &dyn fmt::Display,
    ) -> fmt::Result {
        let files = self
            .plan
            .operations
            .iter()
            .filter(|operation| operation.changes_file())
            .count();
        let retirements = self
            .plan
            .retirements
            .iter()
            .filter(|retirement| retirement.removes_file())
            .count();
        let hooks = self
            .plan
            .hook_activation
            .as_ref()
            .map_or(0, HookActivationSummary::mutation_count);

        writeln!(formatter, "install plan {}", self.digest)?;
        if files == 0 && retirements == 0 && hooks == 0 {
            writeln!(formatter, "changes: none")?;
        } else {
            writeln!(
                formatter,
                "changes: {files} {}, {retirements} {}, {hooks} {}",
                plural(files, "file", "files"),
                plural(retirements, "retirement", "retirements"),
                plural(hooks, "hook setting", "hook settings"),
            )?;
        }
        for operation in self
            .plan
            .operations
            .iter()
            .filter(|operation| operation.changes_file())
        {
            let action = if operation.superseded.is_some() || operation.previous.is_some() {
                "update"
            } else {
                "create"
            };
            writeln!(formatter, "  {action} {}", operation.path)?;
        }
        for retirement in self
            .plan
            .retirements
            .iter()
            .filter(|retirement| retirement.removes_file())
        {
            writeln!(formatter, "  remove {}", retirement.path)?;
        }
        if let Some(activation) = self
            .plan
            .hook_activation
            .as_ref()
            .filter(|activation| activation.mutation_count() > 0)
        {
            writeln!(
                formatter,
                "  set {hooks} {} to {}",
                plural(hooks, "Git hook setting", "Git hook settings"),
                activation.hooks_path(),
            )?;
        }
        write!(formatter, "apply: {apply}")
    }
}

const fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
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

impl fmt::Display for InstallReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match self.status {
            InstallStatus::Applying => "installation applying",
            InstallStatus::Complete => "installation complete",
            InstallStatus::Unapplying => "installation removal in progress",
            InstallStatus::Uninstalled => "installation removed",
        };
        writeln!(formatter, "{state}")?;
        writeln!(formatter, "plan: {}", self.digest)?;
        write!(
            formatter,
            "progress: {}/{} stages",
            self.completed, self.plan.operation_count
        )?;
        match self.status {
            InstallStatus::Complete => write!(formatter, "\nunapply: spice install --unapply"),
            InstallStatus::Uninstalled => write!(formatter, "\npreview: spice install"),
            InstallStatus::Applying | InstallStatus::Unapplying => Ok(()),
        }
    }
}
