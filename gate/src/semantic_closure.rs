//! Canonical per-arm source closures for reusable verifier receipts.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tree_sitter::{Language, Node, Parser};
use wire::{Digest, Domain};

use crate::change_selection::{
    current_source_gates_for_path, current_source_selector_authority, known_workspace_rust_source,
    valid_repository_path,
};
use crate::{GateId, tracked_path_selector_matches, validate_tracked_path_selector};

mod compiler_inputs;
mod dependencies;

use compiler_inputs::compiler_dynamic_boundaries;
use dependencies::dependency_edges;

/// Current source-closure schema.
pub const GATE_SEMANTIC_CLOSURE_VERSION: u64 = 1;

/// How one selected path was understood while deriving arm closures.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateSourceClass {
    /// A path classified by the compiled current-source selector.
    Known,
    /// A path whose bytes can alter selection or execution authority.
    SelectorAuthority,
    /// An authored generated path excluded from source-analysis arms.
    Generated,
    /// A tracked path outside the compiled selector vocabulary.
    Unknown,
    /// A supported language whose syntax tree was incomplete.
    ParserGap,
    /// A source dependency was computed dynamically or could not be resolved.
    DynamicEdge,
}

/// How a consuming arm interprets one statically resolved dependency.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateDependencyMode {
    /// The dependency is parsed as source, so trivia does not alter semantics.
    Syntax,
    /// The dependency's bytes are embedded or otherwise consumed verbatim.
    Exact,
}

/// One canonical repository-local dependency edge.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GateSourceDependency {
    path: String,
    mode: GateDependencyMode,
}

impl GateSourceDependency {
    /// Canonical repository-relative target path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Whether the target contributes syntax or exact bytes.
    #[must_use]
    pub const fn mode(&self) -> GateDependencyMode {
        self.mode
    }
}

/// One path's exact and syntax-normalized identities.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GateClosureSource {
    path: String,
    class: GateSourceClass,
    changed: bool,
    content: Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    syntax: Option<Digest>,
    dependencies: Vec<GateSourceDependency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    diagnostic: Option<String>,
    gates: Vec<GateId>,
}

impl GateClosureSource {
    /// Normalized repository-relative path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Compiled classification used by the closure.
    #[must_use]
    pub const fn class(&self) -> GateSourceClass {
        self.class
    }

    /// Whether the selector reported this path as changed.
    #[must_use]
    pub const fn changed(&self) -> bool {
        self.changed
    }

    /// Exact byte identity.
    #[must_use]
    pub const fn content(&self) -> &Digest {
        &self.content
    }

    /// Trivia-insensitive syntax identity when a complete vendored grammar parsed the path.
    #[must_use]
    pub const fn syntax(&self) -> Option<&Digest> {
        self.syntax.as_ref()
    }

    /// Statically resolved repository-local dependencies.
    #[must_use]
    pub fn dependencies(&self) -> &[GateSourceDependency] {
        &self.dependencies
    }

    /// Exact parser or dependency gap which prevents affected-arm reuse.
    #[must_use]
    pub fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }

    /// Arms whose source closure contains this path.
    #[must_use]
    pub fn gates(&self) -> &[GateId] {
        &self.gates
    }
}

/// Deterministic selected-tree key for one canonical battery arm.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GateArmClosure {
    gate_id: GateId,
    key: Digest,
}

impl GateArmClosure {
    /// Canonical arm identity.
    #[must_use]
    pub const fn gate_id(&self) -> GateId {
        self.gate_id
    }

    /// Identity of the arm's path, syntax, and dependency closure.
    #[must_use]
    pub const fn key(&self) -> &Digest {
        &self.key
    }
}

/// Canonical per-arm closure of one immutable selected tree.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GateSemanticClosure {
    version: u64,
    source: Digest,
    changes: Vec<String>,
    sources: Vec<GateClosureSource>,
    arms: Vec<GateArmClosure>,
}

