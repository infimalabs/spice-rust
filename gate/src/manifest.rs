//! One current gate-manifest grammar and the one-shot predecessor translator.
//!
//! Current manifest bytes carry stable compiled gate identities and inert
//! metadata only. The predecessor grammar is isolated in [`migrate_predecessor`]
//! and is never consulted by ordinary manifest admission.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use serde::de::{Deserializer, Error as _};
use serde::{Deserialize, Serialize};
use toml::{Table, Value};

/// The only current gate-manifest schema.
pub const GATE_MANIFEST_VERSION: u64 = 1;
/// Release that owned the predecessor configuration grammar.
pub const PREDECESSOR_GATE_RELEASE: &str = "0.31.0";

/// One stable compiled gate identity.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum GateId {
    #[serde(rename = "merge-integrity")]
    MergeIntegrity,
    #[serde(rename = "plan-phase")]
    PlanPhase,
    #[serde(rename = "repo-shape")]
    RepoShape,
    #[serde(rename = "staging")]
    Staging,
    #[serde(rename = "repo-docs")]
    RepoDocs,
    #[serde(rename = "config-key-validity")]
    ConfigKeyValidity,
    #[serde(rename = "config-false-disable")]
    ConfigFalseDisable,
    #[serde(rename = "config-tracked-trust")]
    ConfigTrackedTrust,
    #[serde(rename = "formatters")]
    Formatters,
    #[serde(rename = "local-paths")]
    LocalPaths,
    #[serde(rename = "taste")]
    Taste,
    #[serde(rename = "rust-typecheck")]
    RustTypecheck,
    #[serde(rename = "canvas-web-typecheck")]
    CanvasWebTypecheck,
    #[serde(rename = "javascript-unused")]
    JavascriptUnused,
    #[serde(rename = "csharp-unused-candidates")]
    CsharpUnusedCandidates,
    #[serde(rename = "python-typecheck")]
    PythonTypecheck,
    #[serde(rename = "env-policy")]
    EnvPolicy,
    #[serde(rename = "env-name-ledger")]
    EnvNameLedger,
    #[serde(rename = "file-shape")]
    FileShape,
    #[serde(rename = "complexity")]
    Complexity,
    #[serde(rename = "magic-numbers")]
    MagicNumbers,
    #[serde(rename = "markdown-links")]
    MarkdownLinks,
    #[serde(rename = "reachability")]
    Reachability,
    #[serde(rename = "symbol-reachability")]
    SymbolReachability,
    #[serde(rename = "python-unused")]
    PythonUnused,
    #[serde(rename = "assertion-free-tests")]
    AssertionFreeTests,
    #[serde(rename = "private-internals")]
    PrivateInternals,
}

impl GateId {
    /// Every compiled identity in predecessor battery order.
    pub const ALL: [Self; 27] = [
        Self::MergeIntegrity,
        Self::PlanPhase,
        Self::RepoShape,
        Self::Staging,
        Self::RepoDocs,
        Self::ConfigKeyValidity,
        Self::ConfigFalseDisable,
        Self::ConfigTrackedTrust,
        Self::Formatters,
        Self::LocalPaths,
        Self::Taste,
        Self::RustTypecheck,
        Self::CanvasWebTypecheck,
        Self::JavascriptUnused,
        Self::CsharpUnusedCandidates,
        Self::PythonTypecheck,
        Self::EnvPolicy,
        Self::EnvNameLedger,
        Self::FileShape,
        Self::Complexity,
        Self::MagicNumbers,
        Self::MarkdownLinks,
        Self::Reachability,
        Self::SymbolReachability,
        Self::PythonUnused,
        Self::AssertionFreeTests,
        Self::PrivateInternals,
    ];

