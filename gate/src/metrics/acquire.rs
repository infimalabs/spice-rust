//! Acquisition of the complete four-arm structural metric family.
//!
//! The family is acquired in full or not at all, and "in full" means four arms
//! that each carry one explicit state per declared metric. An arm whose input
//! this build cannot reach reports [`EvidenceState::Unavailable`] naming the
//! exact missing input, because the alternative — omitting the arm, or refusing
//! the whole family — spends four declared measurements to say one thing about
//! whichever of them is least ready.

use std::path::Path;

use super::baseline::{BaselineCapture, MagicBaselines, selects};
use super::{
    ArmDeclaration, EvidenceError, EvidenceState, MetricEvidence, MetricId, MetricKey,
    MetricMeasurement, ParserEvidence, ParserId, ResolvedMagicMetric, STRUCTURAL_ARMS,
    StructuralArmEvidence, resolve_magic_metric, scan_magic_regressions, scan_markdown_link_case,
};
use crate::input::Contract;
use crate::jurisdiction::{Entry, Jurisdiction, Shape};
use crate::manifest::GateId;
use crate::policy::PolicyContext;
use crate::tree::SelectedTree;

/// One immutable selected tree and the path inventory the arms read from it.
///
/// The inventory is captured once and shared, rather than walked per arm: the
/// Markdown arm resolves link targets against the whole tracked set, so two
/// walks could disagree about what a link points at.
///
/// The committed baselines are borrowed rather than read here, because they are
/// the one input no selected tree holds: the magic delta compares the selection
/// against the live repository's `HEAD`, which the capture that materialized
/// this tree is the last thing allowed to observe.
#[derive(Debug)]
pub struct MetricSource<'a> {
    root: &'a Path,
    jurisdiction: Jurisdiction,
    contract: &'a Contract,
    baselines: &'a BaselineCapture,
}

impl<'a> MetricSource<'a> {
    /// Capture the tracked structural jurisdiction over one selected tree.
    ///
    /// # Errors
    ///
    /// Refuses a walk that cannot complete over the selected tree.
    pub fn capture(
        tree: &'a SelectedTree,
        contract: &'a Contract,
        baselines: &'a BaselineCapture,
    ) -> Result<Self, MetricAcquisitionError> {
        let jurisdiction = Jurisdiction::walk(tree.root(), contract.policy().ignore())
            .map_err(|error| MetricAcquisitionError::Source(format!("metric walk: {error}")))?;
        Ok(Self {
            root: tree.root(),
            jurisdiction,
            contract,
            baselines,
        })
    }

    /// Immutable selected-tree root every arm reads.
    #[must_use]
    pub const fn root(&self) -> &Path {
        self.root
    }

    /// Every regular file the walk reached, admitted or not, in path order.
    ///
    /// Refused paths belong here even though nothing measures them. The selected
    /// tree holds tracked content only, so this is the tracked inventory a link
    /// target resolves against, and a link pointing into an ignored directory
    /// still names a real tracked file whose case can be wrong.
    fn tracked(&self) -> Vec<String> {
        self.jurisdiction
            .entries()
            .iter()
            .filter(|entry| matches!(entry.shape(), Shape::File))
            .map(|entry| entry.path().to_owned())
            .collect()
    }

    /// Every regular file a metric may measure, in path order.
    fn measurable(&self) -> Vec<&str> {
        self.jurisdiction.measurable().map(Entry::path).collect()
    }
}

/// Acquire all four structural arms from one captured source.
///
/// # Errors
///
/// Returns the evidence refusal that prevented one complete four-arm vector. A
/// file that cannot be read or parsed is not one of these: it is that file's
/// own state, carried in the arm alongside the files that did measure.
pub fn acquire_metrics(
    source: &MetricSource<'_>,
) -> Result<Vec<StructuralArmEvidence>, MetricAcquisitionError> {
    let mut arms = Vec::with_capacity(STRUCTURAL_ARMS.len());
    for declaration in STRUCTURAL_ARMS {
        let metrics = match declaration.id() {
            GateId::MagicNumbers => magic_metrics(source, declaration)?,
            GateId::MarkdownLinks => markdown_metrics(source)?,
            _ => unavailable(declaration, unreached_input(declaration.id()))?,
        };
        arms.push(StructuralArmEvidence::new(declaration.id(), metrics)?);
    }
    Ok(arms)
}