impl GateSemanticClosure {
    /// Read one immutable selected tree and derive every canonical arm closure.
    ///
    /// Generated selectors use the repository's tracked-path grammar. Unknown
    /// paths are retained as explicit evidence: they do not contaminate every
    /// unchanged arm, but any movement in that opaque inventory refuses reuse.
    pub fn capture(
        source: Digest,
        root: &Path,
        paths: &[String],
        changes: &[String],
        generated: &[String],
    ) -> Result<Self, String> {
        if !paths.windows(2).all(|pair| pair[0] < pair[1])
            || paths.iter().any(|path| !valid_repository_path(path))
        {
            return Err("semantic source closure requires sorted unique paths".to_owned());
        }
        for selector in generated {
            validate_tracked_path_selector(selector).map_err(|error| {
                format!("invalid generated-path selector {selector:?}: {error}")
            })?;
        }
        let mut changes = changes.to_vec();
        changes.sort();
        changes.dedup();
        if changes.iter().any(|path| !valid_repository_path(path)) {
            return Err("semantic source closure requires normalized changed paths".to_owned());
        }
        let path_inventory = paths.iter().cloned().collect::<BTreeSet<_>>();
        let compiler_boundaries = compiler_dynamic_boundaries(root, &path_inventory)?;
        let sources = paths
            .iter()
            .map(|path| {
                capture_source(
                    root,
                    path,
                    &changes,
                    generated,
                    &path_inventory,
                    &compiler_boundaries,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let arms = GateId::BATTERY
            .into_iter()
            .map(|gate_id| GateArmClosure {
                gate_id,
                key: arm_key(gate_id, &changes, &sources),
            })
            .collect();
        let closure = Self {
            version: GATE_SEMANTIC_CLOSURE_VERSION,
            source,
            changes,
            sources,
            arms,
        };
        closure.validate()?;
        Ok(closure)
    }

    /// Exact selected-tree identity from the capture that produced this closure.
    #[must_use]
    pub const fn source(&self) -> &Digest {
        &self.source
    }

    /// Canonical changed-path inventory captured at the selector rim.
    #[must_use]
    pub fn changes(&self) -> &[String] {
        &self.changes
    }

    /// Canonically ordered path evidence.
    #[must_use]
    pub fn sources(&self) -> &[GateClosureSource] {
        &self.sources
    }

    /// Canonically ordered arm keys.
    #[must_use]
    pub fn arms(&self) -> &[GateArmClosure] {
        &self.arms
    }

    /// Key for one canonical arm.
    #[must_use]
    pub fn key(&self, gate_id: GateId) -> Option<&Digest> {
        self.arms
            .binary_search_by_key(&gate_id, GateArmClosure::gate_id)
            .ok()
            .map(|index| self.arms[index].key())
    }

    /// Compare two closures without treating whole-tree identity as authority.
    pub fn compare(&self, current: &Self) -> Result<GateClosureComparison, String> {
        self.validate()?;
        current.validate()?;
        let (refused, diagnostics) = comparison_refusals(&self.sources, &current.sources);
        let reusable = GateId::BATTERY
            .into_iter()
            .filter(|gate_id| {
                !always_on(*gate_id)
                    && !refused.contains(gate_id)
                    && self.key(*gate_id) == current.key(*gate_id)
            })
            .collect::<Vec<_>>();
        let invalidated = GateId::BATTERY
            .into_iter()
            .filter(|gate_id| !reusable.contains(gate_id))
            .collect::<Vec<_>>();
        let changed_sources = changed_sources(&self.sources, &current.sources);
        Ok(GateClosureComparison {
            reusable,
            invalidated,
            changed_sources,
            diagnostics,
        })
    }

    /// Canonical JSON bytes.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        wire::encode(self)
    }

    /// Decode only a canonical and internally recomputable closure.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let closure: Self = wire::decode(bytes)
            .map_err(|error| format!("decode semantic source closure: {error}"))?;
        closure.validate()?;
        let canonical = closure
            .encode()
            .map_err(|error| format!("encode semantic source closure: {error}"))?;
        if canonical != bytes {
            return Err("semantic source closure bytes are not canonical".to_owned());
        }
        Ok(closure)
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != GATE_SEMANTIC_CLOSURE_VERSION {
            return Err(format!(
                "unsupported semantic source closure version {}; expected {GATE_SEMANTIC_CLOSURE_VERSION}",
                self.version,
            ));
        }
        if !self.changes.windows(2).all(|pair| pair[0] < pair[1])
            || self.changes.iter().any(|path| !valid_repository_path(path))
        {
            return Err("semantic changed paths must be sorted and unique".to_owned());
        }
        if !self
            .sources
            .windows(2)
            .all(|pair| pair[0].path < pair[1].path)
        {
            return Err("semantic source paths must be sorted and unique".to_owned());
        }
        for source in &self.sources {
            if !valid_repository_path(&source.path)
                || source.gates.windows(2).any(|pair| pair[0] >= pair[1])
                || source
                    .dependencies
                    .windows(2)
                    .any(|pair| pair[0] >= pair[1])
                || source.changed != self.changes.binary_search(&source.path).is_ok()
                || matches!(
                    source.class,
                    GateSourceClass::ParserGap | GateSourceClass::DynamicEdge
                ) != source.diagnostic.is_some()
            {
                return Err("semantic source carries an invalid path or gate inventory".to_owned());
            }
        }
        let source_paths = self
            .sources
            .iter()
            .map(|source| source.path.as_str())
            .collect::<BTreeSet<_>>();
        for source in &self.sources {
            if source
                .dependencies
                .iter()
                .any(|dependency| !source_paths.contains(dependency.path.as_str()))
            {
                return Err(format!(
                    "semantic source {:?} names a dependency outside the selected tree",
                    source.path,
                ));
            }
        }
        let gates = self
            .arms
            .iter()
            .map(GateArmClosure::gate_id)
            .collect::<Vec<_>>();
        if gates != GateId::BATTERY {
            return Err("semantic source closure must cover every canonical arm once".to_owned());
        }
        for arm in &self.arms {
            if arm.key != arm_key(arm.gate_id, &self.changes, &self.sources) {
                return Err(format!(
                    "semantic source key for {} is inconsistent",
                    arm.gate_id.as_str(),
                ));
            }
        }
        Ok(())
    }
}

fn capture_source(
    root: &Path,
    path: &str,
    changes: &[String],
    generated: &[String],
    paths: &BTreeSet<String>,
    compiler_boundaries: &BTreeMap<String, String>,
) -> Result<GateClosureSource, String> {
    let bytes = fs::read(root.join(path))
        .map_err(|error| format!("read semantic source {path:?}: {error}"))?;
    let generated = generated.iter().try_fold(false, |matched, selector| {
        tracked_path_selector_matches(selector, path).map(|current| matched || current)
    })?;
    let (mut class, mut gates) = classify_source(path, generated);
    let analysis = syntax_closure(path, &bytes, paths)?;
    let diagnostic = source_diagnostic(
        path,
        compiler_boundaries.get(path),
        &analysis,
        &mut class,
        &mut gates,
    );
    gates.sort();
    gates.dedup();
    Ok(GateClosureSource {
        path: path.to_owned(),
        class,
        changed: changes
            .binary_search_by(|candidate| candidate.as_str().cmp(path))
            .is_ok(),
        content: Digest::compute(Domain::GateTree, &bytes),
        syntax: analysis.syntax,
        dependencies: analysis.dependencies,
        diagnostic,
        gates,
    })
}

fn classify_source(path: &str, generated: bool) -> (GateSourceClass, Vec<GateId>) {
    if generated {
        return (
            GateSourceClass::Generated,
            vec![
                GateId::MergeIntegrity,
                GateId::PlanPhase,
                GateId::Staging,
                GateId::LocalPaths,
            ],
        );
    }
    if current_source_selector_authority(path) {
        return (GateSourceClass::SelectorAuthority, GateId::BATTERY.to_vec());
    }
    let Ok(mut gates) = current_source_gates_for_path(path) else {
        return (GateSourceClass::Unknown, Vec::new());
    };
    if let Some(implementation) = gate_implementation_gates(path) {
        gates.extend(implementation);
    }
    if rust_compiler_input(path) {
        gates.push(GateId::RustTypecheck);
    }
    if canvas_compiler_input(path) {
        gates.push(GateId::CanvasWebTypecheck);
    }
    if path == "pyproject.toml" {
        gates.push(GateId::PythonTypecheck);
    }
    gates.extend([GateId::MergeIntegrity, GateId::PlanPhase, GateId::Staging]);
    (GateSourceClass::Known, gates)
}

fn source_diagnostic(
    path: &str,
    compiler_boundary: Option<&String>,
    analysis: &SyntaxClosure,
    class: &mut GateSourceClass,
    gates: &mut Vec<GateId>,
) -> Option<String> {
    if let Some(detail) = compiler_boundary {
        *class = GateSourceClass::DynamicEdge;
        gates.extend([GateId::RustTypecheck, GateId::CanvasWebTypecheck]);
        return Some(detail.clone());
    }
    if *class == GateSourceClass::Known
        && syntax_language(path).is_some()
        && analysis.syntax.is_none()
    {
        *class = GateSourceClass::ParserGap;
        return Some(format!("Tree-sitter could not completely parse {path:?}"));
    }
    if *class != GateSourceClass::Known {
        return None;
    }
    analysis.gap.clone().inspect(|_| {
        *class = GateSourceClass::DynamicEdge;
    })
}

/// Difference between retained and current per-arm closures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GateClosureComparison {
    reusable: Vec<GateId>,
    invalidated: Vec<GateId>,
    changed_sources: Vec<String>,
    diagnostics: Vec<String>,
}

