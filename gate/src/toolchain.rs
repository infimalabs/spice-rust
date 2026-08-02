//! Compiled declarations and typed measurements for the toolchain arm family.
//!
//! These eight arms occupy one closed interval in the canonical battery.  A
//! declaration fixes tree jurisdiction, configuration provenance, native
//! component identity, and every external executable/version contract.  A run
//! contributes observations only; it cannot select a program, alter an
//! argument vector, or route policy through the predecessor Python package.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use serde::Serialize;

use crate::manifest::{GateId, PREDECESSOR_GATE_RELEASE};

/// The only report schema emitted by this compiled arm family.
pub const TOOLCHAIN_REPORT_VERSION: u64 = 1;
const NATIVE_ENGINE_VERSION: &str = "spice-gate-toolchain-v1";

/// A compiled source of configuration admitted by one arm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigurationSource {
    source: &'static str,
    field: &'static str,
}

impl ConfigurationSource {
    /// Fixed tracked or compiled source name.
    #[must_use]
    pub const fn source(self) -> &'static str {
        self.source
    }

    /// Exact field or compiled symbol within the source.
    #[must_use]
    pub const fn field(self) -> &'static str {
        self.field
    }
}

/// Complete tree jurisdiction of one compiled arm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TreeJurisdiction {
    roots: &'static [&'static str],
    includes: &'static [&'static str],
    excludes: &'static [&'static str],
    applicability: &'static str,
}

impl TreeJurisdiction {
    /// Roots from which matching begins.
    #[must_use]
    pub const fn roots(self) -> &'static [&'static str] {
        self.roots
    }

    /// Closed include grammar.
    #[must_use]
    pub const fn includes(self) -> &'static [&'static str] {
        self.includes
    }

    /// Closed generated/vendor exclusion grammar.
    #[must_use]
    pub const fn excludes(self) -> &'static [&'static str] {
        self.excludes
    }

    /// Exact condition under which the selected tree is applicable.
    #[must_use]
    pub const fn applicability(self) -> &'static str {
        self.applicability
    }
}

/// One external executable whose identity is fixed by a declaration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolRequirement {
    name: &'static str,
    program: &'static str,
    version_argv: &'static [&'static str],
    exact_version: &'static str,
    role: &'static str,
}

impl ToolRequirement {
    /// Stable evidence name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// Sole executable name. There is no alternate resolver path.
    #[must_use]
    pub const fn program(self) -> &'static str {
        self.program
    }

    /// Fixed version-probe arguments.
    #[must_use]
    pub const fn version_argv(self) -> &'static [&'static str] {
        self.version_argv
    }

    /// Exact admitted probe output.
    #[must_use]
    pub const fn exact_version(self) -> &'static str {
        self.exact_version
    }

    /// Why this executable participates in the arm.
    #[must_use]
    pub const fn role(self) -> &'static str {
        self.role
    }
}

/// One native arm declaration in canonical predecessor-relative order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolchainDeclaration {
    id: GateId,
    native_component: &'static str,
    jurisdiction: TreeJurisdiction,
    configuration: ConfigurationSource,
    tools: &'static [ToolRequirement],
    analysis: &'static [&'static str],
}

impl ToolchainDeclaration {
    /// Stable current gate identity.
    #[must_use]
    pub const fn id(self) -> GateId {
        self.id
    }

    /// Compiled Rust component that owns policy and parsing.
    #[must_use]
    pub const fn native_component(self) -> &'static str {
        self.native_component
    }

    /// Closed selected-tree jurisdiction.
    #[must_use]
    pub const fn jurisdiction(self) -> TreeJurisdiction {
        self.jurisdiction
    }

    /// Sole admitted configuration source.
    #[must_use]
    pub const fn configuration(self) -> ConfigurationSource {
        self.configuration
    }

    /// External tool requirements, empty for native scanners.
    #[must_use]
    pub const fn tools(self) -> &'static [ToolRequirement] {
        self.tools
    }

    /// Fixed direct analysis invocations or native operation names.
    #[must_use]
    pub const fn analysis(self) -> &'static [&'static str] {
        self.analysis
    }
}