/// The exact input an arm needs and this build cannot yet reach.
///
/// Each names one missing thing rather than a general incompleteness, so the
/// arm that becomes reachable first can be built without re-deriving why the
/// other two still are not.
const fn unreached_input(gate_id: GateId) -> &'static str {
    match gate_id {
        GateId::FileShape => concat!(
            "no per-path elastic bound reaches acquisition: the tracked policy document ",
            "selects this scope through file_shape.source_suffixes and the contract ",
            "resolves a bound for a measured path, but the captured source retains only a ",
            "root and a tracked inventory, so neither that policy handle nor the actor ",
            "seed its headroom varies with is in hand"
        ),
        GateId::Complexity => concat!(
            "no bounded lizard 1.23.0 invocation is available: this build resolves no ",
            "external tool contract for a metric parser, and every measured routine ",
            "would also need one elastic bound the battery input contract does not report"
        ),
        // These two arms measure; reaching either would mean the match above
        // stopped covering an arm it names.
        GateId::MagicNumbers => "the magic-number arm is acquired rather than reported unreached",
        GateId::MarkdownLinks => "the Markdown arm is acquired rather than reported unreached",
        _ => "this gate declares no structural arm",
    }
}

/// One whole-arm row per declared metric, each naming the same missing input.
///
/// The row count follows the declaration rather than a fixed one, because
/// `StructuralArmEvidence` requires every declared metric present, and the two
/// paired arms additionally require both of theirs to carry identical keys.
fn unavailable(
    declaration: ArmDeclaration,
    reason: &str,
) -> Result<Vec<MetricEvidence>, MetricAcquisitionError> {
    declaration
        .metrics()
        .iter()
        .map(|metric| {
            MetricEvidence::new(
                *metric,
                MetricKey::Arm,
                EvidenceState::Unavailable {
                    reason: reason.to_owned(),
                },
            )
            .map_err(MetricAcquisitionError::Evidence)
        })
        .collect()
}

/// Measure inline Markdown link case against the tracked inventory.
///
/// The file class is tested here rather than left to the scanner even though the
/// scanner tests it too: offering it every measurable path would turn each of
/// this repository's non-Markdown files into its own `NotApplicable` row. The
/// arm would still fold to the same state, because a measurement outranks a row
/// that had nothing to measure, but every one of those rows would be retained
/// and reported, burying the Markdown the arm is about.
fn markdown_metrics(
    source: &MetricSource<'_>,
) -> Result<Vec<MetricEvidence>, MetricAcquisitionError> {
    let tracked = source.tracked();
    let mut metrics = Vec::new();
    for path in source.measurable() {
        if !path.to_ascii_lowercase().ends_with(".md") {
            continue;
        }
        let state = markdown_state(source.root(), path, &tracked);
        let key = MetricKey::path(path).map_err(MetricAcquisitionError::Evidence)?;
        metrics.push(
            MetricEvidence::new(MetricId::MarkdownLinkCaseMismatches, key, state)
                .map_err(MetricAcquisitionError::Evidence)?,
        );
    }
    if metrics.is_empty() {
        return unavailable_markdown();
    }
    Ok(metrics)
}

/// The one row a tree with no admitted Markdown still owes its arm.
fn unavailable_markdown() -> Result<Vec<MetricEvidence>, MetricAcquisitionError> {
    let state = EvidenceState::NotApplicable {
        reason: "the selected tree admits no Markdown source".to_owned(),
    };
    let evidence = MetricEvidence::new(MetricId::MarkdownLinkCaseMismatches, MetricKey::Arm, state)
        .map_err(MetricAcquisitionError::Evidence)?;
    Ok(vec![evidence])
}

/// One Markdown source's own state: its mismatch count, or why it has none.
///
/// A file that cannot be read is reported as unavailable rather than skipped or
/// counted zero, because a zero here is indistinguishable from a clean file and
/// would let an unreadable source pass as one.
fn markdown_state(root: &Path, path: &str, tracked: &[String]) -> EvidenceState {
    let text = match std::fs::read_to_string(root.join(path)) {
        Ok(text) => text,
        Err(error) => {
            return EvidenceState::Unavailable {
                reason: format!("selected Markdown source could not be read: {error}"),
            };
        }
    };
    match scan_markdown_link_case(path, &text, tracked) {
        Ok(findings) => EvidenceState::measured(MetricMeasurement::new(
            findings.len() as u64,
            ParserEvidence::Native {
                parser: ParserId::MarkdownInlineLinkCaseV1,
            },
        )),
        Err(error) => error.into_evidence_state(),
    }
}

