//! Semantic offline HTML; print pages consume the same named quantities.

use super::super::publication::{Case, Publication, Quantity};
use super::{Result, escape, number};
use std::path::Path;

mod editorial;

pub(super) fn document(publication: &Publication, public: &Path) -> Result<String> {
    let pages = editorial::pages(publication, public)?;
    let mut html = head("Making work land");
    html.push_str("<a class=\"skip\" href=\"#report\">Skip to report</a><nav aria-label=\"Report navigation\">");
    for (index, (title, _)) in pages.iter().enumerate() {
        html.push_str(&format!(
            "<a href=\"#page-{:02}\">{}</a>",
            index + 1,
            escape(title)
        ));
    }
    html.push_str("<a href=\"tables.html\">Complete data and evidence</a><a href=\"making-work-land.pdf\">Print PDF</a></nav><main id=\"report\">");
    for (index, (title, body)) in pages.into_iter().enumerate() {
        let page = index + 1;
        html.push_str(&format!("<article class=\"sheet\" id=\"page-{page:02}\" aria-label=\"Page {page}: {}\"><div class=\"running\"><span>MAKING WORK LAND</span><span>Frozen evidence · {}</span></div><div class=\"page-content\">{body}</div><div class=\"footer\"><span>Evidence-linked field guide · offline edition</span><span>{page:02}</span></div></article>", escape(title), publication.horizon_day));
    }
    html.push_str("</main></body></html>");
    Ok(html)
}