const COMMON_EXCLUDES: &[&str] = &[
    ".git/**",
    "target/**",
    "dist/**",
    "node_modules/**",
    ".venv/**",
    "vendor/**",
];
const FORMATTER_TOOLS: &[ToolRequirement] = &[
    ToolRequirement {
        name: "cargo",
        program: "cargo",
        version_argv: &["--version"],
        exact_version: "cargo 1.97.1 (c980f4866 2026-06-30)",
        role: "workspace formatter driver",
    },
    ToolRequirement {
        name: "rustfmt",
        program: "rustfmt",
        version_argv: &["--version"],
        exact_version: "rustfmt 1.9.0-stable (8bab26f4f6 2026-07-14)",
        role: "Rust formatter",
    },
];
const CANVAS_TOOLS: &[ToolRequirement] = &[
    ToolRequirement {
        name: "cargo",
        program: "cargo",
        version_argv: &["--version"],
        exact_version: "cargo 1.97.1 (c980f4866 2026-06-30)",
        role: "wasm32 typecheck driver",
    },
    ToolRequirement {
        name: "trunk",
        program: "trunk",
        version_argv: &["--version"],
        exact_version: "trunk 0.21.14",
        role: "canvas asset graph validator",
    },
];
const PYRIGHT_TOOLS: &[ToolRequirement] = &[ToolRequirement {
    name: "pyright",
    program: "pyright",
    version_argv: &["--version"],
    exact_version: "pyright 1.1.411",
    role: "Python type analyzer",
}];

/// All eight declarations in their fixed interval of the predecessor battery.
pub const TOOLCHAIN_ARMS: [ToolchainDeclaration; 8] = [
    ToolchainDeclaration {
        id: GateId::Formatters,
        native_component: "gate::toolchain::formatters",
        jurisdiction: TreeJurisdiction {
            roots: &["."],
            includes: &["**/*.rs", "Cargo.toml"],
            excludes: COMMON_EXCLUDES,
            applicability: "at least one selected **/*.rs file",
        },
        configuration: ConfigurationSource {
            source: "rust-toolchain.toml",
            field: "toolchain.channel + toolchain.components[rustfmt]",
        },
        tools: FORMATTER_TOOLS,
        analysis: &["cargo fmt --all -- --check"],
    },
    ToolchainDeclaration {
        id: GateId::LocalPaths,
        native_component: "gate::toolchain::local_paths",
        jurisdiction: TreeJurisdiction {
            roots: &["."],
            includes: &["tracked UTF-8 text"],
            excludes: COMMON_EXCLUDES,
            applicability: "at least one selected UTF-8 text file",
        },
        configuration: ConfigurationSource {
            source: "compiled",
            field: "gate::toolchain::LOCAL_PATH_POLICY_V1",
        },
        tools: &[],
        analysis: &["native scan for absolute workstation path forms"],
    },
    ToolchainDeclaration {
        id: GateId::Taste,
        native_component: "gate::toolchain::taste",
        jurisdiction: TreeJurisdiction {
            roots: &["."],
            includes: &["**/*.md", "**/*.txt", "**/*.rst"],
            excludes: COMMON_EXCLUDES,
            applicability: "at least one selected prose file",
        },
        configuration: ConfigurationSource {
            source: "compiled",
            field: "gate::toolchain::TASTE_POLICY_V1",
        },
        tools: &[],
        analysis: &["native token-aware prose scan"],
    },
    ToolchainDeclaration {
        id: GateId::CanvasWebTypecheck,
        native_component: "gate::toolchain::canvas_web_typecheck",
        jurisdiction: TreeJurisdiction {
            roots: &["app", "canvas"],
            includes: &[
                "app/Cargo.toml",
                "app/src/**/*.rs",
                "app/index.html",
                "app/**/*.css",
                "canvas/Cargo.toml",
                "canvas/src/**/*.rs",
            ],
            excludes: COMMON_EXCLUDES,
            applicability: "app/Cargo.toml and app/index.html are selected",
        },
        configuration: ConfigurationSource {
            source: "rust-toolchain.toml",
            field: "toolchain.channel + toolchain.targets[wasm32-unknown-unknown]",
        },
        tools: CANVAS_TOOLS,
        analysis: &[
            "cargo check --locked --workspace --target wasm32-unknown-unknown",
            "trunk build index.html --release --locked --filehash=false",
        ],
    },
    ToolchainDeclaration {
        id: GateId::JavascriptUnused,
        native_component: "gate::toolchain::javascript_unused",
        jurisdiction: TreeJurisdiction {
            roots: &["."],
            includes: &["**/*.js", "**/*.mjs", "**/*.cjs"],
            excludes: COMMON_EXCLUDES,
            applicability: "at least one selected JavaScript module",
        },
        configuration: ConfigurationSource {
            source: "compiled",
            field: "gate::toolchain::JAVASCRIPT_UNUSED_POLICY_V1",
        },
        tools: &[],
        analysis: &["native JavaScript declaration/reference analysis"],
    },
    ToolchainDeclaration {
        id: GateId::PythonTypecheck,
        native_component: "gate::toolchain::python_typecheck",
        jurisdiction: TreeJurisdiction {
            roots: &["."],
            includes: &["pyproject.toml", "**/*.py", "**/*.pyi"],
            excludes: COMMON_EXCLUDES,
            applicability: "pyproject.toml and at least one selected Python source file",
        },
        configuration: ConfigurationSource {
            source: "pyproject.toml",
            field: "tool.pyright",
        },
        tools: PYRIGHT_TOOLS,
        analysis: &["pyright --level error"],
    },
    ToolchainDeclaration {
        id: GateId::EnvPolicy,
        native_component: "gate::toolchain::env_policy",
        jurisdiction: TreeJurisdiction {
            roots: &["."],
            includes: &[
                "**/*.rs",
                "**/*.py",
                "**/*.js",
                "**/*.mjs",
                "**/*.cjs",
                "**/*.toml",
            ],
            excludes: COMMON_EXCLUDES,
            applicability: "at least one selected source or TOML file",
        },
        configuration: ConfigurationSource {
            source: "compiled",
            field: "gate::toolchain::ENV_ACCESS_POLICY_V1",
        },
        tools: &[],
        analysis: &["native environment access and waiver scan"],
    },
    ToolchainDeclaration {
        id: GateId::EnvNameLedger,
        native_component: "gate::toolchain::env_name_ledger",
        jurisdiction: TreeJurisdiction {
            roots: &["."],
            includes: &[
                "**/*.rs",
                "**/*.py",
                "**/*.js",
                "**/*.mjs",
                "**/*.cjs",
                "**/*.toml",
            ],
            excludes: COMMON_EXCLUDES,
            applicability: "at least one selected source or TOML file",
        },
        configuration: ConfigurationSource {
            source: "compiled",
            field: "gate::toolchain::ENV_NAME_LEDGER_POLICY_V1",
        },
        tools: &[],
        analysis: &["native environment declaration/reference ledger"],
    },
];

