//! Fail-closed policy-arm selection for the ordinary staged-change gate.
//!
//! This module decides only which canonical battery arms are owed. It executes
//! none of them and performs no repository walk: the command rim supplies the
//! exact changed paths and the source digest they came from. Changes to an
//! authority that can alter the selector itself, or a path class this build
//! does not understand, refuse selection before any selected work can begin.
//! The explicit xtask current-source selector retains its richer source-tree
//! knowledge in a separate module under the same local-only contract.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use wire::Digest;

use crate::GateId;

mod current_source;
mod policy;
mod reach;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use current_source::{
    gates_for_path as current_source_gates_for_path, known_workspace_rust_source,
    selector_authority as current_source_selector_authority,
};
pub use current_source::{
    select_current_source_change_gates, select_current_source_change_gates_with_full_verifier,
};
pub use policy::{ChangeSelectionPolicy, ChangeSelectionPolicyInputs};
pub use reach::{
    ChangeArmReach, ChangeReachAuthority, ChangeReachEdge, ChangeReachMode, ChangeReachObject,
    ChangeReachOrigin,
};
use reach::{direct_reach, validate_reach};

/// Current staged-change policy-selection schema.
pub const CHANGE_SELECTION_REPORT_VERSION: u64 = 3;

/// The bounded execution mode admitted by the ordinary gate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeSelectionMode {
    /// Every changed path belongs to a known local class.
    Local,
}

/// One changed path's provenance for one selected canonical policy arm.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeSelectionEdge {
    source: String,
    gate_id: GateId,
}

impl ChangeSelectionEdge {
    fn new(source: &str, gate_id: GateId) -> Self {
        Self {
            source: source.to_owned(),
            gate_id,
        }
    }

    /// Exact repository-relative staged path.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Canonical battery arm selected for the path.
    #[must_use]
    pub const fn gate_id(&self) -> GateId {
        self.gate_id
    }
}

/// Typed staged-path-to-policy plan consumed by the ordinary gate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeSelectionReport {
    version: u64,
    source: Digest,
    mode: ChangeSelectionMode,
    changed_sources: Vec<String>,
    applicable_gate_ids: Vec<GateId>,
    gate_ids: Vec<GateId>,
    edges: Vec<ChangeSelectionEdge>,
    reach: Vec<ChangeArmReach>,
}

impl ChangeSelectionReport {
    /// Current report schema.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Immutable staged source the selection describes.
    #[must_use]
    pub const fn source(&self) -> &Digest {
        &self.source
    }

    /// Bounded local execution.
    #[must_use]
    pub const fn mode(&self) -> ChangeSelectionMode {
        self.mode
    }

    /// Sorted exact staged paths supplied by the command rim.
    #[must_use]
    pub fn changed_sources(&self) -> &[String] {
        &self.changed_sources
    }

    /// Canonical packaged arms applicable to this repository and policy.
    #[must_use]
    pub fn applicable_gate_ids(&self) -> &[GateId] {
        &self.applicable_gate_ids
    }

    /// Canonically ordered synchronous battery arms owed by the change.
    #[must_use]
    pub fn gate_ids(&self) -> &[GateId] {
        &self.gate_ids
    }

    /// Exact per-path reasons each arm entered the plan.
    #[must_use]
    pub fn edges(&self) -> &[ChangeSelectionEdge] {
        &self.edges
    }

    /// Canonically ordered object reach for every selected arm.
    #[must_use]
    pub fn reach(&self) -> &[ChangeArmReach] {
        &self.reach
    }

    /// Exact object reach for one selected arm, when selected.
    #[must_use]
    pub fn reach_for(&self, gate_id: GateId) -> Option<&ChangeArmReach> {
        self.reach
            .binary_search_by_key(&gate_id, ChangeArmReach::gate_id)
            .ok()
            .map(|index| &self.reach[index])
    }

    /// Replace direct path reach with a dependency-expanded canonical plan.
    ///
    /// The report remains the admission boundary: replacement must still cover
    /// every selected arm exactly once, prove every object from one selected
    /// source, and retain all original path-to-arm provenance.
    pub fn with_reach(mut self, reach: Vec<ChangeArmReach>) -> Result<Self, ChangeSelectionError> {
        self.reach = reach;
        self.validate()?;
        Ok(self)
    }