pub(super) fn tables(publication: &Publication) -> String {
    let mut html = head("Making work land — complete evidence alternatives");
    html.push_str("<a class=\"skip\" href=\"#evidence\">Skip to evidence</a><nav aria-label=\"Evidence navigation\"><a href=\"index.html\">Return to field guide</a><a href=\"#quantities\">Quantities</a><a href=\"#campaign\">Campaign nodes and edges</a><a href=\"#continuity\">Lane/session alternatives</a><a href=\"#tasks\">Task support</a><a href=\"data.json\" download>Structured data</a><a href=\"measurements.csv\" download>Measurement CSV</a><a href=\"tasks.csv\" download>Task CSV</a></nav><main class=\"appendix\" id=\"evidence\"><h1>Complete sanitized support</h1><p>These tables are the complete alternatives to the field guide's quantitative and structural figures. Original identities and precise instants belong only to the separately held private evidence snapshot. Report-local aliases reduce disclosure; they are not a guarantee of anonymity.</p><section id=\"quantities\"><h2>Named quantities</h2>");
    for quantity in &publication.quantities {
        html.push_str(&format!("<section id=\"{}\"><h3>{} · {}</h3><dl><dt>Population</dt><dd>{}</dd><dt>Definition</dt><dd>{}</dd><dt>Coverage</dt><dd>Denominator: {}; observed: {}; missing: {}; censored: {}. Unit: {}.</dd><dt>Exclusions</dt><dd>{}</dd></dl>{}</section>",
            quantity.id, quantity.id, escape(&quantity.name), escape(&quantity.population), escape(&quantity.definition), quantity.denominator, quantity.observed, quantity.missing,
            quantity.censored, escape(&quantity.unit), escape(&quantity.exclusions), table(quantity, None)));
    }
    html.push_str(
        "</section><section id=\"campaign\"><h2>Campaign node and edge alternatives</h2>",
    );
    html.push_str(&format!("<p>{}</p><p>Root: {}. All frontier quantities are in Q28; the displayed node limit never changes the delivery cohort.</p>", escape(&publication.campaign.relationship_source), publication.campaign.root));
    html.push_str("<div class=\"table-wrap\"><table><caption>Every displayed campaign node</caption><thead><tr><th scope=\"col\">Task</th><th scope=\"col\">Domain</th><th scope=\"col\">Selected delivery</th><th scope=\"col\">Recorded phase-author lanes</th></tr></thead><tbody>");
    for node in &publication.campaign.nodes {
        html.push_str(&format!(
            "<tr><th scope=\"row\">{}</th><td>{}</td><td>{}</td><td>{}</td></tr>",
            node.task,
            escape(&node.domain),
            node.selected,
            node.authors.join(", ")
        ));
    }
    html.push_str("</tbody></table><table><caption>Every displayed typed relationship</caption><thead><tr><th scope=\"col\">From</th><th scope=\"col\">To</th><th scope=\"col\">Layer</th></tr></thead><tbody>");
    for edge in &publication.campaign.edges {
        html.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
            edge.from,
            edge.to,
            escape(&edge.layer)
        ));
    }
    html.push_str("</tbody></table></div></section><section id=\"continuity\"><h2>Complete lane and provider-session alternatives</h2><p>Identity order is literal, not a ranking. Provider sessions follow declared lane/vendor/session identity; occupant changes are not new workers. Span endpoints are first and last retained observations, not guaranteed activation or termination times.</p><div class=\"table-wrap\"><table><caption>All lane-held exposure rows</caption><thead><tr><th scope=\"col\">Lane</th><th scope=\"col\">Held minutes</th><th scope=\"col\">Admissions</th><th scope=\"col\">Provider spans</th><th scope=\"col\">Compactions</th></tr></thead><tbody>");
    for lane in &publication.continuity.lanes {
        html.push_str(&format!("<tr><th scope=\"row\">{}</th><td class=\"numeric\">{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", lane.lane, number(lane.held_minutes), lane.admitted_episodes, lane.provider_sessions, lane.compactions));
    }
    html.push_str("</tbody></table><table><caption>All observed provider-session spans</caption><thead><tr><th scope=\"col\">Session</th><th scope=\"col\">Lane</th><th scope=\"col\">First day</th><th scope=\"col\">Last day</th><th scope=\"col\">Configurations</th><th scope=\"col\">Declarations</th><th scope=\"col\">Compactions</th><th scope=\"col\">Starts / exits</th></tr></thead><tbody>");
    for span in &publication.continuity.sessions {
        html.push_str(&format!("<tr><th scope=\"row\">{}</th><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{} / {}</td></tr>", span.id, span.lane, span.first_day, span.last_day, span.configurations.join(", "), span.declarations, span.compactions, span.starts, span.exits));
    }
    html.push_str(
        "</tbody></table></div></section><section id=\"cases\"><h2>Curated ordered cases</h2>",
    );
    for case in &publication.cases {
        html.push_str(&format!(
            "<section id=\"{}\">{}</section>",
            case.id,
            case_html(case)
        ));
    }
    html.push_str("</section><section id=\"git\"><h2>Curated Git concordance</h2><p>Fact-recorded implementation and integrated boundaries are primary. Real merge parents and first-parent-to-second-parent ranges describe contribution topology; an unrelated final review head never substitutes for the implementation head.</p><div class=\"table-wrap\"><table><caption>Curated implementation-head joins</caption><thead><tr><th scope=\"col\">Evidence</th><th scope=\"col\">Head</th><th scope=\"col\">Integrated</th><th scope=\"col\">Parents</th><th scope=\"col\">Head is parent / ancestor</th><th scope=\"col\">Merge contribution commits</th></tr></thead><tbody>");
    for case in &publication.git.cases {
        html.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{} / {}</td><td>{}</td></tr>",
            case.evidence,
            case.implementation_head,
            case.integrated,
            case.merge_parents.join(", "),
            case.head_is_merge_parent,
            case.head_is_ancestor,
            case.contribution_commits.map_or_else(
                || "Not a two-parent merge".to_owned(),
                |count| count.to_string()
            )
        ));
    }
    html.push_str("</tbody></table></div></section><section id=\"provenance\"><h2>Provenance applicability rules</h2><p>These rules pin known transformations, not a general per-row migration audit. Pre-migration bytes and execution dates against individual rows are unavailable. Later-schema re-export is not assumed equivalent.</p><div class=\"table-wrap\"><table><caption>Consumed field classifications</caption><thead><tr><th scope=\"col\">Fields</th><th scope=\"col\">Source module</th><th scope=\"col\">Schema step</th><th scope=\"col\">Known transformation</th><th scope=\"col\">Evidence class</th></tr></thead><tbody>");
    for rule in &publication.provenance {
        html.push_str(&format!(
            "<tr><th scope=\"row\">{}</th><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            escape(&rule.fields),
            escape(&rule.source_module),
            escape(&rule.schema_step),
            escape(&rule.transformation),
            escape(&rule.evidence_class)
        ));
    }
    html.push_str("</tbody></table></div></section><section id=\"tasks\"><h2>Every admitted public task support row</h2><p>Selected flags distinguish delivery membership from historical and admission context. Filing lane, phase-author lanes and reviewer lanes are separate. Incomplete and deleted tasks do not become selected deliveries.</p><div class=\"table-wrap\"><table><caption>Complete sanitized task support</caption><thead><tr><th scope=\"col\">Task</th><th scope=\"col\">Domain</th><th scope=\"col\">Selected</th><th scope=\"col\">Admission / closure day</th><th scope=\"col\">Route</th><th scope=\"col\">Filing lane</th><th scope=\"col\">Phase authors</th><th scope=\"col\">Ordered verdicts / reviewers</th></tr></thead><tbody>");
    for task in &publication.tasks {
        html.push_str(&format!("<tr><th scope=\"row\">{}</th><td>{}</td><td>{}</td><td>{} / {}</td><td>{}</td><td>{}</td><td>{}</td><td>{} / {}</td></tr>", task.id, escape(&task.domain), task.selected, task.admitted_day, task.closed_day.as_deref().unwrap_or("open at horizon"), escape(&task.closure_route), task.filing_lane.as_deref().unwrap_or("non-lane/unclassified"), task.phase_authors.join(", "), task.review_verdicts.join(" → "), task.review_lanes.iter().map(|lane| lane.as_deref().unwrap_or("unclassified")).collect::<Vec<_>>().join(" → ")));
    }
    html.push_str("</tbody></table></div></section></main></body></html>");
    html
}