    /// Exact current spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MergeIntegrity => "merge-integrity",
            Self::PlanPhase => "plan-phase",
            Self::RepoShape => "repo-shape",
            Self::Staging => "staging",
            Self::RepoDocs => "repo-docs",
            Self::ConfigKeyValidity => "config-key-validity",
            Self::ConfigFalseDisable => "config-false-disable",
            Self::ConfigTrackedTrust => "config-tracked-trust",
            Self::Formatters => "formatters",
            Self::LocalPaths => "local-paths",
            Self::Taste => "taste",
            Self::RustTypecheck => "rust-typecheck",
            Self::CanvasWebTypecheck => "canvas-web-typecheck",
            Self::JavascriptUnused => "javascript-unused",
            Self::CsharpUnusedCandidates => "csharp-unused-candidates",
            Self::PythonTypecheck => "python-typecheck",
            Self::EnvPolicy => "env-policy",
            Self::EnvNameLedger => "env-name-ledger",
            Self::FileShape => "file-shape",
            Self::Complexity => "complexity",
            Self::MagicNumbers => "magic-numbers",
            Self::MarkdownLinks => "markdown-links",
            Self::Reachability => "reachability",
            Self::SymbolReachability => "symbol-reachability",
            Self::PythonUnused => "python-unused",
            Self::AssertionFreeTests => "assertion-free-tests",
            Self::PrivateInternals => "private-internals",
        }
    }

    fn exact(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|id| id.as_str() == name)
    }

    fn predecessor(name: &str) -> Option<Self> {
        let normalized = name.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        Self::ALL
            .into_iter()
            .find(|id| id.predecessor_name() == normalized)
    }

    /// Exact release 0.31.0 standard-arm spelling.
    ///
    /// Only meaningful where [`Self::predecessor_declared`] holds. An identity
    /// this workspace grew past the release still answers with its current
    /// spelling, because normalizing a predecessor document has to reject that
    /// name by a rule rather than by failing to recognize it.
    #[must_use]
    pub const fn predecessor_name(self) -> &'static str {
        match self {
            Self::CanvasWebTypecheck => concat!("ser", "ve-web-typecheck"),
            _ => self.as_str(),
        }
    }

    /// Whether release 0.31.0's standard battery declared this identity at all.
    ///
    /// The battery kept every arm the release ran, so this answers true for
    /// every identity except the ones this workspace grew past it. Nothing
    /// downstream may attribute a predecessor arm to an identity that answers
    /// false, which is what keeps a migration refusal from citing a step that
    /// never existed.
    #[must_use]
    pub const fn predecessor_declared(self) -> bool {
        !matches!(self, Self::CsharpUnusedCandidates | Self::RustTypecheck)
    }

    #[cfg(test)]
    const fn predecessor_str(self) -> &'static str {
        self.predecessor_name()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PredecessorStandardArm {
    /// Release 0.31.0 answered for this identity with an in-process callable.
    InternalBuiltin,
    /// No release 0.31.0 pre-commit step answered for this identity at all.
    AbsentFromBattery,
}

// Release 0.31.0's packaged `_builtin_pre_commit_steps` battery declares one
// in-process `PreCommitStep` callable for each identity in this exact order,
// except where the pairing says the battery declared nothing. A gate this
// workspace grew past the predecessor has no argv to normalize away from, and
// naming that outright keeps the refusal from inventing a step that never ran.
const PREDECESSOR_STANDARD_ARMS: [(GateId, PredecessorStandardArm); 27] = [
    (
        GateId::MergeIntegrity,
        PredecessorStandardArm::InternalBuiltin,
    ),
    (GateId::PlanPhase, PredecessorStandardArm::InternalBuiltin),
    (GateId::RepoShape, PredecessorStandardArm::InternalBuiltin),
    (GateId::Staging, PredecessorStandardArm::InternalBuiltin),
    (GateId::RepoDocs, PredecessorStandardArm::InternalBuiltin),
    (
        GateId::ConfigKeyValidity,
        PredecessorStandardArm::InternalBuiltin,
    ),
    (
        GateId::ConfigFalseDisable,
        PredecessorStandardArm::InternalBuiltin,
    ),
    (
        GateId::ConfigTrackedTrust,
        PredecessorStandardArm::InternalBuiltin,
    ),
    (GateId::Formatters, PredecessorStandardArm::InternalBuiltin),
    (GateId::LocalPaths, PredecessorStandardArm::InternalBuiltin),
    (GateId::Taste, PredecessorStandardArm::InternalBuiltin),
    (
        GateId::RustTypecheck,
        PredecessorStandardArm::AbsentFromBattery,
    ),
    (
        GateId::CanvasWebTypecheck,
        PredecessorStandardArm::InternalBuiltin,
    ),
    (
        GateId::JavascriptUnused,
        PredecessorStandardArm::InternalBuiltin,
    ),
    (
        GateId::CsharpUnusedCandidates,
        PredecessorStandardArm::AbsentFromBattery,
    ),
    (
        GateId::PythonTypecheck,
        PredecessorStandardArm::InternalBuiltin,
    ),
    (GateId::EnvPolicy, PredecessorStandardArm::InternalBuiltin),
    (
        GateId::EnvNameLedger,
        PredecessorStandardArm::InternalBuiltin,
    ),
    (GateId::FileShape, PredecessorStandardArm::InternalBuiltin),
    (GateId::Complexity, PredecessorStandardArm::InternalBuiltin),
    (
        GateId::MagicNumbers,
        PredecessorStandardArm::InternalBuiltin,
    ),
    (
        GateId::MarkdownLinks,
        PredecessorStandardArm::InternalBuiltin,
    ),
    (
        GateId::Reachability,
        PredecessorStandardArm::InternalBuiltin,
    ),
    (
        GateId::SymbolReachability,
        PredecessorStandardArm::InternalBuiltin,
    ),
    (
        GateId::PythonUnused,
        PredecessorStandardArm::InternalBuiltin,
    ),
    (
        GateId::AssertionFreeTests,
        PredecessorStandardArm::InternalBuiltin,
    ),
    (
        GateId::PrivateInternals,
        PredecessorStandardArm::InternalBuiltin,
    ),
];