impl GateClosureComparison {
    /// Arms whose semantic source closure is unchanged.
    #[must_use]
    pub fn reusable(&self) -> &[GateId] {
        &self.reusable
    }

    /// Arms which must execute or refuse through their existing path.
    #[must_use]
    pub fn invalidated(&self) -> &[GateId] {
        &self.invalidated
    }

    /// Added, removed, moved, or semantically changed paths.
    #[must_use]
    pub fn changed_sources(&self) -> &[String] {
        &self.changed_sources
    }

    /// Explicit reasons affected arms could not reuse retained evidence.
    #[must_use]
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }
}

fn arm_key(gate_id: GateId, changes: &[String], sources: &[GateClosureSource]) -> Digest {
    let mut bytes = Vec::new();
    frame(&mut bytes, &GATE_SEMANTIC_CLOSURE_VERSION.to_be_bytes());
    frame(&mut bytes, gate_id.as_str().as_bytes());
    if selected_changes_gate(gate_id) {
        for change in changes {
            frame(&mut bytes, change.as_bytes());
        }
    }
    let by_path = sources
        .iter()
        .map(|source| (source.path.as_str(), source))
        .collect::<BTreeMap<_, _>>();
    let mut closure = sources
        .iter()
        .filter(|source| source.gates.contains(&gate_id))
        .map(|source| (source.path.clone(), false))
        .collect::<BTreeMap<_, _>>();
    if dependency_sensitive(gate_id) {
        let mut pending = closure.keys().cloned().collect::<Vec<_>>();
        let mut expanded = BTreeSet::new();
        while let Some(path) = pending.pop() {
            if !expanded.insert(path.clone()) {
                continue;
            }
            let Some(source) = by_path.get(path.as_str()) else {
                continue;
            };
            for dependency in &source.dependencies {
                let exact = dependency.mode == GateDependencyMode::Exact;
                closure
                    .entry(dependency.path.clone())
                    .and_modify(|current| *current |= exact)
                    .or_insert(exact);
                if dependency.mode == GateDependencyMode::Syntax {
                    pending.push(dependency.path.clone());
                }
            }
        }
    }
    for (path, exact) in closure {
        let source = by_path
            .get(path.as_str())
            .expect("semantic dependency closure names a selected source");
        frame(&mut bytes, source.path.as_bytes());
        frame(&mut bytes, source_class_name(source.class).as_bytes());
        let digest = if syntax_sensitive(gate_id) && !exact {
            source.syntax.as_ref().unwrap_or(&source.content)
        } else {
            &source.content
        };
        frame(&mut bytes, digest.as_str().as_bytes());
    }
    Digest::compute(Domain::GateReceipt, &bytes)
}

