use serde::{Deserialize, Serialize};

use crate::manifest::GateId;

/// Stable identity of one structural quantity or finding population.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MetricId {
    FileLines,
    FileBytes,
    RoutineCcn,
    RoutineLength,
    RepositoryDocumentCharacters,
    MagicNumberRegressions,
    MarkdownLinkCaseMismatches,
}

impl MetricId {
    /// Every structural metric in deterministic declaration order.
    pub const ALL: [Self; 7] = [
        Self::FileLines,
        Self::FileBytes,
        Self::RoutineCcn,
        Self::RoutineLength,
        Self::RepositoryDocumentCharacters,
        Self::MagicNumberRegressions,
        Self::MarkdownLinkCaseMismatches,
    ];

    /// Stable current identity spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FileLines => "file.lines",
            Self::FileBytes => "file.bytes",
            Self::RoutineCcn => "routine.ccn",
            Self::RoutineLength => "routine.length",
            Self::RepositoryDocumentCharacters => "repository-document.characters",
            Self::MagicNumberRegressions => "magic-number.regressions",
            Self::MarkdownLinkCaseMismatches => "markdown-link.case-mismatches",
        }
    }
}

/// Canonical unit attached to one metric identity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MetricUnit {
    SourceLines,
    Bytes,
    CyclomaticComplexity,
    RoutineLines,
    UnicodeScalarValues,
    Regressions,
    CaseMismatches,
}

impl MetricUnit {
    /// Stable unit spelling retained in reports.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceLines => "source-lines",
            Self::Bytes => "bytes",
            Self::CyclomaticComplexity => "cyclomatic-complexity",
            Self::RoutineLines => "routine-lines",
            Self::UnicodeScalarValues => "unicode-scalar-values",
            Self::Regressions => "regressions",
            Self::CaseMismatches => "case-mismatches",
        }
    }
}

/// Shape of the durable state and report key used by a metric.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeyShape {
    Path,
    PathFunction,
}

/// Stable compiled parser identity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParserId {
    Utf8FileShapeV1,
    LizardCsvV1,
    Utf8DocumentCharactersV1,
    CGrammarMagicDeltaV1,
    MarkdownInlineLinkCaseV1,
}

impl ParserId {
    /// Stable parser identity spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Utf8FileShapeV1 => "utf8-file-shape-v1",
            Self::LizardCsvV1 => "lizard-csv-v1",
            Self::Utf8DocumentCharactersV1 => "utf8-document-characters-v1",
            Self::CGrammarMagicDeltaV1 => "c-grammar-magic-delta-v1",
            Self::MarkdownInlineLinkCaseV1 => "markdown-inline-link-case-v1",
        }
    }
}

/// Whether parsing is in-process or consumes one bounded external tool result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParserMode {
    Native,
    External,
}

/// Sole admitted executable contract for one external parser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolRequirement {
    name: &'static str,
    program: &'static str,
    version_arguments: &'static [&'static str],
    exact_version: &'static str,
    analysis_arguments: &'static [&'static str],
}

impl ToolRequirement {
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }

    #[must_use]
    pub const fn program(self) -> &'static str {
        self.program
    }

    #[must_use]
    pub const fn version_arguments(self) -> &'static [&'static str] {
        self.version_arguments
    }

    #[must_use]
    pub const fn exact_version(self) -> &'static str {
        self.exact_version
    }

    #[must_use]
    pub const fn analysis_arguments(self) -> &'static [&'static str] {
        self.analysis_arguments
    }
}

/// One parser declaration fixed at compile time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParserDeclaration {
    id: ParserId,
    mode: ParserMode,
    input: &'static str,
    tool: Option<ToolRequirement>,
}

impl ParserDeclaration {
    #[must_use]
    pub const fn id(self) -> ParserId {
        self.id
    }

    #[must_use]
    pub const fn mode(self) -> ParserMode {
        self.mode
    }

    #[must_use]
    pub const fn input(self) -> &'static str {
        self.input
    }

    #[must_use]
    pub const fn tool(self) -> Option<ToolRequirement> {
        self.tool
    }
}

/// Complete selected-tree scope of one structural arm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScopeDeclaration {
    inputs: &'static [&'static str],
    exclusions: &'static [&'static str],
    applicability: &'static str,
}