fn predecessor_standard_arm(id: GateId) -> PredecessorStandardArm {
    PREDECESSOR_STANDARD_ARMS
        .into_iter()
        .find_map(|(candidate, arm)| (candidate == id).then_some(arm))
        .expect("every compiled gate has one predecessor standard arm")
}

impl fmt::Display for GateId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for GateId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::exact(&value).ok_or_else(|| {
            D::Error::custom(format!(
                "unknown compiled gate ID {value:?}; expected one exact current ID"
            ))
        })
    }
}

/// Source class recorded by one migrated or compiled declaration field.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OriginLayer {
    Compiled,
    System,
    Repository,
    Worktree,
}

impl fmt::Display for OriginLayer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Compiled => "compiled",
            Self::System => "system",
            Self::Repository => "repository",
            Self::Worktree => "worktree",
        })
    }
}

/// Exact origin of one non-executable manifest field.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    release: String,
    layer: OriginLayer,
    document: String,
    field: Vec<String>,
}

impl Provenance {
    /// Release that understood the source grammar.
    #[must_use]
    pub fn release(&self) -> &str {
        &self.release
    }

    /// Source layer that won resolution.
    #[must_use]
    pub const fn layer(&self) -> OriginLayer {
        self.layer
    }

    /// Exact source document named by migration.
    #[must_use]
    pub fn document(&self) -> &str {
        &self.document
    }

    /// Original, structurally segmented source field.
    #[must_use]
    pub fn field(&self) -> &[String] {
        &self.field
    }

    fn compiled(id: GateId, leaf: &str) -> Self {
        Self {
            release: "current".to_owned(),
            layer: OriginLayer::Compiled,
            document: "<compiled-gate-battery>".to_owned(),
            field: vec![id.as_str().to_owned(), leaf.to_owned()],
        }
    }

    fn validate(&self, field: &str) -> Result<(), GateManifestError> {
        if self.release.trim().is_empty() {
            return Err(GateManifestError::new(format!(
                "{field}.release must be non-empty"
            )));
        }
        if self.document.trim().is_empty() {
            return Err(GateManifestError::new(format!(
                "{field}.document must be non-empty"
            )));
        }
        if self.field.is_empty() || self.field.iter().any(|part| part.is_empty()) {
            return Err(GateManifestError::new(format!(
                "{field}.field must contain non-empty path segments"
            )));
        }
        Ok(())
    }
}

/// Per-field provenance for one gate declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeclarationProvenance {
    id: Provenance,
    enabled: Provenance,
    #[serde(skip_serializing_if = "Option::is_none")]
    display: Option<Provenance>,
}

/// One current, non-executable gate selection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GateDeclaration {
    id: GateId,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    display: Option<String>,
    provenance: DeclarationProvenance,
}

impl GateDeclaration {
    /// Stable compiled identity selected by this row.
    #[must_use]
    pub const fn id(&self) -> GateId {
        self.id
    }

    /// Whether the compiled arm is selected.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Optional inert display label.
    #[must_use]
    pub fn display(&self) -> Option<&str> {
        self.display.as_deref()
    }

    /// Provenance of the ID field.
    #[must_use]
    pub const fn id_provenance(&self) -> &Provenance {
        &self.provenance.id
    }

    /// Provenance of the enablement field.
    #[must_use]
    pub const fn enabled_provenance(&self) -> &Provenance {
        &self.provenance.enabled
    }

    /// Provenance of the optional display field.
    #[must_use]
    pub const fn display_provenance(&self) -> Option<&Provenance> {
        self.provenance.display.as_ref()
    }

    fn compiled(id: GateId) -> Self {
        Self {
            id,
            enabled: true,
            display: None,
            provenance: DeclarationProvenance {
                id: Provenance::compiled(id, "id"),
                enabled: Provenance::compiled(id, "enabled"),
                display: None,
            },
        }
    }

    fn validate(&self, index: usize) -> Result<(), GateManifestError> {
        self.provenance
            .id
            .validate(&format!("gates[{index}].provenance.id"))?;
        self.provenance
            .enabled
            .validate(&format!("gates[{index}].provenance.enabled"))?;
        match (&self.display, &self.provenance.display) {
            (Some(display), Some(provenance)) => {
                if display.trim().is_empty() {
                    return Err(GateManifestError::new(format!(
                        "gates[{index}].display must be non-empty"
                    )));
                }
                provenance.validate(&format!("gates[{index}].provenance.display"))
            }
            (None, None) => Ok(()),
            _ => Err(GateManifestError::new(format!(
                "gates[{index}].display and gates[{index}].provenance.display must appear together"
            ))),
        }
    }
}

/// Complete ordered current gate manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GateManifest {
    version: u64,
    gates: Vec<GateDeclaration>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GateManifestInput {
    version: u64,
    gates: Vec<GateDeclaration>,
}