/// Operational evidence for one declared external tool.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "discovery", rename_all = "kebab-case")]
pub enum ToolEvidence {
    /// The sole declared program was not found.
    Missing {
        /// Stable declaration name.
        name: String,
        /// Exact program name that was searched.
        requested_program: String,
    },
    /// One executable was resolved and probed.
    Identified {
        /// Stable declaration name.
        name: String,
        /// Exact program name that was requested.
        requested_program: String,
        /// Resolved executable path retained as evidence.
        resolved_program: String,
        /// Unmodified version-probe output.
        version: String,
        /// Digest of the resolved executable bytes.
        digest: String,
    },
}

impl ToolEvidence {
    fn name(&self) -> &str {
        match self {
            Self::Missing { name, .. } | Self::Identified { name, .. } => name,
        }
    }

    fn requested_program(&self) -> &str {
        match self {
            Self::Missing {
                requested_program, ..
            }
            | Self::Identified {
                requested_program, ..
            } => requested_program,
        }
    }
}

/// One structured finding produced by a native arm.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ArmFinding {
    /// Selected-tree-relative path.
    pub path: String,
    /// One-based line, when the analyzer has a source locus.
    pub line: Option<u32>,
    /// One-based column, when the analyzer has a source locus.
    pub column: Option<u32>,
    /// Gate-specific retained evidence.
    pub detail: FindingDetail,
}

/// Closed structured evidence vocabulary for the eight arms.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FindingDetail {
    /// Source differs from the formatter's canonical bytes.
    Formatting { formatter: String },
    /// Workstation-local absolute path entered tracked text.
    LocalPath { fragment: String },
    /// A discouraged prose token has a compiled replacement.
    Taste { word: String, replacement: String },
    /// Rust/Wasm canvas analysis diagnostic.
    CanvasType { code: String, message: String },
    /// JavaScript declaration has no admitted references.
    JavascriptUnused {
        symbol: String,
        declaration_line: u32,
        references: u32,
    },
    /// Pyright diagnostic.
    PythonType { code: String, message: String },
    /// Environment access violates the compiled access law.
    EnvironmentAccess { name: String, access: String },
    /// Environment name has inconsistent declaration/reference evidence.
    EnvironmentLedger {
        name: String,
        declarations: u32,
        references: u32,
    },
}