    /// Canonical JSON bytes for this selection.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        wire::encode(self)
    }

    /// Decode only a canonical, internally complete selection.
    pub fn decode(bytes: &[u8]) -> Result<Self, ChangeSelectionError> {
        let report: Self = wire::decode(bytes).map_err(|error| {
            ChangeSelectionError::new(format!("invalid change selection: {error}"))
        })?;
        report.validate()?;
        let canonical = report
            .encode()
            .map_err(|error| ChangeSelectionError::new(error.to_string()))?;
        if canonical != bytes {
            return Err(ChangeSelectionError::new(
                "change-selection report bytes are not canonical",
            ));
        }
        Ok(report)
    }

    fn validate(&self) -> Result<(), ChangeSelectionError> {
        if self.version != CHANGE_SELECTION_REPORT_VERSION {
            return Err(ChangeSelectionError::new(format!(
                "unsupported change-selection version {}; expected {CHANGE_SELECTION_REPORT_VERSION}",
                self.version,
            )));
        }
        validate_sorted_unique("changed source", &self.changed_sources)?;
        if self
            .changed_sources
            .iter()
            .any(|path| !valid_repository_path(path))
        {
            return Err(ChangeSelectionError::new(
                "change selection contains an invalid repository-relative path",
            ));
        }
        validate_gate_inventory("applicable", &self.applicable_gate_ids)?;
        validate_gate_inventory("selected", &self.gate_ids)?;
        if self
            .gate_ids
            .iter()
            .any(|gate_id| !self.applicable_gate_ids.contains(gate_id))
        {
            return Err(ChangeSelectionError::new(
                "change selection names an arm inapplicable to this repository",
            ));
        }
        for required in ALWAYS {
            if !self.applicable_gate_ids.contains(&required) || !self.gate_ids.contains(&required) {
                return Err(ChangeSelectionError::new(format!(
                    "change selection omitted always-on arm {required}"
                )));
            }
        }
        if !self.edges.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err(ChangeSelectionError::new(
                "change-selection edges must be sorted and unique",
            ));
        }
        for edge in &self.edges {
            if !self.changed_sources.contains(&edge.source)
                || !self.gate_ids.contains(&edge.gate_id)
            {
                return Err(ChangeSelectionError::new(
                    "change-selection edge names an unselected source or arm",
                ));
            }
        }
        if let Some(path) = self.changed_sources.iter().find(|path| {
            !self
                .edges
                .iter()
                .any(|edge| edge.source.as_str() == path.as_str())
        }) {
            return Err(ChangeSelectionError::new(format!(
                "changed source {path:?} has no policy-selection edge"
            )));
        }
        if !self.changed_sources.is_empty()
            && let Some(gate_id) = self
                .gate_ids
                .iter()
                .find(|gate_id| !self.edges.iter().any(|edge| edge.gate_id == **gate_id))
        {
            return Err(ChangeSelectionError::new(format!(
                "selected arm {gate_id} has no changed-source policy edge"
            )));
        }
        validate_reach(
            &self.reach,
            &self.changed_sources,
            &self.gate_ids,
            &self.edges,
        )?;
        Ok(())
    }
}

/// Select every applicable packaged policy arm owed by exact staged paths.
///
/// This function is pure and intentionally has no repository parameter. The
/// supplied policy was compiled from the same retained staged tree. A path it
/// cannot prove local refuses rather than omitting evidence or broadening the
/// ordinary gate into a complete verifier.
pub fn select_change_gates(
    source: Digest,
    changed_sources: &[String],
    policy: &ChangeSelectionPolicy,
) -> Result<ChangeSelectionReport, ChangeSelectionError> {
    let applicable = policy.applicable_gates(changed_sources);
    select_with(source, changed_sources, applicable, |path| {
        policy.gates_for_path(path)
    })
}

