use super::*;
use wire::rooms::{DesignNudge, DesignNudged};

/// Record the operator's nudge of one open Design at its witnessed heads.
///
/// The nudge is counted input: it readies a Design's first pass before its
/// maturity steeps, or a later pass once the previous one ended.
pub(super) fn nudge_design(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &DesignNudge,
) -> Outcome<CommandAnswer> {
    let design = match request_body.design.born() {
        Ok(design) => design,
        Err(error) => {
            return refusal(
                "preparation::design-invalid",
                Some(request_body.design.to_string()),
                &format!("name a Design handle with a birth stamp: {error}"),
            );
        }
    };
    let snapshot = match bus.task_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => return store_fault("preparation::heads-unreadable", &error.to_string()),
    };
    let heads = match task_heads(&snapshot, &design) {
        Ok(heads) => heads,
        Err(outcome) => return outcome,
    };
    for witness in [
        Witness::Task {
            task: design.clone(),
            head: heads.statement().position(),
        },
        Witness::Workflow {
            task: design.clone(),
            head: heads.workflow().position(),
            phase: heads.workflow().body().phase.clone(),
        },
    ] {
        if let Err(outcome) = require_witness(command, &witness) {
            return outcome;
        }
    }
    if !wire::intent::is_open_design(heads.statement().body().intent, heads.workflow().body()) {
        return refusal(
            "preparation::nudge-not-open-design",
            Some(request_body.design.to_string()),
            "nudge only an open Design; other work is scheduled by its own workflow",
        );
    }
    let text = format!("nudged {}", request_body.design);
    record(
        bus,
        command,
        request_body,
        &design,
        DesignNudged {
            design: design.clone(),
        },
        Vec::new(),
        text,
    )
}