pub(super) fn q<'a>(publication: &'a Publication, id: &str) -> &'a Quantity {
    publication
        .quantities
        .iter()
        .find(|quantity| quantity.id == id)
        .expect("editorial quantity must be produced by its measurement module")
}
pub(super) fn value(publication: &Publication, id: &str, label: &str) -> String {
    number(
        q(publication, id)
            .values
            .iter()
            .find(|row| row.label == label)
            .expect("editorial quantity label must be produced by its measurement module")
            .value,
    )
}

pub(super) fn table(quantity: &Quantity, filter: Option<&str>) -> String {
    let mut html = format!(
        "<div class=\"table-wrap\"><table><caption>{} · {}</caption><thead><tr><th scope=\"col\">Observation</th><th scope=\"col\" class=\"numeric\">{}</th></tr></thead><tbody>",
        quantity.id,
        escape(&quantity.name),
        escape(&quantity.unit)
    );
    for row in quantity
        .values
        .iter()
        .filter(|row| filter.is_none_or(|filter| row.label.contains(filter)))
    {
        html.push_str(&format!(
            "<tr><th scope=\"row\">{}</th><td class=\"numeric\">{}</td></tr>",
            escape(&row.label),
            number(row.value)
        ));
    }
    html.push_str("</tbody></table></div>");
    html
}

pub(super) fn case_html(case: &Case) -> String {
    let mut html = format!(
        "<h3>{} · {}</h3><p>{}</p><ol class=\"steps\">",
        case.id,
        escape(&case.title),
        escape(&case.reading)
    );
    for step in &case.steps {
        html.push_str(&format!(
            "<li><strong>{}</strong> · {} <small>— {} · {}</small></li>",
            step.task,
            escape(&step.action),
            step.hour,
            step.lane.as_deref().unwrap_or("non-lane/unclassified")
        ));
    }
    html.push_str(&format!(
        "</ol><p class=\"limit\">{}</p>",
        escape(&case.limitation)
    ));
    html
}

fn head(title: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"description\" content=\"An evidence-linked field guide to work landing in a self-hosting appliance\"><title>{}</title><link rel=\"stylesheet\" href=\"assets/style.css\"></head><body>",
        escape(title)
    )
}