const fn source_class_name(class: GateSourceClass) -> &'static str {
    match class {
        GateSourceClass::Known => "known",
        GateSourceClass::SelectorAuthority => "selector-authority",
        GateSourceClass::Generated => "generated",
        GateSourceClass::Unknown => "unknown",
        GateSourceClass::ParserGap => "parser-gap",
        GateSourceClass::DynamicEdge => "dynamic-edge",
    }
}

fn selected_changes_gate(gate_id: GateId) -> bool {
    matches!(
        gate_id,
        GateId::LocalPaths | GateId::Taste | GateId::EnvPolicy
    )
}

fn dependency_sensitive(gate_id: GateId) -> bool {
    matches!(
        gate_id,
        GateId::RustTypecheck
            | GateId::CanvasWebTypecheck
            | GateId::JavascriptUnused
            | GateId::CsharpUnusedCandidates
            | GateId::PythonTypecheck
            | GateId::Reachability
            | GateId::SymbolReachability
            | GateId::PythonUnused
            | GateId::AssertionFreeTests
            | GateId::PrivateInternals
    )
}

fn syntax_sensitive(gate_id: GateId) -> bool {
    matches!(
        gate_id,
        GateId::RustTypecheck
            | GateId::CanvasWebTypecheck
            | GateId::JavascriptUnused
            | GateId::CsharpUnusedCandidates
            | GateId::PythonTypecheck
            | GateId::EnvPolicy
            | GateId::EnvNameLedger
            | GateId::Complexity
            | GateId::MagicNumbers
            | GateId::Reachability
            | GateId::SymbolReachability
            | GateId::PythonUnused
            | GateId::AssertionFreeTests
            | GateId::PrivateInternals
    )
}

