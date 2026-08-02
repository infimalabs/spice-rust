use std::fmt::Write as _;

use super::{
    CORE_REPORT_VERSION, ConfigInventoryEvidence, CoreArmInput, CoreArmReport, CoreEvidence,
    CoreInputs, CoreObservation, CoreReport, CoreStanding, MergeIntegrityEvidence,
    MergeTreeCompletion, PlanPhaseEvidence,
};

/// Evaluate one complete immutable core-family battery.
#[must_use]
pub fn evaluate_core(inputs: &CoreInputs) -> CoreReport {
    let arms = inputs.arms().iter().map(evaluate_arm).collect();
    CoreReport {
        version: CORE_REPORT_VERSION,
        tree: inputs.tree().clone(),
        arms,
    }
}

pub(super) fn evaluate_arm(input: &CoreArmInput) -> CoreArmReport {
    let (standing, diagnostic) = match input.observation() {
        CoreObservation::Measured { evidence } => {
            let (standing, diagnostic) = evaluate_evidence(evidence);
            (standing, diagnostic)
        }
        CoreObservation::Unavailable { reason } => (
            CoreStanding::Unavailable,
            format!("{} unavailable: {reason}", input.id()),
        ),
        CoreObservation::Unsupported { reason, recovery } => (
            CoreStanding::Unsupported,
            format!("{} unsupported: {reason}; recovery: {recovery}", input.id()),
        ),
        CoreObservation::Unmeasured { reason } => (
            CoreStanding::Unmeasured,
            format!("{} unmeasured: {reason}", input.id()),
        ),
        CoreObservation::TimedOut { milliseconds } => (
            CoreStanding::TimedOut,
            format!("{} timed out after {milliseconds}ms", input.id()),
        ),
        CoreObservation::Skipped { reason } => (
            CoreStanding::Skipped,
            format!("{} explicitly skipped: {reason}", input.id()),
        ),
    };
    CoreArmReport {
        id: input.id(),
        tree: input.tree().clone(),
        standing,
        provenance: input.provenance().clone(),
        diagnostic,
        observation: input.observation().clone(),
    }
}

fn evaluate_evidence(evidence: &CoreEvidence) -> (CoreStanding, String) {
    match evidence {
        CoreEvidence::MergeIntegrity(value) => evaluate_merge(value),
        CoreEvidence::PlanPhase(value) => evaluate_plan(value),
        CoreEvidence::RepoShape(value) => findings(
            "repo shape",
            "repo shape: ok",
            "repository shape violations",
            value.findings(),
        ),
        CoreEvidence::Staging(value) if value.paths().is_empty() => {
            passed("staging: no partially staged files")
        }
        CoreEvidence::Staging(value) => failed(format!(
            "partially staged files; stage the whole file or stash the rest:\n{}",
            value
                .paths()
                .iter()
                .map(|path| format!("  {path}"))
                .collect::<Vec<_>>()
                .join("\n")
        )),
        CoreEvidence::RepoDocs(value) => evaluate_docs(value.measurements()),
        CoreEvidence::ConfigKeyValidity(value) => findings(
            "config key validity",
            "config key validity: ok",
            "invalid layered configuration",
            value.findings(),
        ),
        CoreEvidence::ConfigFalseDisable(value) => {
            evaluate_inventory("false-disable governance", value)
        }
        CoreEvidence::ConfigTrackedTrust(value) => {
            evaluate_inventory("tracked-file trust governance", value)
        }
    }
}

fn evaluate_merge(evidence: &MergeIntegrityEvidence) -> (CoreStanding, String) {
    match evidence {
        MergeIntegrityEvidence::NoMerge => passed("merge integrity: no merge in progress"),
        MergeIntegrityEvidence::StagedContent {
            head_tree,
            staged_tree,
        } => passed(format!(
            "merge integrity: staged tree {staged_tree} differs from HEAD {head_tree}"
        )),
        MergeIntegrityEvidence::Compared {
            head_tree,
            expected_tree,
            completion: MergeTreeCompletion::Clean,
            conflicted_paths: _,
        } if expected_tree == head_tree => passed(format!(
            "merge integrity: merge-tree result equals HEAD {head_tree}"
        )),
        MergeIntegrityEvidence::Compared {
            head_tree,
            expected_tree,
            completion,
            conflicted_paths,
        } => {
            let mut diagnostic = format!(
                "empty merge refused: MERGE_HEAD exists, but the staged tree still equals HEAD \
                 ({head_tree}) while git merge-tree computes {expected_tree}. Committing now would \
                 discard integrated content."
            );
            match (*completion, conflicted_paths.as_slice()) {
                (MergeTreeCompletion::Clean, _) => diagnostic.push_str(
                    " Recover without removing MERGE_HEAD by installing the computed merge tree, \
                     verify staged content, then retry git commit.",
                ),
                (MergeTreeCompletion::Conflicted, []) => diagnostic.push_str(
                    " Cannot print a safe restart recipe because git merge-tree did not name the \
                     conflicted paths. Re-run the merge manually and resolve its conflicts before \
                     committing.",
                ),
                (MergeTreeCompletion::Conflicted, paths) => {
                    let _ = write!(
                        diagnostic,
                        " The discarded merge is conflicted at: {}. Restart the merge, resolve \
                         each conflict, stage each resolution, then retry git commit with \
                         MERGE_HEAD intact.",
                        paths.join(", ")
                    );
                }
            }
            failed(diagnostic)
        }
    }
}

