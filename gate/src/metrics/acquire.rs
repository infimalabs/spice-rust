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
    ArmDeclaration, ComplexityRoutine, EvidenceError, EvidenceState, MetricEvidence, MetricId,
    MetricKey, MetricMeasurement, MetricStateBinding, ParseError, ParserEvidence, ParserId,
    ResolvedMagicMetric, STRUCTURAL_ARMS, StructuralArmEvidence, measure_complexity,
    measure_file_shape, resolve_magic_metric, resolve_metric_rule, scan_magic_regressions,
    scan_markdown_link_case,
};
use crate::core::ActorCapture;
use crate::input::Contract;
use crate::jurisdiction::{Entry, Jurisdiction, Shape};
use crate::manifest::GateId;
use crate::policy::PolicyContext;
use crate::state::LatchEntry;
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
///
/// The acting actor and the durable latches are borrowed for the same reason,
/// and are what a bounded arm needs beyond bytes: an elastic limit varies with
/// the actor a path's headroom is seeded from, and collapses to the base limit
/// for a path already latched. Both come from the caller that reads the ledger
/// this run publishes as its state authority, so the limit the file-shape and
/// complexity arms judge against and the limit that ledger would latch are one
/// reading rather than two.
#[derive(Debug)]
pub struct MetricSource<'a> {
    root: &'a Path,
    jurisdiction: Jurisdiction,
    contract: &'a Contract,
    baselines: &'a BaselineCapture,
    actor: &'a ActorCapture,
    latches: &'a [LatchEntry],
}

