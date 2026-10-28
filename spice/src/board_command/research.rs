//! Current-question guidance, independent of allocation maturity and task AC.

use wire::rooms::{DesignCoverage, DesignItems};

pub(crate) fn design_coverage(
    statement: &wire::TaskHead,
    items: Option<&DesignItems>,
) -> Option<DesignCoverage> {
    if !statement.intent.is_some_and(wire::TaskIntent::is_design) {
        return None;
    }
    let quiet = DesignItems::default();
    Some(items.unwrap_or(&quiet).coverage(&statement.acceptance))
}

pub(crate) fn design_research(
    statement: &wire::TaskHead,
    items: Option<&DesignItems>,
) -> Option<String> {
    design_coverage(statement, items).map(|coverage| {
        let handle =
            wire::TaskHandle::for_filing(&statement.project, &statement.born, statement.filing);
        coverage.research_guidance(&statement.acceptance, &handle)
    })
}