fn rust_compiler_input(path: &str) -> bool {
    known_workspace_rust_source(path)
        || Path::new(path)
            .file_name()
            .is_some_and(|name| name == "Cargo.toml")
        || matches!(
            path,
            "Cargo.lock" | "rust-toolchain.toml" | ".cargo/config.toml"
        )
}

fn gate_implementation_gates(path: &str) -> Option<Vec<GateId>> {
    if !path.starts_with("gate/src/") || !path.ends_with(".rs") || test_only_gate_source(path) {
        return None;
    }
    if path == "gate/src/toolchain.rs" || path.starts_with("gate/src/toolchain/") {
        return Some(
            crate::toolchain::TOOLCHAIN_ARMS
                .iter()
                .map(|declaration| declaration.id())
                .collect(),
        );
    }
    if path == "gate/src/metrics.rs" || path.starts_with("gate/src/metrics/") {
        return Some(
            crate::metrics::STRUCTURAL_ARMS
                .iter()
                .map(|declaration| declaration.id())
                .collect(),
        );
    }
    if path == "gate/src/hygiene.rs" || path.starts_with("gate/src/hygiene/") {
        return Some(
            crate::hygiene::HYGIENE_ARMS
                .iter()
                .map(|declaration| declaration.id())
                .collect(),
        );
    }
    if path == "gate/src/core.rs" || path.starts_with("gate/src/core/") {
        return Some(
            crate::core::core_declarations()
                .iter()
                .map(|declaration| declaration.id())
                .collect(),
        );
    }
    Some(GateId::BATTERY.to_vec())
}

fn test_only_gate_source(path: &str) -> bool {
    path.contains("/tests/") || path.ends_with("/tests.rs") || path.ends_with("_tests.rs")
}

fn canvas_compiler_input(path: &str) -> bool {
    let path = Path::new(path);
    let components = path.components().collect::<Vec<_>>();
    path.file_name().is_some_and(|name| name == "Cargo.toml")
        || matches!(
            path.to_str(),
            Some("Cargo.lock" | "rust-toolchain.toml" | ".cargo/config.toml")
        )
        || path.extension().is_some_and(|extension| extension == "rs")
            && (components
                .first()
                .is_some_and(|root| root.as_os_str() == "src")
                || components
                    .get(1)
                    .is_some_and(|directory| directory.as_os_str() == "src"))
}

fn always_on(gate_id: GateId) -> bool {
    matches!(
        gate_id,
        GateId::MergeIntegrity | GateId::PlanPhase | GateId::Staging
    )
}