impl<'a> MetricSource<'a> {
    /// Capture the tracked structural jurisdiction over one selected tree.
    ///
    /// # Errors
    ///
    /// Refuses a walk that cannot complete over the selected tree.
    // Every argument past the tree is an observation of the live repository or
    // the run's state authority that no selected tree can supply, so there is
    // nothing here to group into a narrower type that would not just be this
    // list under one name.
    pub fn capture(
        tree: &'a SelectedTree,
        contract: &'a Contract,
        baselines: &'a BaselineCapture,
        actor: &'a ActorCapture,
        latches: &'a [LatchEntry],
    ) -> Result<Self, MetricAcquisitionError> {
        let jurisdiction = Jurisdiction::walk(tree.root(), contract.policy().ignore())
            .map_err(|error| MetricAcquisitionError::Source(format!("metric walk: {error}")))?;
        Ok(Self {
            root: tree.root(),
            jurisdiction,
            contract,
            baselines,
            actor,
            latches,
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
            GateId::Complexity => complexity_metrics(source)?,
            GateId::FileShape => file_shape_metrics(source, declaration)?,
            GateId::MagicNumbers => magic_metrics(source, declaration)?,
            GateId::MarkdownLinks => markdown_metrics(source)?,
            _ => unavailable(declaration, unreached_input(declaration.id()))?,
        };
        arms.push(StructuralArmEvidence::new(declaration.id(), metrics)?);
    }
    Ok(arms)
}

/// The exact input an arm needs and this build cannot reach.
///
/// Every structural arm now acquires, so all four say that rather than naming a
/// missing input. Reaching one of them would mean the dispatch above stopped
/// covering an arm it names, and the row says so instead of reporting an
/// unavailability that has no cause. The last is for a gate declaring no
/// structural arm at all, which `STRUCTURAL_ARMS` holds none of.
const fn unreached_input(gate_id: GateId) -> &'static str {
    match gate_id {
        GateId::Complexity => "the complexity arm is acquired rather than reported unreached",
        GateId::FileShape => "the file-shape arm is acquired rather than reported unreached",
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
    declared_rows(
        declaration,
        &MetricKey::Arm,
        &EvidenceState::Unavailable {
            reason: reason.to_owned(),
        },
    )
}

/// Every metric an arm declares, carrying one shared state under one key.
///
/// Both callers need the same guarantee for different reasons: a whole-arm row
/// set has to cover the declaration, and a per-path row set has to cover it
/// with identical keys. Building both here is what makes a paired arm's
/// keysets equal by construction rather than by two call sites agreeing.
fn declared_rows(
    declaration: ArmDeclaration,
    key: &MetricKey,
    state: &EvidenceState,
) -> Result<Vec<MetricEvidence>, MetricAcquisitionError> {
    declaration
        .metrics()
        .iter()
        .map(|metric| {
            MetricEvidence::new(*metric, key.clone(), state.clone())
                .map_err(MetricAcquisitionError::Evidence)
        })
        .collect()
}

/// Measure every selected source file's shape against its own elastic bound.
///
/// The file class is tested here for the reason the other two acquiring arms
/// test theirs: a row per unselected path would bury the sources the arm is
/// about. What is additionally selected out is a repository-truth document,
/// even when its suffix is in the class. Those answer to a character bound on
/// the repository-document arm, and a document measured here as well would be
/// held to two limits at once and could fail its file-shape one while passing
/// the bound the predecessor actually holds it to.
fn file_shape_metrics(
    source: &MetricSource<'_>,
    declaration: ArmDeclaration,
) -> Result<Vec<MetricEvidence>, MetricAcquisitionError> {
    let policy = source.contract.rule_policy();
    let suffixes = policy.document().file_classes().file_shape();
    let mut metrics = Vec::new();
    for path in source.measurable() {
        if !selects(path, suffixes) {
            continue;
        }
        let document = policy.selects_repository_document(path).map_err(|error| {
            MetricAcquisitionError::Source(format!(
                "repository-document selection for {path:?}: {error}"
            ))
        })?;
        if document {
            continue;
        }
        let key = MetricKey::path(path).map_err(MetricAcquisitionError::Evidence)?;
        metrics.extend(file_shape_rows(source, declaration, path, &key)?);
    }
    if metrics.is_empty() {
        return declared_rows(
            declaration,
            &MetricKey::Arm,
            &EvidenceState::NotApplicable {
                reason: "the selected tree admits no declared source file class".to_owned(),
            },
        );
    }
    Ok(metrics)
}

/// One selected file's paired rows: its measured shape, or why it has none.
///
/// A file that cannot be read or decoded reports that once and both rows carry
/// it, because the two metrics answer for the same bytes: a lines row that
/// measured beside a bytes row that refused would claim this file was half
/// observed, and the arm would report a keyset its declaration forbids.
fn file_shape_rows(
    source: &MetricSource<'_>,
    declaration: ArmDeclaration,
    path: &str,
    key: &MetricKey,
) -> Result<Vec<MetricEvidence>, MetricAcquisitionError> {
    let bytes = match std::fs::read(source.root().join(path)) {
        Ok(bytes) => bytes,
        Err(error) => {
            return declared_rows(
                declaration,
                key,
                &EvidenceState::Unavailable {
                    reason: format!("selected source file could not be read: {error}"),
                },
            );
        }
    };
    let shape = match measure_file_shape(path, &bytes) {
        Ok(shape) => shape,
        Err(error) => return declared_rows(declaration, key, &error.into_evidence_state()),
    };
    [
        (MetricId::FileLines, shape.lines()),
        (MetricId::FileBytes, shape.bytes()),
    ]
    .into_iter()
    .map(|(metric, value)| bounded_row(source, path, key, metric, value))
    .collect()
}

/// One measured quantity's row, judged against the limit it was decided under.
///
/// A bound that cannot be resolved, bound, or decided is this row's own
/// unavailability rather than the family's refusal, matching what an unreadable
/// file gets: the paths whose bounds did resolve still speak for the arm, and
/// the arm still carries one explicit state per declared metric.
fn bounded_row(
    source: &MetricSource<'_>,
    path: &str,
    key: &MetricKey,
    metric: MetricId,
    value: u64,
) -> Result<MetricEvidence, MetricAcquisitionError> {
    match bounded_evidence(source, path, key, metric, value) {
        Ok(evidence) => Ok(evidence),
        Err(reason) => {
            MetricEvidence::new(metric, key.clone(), EvidenceState::Unavailable { reason })
                .map_err(MetricAcquisitionError::Evidence)
        }
    }
}

/// Resolve one path's rule, decide its bound, and carry both on the value.
///
/// The decision travels on the measurement rather than being compared here, so
/// an over-limit file fails through the latch binding every consumer already
/// reads. The latch is the ledger's, and a latched path is held to its base
/// limit: the elastic headroom a ratchet lends was already spent by the breach
/// that latched it.
fn bounded_evidence(
    source: &MetricSource<'_>,
    path: &str,
    key: &MetricKey,
    metric: MetricId,
    value: u64,
) -> Result<MetricEvidence, String> {
    let name = metric.as_str();
    let rule = resolve_metric_rule(
        source.contract.rule_policy(),
        metric,
        &PolicyContext::new().with_path(path),
    )
    .map_err(|error| format!("{name} bound did not resolve for {path:?}: {error}"))?;
    let binding = MetricStateBinding::new(
        metric,
        key.clone(),
        value,
        ParserEvidence::Native {
            parser: ParserId::Utf8FileShapeV1,
        },
        rule,
    )
    .map_err(|error| format!("{name} did not bind for {path:?}: {error}"))?;
    let actor_inputs = source
        .actor
        .actor_inputs(path)
        .map_err(|error| format!("actor provenance for selected source {path:?}: {error}"))?;
    let latched = source.latches.contains(binding.entry());
    let decision = binding
        .decide(latched, &actor_inputs)
        .map_err(|error| format!("active {name} bound for {path:?}: {error}"))?;
    binding
        .measured(&decision)
        .map_err(|error| format!("{name} evidence for {path:?}: {error}"))
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

/// Measure every routine in the selected complexity scope.
///
/// The two metrics are acquired together rather than in two passes because they
/// are read from one parse of one file: measuring them apart would let a routine
/// contribute its length to one metric and, after a re-parse, not exist for the
/// other. The arm requires both metrics to carry identical keys, and one parse
/// is what makes that true rather than checked.
///
/// A path whose class has no vendored grammar carries that refusal as its own
/// pair of rows, so a language the policy selects and this build cannot parse is
/// named instead of measuring as a file with no routines, which is what a silent
/// skip and a clean parse of an empty file are indistinguishable from.
fn complexity_metrics(
    source: &MetricSource<'_>,
) -> Result<Vec<MetricEvidence>, MetricAcquisitionError> {
    let suffixes = source
        .contract
        .rule_policy()
        .document()
        .file_classes()
        .complexity();
    let mut metrics = Vec::new();
    for path in source.measurable() {
        if !selects(path, suffixes) {
            continue;
        }
        complexity_rows(source, path, &mut metrics)?;
    }
    if metrics.is_empty() {
        return not_applicable_complexity();
    }
    Ok(metrics)
}

/// One selected file's rows: two per routine, or one pair naming its refusal.
fn complexity_rows(
    source: &MetricSource<'_>,
    path: &str,
    metrics: &mut Vec<MetricEvidence>,
) -> Result<(), MetricAcquisitionError> {
    let routines = match read_complexity(source, path) {
        Ok(routines) => routines,
        Err(state) => return whole_file_complexity(path, &state, metrics),
    };
    for routine in routines {
        for (metric, value) in [
            (MetricId::RoutineCcn, routine.ccn()),
            (MetricId::RoutineLength, routine.length()),
        ] {
            metrics.push(routine_evidence(
                source,
                path,
                routine.function(),
                metric,
                value,
            )?);
        }
    }
    Ok(())
}

/// Parse one selected file, or the state explaining why it was not measured.
fn read_complexity(
    source: &MetricSource<'_>,
    path: &str,
) -> Result<Vec<ComplexityRoutine>, EvidenceState> {
    let text = std::fs::read_to_string(source.root().join(path)).map_err(|error| {
        EvidenceState::Unavailable {
            reason: format!("selected complexity source could not be read: {error}"),
        }
    })?;
    measure_complexity(path, &text).map_err(ParseError::into_evidence_state)
}

/// One routine's bounded measurement, decided against its own elastic limit.
///
/// The bound is taken through [`MetricStateBinding`], the same route the durable
/// ledger reconciles a latch through, so the limit this arm judges against and
/// the limit that ledger would latch are one computation. A binding, key, or
/// bound failure becomes this routine's own state rather than the arm's, because
/// one unresolvable routine is not evidence about the rest of the tree.
fn routine_evidence(
    source: &MetricSource<'_>,
    path: &str,
    function: &str,
    metric: MetricId,
    value: u64,
) -> Result<MetricEvidence, MetricAcquisitionError> {
    let key = MetricKey::function(path, function).map_err(MetricAcquisitionError::Evidence)?;
    match bound_routine(source, path, key.clone(), metric, value) {
        Ok(evidence) => Ok(evidence),
        Err(reason) => MetricEvidence::new(metric, key, EvidenceState::Unavailable { reason })
            .map_err(MetricAcquisitionError::Evidence),
    }
}

/// Resolve, bind, and decide one routine's bound, or say what refused.
fn bound_routine(
    source: &MetricSource<'_>,
    path: &str,
    key: MetricKey,
    metric: MetricId,
    value: u64,
) -> Result<MetricEvidence, String> {
    let rule = resolve_metric_rule(
        source.contract.rule_policy(),
        metric,
        &PolicyContext::new().with_path(path),
    )
    .map_err(|error| {
        format!(
            "{} bound did not resolve for {path:?}: {error}",
            metric.as_str()
        )
    })?;
    let binding = MetricStateBinding::new(
        metric,
        key,
        value,
        ParserEvidence::Native {
            parser: ParserId::TreeSitterComplexityV1,
        },
        rule,
    )
    .map_err(|error| format!("{} did not bind for {path:?}: {error}", metric.as_str()))?;
    let actor_inputs = source
        .actor
        .actor_inputs(path)
        .map_err(|error| format!("actor provenance for {path:?}: {error}"))?;
    let latched = source.latches.contains(binding.entry());
    let decision = binding.decide(latched, &actor_inputs).map_err(|error| {
        format!(
            "{} bound did not decide for {path:?}: {error}",
            metric.as_str()
        )
    })?;
    binding
        .measured(&decision)
        .map_err(|error| format!("{} evidence for {path:?}: {error}", metric.as_str()))
}

/// A file the parser refused speaks once per metric, on one shared key.
///
/// The key names the file and no routine, because the refusal is about the file:
/// naming a routine would invent an identity the parse never produced. Both
/// metrics carry it, since the arm admits no row present in one and absent from
/// the other.
fn whole_file_complexity(
    path: &str,
    state: &EvidenceState,
    metrics: &mut Vec<MetricEvidence>,
) -> Result<(), MetricAcquisitionError> {
    let key =
        MetricKey::function(path, WHOLE_FILE_ROUTINE).map_err(MetricAcquisitionError::Evidence)?;
    for metric in [MetricId::RoutineCcn, MetricId::RoutineLength] {
        metrics.push(
            MetricEvidence::new(metric, key.clone(), state.clone())
                .map_err(MetricAcquisitionError::Evidence)?,
        );
    }
    Ok(())
}

/// The function position of a row that speaks for a file rather than a routine.
const WHOLE_FILE_ROUTINE: &str = "<file>";

/// The rows a tree with no parsable complexity source still owes its arm.
fn not_applicable_complexity() -> Result<Vec<MetricEvidence>, MetricAcquisitionError> {
    let mut metrics = Vec::new();
    for metric in [MetricId::RoutineCcn, MetricId::RoutineLength] {
        let state = EvidenceState::NotApplicable {
            reason: "the selected tree admits no declared complexity source class".to_owned(),
        };
        metrics.push(
            MetricEvidence::new(metric, MetricKey::Arm, state)
                .map_err(MetricAcquisitionError::Evidence)?,
        );
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