impl GateManifest {
    /// Admit one complete current manifest.
    ///
    /// # Errors
    ///
    /// Refuses a moved schema version, omission, duplicate, reordering, empty
    /// metadata, or incomplete provenance.
    pub fn new(version: u64, gates: Vec<GateDeclaration>) -> Result<Self, GateManifestError> {
        if version != GATE_MANIFEST_VERSION {
            return Err(GateManifestError::new(format!(
                "gate manifest version {version} is unsupported; expected {GATE_MANIFEST_VERSION}"
            )));
        }
        if gates.len() != GateId::ALL.len() {
            return Err(GateManifestError::new(format!(
                "gate manifest must declare exactly {} compiled IDs; observed {}",
                GateId::ALL.len(),
                gates.len()
            )));
        }
        for (index, (declaration, expected)) in gates.iter().zip(GateId::ALL).enumerate() {
            if declaration.id != expected {
                return Err(GateManifestError::new(format!(
                    "gates[{index}].id must be {expected:?}; observed {:?}",
                    declaration.id
                )));
            }
            declaration.validate(index)?;
        }
        Ok(Self { version, gates })
    }

    /// Parse ordinary current manifest bytes without consulting predecessor syntax.
    ///
    /// # Errors
    ///
    /// Returns TOML syntax or typed current-manifest admission failures.
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Current schema version.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Complete declarations in compiled battery order.
    #[must_use]
    pub fn gates(&self) -> &[GateDeclaration] {
        &self.gates
    }

    /// Render deterministic current TOML bytes.
    #[must_use]
    pub fn render(&self) -> String {
        toml::to_string_pretty(self).expect("an admitted gate manifest must serialize")
    }
}

impl Default for GateManifest {
    fn default() -> Self {
        Self {
            version: GATE_MANIFEST_VERSION,
            gates: GateId::ALL
                .into_iter()
                .map(GateDeclaration::compiled)
                .collect(),
        }
    }
}

impl<'de> Deserialize<'de> for GateManifest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let input = GateManifestInput::deserialize(deserializer)?;
        Self::new(input.version, input.gates).map_err(D::Error::custom)
    }
}

/// Why current manifest admission failed after decoding its field shapes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GateManifestError {
    message: String,
}

impl GateManifestError {
    fn new(message: String) -> Self {
        Self { message }
    }
}

impl fmt::Display for GateManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for GateManifestError {}

/// One already-acquired predecessor configuration document.
#[derive(Clone, Copy, Debug)]
pub struct PredecessorDocument<'a> {
    layer: OriginLayer,
    location: &'a str,
    text: &'a str,
}

impl<'a> PredecessorDocument<'a> {
    fn new(layer: OriginLayer, location: &'a str, text: &'a str) -> Self {
        Self {
            layer,
            location,
            text,
        }
    }

    /// Exact source location named by diagnostics and provenance.
    #[must_use]
    pub const fn location(self) -> &'a str {
        self.location
    }

    /// Exact acquired predecessor TOML.
    #[must_use]
    pub const fn text(self) -> &'a str {
        self.text
    }
}

/// Fixed predecessor source slots in documented resolution order.
#[derive(Clone, Copy, Debug)]
pub struct PredecessorInputs<'a> {
    system: PredecessorDocument<'a>,
    repository: Option<PredecessorDocument<'a>>,
    worktree: Option<PredecessorDocument<'a>>,
}

impl<'a> PredecessorInputs<'a> {
    /// Bind the required packaged system source.
    #[must_use]
    pub fn new(location: &'a str, text: &'a str) -> Self {
        Self {
            system: PredecessorDocument::new(OriginLayer::System, location, text),
            repository: None,
            worktree: None,
        }
    }

    /// Add the optional tracked repository source.
    #[must_use]
    pub fn repository(mut self, location: &'a str, text: &'a str) -> Self {
        self.repository = Some(PredecessorDocument::new(
            OriginLayer::Repository,
            location,
            text,
        ));
        self
    }

    /// Add the optional operator-owned worktree source.
    #[must_use]
    pub fn worktree(mut self, location: &'a str, text: &'a str) -> Self {
        self.worktree = Some(PredecessorDocument::new(
            OriginLayer::Worktree,
            location,
            text,
        ));
        self
    }

    fn layers(self) -> impl Iterator<Item = PredecessorDocument<'a>> {
        std::iter::once(self.system)
            .chain(self.repository)
            .chain(self.worktree)
    }
}

/// Stable predecessor migration refusal category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationErrorKind {
    Syntax,
    Malformed,
    UnknownGate,
    DuplicateGate,
    UndeclaredArm,
    UnsupportedField,
    ConflictingExecution,
    UnresolvedMount,
    ExecutableReplacement,
}