impl FindingDetail {
    const fn gate(&self) -> GateId {
        match self {
            Self::Formatting { .. } => GateId::Formatters,
            Self::LocalPath { .. } => GateId::LocalPaths,
            Self::Taste { .. } => GateId::Taste,
            Self::CanvasType { .. } => GateId::CanvasWebTypecheck,
            Self::JavascriptUnused { .. } => GateId::JavascriptUnused,
            Self::PythonType { .. } => GateId::PythonTypecheck,
            Self::EnvironmentAccess { .. } => GateId::EnvPolicy,
            Self::EnvironmentLedger { .. } => GateId::EnvNameLedger,
        }
    }
}

/// Typed result of one selected arm.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArmOutcome {
    /// Analysis completed without findings.
    Passed,
    /// Analysis completed with structured policy findings.
    Findings,
    /// A sole declared executable could not be discovered.
    ToolUnavailable,
    /// A discovered executable did not match its exact version contract.
    UnsupportedVersion,
    /// The declaration's project marker or file class was absent.
    NotApplicable,
    /// A compiled/tracked explicit exclusion removed the arm from this tree.
    Excluded,
    /// Bounded analysis exceeded its compiled deadline.
    Timeout,
    /// Analysis output or selected source could not be parsed.
    ParseFailure,
    /// A direct analysis process returned an unexplained nonzero completion.
    NonzeroAnalysis,
}

impl ArmOutcome {
    /// Only contract-declared absence and exclusion are skips.
    #[must_use]
    pub const fn is_skip(self) -> bool {
        matches!(self, Self::NotApplicable | Self::Excluded)
    }

    /// Every result other than pass or an explicit skip refuses the battery.
    #[must_use]
    pub const fn is_failure(self) -> bool {
        !matches!(self, Self::Passed | Self::NotApplicable | Self::Excluded)
    }
}

/// Raw native-run observation to be measured against the compiled declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArmObservation {
    /// Stable current gate identity.
    pub gate_id: GateId,
    /// Typed operational result.
    pub outcome: ArmOutcome,
    /// Exact human-readable cause; never synthesized from exit status alone.
    pub reason: String,
    /// External tool evidence in any input order.
    pub tools: Vec<ToolEvidence>,
    /// Structured findings in any input order.
    pub findings: Vec<ArmFinding>,
    /// Nonzero completion code, present only for `NonzeroAnalysis`.
    pub exit_code: Option<i32>,
}

/// Canonical retained measurement of one arm.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArmMeasurement {
    /// Stable current gate identity.
    pub gate_id: GateId,
    /// Typed operational result.
    pub outcome: ArmOutcome,
    /// Exact retained cause.
    pub reason: String,
    /// Sole configuration source.
    pub configuration_source: String,
    /// Exact field or compiled symbol used from the source.
    pub configuration_field: String,
    /// Native Rust component and schema that performed policy/parsing.
    pub native_engine: String,
    /// Complete roots retained in declaration order.
    pub roots: Vec<String>,
    /// Complete includes retained in declaration order.
    pub includes: Vec<String>,
    /// Complete exclusions retained in declaration order.
    pub excludes: Vec<String>,
    /// Exact applicability predicate.
    pub applicability: String,
    /// External evidence in declaration order.
    pub tools: Vec<ToolEvidence>,
    /// Findings sorted by path, locus, and typed detail.
    pub findings: Vec<ArmFinding>,
    /// Nonzero completion code when that state was observed.
    pub exit_code: Option<i32>,
}

/// Canonical ordered report for the complete eight-arm family.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ToolchainReport {
    /// Current report schema.
    pub version: u64,
    /// Exactly eight measurements in compiled order.
    pub arms: Vec<ArmMeasurement>,
}