fn evaluate_plan(evidence: &PlanPhaseEvidence) -> (CoreStanding, String) {
    match evidence {
        PlanPhaseEvidence::NoActiveClaim => passed("plan phase: no active claim"),
        PlanPhaseEvidence::Active { task, phase } if phase.as_str() == "plan" => failed(format!(
            "git commit refused: active claimed task {task} is still in plan phase"
        )),
        PlanPhaseEvidence::Active { task, phase } => passed(format!(
            "plan phase: active claimed task {task} is in {} phase",
            phase.as_str()
        )),
    }
}

fn findings(
    label: &str,
    pass_diagnostic: &str,
    failure_header: &str,
    values: &[super::CoreFinding],
) -> (CoreStanding, String) {
    if values.is_empty() {
        return passed(pass_diagnostic);
    }
    let mut diagnostic = failure_header.to_owned();
    for finding in values {
        let path = finding
            .path()
            .map_or_else(String::new, |path| format!(" {path}"));
        let _ = write!(
            diagnostic,
            "\n  {label}.{}{}: {}",
            finding.code(),
            path,
            finding.detail()
        );
    }
    failed(diagnostic)
}

fn evaluate_docs(values: &[super::DocMeasurement]) -> (CoreStanding, String) {
    let over: Vec<_> = values
        .iter()
        .filter_map(|measurement| {
            measurement
                .limit()
                .filter(|limit| measurement.characters() > *limit)
                .map(|limit| (measurement, limit))
        })
        .collect();
    if over.is_empty() {
        return passed(format!(
            "repo-truth docs: {} document measurements within bounds",
            values.len()
        ));
    }
    let mut diagnostic = if over.iter().all(|(measurement, _)| measurement.peer_claim()) {
        concat!(
            "repo-truth docs hit peer-held bound claims; keep this change append-only or move ",
            "to another seam:"
        )
        .to_owned()
    } else {
        "repo-truth docs exceed the character cap; tighten the doctrine:".to_owned()
    };
    for (measurement, limit) in over {
        let _ = write!(
            diagnostic,
            "\n  {}: {} characters (cap {})",
            measurement.path(),
            measurement.characters(),
            limit
        );
    }
    failed(diagnostic)
}

fn evaluate_inventory(label: &str, evidence: &ConfigInventoryEvidence) -> (CoreStanding, String) {
    let missing: Vec<_> = evidence
        .declared()
        .iter()
        .filter(|path| !evidence.observed().contains(path))
        .collect();
    let unexpected: Vec<_> = evidence
        .observed()
        .iter()
        .filter(|path| !evidence.declared().contains(path))
        .collect();
    if missing.is_empty() && unexpected.is_empty() && evidence.violations().is_empty() {
        return passed(format!("{label}: compiled inventory matches declarations"));
    }
    let render_paths = |paths: &[&Vec<String>]| {
        if paths.is_empty() {
            "<none>".to_owned()
        } else {
            paths
                .iter()
                .map(|path| path.join("."))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    let mut diagnostic = format!(
        "{label}: shared consumer inventory drift; missing={}; undeclared={}",
        render_paths(&missing),
        render_paths(&unexpected)
    );
    for violation in evidence.violations() {
        let _ = write!(diagnostic, "; {}: {}", violation.code(), violation.detail());
    }
    failed(diagnostic)
}

fn passed(diagnostic: impl Into<String>) -> (CoreStanding, String) {
    (CoreStanding::Passed, diagnostic.into())
}

fn failed(diagnostic: impl Into<String>) -> (CoreStanding, String) {
    (CoreStanding::Failed, diagnostic.into())
}