impl fmt::Display for MigrationErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Syntax => "syntax",
            Self::Malformed => "malformed",
            Self::UnknownGate => "unknown-gate",
            Self::DuplicateGate => "duplicate-gate",
            Self::UndeclaredArm => "undeclared-arm",
            Self::UnsupportedField => "unsupported-field",
            Self::ConflictingExecution => "conflicting-execution",
            Self::UnresolvedMount => "unresolved-mount",
            Self::ExecutableReplacement => "executable-replacement",
        })
    }
}

/// One exact, release-owned predecessor migration refusal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationError {
    kind: MigrationErrorKind,
    layer: OriginLayer,
    document: String,
    field: Vec<String>,
    message: String,
    recovery: String,
}

impl MigrationError {
    /// Stable refusal category.
    #[must_use]
    pub const fn kind(&self) -> MigrationErrorKind {
        self.kind
    }

    /// Exact predecessor source layer.
    #[must_use]
    pub const fn layer(&self) -> OriginLayer {
        self.layer
    }

    /// Exact predecessor document.
    #[must_use]
    pub fn document(&self) -> &str {
        &self.document
    }

    /// Structurally segmented predecessor field.
    #[must_use]
    pub fn field(&self) -> &[String] {
        &self.field
    }

    /// Concrete edit or selection recovery.
    #[must_use]
    pub fn recovery(&self) -> &str {
        &self.recovery
    }
}

impl fmt::Display for MigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            concat!(
                "predecessor gate migration release={} kind={} layer={} ",
                "document={} field={}: {}; recovery={}"
            ),
            PREDECESSOR_GATE_RELEASE,
            self.kind,
            self.layer,
            self.document,
            display_path(&self.field),
            self.message,
            self.recovery
        )
    }
}

impl Error for MigrationError {}

#[derive(Clone, Debug)]
struct SourceRef {
    layer: OriginLayer,
    document: String,
    field: Vec<String>,
}

impl SourceRef {
    fn new(document: PredecessorDocument<'_>, field: Vec<String>) -> Self {
        Self {
            layer: document.layer,
            document: document.location.to_owned(),
            field,
        }
    }

    fn child(&self, field: &str) -> Self {
        let mut source = self.clone();
        source.field.push(field.to_owned());
        source
    }

    fn provenance(&self) -> Provenance {
        Provenance {
            release: PREDECESSOR_GATE_RELEASE.to_owned(),
            layer: self.layer,
            document: self.document.clone(),
            field: self.field.clone(),
        }
    }

    fn error(
        &self,
        kind: MigrationErrorKind,
        message: impl Into<String>,
        recovery: impl Into<String>,
    ) -> MigrationError {
        MigrationError {
            kind,
            layer: self.layer,
            document: self.document.clone(),
            field: self.field.clone(),
            message: message.into(),
            recovery: recovery.into(),
        }
    }
}

#[derive(Clone, Debug)]
struct ResolvedEntry {
    value: Value,
    root: SourceRef,
    sources: BTreeMap<Vec<String>, SourceRef>,
    /// Whether a document named this identity rather than inheriting the
    /// implicit compiled entry.
    named: bool,
}

fn builtin_root(document: PredecessorDocument<'_>) -> SourceRef {
    SourceRef::new(
        document,
        vec!["policy".to_owned(), "pre_commit_builtins".to_owned()],
    )
}

impl ResolvedEntry {
    fn implicit(system: PredecessorDocument<'_>) -> Self {
        let root = builtin_root(system);
        let mut sources = BTreeMap::new();
        sources.insert(Vec::new(), root.clone());
        Self {
            value: Value::Boolean(true),
            root,
            sources,
            named: false,
        }
    }

    fn merge(&mut self, incoming: &Value, source: SourceRef) {
        self.named = true;
        if let Value::Table(incoming) = incoming {
            if !self.value.is_table() {
                self.value = Value::Table(Table::new());
                self.sources.clear();
            }
            self.root = source.clone();
            let table = self
                .value
                .as_table_mut()
                .expect("entry was reset to a table");
            merge_entry_table(table, incoming, &mut self.sources, &source, &mut Vec::new());
        } else {
            self.value = incoming.clone();
            self.root = source.clone();
            self.sources.clear();
            self.sources.insert(Vec::new(), source);
        }
    }

    fn source(&self, field: &str) -> &SourceRef {
        self.sources
            .get(&vec![field.to_owned()])
            .unwrap_or(&self.root)
    }
}

fn merge_entry_table(
    destination: &mut Table,
    incoming: &Table,
    sources: &mut BTreeMap<Vec<String>, SourceRef>,
    root: &SourceRef,
    prefix: &mut Vec<String>,
) {
    for (field, value) in incoming {
        prefix.push(field.clone());
        if let Value::Table(incoming_child) = value {
            if !destination.get(field).is_some_and(Value::is_table) {
                forget_sources(sources, prefix);
                destination.insert(field.clone(), Value::Table(Table::new()));
            }
            let child = destination
                .get_mut(field)
                .and_then(Value::as_table_mut)
                .expect("entry child was reset to a table");
            merge_entry_table(child, incoming_child, sources, root, prefix);
        } else {
            forget_sources(sources, prefix);
            destination.insert(field.clone(), value.clone());
            sources.insert(prefix.clone(), root.child_path(prefix));
        }
        prefix.pop();
    }
}