/// Measure new comparison-adjacent literals against each path's committed text.
///
/// The file class is tested here for the same reason the Markdown arm tests its
/// own: offering the scanner every measurable path would bury the arm under one
/// `NotApplicable` row per non-source file. What is deliberately *not* filtered
/// out is a declared class this parser cannot read exactly, because that path is
/// in scope and unmeasured, and the arm has to say so.
fn magic_metrics(
    source: &MetricSource<'_>,
    declaration: ArmDeclaration,
) -> Result<Vec<MetricEvidence>, MetricAcquisitionError> {
    let baselines = match source.baselines {
        BaselineCapture::Reached(baselines) => baselines,
        BaselineCapture::Unreachable(reason) => {
            return unavailable(
                declaration,
                &format!("committed magic-number baselines could not be read: {reason}"),
            );
        }
    };
    let suffixes = source
        .contract
        .rule_policy()
        .document()
        .file_classes()
        .magic();
    let mut metrics = Vec::new();
    for path in source.measurable() {
        if !selects(path, suffixes) {
            continue;
        }
        let state = magic_state(source, path, baselines);
        let key = MetricKey::path(path).map_err(MetricAcquisitionError::Evidence)?;
        metrics.push(
            MetricEvidence::new(MetricId::MagicNumberRegressions, key, state)
                .map_err(MetricAcquisitionError::Evidence)?,
        );
    }
    if metrics.is_empty() {
        return not_applicable_magic();
    }
    Ok(metrics)
}

/// The one row a tree with no declared magic source still owes its arm.
fn not_applicable_magic() -> Result<Vec<MetricEvidence>, MetricAcquisitionError> {
    let state = EvidenceState::NotApplicable {
        reason: "the selected tree admits no declared magic-number source class".to_owned(),
    };
    let evidence = MetricEvidence::new(MetricId::MagicNumberRegressions, MetricKey::Arm, state)
        .map_err(MetricAcquisitionError::Evidence)?;
    Ok(vec![evidence])
}

/// One source file's own state: its regression count, or why it has none.
///
/// A path `HEAD` does not carry is reported inapplicable rather than scanned
/// against nothing. A regression is a literal this file did not have before, so
/// a file that did not exist before has no delta to take: scanning it against an
/// empty baseline would count every literal it was born with, and the count
/// would be a property of the file's size rather than of any change to it. The
/// fold lets a measurement outrank inapplicability, so the paths that do have a
/// baseline still speak for the arm.
fn magic_state(source: &MetricSource<'_>, path: &str, baselines: &MagicBaselines) -> EvidenceState {
    let Some(baseline) = baselines.text(path) else {
        let reason = match baselines.commit() {
            Some(commit) => {
                format!("{path:?} is absent from {commit}, so it has no baseline to regress from")
            }
            None => format!("{path:?} has no baseline: this repository has no commit yet"),
        };
        return EvidenceState::NotApplicable { reason };
    };
    let resolved = match resolve_magic_metric(
        source.contract.rule_policy(),
        &PolicyContext::new().with_path(path),
    ) {
        Ok(resolved) => resolved,
        Err(error) => {
            return EvidenceState::Unavailable {
                reason: format!("magic threshold did not resolve for {path:?}: {error}"),
            };
        }
    };
    let current = match std::fs::read_to_string(source.root().join(path)) {
        Ok(text) => text,
        Err(error) => {
            return EvidenceState::Unavailable {
                reason: format!("selected magic-number source could not be read: {error}"),
            };
        }
    };
    measured_magic(path, &current, baseline, &resolved)
}

/// The delta itself, once both sides of the comparison are in hand.
fn measured_magic(
    path: &str,
    current: &str,
    baseline: &str,
    resolved: &ResolvedMagicMetric,
) -> EvidenceState {
    match scan_magic_regressions(path, current, Some(baseline), resolved.threshold()) {
        Ok(findings) => EvidenceState::measured(resolved.measurement(
            findings.len() as u64,
            ParserEvidence::Native {
                parser: ParserId::CGrammarMagicDeltaV1,
            },
        )),
        Err(error) => error.into_evidence_state(),
    }
}

/// Why one complete structural family could not be assembled.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetricAcquisitionError {
    /// The selected tree could not be walked.
    Source(String),
    /// Assembled evidence violated its compiled declaration.
    Evidence(EvidenceError),
}

impl From<EvidenceError> for MetricAcquisitionError {
    fn from(error: EvidenceError) -> Self {
        Self::Evidence(error)
    }
}

impl std::fmt::Display for MetricAcquisitionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source(reason) => formatter.write_str(reason),
            Self::Evidence(error) => write!(formatter, "structural metric evidence: {error}"),
        }
    }
}

impl std::error::Error for MetricAcquisitionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Source(_) => None,
            Self::Evidence(error) => Some(error),
        }
    }
}