impl ToolchainReport {
    /// Deterministic JSON evidence with one trailing newline.
    #[must_use]
    pub fn canonical_json(&self) -> Vec<u8> {
        let mut bytes =
            serde_json::to_vec(self).expect("toolchain reports contain only JSON values");
        bytes.push(b'\n');
        bytes
    }
}

/// Why observations could not become one canonical report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolchainError {
    /// Observation named a gate outside this family.
    ForeignGate(GateId),
    /// Two observations named the same gate.
    DuplicateGate(GateId),
    /// The complete family omitted a gate.
    MissingGate(GateId),
    /// A reason was empty or multiline.
    InvalidReason(GateId),
    /// Findings did not match the outcome or gate.
    InvalidFindings { gate: GateId, evidence: String },
    /// Completion code did not match the outcome.
    InvalidExitCode(GateId),
    /// Tool evidence did not match the declaration or outcome.
    InvalidToolEvidence { gate: GateId, evidence: String },
}

impl fmt::Display for ToolchainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ForeignGate(gate) => {
                write!(formatter, "{} is not a toolchain arm", gate.as_str())
            }
            Self::DuplicateGate(gate) => {
                write!(formatter, "{} has duplicate observations", gate.as_str())
            }
            Self::MissingGate(gate) => write!(formatter, "{} has no observation", gate.as_str()),
            Self::InvalidReason(gate) => {
                write!(
                    formatter,
                    "{} has an empty or multiline reason",
                    gate.as_str()
                )
            }
            Self::InvalidFindings { gate, evidence } => {
                write!(
                    formatter,
                    "{} findings are invalid: {evidence}",
                    gate.as_str()
                )
            }
            Self::InvalidExitCode(gate) => {
                write!(
                    formatter,
                    "{} has an outcome-inconsistent exit code",
                    gate.as_str()
                )
            }
            Self::InvalidToolEvidence { gate, evidence } => {
                write!(
                    formatter,
                    "{} tool evidence is invalid: {evidence}",
                    gate.as_str()
                )
            }
        }
    }
}

impl Error for ToolchainError {}

/// Measure a complete, unordered observation set into the fixed arm order.
///
/// # Errors
///
/// Returns the first foreign/duplicate/missing arm or inconsistent typed
/// observation. No partial report is returned.
pub fn measure_toolchain(
    observations: impl IntoIterator<Item = ArmObservation>,
) -> Result<ToolchainReport, ToolchainError> {
    let mut remaining: Vec<ArmObservation> = observations.into_iter().collect();
    let mut seen = BTreeSet::new();
    for observation in &remaining {
        if declaration(observation.gate_id).is_none() {
            return Err(ToolchainError::ForeignGate(observation.gate_id));
        }
        if !seen.insert(observation.gate_id) {
            return Err(ToolchainError::DuplicateGate(observation.gate_id));
        }
    }

    let mut arms = Vec::with_capacity(TOOLCHAIN_ARMS.len());
    for arm in TOOLCHAIN_ARMS {
        let Some(index) = remaining
            .iter()
            .position(|observation| observation.gate_id == arm.id)
        else {
            return Err(ToolchainError::MissingGate(arm.id));
        };
        arms.push(measure_arm(arm, remaining.swap_remove(index))?);
    }
    debug_assert!(remaining.is_empty());
    Ok(ToolchainReport {
        version: TOOLCHAIN_REPORT_VERSION,
        arms,
    })
}

fn declaration(gate: GateId) -> Option<ToolchainDeclaration> {
    TOOLCHAIN_ARMS.into_iter().find(|arm| arm.id == gate)
}

fn measure_arm(
    declaration: ToolchainDeclaration,
    mut observation: ArmObservation,
) -> Result<ArmMeasurement, ToolchainError> {
    let gate = declaration.id;
    if observation.reason.is_empty() || observation.reason.contains(['\n', '\r']) {
        return Err(ToolchainError::InvalidReason(gate));
    }
    validate_exit_code(gate, observation.outcome, observation.exit_code)?;
    validate_findings(gate, observation.outcome, &observation.findings)?;
    order_and_validate_tools(declaration, &observation.outcome, &mut observation.tools)?;
    observation.findings.sort();
    if observation
        .findings
        .windows(2)
        .any(|pair| pair[0] == pair[1])
    {
        return Err(ToolchainError::InvalidFindings {
            gate,
            evidence: "duplicate structured finding".to_owned(),
        });
    }

    let jurisdiction = declaration.jurisdiction;
    let configuration = declaration.configuration;
    Ok(ArmMeasurement {
        gate_id: gate,
        outcome: observation.outcome,
        reason: observation.reason,
        configuration_source: configuration.source.to_owned(),
        configuration_field: configuration.field.to_owned(),
        native_engine: format!("{}@{NATIVE_ENGINE_VERSION}", declaration.native_component),
        roots: owned(jurisdiction.roots),
        includes: owned(jurisdiction.includes),
        excludes: owned(jurisdiction.excludes),
        applicability: jurisdiction.applicability.to_owned(),
        tools: observation.tools,
        findings: observation.findings,
        exit_code: observation.exit_code,
    })
}

fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn validate_exit_code(
    gate: GateId,
    outcome: ArmOutcome,
    exit_code: Option<i32>,
) -> Result<(), ToolchainError> {
    let valid = match outcome {
        ArmOutcome::NonzeroAnalysis => exit_code.is_some_and(|code| code != 0),
        _ => exit_code.is_none(),
    };
    if valid {
        Ok(())
    } else {
        Err(ToolchainError::InvalidExitCode(gate))
    }
}

fn validate_findings(
    gate: GateId,
    outcome: ArmOutcome,
    findings: &[ArmFinding],
) -> Result<(), ToolchainError> {
    if (outcome == ArmOutcome::Findings) != !findings.is_empty() {
        return Err(ToolchainError::InvalidFindings {
            gate,
            evidence: "only findings outcomes carry a nonempty finding list".to_owned(),
        });
    }
    for finding in findings {
        if finding.path.is_empty()
            || finding.path.starts_with('/')
            || finding.path.split('/').any(|part| part == "..")
            || finding.line == Some(0)
            || finding.column == Some(0)
            || finding.detail.gate() != gate
        {
            return Err(ToolchainError::InvalidFindings {
                gate,
                evidence: format!("malformed or gate-mismatched finding at {:?}", finding.path),
            });
        }
    }
    Ok(())
}

fn order_and_validate_tools(
    declaration: ToolchainDeclaration,
    outcome: &ArmOutcome,
    evidence: &mut [ToolEvidence],
) -> Result<(), ToolchainError> {
    let gate = declaration.id;
    let declared = declaration.tools;
    if declared.is_empty() {
        return validate_native_tool_state(gate, outcome, evidence);
    }
    validate_tool_rows(gate, declared, evidence)?;
    evidence.sort_by_key(|item| declared_tool_position(declared, item));
    validate_external_tool_outcome(gate, declared, outcome, evidence)
}

fn validate_native_tool_state(
    gate: GateId,
    outcome: &ArmOutcome,
    evidence: &[ToolEvidence],
) -> Result<(), ToolchainError> {
    if !evidence.is_empty() {
        return Err(ToolchainError::InvalidToolEvidence {
            gate,
            evidence: "native scanner declared no external executable".to_owned(),
        });
    }
    if matches!(
        outcome,
        ArmOutcome::ToolUnavailable | ArmOutcome::UnsupportedVersion
    ) {
        return Err(ToolchainError::InvalidToolEvidence {
            gate,
            evidence: "native scanner cannot report an external-tool state".to_owned(),
        });
    }
    Ok(())
}

fn validate_tool_rows(
    gate: GateId,
    declared: &[ToolRequirement],
    evidence: &[ToolEvidence],
) -> Result<(), ToolchainError> {
    let mut names = BTreeSet::new();
    for item in evidence {
        if !names.insert(item.name()) {
            return Err(ToolchainError::InvalidToolEvidence {
                gate,
                evidence: format!("duplicate {} evidence", item.name()),
            });
        }
        let Some(requirement) = declared.iter().find(|tool| tool.name == item.name()) else {
            return Err(ToolchainError::InvalidToolEvidence {
                gate,
                evidence: format!("undeclared {} evidence", item.name()),
            });
        };
        if item.requested_program() != requirement.program {
            return Err(ToolchainError::InvalidToolEvidence {
                gate,
                evidence: format!("{} used an undeclared program", item.name()),
            });
        }
        if let ToolEvidence::Identified {
            resolved_program,
            version,
            digest,
            ..
        } = item
            && (resolved_program.is_empty() || version.is_empty() || digest.is_empty())
        {
            return Err(ToolchainError::InvalidToolEvidence {
                gate,
                evidence: format!("{} has incomplete identity evidence", item.name()),
            });
        }
    }
    Ok(())
}