/// Select packaged reach after both full-verifier merge parents were admitted.
///
/// Known authority paths may select every applicable arm only on this explicit
/// merge surface. Their reach is repository-exhaustive: a reused arm consumes
/// its matching parent receipt, while any invalidated arm executes its complete
/// invariant rather than a path-focused approximation. Unknown paths and
/// sources outside compiled repository structure continue to refuse.
pub fn select_change_gates_with_parent_evidence(
    source: Digest,
    changed_sources: &[String],
    policy: &ChangeSelectionPolicy,
) -> Result<ChangeSelectionReport, ChangeSelectionError> {
    let applicable = policy.applicable_gates(changed_sources);
    let mut report = select_with(source, changed_sources, applicable.clone(), |path| {
        if policy.selection_authority(path) {
            Ok(applicable.clone())
        } else {
            policy.gates_for_path(path)
        }
    })?;
    let authority_paths = report
        .changed_sources()
        .iter()
        .filter(|path| policy.selection_authority(path))
        .cloned()
        .collect::<BTreeSet<_>>();
    if authority_paths.is_empty() {
        return Ok(report);
    }
    let reach = report
        .reach()
        .iter()
        .map(|current| {
            let gate_id = current.gate_id();
            let mut provenance = Vec::new();
            if ALWAYS.contains(&gate_id) {
                provenance.push(ChangeReachEdge::always(ChangeReachObject::Repository));
            }
            provenance.extend(
                report
                    .edges()
                    .iter()
                    .filter(|edge| edge.gate_id() == gate_id)
                    .map(|edge| {
                        ChangeReachEdge::changed(edge.source(), ChangeReachObject::Repository)
                    }),
            );
            provenance.sort();
            provenance.dedup();
            ChangeArmReach::exhaustive(
                gate_id,
                ChangeReachAuthority::SelectionPolicy,
                "an admitted merge parent owns changed selection authority",
                vec![ChangeReachObject::Repository],
                provenance,
            )
        })
        .collect();
    report = report.with_reach(reach)?;
    Ok(report)
}

fn select_with(
    source: Digest,
    changed_sources: &[String],
    applicable_gate_ids: Vec<GateId>,
    mut gates_for_path: impl FnMut(&str) -> Result<Vec<GateId>, ChangeSelectionError>,
) -> Result<ChangeSelectionReport, ChangeSelectionError> {
    let mut changed_sources = changed_sources.to_vec();
    changed_sources.sort();
    changed_sources.dedup();
    if let Some(path) = changed_sources
        .iter()
        .find(|path| !valid_repository_path(path))
    {
        return Err(ChangeSelectionError::new(format!(
            "{path:?} is not a normalized repository-relative path"
        )));
    }

    validate_gate_inventory("applicable", &applicable_gate_ids)?;
    let applicable = applicable_gate_ids.iter().copied().collect::<BTreeSet<_>>();
    let mut selected = ALWAYS.into_iter().collect::<BTreeSet<_>>();
    let mut edges = Vec::new();
    for path in &changed_sources {
        let gates = gates_for_path(path)?;
        let gates = ALWAYS
            .into_iter()
            .chain(gates)
            .filter(|gate_id| applicable.contains(gate_id))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        selected.extend(gates.iter().copied());
        edges.extend(
            gates
                .iter()
                .copied()
                .map(|gate_id| ChangeSelectionEdge::new(path, gate_id)),
        );
    }
    let gate_ids = selected.into_iter().collect::<Vec<_>>();
    edges.sort();
    edges.dedup();
    let reach = direct_reach(&changed_sources, &gate_ids, &edges);
    let report = ChangeSelectionReport {
        version: CHANGE_SELECTION_REPORT_VERSION,
        source,
        mode: ChangeSelectionMode::Local,
        changed_sources,
        applicable_gate_ids,
        gate_ids,
        edges,
        reach,
    };
    report.validate()?;
    Ok(report)
}

const ALWAYS: [GateId; 3] = [GateId::MergeIntegrity, GateId::PlanPhase, GateId::Staging];

pub(crate) fn valid_repository_path(path: &str) -> bool {
    valid_text(path)
        && !path.starts_with('/')
        && !path.starts_with("./")
        && !path.split('/').any(|part| part.is_empty() || part == "..")
}

fn valid_text(value: &str) -> bool {
    !value.trim().is_empty() && !value.contains(['\0', '\r', '\n'])
}

fn validate_sorted_unique(label: &str, values: &[String]) -> Result<(), ChangeSelectionError> {
    if values.windows(2).all(|pair| pair[0] < pair[1]) {
        Ok(())
    } else {
        Err(ChangeSelectionError::new(format!(
            "{label} inventory must be sorted and unique"
        )))
    }
}

fn validate_gate_inventory(label: &str, gate_ids: &[GateId]) -> Result<(), ChangeSelectionError> {
    if gate_ids.is_empty()
        || !gate_ids.windows(2).all(|pair| pair[0] < pair[1])
        || gate_ids
            .iter()
            .any(|gate_id| !GateId::BATTERY.contains(gate_id))
    {
        Err(ChangeSelectionError::new(format!(
            "change selection requires unique canonical {label} arms in manifest order"
        )))
    } else {
        Ok(())
    }
}

/// Invalid changed-path input or malformed retained selection evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeSelectionError {
    detail: String,
}

impl ChangeSelectionError {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }

    /// Exact refusal detail.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for ChangeSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl Error for ChangeSelectionError {}

#[cfg(test)]
#[path = "change_selection/tests.rs"]
mod tests;