fn comparison_refusals(
    retained: &[GateClosureSource],
    current: &[GateClosureSource],
) -> (BTreeSet<GateId>, Vec<String>) {
    let changed = changed_sources(retained, current)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut refused = BTreeSet::new();
    let mut diagnostics = BTreeSet::new();
    for source in retained.iter().chain(current) {
        match source.class {
            GateSourceClass::ParserGap => {
                refused.extend(
                    source
                        .gates
                        .iter()
                        .copied()
                        .filter(|gate_id| syntax_sensitive(*gate_id)),
                );
                diagnostics.insert(source.diagnostic.clone().unwrap_or_else(|| {
                    format!(
                        "semantic syntax closure is incomplete for {:?}",
                        source.path
                    )
                }));
            }
            GateSourceClass::DynamicEdge => {
                refused.extend(
                    source
                        .gates
                        .iter()
                        .copied()
                        .filter(|gate_id| dependency_sensitive(*gate_id)),
                );
                diagnostics.insert(source.diagnostic.clone().unwrap_or_else(|| {
                    format!(
                        "semantic dependency closure is incomplete for {:?}",
                        source.path
                    )
                }));
            }
            GateSourceClass::Unknown if changed.contains(&source.path) => {
                refused.extend(GateId::BATTERY);
                diagnostics.insert(format!(
                    "unknown selected source {:?} moved; no arm closure is provable",
                    source.path,
                ));
            }
            GateSourceClass::SelectorAuthority if changed.contains(&source.path) => {
                refused.extend(GateId::BATTERY);
                diagnostics.insert(format!(
                    "selector authority {:?} moved; every arm must execute",
                    source.path,
                ));
            }
            GateSourceClass::Known
            | GateSourceClass::Generated
            | GateSourceClass::Unknown
            | GateSourceClass::SelectorAuthority => {}
        }
    }
    (refused, diagnostics.into_iter().collect())
}

fn changed_sources(retained: &[GateClosureSource], current: &[GateClosureSource]) -> Vec<String> {
    let retained = retained
        .iter()
        .map(|source| (source.path.clone(), source.content.clone()))
        .collect::<BTreeMap<_, _>>();
    let current = current
        .iter()
        .map(|source| (source.path.clone(), source.content.clone()))
        .collect::<BTreeMap<_, _>>();
    changed_map(&retained, &current)
}

fn changed_map<T: Eq>(
    retained: &BTreeMap<String, T>,
    current: &BTreeMap<String, T>,
) -> Vec<String> {
    retained
        .keys()
        .chain(current.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| retained.get(path) != current.get(path))
        .collect()
}

struct SyntaxClosure {
    syntax: Option<Digest>,
    dependencies: Vec<GateSourceDependency>,
    gap: Option<String>,
}

fn syntax_closure(
    path: &str,
    bytes: &[u8],
    paths: &BTreeSet<String>,
) -> Result<SyntaxClosure, String> {
    let Some(language) = syntax_language(path) else {
        return Ok(SyntaxClosure {
            syntax: None,
            dependencies: Vec::new(),
            gap: None,
        });
    };
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .map_err(|error| format!("load semantic grammar for {path:?}: {error}"))?;
    let Some(tree) = parser.parse(bytes, None) else {
        return Err(format!("semantic parser returned no tree for {path:?}"));
    };
    let root = tree.root_node();
    if root.has_error() {
        return Ok(SyntaxClosure {
            syntax: None,
            dependencies: Vec::new(),
            gap: None,
        });
    }
    let mut semantic = Vec::new();
    syntax_node(root, bytes, &mut semantic);
    let (mut dependencies, gap) = dependency_edges(path, root, bytes, paths);
    dependencies.sort();
    dependencies.dedup();
    Ok(SyntaxClosure {
        syntax: Some(Digest::compute(Domain::GateTree, &semantic)),
        dependencies,
        gap,
    })
}

fn syntax_node(node: Node<'_>, bytes: &[u8], into: &mut Vec<u8>) {
    if comment_node(node.kind()) {
        return;
    }
    frame(into, node.kind().as_bytes());
    if node.child_count() == 0 {
        if let Ok(text) = node.utf8_text(bytes) {
            frame(into, text.as_bytes());
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        syntax_node(child, bytes, into);
    }
}

fn syntax_language(path: &str) -> Option<Language> {
    match Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())?
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => Some(Language::new(tree_sitter_rust::LANGUAGE)),
        "py" | "pyi" => Some(Language::new(tree_sitter_python::LANGUAGE)),
        "js" | "mjs" | "cjs" | "jsx" => Some(Language::new(tree_sitter_javascript::LANGUAGE)),
        "cs" => Some(Language::new(tree_sitter_c_sharp::LANGUAGE)),
        "lua" => Some(Language::new(tree_sitter_lua::LANGUAGE)),
        _ => None,
    }
}

fn comment_node(kind: &str) -> bool {
    kind == "comment" || kind.ends_with("_comment")
}

fn frame(into: &mut Vec<u8>, bytes: &[u8]) {
    into.extend_from_slice(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
    into.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests;