fn declared_tool_position(declared: &[ToolRequirement], evidence: &ToolEvidence) -> usize {
    declared
        .iter()
        .position(|tool| tool.name == evidence.name())
        .expect("unknown tools were refused before ordering")
}

fn validate_external_tool_outcome(
    gate: GateId,
    declared: &[ToolRequirement],
    outcome: &ArmOutcome,
    evidence: &[ToolEvidence],
) -> Result<(), ToolchainError> {
    match outcome {
        ArmOutcome::NotApplicable | ArmOutcome::Excluded => Ok(()),
        ArmOutcome::ToolUnavailable => {
            require_all_declared(gate, declared, evidence)?;
            if evidence
                .iter()
                .any(|item| matches!(item, ToolEvidence::Missing { .. }))
            {
                Ok(())
            } else {
                Err(ToolchainError::InvalidToolEvidence {
                    gate,
                    evidence: "tool-unavailable result has no missing program".to_owned(),
                })
            }
        }
        ArmOutcome::UnsupportedVersion => {
            require_all_identified(gate, declared, evidence)?;
            if evidence
                .iter()
                .zip(declared)
                .any(|(item, tool)| identified_version(item) != Some(tool.exact_version))
            {
                Ok(())
            } else {
                Err(ToolchainError::InvalidToolEvidence {
                    gate,
                    evidence: "unsupported-version result matched every exact version".to_owned(),
                })
            }
        }
        ArmOutcome::Passed
        | ArmOutcome::Findings
        | ArmOutcome::Timeout
        | ArmOutcome::ParseFailure
        | ArmOutcome::NonzeroAnalysis => {
            require_all_identified(gate, declared, evidence)?;
            if evidence
                .iter()
                .zip(declared)
                .all(|(item, tool)| identified_version(item) == Some(tool.exact_version))
            {
                Ok(())
            } else {
                Err(ToolchainError::InvalidToolEvidence {
                    gate,
                    evidence: "operational result did not use every exact tool version".to_owned(),
                })
            }
        }
    }
}

fn require_all_declared(
    gate: GateId,
    declared: &[ToolRequirement],
    evidence: &[ToolEvidence],
) -> Result<(), ToolchainError> {
    if evidence.len() == declared.len() {
        Ok(())
    } else {
        Err(ToolchainError::InvalidToolEvidence {
            gate,
            evidence: "one evidence row is required for every declared tool".to_owned(),
        })
    }
}

fn require_all_identified(
    gate: GateId,
    declared: &[ToolRequirement],
    evidence: &[ToolEvidence],
) -> Result<(), ToolchainError> {
    require_all_declared(gate, declared, evidence)?;
    if evidence
        .iter()
        .all(|item| matches!(item, ToolEvidence::Identified { .. }))
    {
        Ok(())
    } else {
        Err(ToolchainError::InvalidToolEvidence {
            gate,
            evidence: "identified evidence is required for every declared tool".to_owned(),
        })
    }
}

fn identified_version(evidence: &ToolEvidence) -> Option<&str> {
    match evidence {
        ToolEvidence::Identified { version, .. } => Some(version),
        ToolEvidence::Missing { .. } => None,
    }
}

/// Normalized result retained from one supported predecessor scanner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PredecessorResult {
    /// Predecessor scanner completed cleanly.
    Passed,
    /// Predecessor scanner emitted normalized structured findings.
    Findings(Vec<ArmFinding>),
}

/// One normalized release-owned predecessor observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PredecessorObservation {
    /// Exact 0.31.0 arm name, including `serve-web-typecheck`.
    pub gate_name: String,
    /// Normalized retained result.
    pub result: PredecessorResult,
}

/// Exact refusal and recovery for a predecessor observation without parity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationRefusal {
    /// Release that owned the incompatible behavior.
    pub release: &'static str,
    /// Current stable identity that supersedes the old arm.
    pub gate_id: GateId,
    /// Exact predecessor configuration field.
    pub source_field: String,
    /// Why old evidence cannot be interpreted as current evidence.
    pub reason: &'static str,
    /// Exact current recovery action.
    pub recovery: &'static str,
}