impl ScopeDeclaration {
    #[must_use]
    pub const fn inputs(self) -> &'static [&'static str] {
        self.inputs
    }

    #[must_use]
    pub const fn exclusions(self) -> &'static [&'static str] {
        self.exclusions
    }

    #[must_use]
    pub const fn applicability(self) -> &'static str {
        self.applicability
    }
}

/// One compiled metric and the policy/state inputs it consumes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MetricDeclaration {
    id: MetricId,
    owner: GateId,
    unit: MetricUnit,
    key: KeyShape,
    parser: ParserId,
    policy_field: &'static str,
    latch_kind: Option<&'static str>,
}

impl MetricDeclaration {
    #[must_use]
    pub const fn id(self) -> MetricId {
        self.id
    }

    #[must_use]
    pub const fn owner(self) -> GateId {
        self.owner
    }

    #[must_use]
    pub const fn unit(self) -> MetricUnit {
        self.unit
    }

    #[must_use]
    pub const fn key(self) -> KeyShape {
        self.key
    }

    #[must_use]
    pub const fn parser(self) -> ParserId {
        self.parser
    }

    #[must_use]
    pub const fn policy_field(self) -> &'static str {
        self.policy_field
    }

    #[must_use]
    pub const fn latch_kind(self) -> Option<&'static str> {
        self.latch_kind
    }
}

/// One of the four structural arms in fixed predecessor-relative order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArmDeclaration {
    id: GateId,
    parser: ParserDeclaration,
    metrics: &'static [MetricId],
    scope: ScopeDeclaration,
}

impl ArmDeclaration {
    #[must_use]
    pub const fn id(self) -> GateId {
        self.id
    }

    #[must_use]
    pub const fn parser(self) -> ParserDeclaration {
        self.parser
    }

    #[must_use]
    pub const fn metrics(self) -> &'static [MetricId] {
        self.metrics
    }

    #[must_use]
    pub const fn scope(self) -> ScopeDeclaration {
        self.scope
    }
}

const LIZARD: ToolRequirement = ToolRequirement {
    name: "lizard",
    program: "lizard",
    version_arguments: &["--version"],
    exact_version: "1.23.0",
    analysis_arguments: &["--csv"],
};

const FILE_SHAPE_METRICS: &[MetricId] = &[MetricId::FileLines, MetricId::FileBytes];
const COMPLEXITY_METRICS: &[MetricId] = &[MetricId::RoutineCcn, MetricId::RoutineLength];
const MAGIC_METRICS: &[MetricId] = &[MetricId::MagicNumberRegressions];
const MARKDOWN_METRICS: &[MetricId] = &[MetricId::MarkdownLinkCaseMismatches];

/// All quantities declared by this slice, including the document-character
/// metric consumed by the earlier `repo-docs` arm.
pub const STRUCTURAL_METRICS: [MetricDeclaration; 7] = [
    metric(
        MetricId::FileLines,
        GateId::FileShape,
        MetricUnit::SourceLines,
        KeyShape::Path,
        ParserId::Utf8FileShapeV1,
        "file_loc",
        Some("file_loc"),
    ),
    metric(
        MetricId::FileBytes,
        GateId::FileShape,
        MetricUnit::Bytes,
        KeyShape::Path,
        ParserId::Utf8FileShapeV1,
        "file_bytes",
        Some("file_bytes"),
    ),
    metric(
        MetricId::RoutineCcn,
        GateId::Complexity,
        MetricUnit::CyclomaticComplexity,
        KeyShape::PathFunction,
        ParserId::LizardCsvV1,
        "routine_ccn",
        Some("complexity_ccn"),
    ),
    metric(
        MetricId::RoutineLength,
        GateId::Complexity,
        MetricUnit::RoutineLines,
        KeyShape::PathFunction,
        ParserId::LizardCsvV1,
        "routine_length",
        Some("complexity_length"),
    ),
    metric(
        MetricId::RepositoryDocumentCharacters,
        GateId::RepoDocs,
        MetricUnit::UnicodeScalarValues,
        KeyShape::Path,
        ParserId::Utf8DocumentCharactersV1,
        "repo_truth_doc_chars",
        Some("repo_doc_chars"),
    ),
    metric(
        MetricId::MagicNumberRegressions,
        GateId::MagicNumbers,
        MetricUnit::Regressions,
        KeyShape::Path,
        ParserId::CGrammarMagicDeltaV1,
        "magic.examine_threshold",
        None,
    ),
    metric(
        MetricId::MarkdownLinkCaseMismatches,
        GateId::MarkdownLinks,
        MetricUnit::CaseMismatches,
        KeyShape::Path,
        ParserId::MarkdownInlineLinkCaseV1,
        "tracked markdown inventory",
        None,
    ),
];

