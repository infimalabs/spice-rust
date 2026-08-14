//! Fail-closed policy-arm selection for the ordinary staged-change gate.
//!
//! This module decides only which canonical battery arms are owed. It executes
//! none of them and performs no repository walk: the command rim supplies the
//! exact changed paths and the source digest they came from. Changes to an
//! authority that can alter the selector itself, or a path class this build
//! does not understand, refuse selection before any selected work can begin.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use wire::Digest;

use crate::GateId;

/// Current staged-change policy-selection schema.
pub const CHANGE_SELECTION_REPORT_VERSION: u64 = 1;

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
    gate_ids: Vec<GateId>,
    edges: Vec<ChangeSelectionEdge>,
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
        Ok(())
    }
}

/// Select every canonical policy arm owed by exact staged paths.
///
/// This function is pure and intentionally has no repository parameter. A path
/// class this build cannot prove is safe refuses rather than omitting evidence
/// or silently broadening the ordinary gate into a complete verifier.
pub fn select_change_gates(
    source: Digest,
    changed_sources: &[String],
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

    let mut selected = ALWAYS.into_iter().collect::<BTreeSet<_>>();
    let mut edges = Vec::new();
    for path in &changed_sources {
        let gates = gates_for_path(path)?;
        let gates = ALWAYS
            .into_iter()
            .chain(gates)
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
    let report = ChangeSelectionReport {
        version: CHANGE_SELECTION_REPORT_VERSION,
        source,
        mode: ChangeSelectionMode::Local,
        changed_sources,
        gate_ids,
        edges,
    };
    report.validate()?;
    Ok(report)
}

const ALWAYS: [GateId; 3] = [GateId::MergeIntegrity, GateId::PlanPhase, GateId::Staging];
const RUST: [GateId; 5] = [
    GateId::Formatters,
    GateId::EnvPolicy,
    GateId::Complexity,
    GateId::MagicNumbers,
    GateId::MarkdownLinks,
];
const JAVASCRIPT: [GateId; 5] = [
    GateId::JavascriptUnused,
    GateId::EnvPolicy,
    GateId::EnvNameLedger,
    GateId::Complexity,
    GateId::MagicNumbers,
];
const DOCUMENTATION: [GateId; 3] = [GateId::RepoDocs, GateId::Taste, GateId::MarkdownLinks];
const PYTHON: [GateId; 10] = [
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
];
const CSHARP: [GateId; 3] = [
    GateId::CsharpUnusedCandidates,
    GateId::Complexity,
    GateId::MagicNumbers,
];
const SHELL: [GateId; 4] = [
    GateId::EnvPolicy,
    GateId::EnvNameLedger,
    GateId::Complexity,
    GateId::MagicNumbers,
];

fn gates_for_path(path: &str) -> Result<Vec<GateId>, ChangeSelectionError> {
    if selector_authority(path) {
        return Err(ChangeSelectionError::new(format!(
            "{path:?} changes gate or toolchain selection authority"
        )));
    }

    let mut gates = BTreeSet::from([GateId::LocalPaths, GateId::FileShape]);
    let extension = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    gates.extend(class_gates(path, extension.as_str())?);
    Ok(gates.into_iter().collect())
}

fn class_gates(path: &str, extension: &str) -> Result<Vec<GateId>, ChangeSelectionError> {
    match extension {
        "rs" => rust_gates(path),
        "js" | "mjs" | "cjs" => Ok(with_frontend(path, &JAVASCRIPT)),
        "html" | "css" => Ok(with_frontend(path, &[])),
        "md" | "txt" | "rst" => Ok(DOCUMENTATION.to_vec()),
        "py" | "pyi" => Ok(PYTHON.to_vec()),
        "cs" => Ok(CSHARP.to_vec()),
        "sh" | "bash" | "zsh" => Ok(SHELL.to_vec()),
        "toml" | "json" | "yaml" | "yml" | "lock" => configuration_gates(path),
        "" if known_extensionless(path) => Ok(vec![GateId::RepoShape]),
        _ if frontend_path(path) => Ok(vec![GateId::CanvasWebTypecheck]),
        _ => Err(ChangeSelectionError::new(format!(
            "{path:?} has no compiled change-local policy class"
        ))),
    }
}

fn rust_gates(path: &str) -> Result<Vec<GateId>, ChangeSelectionError> {
    if !known_workspace_rust_source(path) {
        return Err(ChangeSelectionError::new(format!(
            "{path:?} is Rust source outside every compiled workspace root"
        )));
    }
    Ok(RUST.to_vec())
}

fn configuration_gates(path: &str) -> Result<Vec<GateId>, ChangeSelectionError> {
    if Path::new(path).file_name().and_then(|name| name.to_str()) == Some("Cargo.toml")
        && !known_workspace_manifest(path)
    {
        return Err(ChangeSelectionError::new(format!(
            "{path:?} is a package manifest outside every compiled workspace root"
        )));
    }
    let mut gates = vec![GateId::RepoShape];
    if frontend_path(path) || matches!(path, "package.json" | "package-lock.json") {
        gates.push(GateId::CanvasWebTypecheck);
    }
    Ok(gates)
}

fn with_frontend(path: &str, base: &[GateId]) -> Vec<GateId> {
    let mut gates = base.to_vec();
    if frontend_path(path) {
        gates.push(GateId::CanvasWebTypecheck);
    }
    gates
}

fn known_workspace_rust_source(path: &str) -> bool {
    let mut components = path.split('/');
    let Some(root) = components.next() else {
        return false;
    };
    if matches!(root, "src" | "tests" | "examples" | "benches") {
        return true;
    }
    if !known_workspace_package_root(root) {
        return false;
    }
    match components.next() {
        Some("src" | "tests" | "examples" | "benches") => true,
        Some("build.rs") => components.next().is_none(),
        Some("build") => root == "fold",
        _ => false,
    }
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

fn selector_authority(path: &str) -> bool {
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
            | "spice/src/cli.rs"
            | "spice/src/cli/contracts.rs"
            | "spice/src/cli/mounts.rs"
            | "spice/src/cli/syntax.rs"
            | "spice/src/gate.rs"
            | "spice/src/gate_command.rs"
            | "spice/src/install/git-hooks/pre-commit"
            | "spice/src/install/git_hooks.rs"
            | "spice/src/lib.rs"
            | "spice/src/main.rs"
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

    fn refusal(path: &str) -> ChangeSelectionError {
        select_change_gates(
            Digest::compute(wire::Domain::GateTree, b"change selection refusal"),
            &[path.to_owned()],
        )
        .expect_err("unclassified or authoritative paths must refuse")
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
    fn classification_gaps_refuse_with_the_exact_path_and_condition() {
        for (path, condition) in [
            (
                "assets/opaque.bin",
                "has no compiled change-local policy class",
            ),
            (
                "tools/outside.rs",
                "is Rust source outside every compiled workspace root",
            ),
            (
                "gate/tools/outside.rs",
                "is Rust source outside every compiled workspace root",
            ),
            (
                "tools/component/Cargo.toml",
                "is a package manifest outside every compiled workspace root",
            ),
        ] {
            assert_eq!(refusal(path).detail(), format!("{path:?} {condition}"));
        }
    }

    #[test]
    fn selector_command_and_hook_authorities_refuse_with_the_exact_path() {
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
            "spice/src/cli.rs",
            "spice/src/cli/contracts.rs",
            "spice/src/cli/mounts.rs",
            "spice/src/cli/syntax.rs",
            "spice/src/gate.rs",
            "spice/src/gate_command.rs",
            "spice/src/gate_command/pre_commit.rs",
            "spice/src/install/git-hooks/pre-commit",
            "spice/src/install/git_hooks.rs",
            "spice/src/lib.rs",
            "spice/src/main.rs",
            "xtask/src/lib.rs",
            "xtask/src/main.rs",
        ] {
            assert_eq!(
                refusal(path).detail(),
                format!("{path:?} changes gate or toolchain selection authority"),
                "selector authority did not refuse precisely: {path}",
            );
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
        assert_eq!(
            report.gate_ids(),
            BTreeSet::from([
                GateId::MergeIntegrity,
                GateId::PlanPhase,
                GateId::Staging,
                GateId::LocalPaths,
                GateId::FileShape,
                GateId::RepoDocs,
                GateId::Taste,
                GateId::MarkdownLinks,
                GateId::CanvasWebTypecheck,
                GateId::JavascriptUnused,
                GateId::EnvPolicy,
                GateId::EnvNameLedger,
                GateId::Complexity,
                GateId::MagicNumbers,
                GateId::Formatters,
            ])
            .into_iter()
            .collect::<Vec<_>>(),
        );
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
    fn local_report_round_trips_canonically_and_cannot_decode_exhaustive_mode() {
        let report = selection(&["README.md", "wire/src/lib.rs"]);
        let encoded = report.encode().expect("encode local selection");
        assert_eq!(
            ChangeSelectionReport::decode(&encoded).expect("decode canonical local selection"),
            report,
        );

        let encoded = String::from_utf8(encoded).expect("selection JSON is UTF-8");
        let exhaustive = encoded.replace("\"mode\":\"local\"", "\"mode\":\"exhaustive\"");
        let error = ChangeSelectionReport::decode(exhaustive.as_bytes())
            .expect_err("ordinary reports cannot retain exhaustive authority");
        assert!(error.detail().contains("invalid change selection"));
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
