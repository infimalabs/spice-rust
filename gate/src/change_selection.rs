//! Fail-closed policy-arm selection for the ordinary staged-change gate.
//!
//! This module decides only which canonical battery arms are owed. It executes
//! none of them and performs no repository walk: the command rim supplies the
//! exact changed paths and the source digest they came from. Changes to an
//! authority that can alter the selector itself, or a path class this build
//! does not understand, escalate to the complete synchronous battery.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use wire::Digest;

use crate::GateId;

/// Current staged-change policy-selection schema.
pub const CHANGE_SELECTION_REPORT_VERSION: u64 = 1;

/// Whether the ordinary gate proved a local arm set or escalated deliberately.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeSelectionMode {
    /// Every changed path belongs to a known local class.
    Local,
    /// An authority or unknown path requires every synchronous battery arm.
    Exhaustive,
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
    gate_ids: Vec<GateId>,
    edges: Vec<ChangeSelectionEdge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    escalation: Option<String>,
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

    /// Local execution or explicit complete-battery escalation.
    #[must_use]
    pub const fn mode(&self) -> ChangeSelectionMode {
        self.mode
    }

    /// Sorted exact staged paths supplied by the command rim.
    #[must_use]
    pub fn changed_sources(&self) -> &[String] {
        &self.changed_sources
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

    /// Why local evidence was insufficient, only in exhaustive mode.
    #[must_use]
    pub fn escalation(&self) -> Option<&str> {
        self.escalation.as_deref()
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
        if self.gate_ids.is_empty()
            || !self.gate_ids.windows(2).all(|pair| pair[0] < pair[1])
            || self
                .gate_ids
                .iter()
                .any(|gate_id| !GateId::BATTERY.contains(gate_id))
        {
            return Err(ChangeSelectionError::new(
                "change selection requires unique canonical battery arms in manifest order",
            ));
        }
        for required in ALWAYS {
            if !self.gate_ids.contains(&required) {
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
        match (self.mode, self.escalation.as_deref()) {
            (ChangeSelectionMode::Local, None) => {}
            (ChangeSelectionMode::Exhaustive, Some(reason)) if valid_text(reason) => {
                if self.gate_ids != GateId::BATTERY {
                    return Err(ChangeSelectionError::new(
                        "exhaustive change selection must name the complete battery",
                    ));
                }
            }
            _ => {
                return Err(ChangeSelectionError::new(
                    "local selection forbids escalation and exhaustive selection requires it",
                ));
            }
        }
        Ok(())
    }
}

/// Select every canonical policy arm owed by exact staged paths.
///
/// This function is pure and intentionally has no repository parameter. A path
/// class this build cannot prove is safe causes a complete-battery plan rather
/// than omission or success with unknown evidence.
pub fn select_change_gates(
    source: Digest,
    changed_sources: &[String],
) -> Result<ChangeSelectionReport, ChangeSelectionError> {
    let mut changed_sources = changed_sources.to_vec();
    changed_sources.sort();
    changed_sources.dedup();
    if changed_sources
        .iter()
        .any(|path| !valid_repository_path(path))
    {
        return Err(ChangeSelectionError::new(
            "changed sources must be normalized repository-relative paths",
        ));
    }

    let mut selected = ALWAYS.into_iter().collect::<BTreeSet<_>>();
    let mut per_path = Vec::new();
    let mut escalations = Vec::new();
    for path in &changed_sources {
        let (gates, escalation) = gates_for_path(path);
        let gates = ALWAYS
            .into_iter()
            .chain(gates)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        selected.extend(gates.iter().copied());
        per_path.push((path.clone(), gates));
        if let Some(reason) = escalation {
            escalations.push(reason);
        }
    }
    let mode = if escalations.is_empty() {
        ChangeSelectionMode::Local
    } else {
        selected = GateId::BATTERY.into_iter().collect();
        ChangeSelectionMode::Exhaustive
    };
    let gate_ids = selected.into_iter().collect::<Vec<_>>();
    let mut edges = Vec::new();
    for (path, local) in per_path {
        let gates = if mode == ChangeSelectionMode::Exhaustive {
            gate_ids.as_slice()
        } else {
            local.as_slice()
        };
        edges.extend(
            gates
                .iter()
                .copied()
                .map(|gate_id| ChangeSelectionEdge::new(&path, gate_id)),
        );
    }
    edges.sort();
    edges.dedup();
    let report = ChangeSelectionReport {
        version: CHANGE_SELECTION_REPORT_VERSION,
        source,
        mode,
        changed_sources,
        gate_ids,
        edges,
        escalation: (!escalations.is_empty()).then(|| escalations.join("; ")),
    };
    report.validate()?;
    Ok(report)
}

const ALWAYS: [GateId; 3] = [GateId::MergeIntegrity, GateId::PlanPhase, GateId::Staging];

fn gates_for_path(path: &str) -> (Vec<GateId>, Option<String>) {
    if exhaustive_authority(path) {
        return (
            Vec::new(),
            Some(format!(
                "{path:?} changes gate or toolchain selection authority"
            )),
        );
    }

    let mut gates = BTreeSet::from([GateId::LocalPaths, GateId::FileShape]);
    let extension = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "rs" => {
            if !known_workspace_rust_source(path) {
                return (
                    Vec::new(),
                    Some(format!(
                        "{path:?} is Rust source outside every compiled workspace root"
                    )),
                );
            }
            gates.extend([
                GateId::Formatters,
                GateId::EnvPolicy,
                GateId::Complexity,
                GateId::MagicNumbers,
                GateId::MarkdownLinks,
            ]);
        }
        "js" | "mjs" | "cjs" => {
            gates.extend([
                GateId::JavascriptUnused,
                GateId::EnvPolicy,
                GateId::EnvNameLedger,
                GateId::Complexity,
                GateId::MagicNumbers,
            ]);
            if frontend_path(path) {
                gates.insert(GateId::CanvasWebTypecheck);
            }
        }
        "html" | "css" => {
            if frontend_path(path) {
                gates.insert(GateId::CanvasWebTypecheck);
            }
        }
        "md" | "txt" | "rst" => {
            gates.extend([GateId::RepoDocs, GateId::Taste, GateId::MarkdownLinks]);
        }
        "py" | "pyi" => {
            gates.extend([
                GateId::PythonTypecheck,
                GateId::EnvPolicy,
                GateId::EnvNameLedger,
                GateId::Complexity,
                GateId::MagicNumbers,
                GateId::Reachability,
                GateId::SymbolReachability,
                GateId::PythonUnused,
                GateId::AssertionFreeTests,
                GateId::PrivateInternals,
            ]);
        }
        "cs" => {
            gates.extend([
                GateId::CsharpUnusedCandidates,
                GateId::Complexity,
                GateId::MagicNumbers,
            ]);
        }
        "sh" | "bash" | "zsh" => {
            gates.extend([
                GateId::EnvPolicy,
                GateId::EnvNameLedger,
                GateId::Complexity,
                GateId::MagicNumbers,
            ]);
        }
        "toml" | "json" | "yaml" | "yml" | "lock" => {
            if Path::new(path).file_name().and_then(|name| name.to_str()) == Some("Cargo.toml")
                && !known_workspace_manifest(path)
            {
                return (
                    Vec::new(),
                    Some(format!(
                        "{path:?} is a package manifest outside every compiled workspace root"
                    )),
                );
            }
            gates.insert(GateId::RepoShape);
            if frontend_path(path) || matches!(path, "package.json" | "package-lock.json") {
                gates.insert(GateId::CanvasWebTypecheck);
            }
        }
        "" if known_extensionless(path) => {
            gates.insert(GateId::RepoShape);
        }
        _ if frontend_path(path) => {
            gates.insert(GateId::CanvasWebTypecheck);
        }
        _ => {
            return (
                Vec::new(),
                Some(format!(
                    "{path:?} has no compiled change-local policy class"
                )),
            );
        }
    }
    (gates.into_iter().collect(), None)
}

fn known_workspace_rust_source(path: &str) -> bool {
    let Some(root) = path.split('/').next() else {
        return false;
    };
    matches!(root, "src" | "tests" | "examples" | "benches") || known_workspace_package_root(root)
}

fn known_workspace_manifest(path: &str) -> bool {
    let mut components = path.split('/');
    let Some(root) = components.next() else {
        return false;
    };
    components.next() == Some("Cargo.toml")
        && components.next().is_none()
        && known_workspace_package_root(root)
}

fn known_workspace_package_root(root: &str) -> bool {
    matches!(
        root,
        "app"
            | "artifact"
            | "atomic"
            | "board"
            | "bus"
            | "canvas"
            | "consent"
            | "dialect"
            | "dist"
            | "fold"
            | "gate"
            | "journal"
            | "lane"
            | "listen"
            | "proof"
            | "sill"
            | "spice"
            | "steer"
            | "tape"
            | "taste"
            | "vendor"
            | "wire"
            | "xtask"
    )
}

fn exhaustive_authority(path: &str) -> bool {
    matches!(
        path,
        ".cargo/config.toml"
            | ".githooks/pre-commit"
            | "spice-rust.toml"
            | "baseline.toml"
            | "rust-toolchain.toml"
            | "Cargo.lock"
            | "Cargo.toml"
            | "gate/src/battery/manifest.toml"
            | "gate/src/change_selection.rs"
            | "gate/src/lib.rs"
            | "gate/src/manifest.rs"
            | "gate/src/test_evidence.rs"
            | "gate/src/workspace_tests.rs"
            | "spice/src/change_gate.rs"
            | "spice/src/gate.rs"
            | "spice/src/gate_command.rs"
            | "spice/src/install/git-hooks/pre-commit"
            | "spice/src/lib.rs"
            | "xtask/src/lib.rs"
            | "xtask/src/main.rs"
    ) || path.starts_with("gate/src/workspace_tests/")
        || path.starts_with("spice/src/gate_command/pre_commit")
}

fn frontend_path(path: &str) -> bool {
    path.starts_with("app/") || path.starts_with("canvas/") || path.starts_with("package")
}

fn known_extensionless(path: &str) -> bool {
    matches!(path, ".gitignore" | ".gitattributes" | "LICENSE")
        || path.starts_with(".githooks/")
        || path.starts_with("spice/src/install/git-hooks/")
}

fn valid_repository_path(path: &str) -> bool {
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
mod tests {
    use super::*;

    fn selection(paths: &[&str]) -> ChangeSelectionReport {
        select_change_gates(
            Digest::compute(wire::Domain::GateTree, b"change selection fixture"),
            &paths
                .iter()
                .map(|path| (*path).to_owned())
                .collect::<Vec<_>>(),
        )
        .expect("select representative changes")
    }

    fn contains(report: &ChangeSelectionReport, gates: &[GateId]) {
        for gate in gates {
            assert!(
                report.gate_ids().contains(gate),
                "{} is absent from {:?}",
                gate,
                report.gate_ids(),
            );
        }
    }

    #[test]
    fn backend_frontend_docs_and_config_select_their_canonical_checks() {
        let backend = selection(&["journal/src/lib.rs"]);
        assert_eq!(backend.mode(), ChangeSelectionMode::Local);
        contains(
            &backend,
            &[
                GateId::Formatters,
                GateId::EnvPolicy,
                GateId::FileShape,
                GateId::Complexity,
                GateId::MagicNumbers,
                GateId::MarkdownLinks,
            ],
        );

        let frontend = selection(&["app/browser/canvas-coordinator.mjs"]);
        assert_eq!(frontend.mode(), ChangeSelectionMode::Local);
        contains(
            &frontend,
            &[
                GateId::CanvasWebTypecheck,
                GateId::JavascriptUnused,
                GateId::EnvPolicy,
                GateId::EnvNameLedger,
            ],
        );

        let documentation = selection(&["DEVELOPMENT.md"]);
        contains(
            &documentation,
            &[GateId::RepoDocs, GateId::Taste, GateId::MarkdownLinks],
        );

        let configuration = selection(&["gate/Cargo.toml"]);
        contains(
            &configuration,
            &[GateId::RepoShape, GateId::LocalPaths, GateId::FileShape],
        );
    }

    #[test]
    fn policy_authority_and_unknown_paths_escalate_to_every_battery_arm() {
        for path in [
            "spice-rust.toml",
            "assets/opaque.bin",
            "tools/outside.rs",
            "tools/component/Cargo.toml",
        ] {
            let report = selection(&[path]);
            assert_eq!(report.mode(), ChangeSelectionMode::Exhaustive);
            assert_eq!(report.gate_ids(), GateId::BATTERY);
            assert!(report.escalation().is_some());
            assert_eq!(
                ChangeSelectionReport::decode(&report.encode().expect("encode selection"))
                    .expect("decode selection"),
                report,
            );
        }
    }

    #[test]
    fn selector_command_and_hook_authorities_escalate_to_every_battery_arm() {
        for path in [
            ".cargo/config.toml",
            ".githooks/pre-commit",
            "Cargo.lock",
            "Cargo.toml",
            "baseline.toml",
            "rust-toolchain.toml",
            "spice-rust.toml",
            "gate/src/battery/manifest.toml",
            "gate/src/change_selection.rs",
            "gate/src/lib.rs",
            "gate/src/manifest.rs",
            "gate/src/test_evidence.rs",
            "gate/src/workspace_tests.rs",
            "gate/src/workspace_tests/selection.rs",
            "gate/src/workspace_tests/source_reach.rs",
            "spice/src/change_gate.rs",
            "spice/src/gate.rs",
            "spice/src/gate_command.rs",
            "spice/src/gate_command/pre_commit.rs",
            "spice/src/install/git-hooks/pre-commit",
            "spice/src/lib.rs",
            "xtask/src/lib.rs",
            "xtask/src/main.rs",
        ] {
            let report = selection(&[path]);
            assert_eq!(
                report.mode(),
                ChangeSelectionMode::Exhaustive,
                "selector authority stayed local: {path}",
            );
            assert_eq!(report.gate_ids(), GateId::BATTERY);
            assert_eq!(report.changed_sources(), [path]);
            assert_eq!(report.edges().len(), GateId::BATTERY.len());
        }
    }

    #[test]
    fn old_and_new_cross_class_paths_retain_both_policy_families() {
        let report = selection(&[
            "README.fixture.md",
            "app/browser/renamed.mjs",
            "app/src/renamed.rs",
            "wire/src/renamed.rs",
        ]);

        assert_eq!(report.mode(), ChangeSelectionMode::Local);
        contains(
            &report,
            &[
                GateId::RepoDocs,
                GateId::Taste,
                GateId::Formatters,
                GateId::CanvasWebTypecheck,
                GateId::JavascriptUnused,
            ],
        );
        for path in report.changed_sources() {
            assert!(report.edges().iter().any(|edge| edge.source() == path));
        }
    }

    #[test]
    fn every_changed_path_and_always_on_arm_retains_provenance() {
        let report = selection(&["README.md", "wire/src/lib.rs"]);
        for gate_id in ALWAYS {
            assert!(report.gate_ids().contains(&gate_id));
        }
        for path in report.changed_sources() {
            assert!(report.edges().iter().any(|edge| edge.source() == path));
        }
    }
}