impl SourceRef {
    fn child_path(&self, suffix: &[String]) -> Self {
        let mut source = self.clone();
        source.field.extend_from_slice(suffix);
        source
    }
}

fn forget_sources(sources: &mut BTreeMap<Vec<String>, SourceRef>, prefix: &[String]) {
    sources.retain(|path, _| !path.starts_with(prefix));
}

#[derive(Default)]
struct CommandRegistry {
    values: BTreeMap<String, (Value, SourceRef)>,
}

impl CommandRegistry {
    fn apply(&mut self, table: &Table, document: PredecessorDocument<'_>) {
        self.apply_table(table, document, &mut Vec::new());
    }

    fn apply_table(
        &mut self,
        table: &Table,
        document: PredecessorDocument<'_>,
        prefix: &mut Vec<String>,
    ) {
        for (name, value) in table {
            prefix.push(name.clone());
            let dotted = prefix.join(".");
            if let Value::Table(child) = value {
                self.values.remove(&dotted);
                self.apply_table(child, document, prefix);
            } else {
                self.values
                    .retain(|key, _| key != &dotted && !key.starts_with(&(dotted.clone() + ".")));
                self.values.insert(
                    dotted,
                    (
                        value.clone(),
                        SourceRef::new(
                            document,
                            std::iter::once("commands".to_owned())
                                .chain(prefix.iter().cloned())
                                .collect(),
                        ),
                    ),
                );
            }
            prefix.pop();
        }
    }

    fn resolve(&self, name: &str, source: &SourceRef) -> Result<Vec<String>, MigrationError> {
        let name = name.trim();
        let Some((value, command_source)) = self.values.get(name) else {
            return Err(source.error(
                MigrationErrorKind::UnresolvedMount,
                format!("mounted command {name:?} is not declared"),
                "remove the mounted replacement and select the compiled gate ID",
            ));
        };
        if value == &Value::Boolean(false) {
            return Err(source.error(
                MigrationErrorKind::UnresolvedMount,
                format!("mounted command {name:?} is disabled"),
                "remove the mounted replacement and select the compiled gate ID",
            ));
        }
        command_argv(value, command_source)
    }
}