const fn metric(
    id: MetricId,
    owner: GateId,
    unit: MetricUnit,
    key: KeyShape,
    parser: ParserId,
    policy_field: &'static str,
    latch_kind: Option<&'static str>,
) -> MetricDeclaration {
    MetricDeclaration {
        id,
        owner,
        unit,
        key,
        parser,
        policy_field,
        latch_kind,
    }
}

/// The four named arms in the exact interval frozen by [`GateId::ALL`].
pub const STRUCTURAL_ARMS: [ArmDeclaration; 4] = [
    ArmDeclaration {
        id: GateId::FileShape,
        parser: parser(
            ParserId::Utf8FileShapeV1,
            ParserMode::Native,
            "selected source-file bytes",
            None,
        ),
        metrics: FILE_SHAPE_METRICS,
        scope: scope(
            &["staged paths selected by policy.file_shape.source_suffixes"],
            &[
                "generated source patterns",
                "generated lockfiles",
                "repository-truth documents",
                "binary or invalid UTF-8 files",
            ],
            "one selected regular source file",
        ),
    },
    ArmDeclaration {
        id: GateId::Complexity,
        parser: parser(
            ParserId::LizardCsvV1,
            ParserMode::External,
            "bounded lizard --csv stdout",
            Some(LIZARD),
        ),
        metrics: COMPLEXITY_METRICS,
        scope: scope(
            &["staged paths selected by policy.languages.complexity"],
            &["excluded, absent, or unsupported-suffix paths"],
            "one selected complexity source file",
        ),
    },
    ArmDeclaration {
        id: GateId::MagicNumbers,
        parser: parser(
            ParserId::CGrammarMagicDeltaV1,
            ParserMode::Native,
            "current and baseline UTF-8 source text",
            None,
        ),
        metrics: MAGIC_METRICS,
        scope: scope(
            &["staged paths selected by policy.languages.magic"],
            &[
                "excluded paths",
                "source classes without an exact current parser",
            ],
            "one supported staged source file and its baseline blob state",
        ),
    },
    ArmDeclaration {
        id: GateId::MarkdownLinks,
        parser: parser(
            ParserId::MarkdownInlineLinkCaseV1,
            ParserMode::Native,
            "tracked-path inventory and tracked Markdown text",
            None,
        ),
        metrics: MARKDOWN_METRICS,
        scope: scope(
            &["tracked .md paths after staged rename/delete projection"],
            &[
                "external URLs",
                "anchors",
                "absolute paths",
                "targets outside the tree",
            ],
            "one tracked Markdown source and a complete tracked-path inventory",
        ),
    },
];

pub(crate) const fn metric_parser_declaration(metric: MetricId) -> ParserDeclaration {
    match metric {
        MetricId::FileLines | MetricId::FileBytes => STRUCTURAL_ARMS[0].parser(),
        MetricId::RoutineCcn | MetricId::RoutineLength => STRUCTURAL_ARMS[1].parser(),
        MetricId::RepositoryDocumentCharacters => parser(
            ParserId::Utf8DocumentCharactersV1,
            ParserMode::Native,
            "repository-truth document bytes",
            None,
        ),
        MetricId::MagicNumberRegressions => STRUCTURAL_ARMS[2].parser(),
        MetricId::MarkdownLinkCaseMismatches => STRUCTURAL_ARMS[3].parser(),
    }
}

const fn parser(
    id: ParserId,
    mode: ParserMode,
    input: &'static str,
    tool: Option<ToolRequirement>,
) -> ParserDeclaration {
    ParserDeclaration {
        id,
        mode,
        input,
        tool,
    }
}

const fn scope(
    inputs: &'static [&'static str],
    exclusions: &'static [&'static str],
    applicability: &'static str,
) -> ScopeDeclaration {
    ScopeDeclaration {
        inputs,
        exclusions,
        applicability,
    }
}
