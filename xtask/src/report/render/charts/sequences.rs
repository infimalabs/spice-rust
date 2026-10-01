//! Ordered observations and explicitly schematic mechanisms.

use super::{Publication, Quantity, Result, escape, figure, number, opening};
use std::path::Path;

pub(in super::super) fn correction(p: &Publication, public: &Path, name: &str) -> Result<String> {
    let case = p
        .cases
        .iter()
        .find(|case| case.id == "E01")
        .ok_or("correction case missing")?;
    let tasks = case
        .steps
        .iter()
        .map(|step| step.task.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut svg = opening(385.0, &case.title, name);
    for (index, task) in tasks.iter().enumerate() {
        let x = 275.0 + index as f64 * 215.0;
        svg.push_str(&format!("<text x=\"{x}\" y=\"22\" text-anchor=\"middle\">{task}</text><path d=\"M{x} 32V366\" stroke=\"#d6d2e4\" stroke-width=\"2\"/>"));
    }
    svg.push_str("<text x=\"0\" y=\"22\">Canonical sequence ↓</text>");
    let mut previous = None;
    for (index, step) in case.steps.iter().enumerate() {
        let column = tasks
            .iter()
            .position(|task| *task == step.task)
            .ok_or("case task missing")?;
        let x = 275.0 + column as f64 * 215.0;
        let y = 60.0 + index as f64 * 55.0;
        if let Some((px, py)) = previous {
            svg.push_str(&format!("<path d=\"M{px} {py}V{}H{x}V{y}\" stroke=\"#6554a4\" stroke-width=\"2\" fill=\"none\"/>",(py+y)/2.0));
        }
        svg.push_str(&format!("<text x=\"0\" y=\"{}\">{:02} · {}</text><text x=\"0\" y=\"{}\" style=\"font-size:11px\">{}</text><circle cx=\"{x}\" cy=\"{y}\" r=\"7\" fill=\"#6554a4\"/><text x=\"{}\" y=\"{}\">{}</text><text x=\"{}\" y=\"{}\" style=\"font-size:11px\">{}</text>",y-3.0,index+1,escape(&step.action),y+13.0,escape(&step.hour),x+14.0,y-3.0,escape(&step.action),x+14.0,y+13.0,step.lane.as_deref().unwrap_or("unattributed")));
        previous = Some((x, y));
    }
    svg.push_str("</svg>");
    figure(
        public,
        name,
        svg,
        "E01 · Vertical spacing encodes sequence, not elapsed duration. Hour-coarsened instants can be equal while canonical order remains distinct. Parent and correction have separate task columns. The command record proves the implementation-only request; the journal proves the resulting flow. <a href=\"tables.html#E01\">Complete ordered case</a>.",
    )
}

pub(in super::super) fn flows(q: &Quantity, public: &Path, name: &str) -> Result<String> {
    let mut svg = opening(225.0, &q.name, name);
    svg.push_str("<text x=\"0\" y=\"18\">Recorded correction flow · nodes show phase order</text><text x=\"645\" y=\"18\" text-anchor=\"end\">Tasks</text>");
    for (index, row) in q.values.iter().enumerate() {
        let y = 47.0 + index as f64 * 36.0;
        let phases = row
            .label
            .strip_prefix("Recorded ")
            .unwrap_or(&row.label)
            .split(" → ")
            .collect::<Vec<_>>();
        for (column, phase) in phases.iter().enumerate() {
            let x = 30.0 + column as f64 * 180.0;
            if column > 0 {
                svg.push_str(&format!(
                    "<path d=\"M{} {y}H{}\" stroke=\"#6554a4\" stroke-width=\"2\"/>",
                    x - 58.0,
                    x
                ));
            }
            svg.push_str(&format!("<rect x=\"{x}\" y=\"{}\" width=\"122\" height=\"26\" rx=\"3\" fill=\"#efecf7\" stroke=\"#6554a4\"/><text x=\"{}\" y=\"{}\" text-anchor=\"middle\">{}</text>",y-13.0,x+61.0,y+4.0,escape(phase)));
        }
        svg.push_str(&format!(
            "<text x=\"645\" y=\"{}\" text-anchor=\"end\">{}</text>",
            y + 4.0,
            number(row.value)
        ));
    }
    svg.push_str("</svg>");
    figure(
        public,
        name,
        svg,
        "Q16 · Counts of resulting recorded flows; the connectors are unweighted order, not proportional throughput. Flow-ending todo is not relabeled todo → review. Request operands are not generally recoverable from the resulting flow. <a href=\"tables.html#Q16\">Complete flow inventory</a>.",
    )
}

pub(in super::super) fn proof(public: &Path, name: &str) -> Result<String> {
    let mut svg = opening(
        260.0,
        "Selected identity, retained bytes and proof admission",
        name,
    );
    svg.push_str("<text x=\"0\" y=\"18\">Mechanism schematic · no measured volume or time</text>");
    for (index, (title, detail)) in [
        ("Selected identity", "Tree + semantic inputs"),
        ("Materialize", "Write only changed bytes"),
        ("Admit proof", "Seal + verify identity"),
    ]
    .into_iter()
    .enumerate()
    {
        let x = index as f64 * 224.0;
        svg.push_str(&format!("<rect x=\"{x}\" y=\"55\" width=\"210\" height=\"90\" fill=\"#efecf7\" stroke=\"#6554a4\"/><text x=\"{}\" y=\"90\">{title}</text><text x=\"{}\" y=\"115\" style=\"font-size:12px\">{detail}</text>",x+12.0,x+12.0));
        if index > 0 {
            svg.push_str(&format!(
                "<path d=\"M{} 100h14\" stroke=\"#6554a4\" stroke-width=\"2\"/>",
                x - 14.0
            ));
        }
    }
    svg.push_str("<path d=\"M330 145V180H560V145\" stroke=\"#6554a4\" stroke-width=\"2\" fill=\"none\"/><text x=\"440\" y=\"204\" text-anchor=\"middle\">Unchanged bytes preserve timestamps.</text><text x=\"440\" y=\"227\" text-anchor=\"middle\">They do not waive validation obligations.</text></svg>");
    figure(
        public,
        name,
        svg,
        "E03 · Schematic dependency, not an observed task path or performance estimate. Root admission, compiler jobs and test workers are separate resource bounds. Q30 reports only seven retained seal observations; it does not establish a reuse rate. <a href=\"tables.html#Q30\">Retained proof coverage</a>.",
    )
}

pub(in super::super) fn reviews(q: &Quantity, public: &Path, name: &str) -> Result<String> {
    let mut groups = std::collections::BTreeMap::<&str, [f64; 2]>::new();
    for row in &q.values {
        if let Some((window, verdict)) = row.label.rsplit_once(" / ") {
            let index = usize::from(verdict == "first finding");
            groups.entry(window).or_default()[index] += row.value;
        }
    }
    let mut groups = groups.into_iter().collect::<Vec<_>>();
    groups.sort_by_key(|(group, _)| {
        if group.starts_with("Before") {
            0
        } else if group.starts_with("After blocking") {
            1
        } else {
            2
        }
    });
    let mut svg = opening(265.0, &q.name, name);
    svg.push_str(
        "<text x=\"0\" y=\"18\">First clean / first finding · count within each window</text>",
    );
    for (index, (group, counts)) in groups.iter().enumerate() {
        let y = 40.0 + index as f64 * 64.0;
        let total = counts.iter().sum::<f64>();
        let clean_width = counts[0] / total.max(1.0) * 660.0;
        svg.push_str(&format!("<text x=\"0\" y=\"{y}\">{} · n={}</text><rect x=\"0\" y=\"{}\" width=\"{clean_width:.2}\" height=\"28\" fill=\"#2a78d6\"/><rect x=\"{clean_width:.2}\" y=\"{}\" width=\"{:.2}\" height=\"28\" fill=\"#c05629\"/><text x=\"{}\" y=\"{}\" text-anchor=\"middle\" style=\"fill:#fff\">{}</text><text x=\"{}\" y=\"{}\" text-anchor=\"middle\" style=\"fill:#fff\">{}</text>",escape(&super::short(group)),number(total),y+10.0,y+10.0,660.0-clean_width,clean_width/2.0,y+29.0,number(counts[0]),clean_width+(660.0-clean_width)/2.0,y+29.0,number(counts[1])));
    }
    svg.push_str("</svg>");
    figure(
        public,
        name,
        svg,
        "Q12 · Blue: first clean. Orange: first finding. Equal-width rows encode within-window shares, not volume or a treatment effect. The 131 selected deliveries with no recorded verdict are outside these rows, not failures. <a href=\"tables.html#Q12\">All counts and denominator rules</a>.",
    )
}