/// Result of comparing one predecessor observation with a current measurement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PredecessorComparison {
    /// Normalized scanner result is identical.
    Equivalent,
    /// Incompatible execution semantics are retained as an explicit boundary.
    Refused(MigrationRefusal),
}

/// Why a supported predecessor comparison was malformed or unequal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PredecessorComparisonError {
    /// Name was not one of the eight release-owned predecessor arms.
    UnknownGate(String),
    /// Current measurement named a different stable gate.
    GateMismatch { expected: GateId, actual: GateId },
    /// Supported pure-scanner results differed.
    ResultMismatch(GateId),
}

impl fmt::Display for PredecessorComparisonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownGate(gate) => write!(
                formatter,
                "release {PREDECESSOR_GATE_RELEASE} has no toolchain arm {gate:?}"
            ),
            Self::GateMismatch { expected, actual } => write!(
                formatter,
                "predecessor {} observation was compared with current {}",
                expected.as_str(),
                actual.as_str()
            ),
            Self::ResultMismatch(gate) => {
                write!(
                    formatter,
                    "{} predecessor/current results differ",
                    gate.as_str()
                )
            }
        }
    }
}

impl Error for PredecessorComparisonError {}

/// Compare a supported normalized scanner observation or return the exact
/// release boundary for an incompatible predecessor executor.
///
/// # Errors
///
/// Returns an unknown name, gate mismatch, or unequal supported result.
pub fn compare_predecessor(
    predecessor: &PredecessorObservation,
    current: &ArmMeasurement,
) -> Result<PredecessorComparison, PredecessorComparisonError> {
    let Some(gate) = predecessor_gate(&predecessor.gate_name) else {
        return Err(PredecessorComparisonError::UnknownGate(
            predecessor.gate_name.clone(),
        ));
    };
    if current.gate_id != gate {
        return Err(PredecessorComparisonError::GateMismatch {
            expected: gate,
            actual: current.gate_id,
        });
    }
    if let Some(refusal) = predecessor_refusal(gate) {
        return Ok(PredecessorComparison::Refused(refusal));
    }

    let equivalent = match (&predecessor.result, current.outcome) {
        (PredecessorResult::Passed, ArmOutcome::Passed) => true,
        (PredecessorResult::Findings(findings), ArmOutcome::Findings) => {
            let mut normalized = findings.clone();
            normalized.sort();
            normalized == current.findings
        }
        _ => false,
    };
    if equivalent {
        Ok(PredecessorComparison::Equivalent)
    } else {
        Err(PredecessorComparisonError::ResultMismatch(gate))
    }
}

fn predecessor_gate(name: &str) -> Option<GateId> {
    match name {
        "formatters" => Some(GateId::Formatters),
        "local-paths" => Some(GateId::LocalPaths),
        "taste" => Some(GateId::Taste),
        "serve-web-typecheck" => Some(GateId::CanvasWebTypecheck),
        "javascript-unused" => Some(GateId::JavascriptUnused),
        "python-typecheck" => Some(GateId::PythonTypecheck),
        "env-policy" => Some(GateId::EnvPolicy),
        "env-name-ledger" => Some(GateId::EnvNameLedger),
        _ => None,
    }
}

fn predecessor_refusal(gate: GateId) -> Option<MigrationRefusal> {
    let (name, reason, recovery) = match gate {
        GateId::Formatters => (
            "formatters",
            "0.31.0 mutated and re-staged Python files through a range-selected ruff",
            concat!(
                "rerun formatters with cargo 1.97.1 and rustfmt 1.9.0-stable ",
                "under the current declaration"
            ),
        ),
        GateId::CanvasWebTypecheck => (
            "serve-web-typecheck",
            "0.31.0 checked removed serve JavaScript with an unpinned npm TypeScript package",
            "rerun canvas-web-typecheck against app/canvas with cargo 1.97.1 and trunk 0.21.14",
        ),
        GateId::PythonTypecheck => (
            "python-typecheck",
            "0.31.0 selected pyright or uvx dynamically and admitted a version range",
            "install pyright 1.1.411 and rerun python-typecheck through its sole current program",
        ),
        _ => return None,
    };
    Some(MigrationRefusal {
        release: PREDECESSOR_GATE_RELEASE,
        gate_id: gate,
        source_field: format!("policy.pre_commit_builtins.{name}"),
        reason,
        recovery,
    })
}

#[cfg(test)]
include!("toolchain/fixtures.rs");
