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

pub(in super::super) fn review_braid(p: &Publication, public: &Path, name: &str) -> Result<String> {
    const SELECTED_TASK_COUNT: usize = 1_500;
    const RIGHT_LABEL_THRESHOLD: f64 = 550.0;
    const LABELS: [&str; 7] = [
        "No recorded verdict",
        "Clean",
        "Finding",
        "Finding → clean",
        "Finding → finding → clean",
        "3 findings → clean",
        "4 findings → clean",
    ];
    const FILLS: [&str; 7] = [
        "#c9ced1", "#2a78d6", "#c05629", "#8c7bc2", "#7965b2", "#6554a4", "#4f3c91",
    ];
    let mut counts = [0_usize; LABELS.len()];
    for task in p.tasks.iter().filter(|task| task.selected) {
        let verdicts = &task.review_verdicts;
        let category = match verdicts.as_slice() {
            [] => 0,
            [verdict] if verdict == "clean" => 1,
            [verdict] if verdict == "finding" => 2,
            _ if (2..=5).contains(&verdicts.len())
                && verdicts.last().is_some_and(|verdict| verdict == "clean")
                && verdicts[..verdicts.len() - 1]
                    .iter()
                    .all(|verdict| verdict == "finding") =>
            {
                verdicts.len() + 1
            }
            _ => return Err("selected task has an unsupported ordered verdict history".into()),
        };
        counts[category] += 1;
    }
    let total = counts.iter().sum::<usize>();
    if total != SELECTED_TASK_COUNT {
        return Err("ordered verdict braid does not cover the selected cohort".into());
    }
    let scale = 112.5 / total as f64;
    let round_x = [210.0, 300.0, 390.0, 480.0, 570.0];
    let mut svg = opening(250.0, "Every selected task's ordered review history", name);
    svg.push_str(
        "<text x=\"0\" y=\"17\">1,500 selected tasks · ribbon thickness is task count</text>",
    );
    for (round, x) in round_x.iter().enumerate() {
        svg.push_str(&format!(
            "<path d=\"M{x} 30V228\" stroke=\"#e3e5e7\"/><text x=\"{x}\" y=\"27\" text-anchor=\"middle\" style=\"font-size:10px\">R{}</text>",
            round + 1
        ));
    }
    let mut source_y = 50.0;
    let mut destination_y = 50.0;
    let label_y = [58.0, 105.0, 145.0, 166.0, 187.0, 207.0, 225.0];
    for (category, count) in counts.iter().copied().enumerate() {
        let thickness = count as f64 * scale;
        let source_top = source_y;
        let destination_top = destination_y;
        let destination_bottom = destination_top + thickness;
        let end_x = match category {
            0 => 160.0,
            1 | 2 => round_x[0],
            _ => round_x[category - 2],
        };
        svg.push_str(&format!(
            "<path d=\"M70 {source_top:.3}C115 {source_top:.3} 140 {destination_top:.3} 170 {destination_top:.3}\
             H{end_x}V{destination_bottom:.3}H170C140 {destination_bottom:.3} 115 {:.3} 70 {:.3}Z\" \
             fill=\"{}\" fill-opacity=\".86\"><title>{}: {count}</title></path>",
            source_top + thickness,
            source_top + thickness,
            FILLS[category],
            LABELS[category]
        ));
        let center = destination_top + thickness / 2.0;
        if category > 0 {
            let rounds = if category <= 2 { 1 } else { category - 1 };
            for (round, x) in round_x.iter().take(rounds).enumerate() {
                let clean = category == 1 || (category >= 3 && round + 1 == rounds);
                svg.push_str(&format!(
                    "<circle cx=\"{}\" cy=\"{center:.3}\" r=\"4.5\" fill=\"{}\" stroke=\"#faf7ef\" stroke-width=\"1.5\"><title>{}</title></circle>",
                    x,
                    if clean { "#2a78d6" } else { "#c05629" },
                    if clean { "clean" } else { "finding" }
                ));
            }
        }
        let (label_x, anchor) = if end_x >= RIGHT_LABEL_THRESHOLD {
            (658.0, "end")
        } else {
            (end_x + 8.0, "start")
        };
        svg.push_str(&format!(
            "<path d=\"M{} {center:.3}V{}\" stroke=\"#48565c\" stroke-width=\".7\"/>\
             <text x=\"{label_x}\" y=\"{}\" text-anchor=\"{anchor}\" style=\"font-size:10px\">{} · {count}</text>",
            end_x + 5.0,
            label_y[category] - 4.0,
            label_y[category],
            LABELS[category]
        ));
        source_y += thickness;
        destination_y = destination_bottom + 6.0;
    }
    svg.push_str("<path d=\"M70 50V162.5\" stroke=\"#242d31\"/><text x=\"0\" y=\"243\" style=\"font-size:10px\">Orange node: finding · blue node: clean · R: canonical verdict round, not elapsed time</text></svg>");
    figure(
        public,
        name,
        svg,
        "Q13–Q14 · All 1,500 selected tasks appear once. Exact histories: 131 without a verdict; 1,104 clean; 127 finding; 104 finding → clean; 27 two findings → clean; six three findings → clean; one four findings → clean. Ribbon thickness is proportional to task count; verdict nodes preserve canonical order, not duration. <a href=\"tables.html#Q13\">Round counts</a> and <a href=\"tables.html#tasks\">task alternatives</a>.",
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