/// Translate predecessor layers once into the sole current manifest shape.
///
/// # Errors
///
/// Returns the first exact syntax, name, identity, shape, layer-merge, mount,
/// or executable-declaration refusal. No partial manifest is returned.
pub fn migrate_predecessor(inputs: PredecessorInputs<'_>) -> Result<GateManifest, MigrationError> {
    let mut entries = GateId::ALL
        .into_iter()
        .map(|id| (id, ResolvedEntry::implicit(inputs.system)))
        .collect::<BTreeMap<_, _>>();
    let mut commands = CommandRegistry::default();
    for document in inputs.layers() {
        let root = parse_document(document)?;
        if let Some(table) = root.get("commands").and_then(Value::as_table) {
            commands.apply(table, document);
        }
        let Some(policy) = root.get("policy") else {
            continue;
        };
        let policy = policy.as_table().ok_or_else(|| {
            SourceRef::new(document, vec!["policy".to_owned()]).error(
                MigrationErrorKind::Malformed,
                "policy must be a table",
                "restore [policy] as a TOML table",
            )
        })?;
        let Some(raw_registry) = policy.get("pre_commit_builtins") else {
            continue;
        };
        let source = builtin_root(document);
        match raw_registry {
            Value::Boolean(false) => entries
                .iter_mut()
                .filter(|(id, _)| id.predecessor_declared())
                .for_each(|(_, entry)| entry.merge(raw_registry, source.clone())),
            Value::Table(registry) => apply_builtin_layer(&mut entries, registry, document)?,
            _ => {
                return Err(source.error(
                    MigrationErrorKind::Malformed,
                    "pre_commit_builtins must be false or a table",
                    "set pre_commit_builtins = false or restore its typed table",
                ));
            }
        }
    }

    let gates = GateId::ALL
        .into_iter()
        .map(|id| {
            interpret_entry(
                id,
                entries.remove(&id).expect("all IDs initialized"),
                &commands,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    GateManifest::new(GATE_MANIFEST_VERSION, gates).map_err(|error| {
        builtin_root(inputs.system).error(
            MigrationErrorKind::Malformed,
            error.to_string(),
            "restore the complete predecessor builtin registry",
        )
    })
}

fn parse_document(document: PredecessorDocument<'_>) -> Result<Table, MigrationError> {
    toml::from_str::<Table>(document.text).map_err(|error| {
        SourceRef::new(document, vec!["<document>".to_owned()]).error(
            MigrationErrorKind::Syntax,
            error.to_string(),
            "repair the predecessor TOML before migration",
        )
    })
}

fn apply_builtin_layer(
    entries: &mut BTreeMap<GateId, ResolvedEntry>,
    registry: &Table,
    document: PredecessorDocument<'_>,
) -> Result<(), MigrationError> {
    let mut names = BTreeMap::new();
    for (raw_name, value) in registry {
        let path = vec![
            "policy".to_owned(),
            "pre_commit_builtins".to_owned(),
            raw_name.clone(),
        ];
        let source = SourceRef::new(document, path);
        let Some(id) = GateId::predecessor(raw_name) else {
            return Err(source.error(
                MigrationErrorKind::UnknownGate,
                format!("unknown predecessor builtin {raw_name:?}"),
                "remove the unknown entry or select one declared compiled gate ID",
            ));
        };
        if let Some(previous) = names.insert(id, raw_name) {
            return Err(source.error(
                MigrationErrorKind::DuplicateGate,
                format!(
                    concat!(
                        "equivalent predecessor spellings {previous:?} and ",
                        "{raw_name:?} both select {id}"
                    ),
                    previous = previous,
                    raw_name = raw_name,
                    id = id
                ),
                "keep one spelling for this builtin in the source layer",
            ));
        }
        validate_entry(value, &source)?;
        entries
            .get_mut(&id)
            .expect("known gate ID was initialized")
            .merge(value, source);
    }
    Ok(())
}

fn validate_entry(value: &Value, source: &SourceRef) -> Result<(), MigrationError> {
    match value {
        Value::Boolean(_) => Ok(()),
        Value::String(name) if !name.trim().is_empty() => Ok(()),
        Value::String(_) => Err(source.error(
            MigrationErrorKind::Malformed,
            "mounted command name must be non-empty",
            "remove the empty replacement and select the compiled gate ID",
        )),
        Value::Table(table) => validate_entry_table(table, source),
        _ => Err(source.error(
            MigrationErrorKind::Malformed,
            "builtin entry must be true, false, a mounted command name, or a table",
            "replace the value with true, false, or a typed current gate selection",
        )),
    }
}

fn validate_entry_table(table: &Table, source: &SourceRef) -> Result<(), MigrationError> {
    const ADMITTED: &[&str] = &[
        "label",
        "mount",
        "run",
        "argv",
        "formatter",
        "enabled",
        "scopes",
    ];
    if let Some(field) = table
        .keys()
        .find(|field| !ADMITTED.contains(&field.as_str()))
    {
        return Err(source.child(field).error(
            MigrationErrorKind::UnsupportedField,
            format!("unsupported predecessor command-step field {field:?}"),
            "remove the field and select the compiled gate ID",
        ));
    }
    for field in ["formatter", "scopes"] {
        if table.contains_key(field) {
            return Err(source.child(field).error(
                MigrationErrorKind::UnsupportedField,
                format!("predecessor builtin field {field:?} has no current declaration meaning"),
                format!("remove {field:?} and select the compiled gate ID"),
            ));
        }
    }
    let executable = ["mount", "run", "argv"]
        .into_iter()
        .filter(|field| table.contains_key(*field))
        .collect::<Vec<_>>();
    if executable.len() > 1 {
        return Err(source.child(executable[1]).error(
            MigrationErrorKind::ConflictingExecution,
            format!(
                "execution selectors {} appear together",
                executable.join(", ")
            ),
            "remove every executable selector and select the compiled gate ID",
        ));
    }
    if let Some(label) = table.get("label")
        && label.as_str().is_none_or(|value| value.trim().is_empty())
    {
        return Err(source.child("label").error(
            MigrationErrorKind::Malformed,
            "label must be a non-empty string",
            "remove the label or replace it with non-empty display text",
        ));
    }
    if let Some(enabled) = table.get("enabled")
        && !enabled.is_bool()
    {
        return Err(source.child("enabled").error(
            MigrationErrorKind::Malformed,
            "enabled must be true or false",
            "replace enabled with a boolean",
        ));
    }
    if let Some(mount) = table.get("mount")
        && mount.as_str().is_none_or(|value| value.trim().is_empty())
    {
        return Err(source.child("mount").error(
            MigrationErrorKind::Malformed,
            "mount must be a non-empty mounted command name",
            "remove mount and select the compiled gate ID",
        ));
    }
    for field in ["run", "argv"] {
        if let Some(value) = table.get(field) {
            command_argv(value, &source.child(field))?;
        }
    }
    Ok(())
}

fn command_argv(value: &Value, source: &SourceRef) -> Result<Vec<String>, MigrationError> {
    let argv = match value {
        Value::String(command) => shell_words::split(command).map_err(|error| {
            source.error(
                MigrationErrorKind::Malformed,
                format!("command string is malformed: {error}"),
                "remove the executable replacement and select the compiled gate ID",
            )
        })?,
        Value::Array(values) if values.iter().all(Value::is_str) => values
            .iter()
            .map(|value| value.as_str().expect("checked string array").to_owned())
            .collect(),
        _ => {
            return Err(source.error(
                MigrationErrorKind::Malformed,
                "command must be a shell string or an argv string list",
                "remove the executable replacement and select the compiled gate ID",
            ));
        }
    };
    if argv.is_empty() || argv[0].is_empty() {
        return Err(source.error(
            MigrationErrorKind::Malformed,
            "command argv must contain a non-empty program",
            "remove the executable replacement and select the compiled gate ID",
        ));
    }
    Ok(argv)
}

fn interpret_entry(
    id: GateId,
    entry: ResolvedEntry,
    commands: &CommandRegistry,
) -> Result<GateDeclaration, MigrationError> {
    let id_source = entry.root.provenance();
    match &entry.value {
        Value::Boolean(enabled) => {
            undeclared_refusal(id, &entry)?;
            Ok(GateDeclaration {
                id,
                enabled: *enabled,
                display: None,
                provenance: DeclarationProvenance {
                    id: id_source,
                    enabled: entry.root.provenance(),
                    display: None,
                },
            })
        }
        Value::String(mount) => {
            let argv = commands.resolve(mount, &entry.root)?;
            Err(executable_refusal(id, &entry.root, &argv))
        }
        Value::Table(table) => {
            if let Some((field, value)) = ["mount", "run", "argv"]
                .into_iter()
                .find_map(|field| table.get(field).map(|value| (field, value)))
            {
                let source = entry.source(field);
                let argv = if field == "mount" {
                    commands
                        .resolve(value.as_str().expect("validated mount is a string"), source)?
                } else {
                    command_argv(value, source)?
                };
                return Err(executable_refusal(id, source, &argv));
            }
            undeclared_refusal(id, &entry)?;
            let enabled = table
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let display = table
                .get("label")
                .and_then(Value::as_str)
                .map(str::trim)
                .map(str::to_owned);
            let display_source = display.as_ref().map(|_| entry.source("label").provenance());
            Ok(GateDeclaration {
                id,
                enabled,
                display,
                provenance: DeclarationProvenance {
                    id: id_source,
                    enabled: entry.source("enabled").provenance(),
                    display: display_source,
                },
            })
        }
        _ => unreachable!("every layer value was validated before merge"),
    }
}

/// Refuse a declaration for an identity the predecessor release never declared.
///
/// [`GateId::predecessor`] answers for every current identity, so a document
/// naming a gate this workspace grew past resolves and is then rejected by this
/// rule rather than by failing to recognize the name. Only a named entry can
/// trip it: every identity starts from an implicit entry the migration itself
/// supplies, and refusing that would refuse every migration outright.
fn undeclared_refusal(id: GateId, entry: &ResolvedEntry) -> Result<(), MigrationError> {
    if !entry.named || id.predecessor_declared() {
        return Ok(());
    }
    Err(entry.root.error(
        MigrationErrorKind::UndeclaredArm,
        format!(
            concat!(
                "predecessor builtin selects compiled Rust gate {id}; release ",
                "{release} declared no standard arm for that identity, so no ",
                "predecessor declaration answers for it"
            ),
            id = id,
            release = PREDECESSOR_GATE_RELEASE
        ),
        format!("remove the entry and let the compiled battery declare {id}"),
    ))
}

fn executable_refusal(id: GateId, source: &SourceRef, argv: &[String]) -> MigrationError {
    match predecessor_standard_arm(id) {
        PredecessorStandardArm::InternalBuiltin => source.error(
            MigrationErrorKind::ExecutableReplacement,
            format!(
                concat!(
                    "resolved executable argv {argv:?} cannot normalize to the ",
                    "release-declared standard arm for compiled Rust gate {id}; ",
                    "release {release} declares that standard arm as an internal ",
                    "PreCommitStep builtin, not executable argv"
                ),
                argv = argv,
                id = id,
                release = PREDECESSOR_GATE_RELEASE
            ),
            format!("remove the executable replacement and select compiled gate ID {id}"),
        ),
        PredecessorStandardArm::AbsentFromBattery => source.error(
            MigrationErrorKind::ExecutableReplacement,
            format!(
                concat!(
                    "resolved executable argv {argv:?} cannot normalize to compiled Rust ",
                    "gate {id}; release {release} declared no standard arm for that ",
                    "identity, so no predecessor argv answers for it"
                ),
                argv = argv,
                id = id,
                release = PREDECESSOR_GATE_RELEASE
            ),
            format!("remove the executable replacement and select compiled gate ID {id}"),
        ),
    }
}

fn display_path(path: &[String]) -> String {
    path.iter()
        .map(|part| {
            if part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            {
                part.clone()
            } else {
                format!("{part:?}")
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod fixtures;
