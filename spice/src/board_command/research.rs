//! Current-question guidance, independent of allocation maturity and task AC.

use wire::rooms::{DesignCoverage, DesignItems};

#[cfg(test)]
mod tests;

pub(crate) fn design_coverage(
    intent: Option<wire::TaskIntent>,
    workflow: &wire::WorkflowHead,
    questions: &[String],
    items: Option<&DesignItems>,
) -> Option<DesignCoverage> {
    if !wire::intent::is_open_design(intent, workflow) {
        return None;
    }
    let quiet = DesignItems::default();
    Some(items.unwrap_or(&quiet).coverage(questions))
}

pub(crate) fn design_research(
    statement: &wire::TaskHead,
    workflow: &wire::WorkflowHead,
    items: Option<&DesignItems>,
) -> Option<String> {
    design_coverage(statement.intent, workflow, &statement.acceptance, items).map(|coverage| {
        let handle =
            wire::TaskHandle::for_filing(&statement.project, &statement.born, statement.filing);
        coverage.research_guidance(&statement.acceptance, &handle)
    })
}
