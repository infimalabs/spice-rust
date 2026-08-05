//! Compiled structural metric arms and their complete evidence vocabulary.
//!
//! The predecessor spread file, routine, document, magic-number, and Markdown
//! measurements across Python callables. This module declares the sole current
//! metric identities and parsers. Every selected scope produces an explicit
//! [`MetricEvidence`] state; a missing parser, tool, file class, or policy
//! context is therefore evidence and never an omitted row or invented zero.

#[cfg(not(target_arch = "wasm32"))]
mod acquire;
mod declaration;
mod evidence;
mod parity;
mod parser;
mod policy_binding;
mod state_binding;

#[cfg(not(target_arch = "wasm32"))]
pub use acquire::{MetricAcquisitionError, MetricSource, acquire_metrics};
pub use declaration::{
    ArmDeclaration, KeyShape, MetricDeclaration, MetricId, MetricUnit, ParserDeclaration, ParserId,
    ParserMode, STRUCTURAL_ARMS, STRUCTURAL_METRICS, ScopeDeclaration, ToolRequirement,
};
pub use evidence::{
    EvidenceError, EvidenceState, FunctionKey, LatchBinding, MetricEvidence, MetricKey,
    MetricMeasurement, MetricReport, ParserEvidence, RuleBinding, STRUCTURAL_METRIC_REPORT_VERSION,
    StructuralArmEvidence,
};
pub use parity::{
    MetricParity, ParityError, PredecessorMetricObservation, PredecessorMetricResult,
    compare_predecessor_metric,
};
pub use parser::{
    ComplexityRecord, FileShapeMeasurement, MagicFinding, MarkdownLinkFinding, ParseError,
    RepositoryDocumentMeasurement, measure_file_shape, measure_repository_document,
    parse_lizard_csv, scan_magic_regressions, scan_markdown_link_case,
};
pub use policy_binding::{
    PolicyBindingError, ResolvedMagicMetric, ResolvedMetricRule, resolve_magic_metric,
    resolve_metric_rule,
};
pub use state_binding::{MetricStateBinding, StateBindingError};
