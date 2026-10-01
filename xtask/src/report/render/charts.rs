//! Deterministic, directly labeled vectors with complete semantic alternatives.

use super::super::publication::{Publication, Quantity};
use super::{Result, escape, number};
use std::path::Path;

pub(super) mod intervals;
pub(super) mod sequences;
pub(super) mod topology;

// Figure order is editorial; colour belongs to the visual family, never rank.
pub(super) const ATLAS: [(&str, &str, &str); 20] = [
    ("delivery-calendar", "A month, not an average day", "Time"),
    ("delivery-composition", "Where the work lives", "Magnitude"),
    (
        "integration-windows",
        "Integration is not activation",
        "Time",
    ),
    ("phase-outcomes", "Same follow-up, different mix", "Flow"),
    ("waiting-clock", "The queue has a long tail", "Magnitude"),
    ("held-clock", "A different clock", "Magnitude"),
    (
        "admission-disposition",
        "The unfinished denominator",
        "Magnitude",
    ),
    ("inter-claim-gaps", "Between claims", "Magnitude"),
    (
        "first-review-verdict",
        "First clean is not final clean",
        "Flow",
    ),
    (
        "ordered-review-rounds",
        "Most reviews stop at one",
        "Magnitude",
    ),
    (
        "correction-flows",
        "The correction has its own route",
        "Structure",
    ),
    (
        "correction-circuit",
        "Finding, correction, re-review",
        "Structure",
    ),
    (
        "recorded-materialization-seconds",
        "Unchanged bytes, avoided seconds",
        "Magnitude",
    ),
    (
        "recorded-materialization-crates",
        "Recompilation is a separate measure",
        "Magnitude",
    ),
    (
        "proof-boundary",
        "Reuse credit, keep the obligation",
        "Structure",
    ),
    ("session-spans", "Stable lanes, changing sessions", "Time"),
    (
        "lane-exposure",
        "Exposure does not rank workers",
        "Magnitude",
    ),
    ("campaign-origins", "One seed, many hands", "Topology"),
    (
        "campaign-layers",
        "Three relations, one node set",
        "Topology",
    ),
    (
        "git-joins",
        "The implementation is a merge parent",
        "Topology",
    ),
];

const WIDTH: f64 = 660.0;
const ROW: f64 = 35.0;
const TOP: f64 = 34.0;
const LABEL: f64 = 280.0;
const VALUE: f64 = 58.0;
const COLORS: [&str; 6] = [
    "#2a78d6", "#c05629", "#6554a4", "#737329", "#b34d7a", "#128c68",
];
const SVG_FONT_FACE: &str =
    "@font-face{font-family:LMSans;src:url('lmsans10-regular.otf') format('opentype')}";

pub(super) fn bars(
    quantity: &Quantity,
    labels: Option<&[&str]>,
    public: &Path,
    name: &str,
) -> Result<String> {
    let rows = quantity
        .values
        .iter()
        .filter(|row| labels.is_none_or(|labels| labels.contains(&row.label.as_str())))
        .collect::<Vec<_>>();
    let maximum = rows
        .iter()
        .map(|row| row.value)
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let mut svg = opening(TOP + rows.len() as f64 * ROW + ROW, &quantity.name, name);
    svg.push_str(&format!(
        "<text x=\"0\" y=\"18\" style=\"fill:#48565c\">{}</text>",
        escape(&format!(
            "{} · denominator {} · {}",
            quantity.id, quantity.denominator, quantity.unit
        ))
    ));
    for (index, row) in rows.iter().enumerate() {
        let y = TOP + index as f64 * ROW;
        let width = row.value / maximum * (WIDTH - LABEL - VALUE);
        svg.push_str(&format!("<text x=\"0\" y=\"{}\">{}</text><path d=\"M{LABEL} {}H602\" stroke=\"#dde1e3\"/><rect x=\"{LABEL}\" y=\"{y}\" width=\"{width:.3}\" height=\"23\" fill=\"#c05629\"/><text x=\"{}\" y=\"{}\" text-anchor=\"end\">{}</text>",y+16.0,escape(&short(&row.label)),y+24.0,WIDTH-2.0,y+16.0,number(row.value)));
    }
    svg.push_str("</svg>");
    figure(
        public,
        name,
        svg,
        &format!(
            "{} <a href=\"tables.html#{}\">Complete definition and data: {}</a>.",
            escape(&quantity.definition),
            quantity.id,
            quantity.id
        ),
    )
}

pub(super) fn outcomes(publication: &Publication, public: &Path, name: &str) -> Result<String> {
    const OUTCOME_ROW: f64 = 20.0;
    // Narrower segments cannot hold a centered count; Q06 still lists it.
    const LABEL_MIN_WIDTH: f64 = 36.0;
    let record = publication
        .quantities
        .iter()
        .find(|quantity| quantity.id == "Q06")
        .ok_or("phase outcome quantity missing")?;
    let mut groups = std::collections::BTreeMap::<String, Vec<(&str, f64)>>::new();
    for row in &record.values {
        let (group, outcome) = row
            .label
            .rsplit_once(" / ")
            .ok_or("outcome label lacks phase grouping")?;
        groups
            .entry(group.to_owned())
            .or_default()
            .push((outcome, row.value));
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
    let mut svg = opening(
        TOP + groups.len() as f64 * (OUTCOME_ROW + OUTCOME_ROW) + OUTCOME_ROW,
        "Phase outcomes within 24 hours",
        name,
    );
    svg.push_str(&format!("<defs><pattern id=\"{name}-release\" width=\"7\" height=\"7\" patternUnits=\"userSpaceOnUse\"><rect width=\"7\" height=\"7\" fill=\"#c05629\"/><path d=\"M-2 2L2-2M0 7L7 0M5 9L9 5\" stroke=\"#faf7ef\" stroke-width=\"1\"/></pattern><pattern id=\"{name}-lapse\" width=\"7\" height=\"7\" patternUnits=\"userSpaceOnUse\"><rect width=\"7\" height=\"7\" fill=\"#6554a4\"/><circle cx=\"3\" cy=\"3\" r=\"1\" fill=\"#faf7ef\"/></pattern></defs><text x=\"0\" y=\"18\">First boundary in 24h · shares within each row</text>"));
    for (index, (group, outcomes)) in groups.iter().enumerate() {
        let y = TOP + index as f64 * (OUTCOME_ROW + OUTCOME_ROW);
        let total = outcomes.iter().map(|(_, count)| count).sum::<f64>();
        svg.push_str(&format!(
            "<text x=\"0\" y=\"{}\">{} · n={}</text>",
            y + 13.0,
            escape(&short(group)),
            number(total)
        ));
        let mut x = 0.0;
        for (outcome, count) in outcomes {
            let fill = match *outcome {
                "phase completed" => COLORS[0].to_owned(),
                "released" => format!("url(#{name}-release)"),
                "lapsed" => format!("url(#{name}-lapse)"),
                _ => COLORS[3].to_owned(),
            };
            let width = count / total.max(1.0) * WIDTH;
            svg.push_str(&format!("<rect x=\"{x:.3}\" y=\"{}\" width=\"{width:.3}\" height=\"19\" fill=\"{fill}\"><title>{}: {}</title></rect>",y+OUTCOME_ROW,escape(outcome),number(*count)));
            if width >= LABEL_MIN_WIDTH {
                svg.push_str(&format!("<text x=\"{:.3}\" y=\"{}\" text-anchor=\"middle\" style=\"fill:#faf7ef\">{}</text>",x+width/2.0,y+OUTCOME_ROW+14.0,number(*count)));
            }
            x += width;
        }
    }
    svg.push_str("</svg>");
    figure(
        public,
        name,
        svg,
        "Solid blue: completed. Orange stripe: released. Violet dots: lapsed. Olive: deleted or transferred. Labels give counts where space permits; every count, including narrow segments, is in <a href=\"tables.html#Q06\">Q06</a>. Equal-width rows show within-group shares, not throughput. <a href=\"tables.html#Q07\">Q07 exposure</a>.",
    )
}

pub(super) fn continuity(publication: &Publication, public: &Path, name: &str) -> Result<String> {
    const EXPOSURE_ROW: f64 = 20.0;
    let lanes = publication
        .continuity
        .lanes
        .iter()
        .filter(|lane| lane.admitted_episodes > 0)
        .collect::<Vec<_>>();
    let maximum = lanes
        .iter()
        .map(|lane| lane.held_minutes)
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let mut svg = opening(
        TOP + lanes.len() as f64 * EXPOSURE_ROW + EXPOSURE_ROW,
        "Lane-stable exposure and provider spans",
        name,
    );
    svg.push_str("<text x=\"0\" y=\"18\">Lane · episodes / provider spans / compactions · bar: held minutes</text>");
    for (index, lane) in lanes.iter().enumerate() {
        let y = TOP + index as f64 * EXPOSURE_ROW;
        svg.push_str(&format!("<text x=\"0\" y=\"{}\">{} · {} / {} / {}</text><rect x=\"{LABEL}\" y=\"{y}\" height=\"13\" width=\"{:.3}\" fill=\"#c05629\"/><text x=\"{}\" y=\"{}\" text-anchor=\"end\">{}</text>",y+12.0,lane.lane,lane.admitted_episodes,lane.provider_sessions,lane.compactions,lane.held_minutes/maximum*(WIDTH-LABEL-VALUE),WIDTH-2.0,y+12.0,number(lane.held_minutes)));
    }
    svg.push_str("</svg>");
    figure(
        public,
        name,
        svg,
        &format!(
            "All {} lanes with in-window admissions, in literal identity order. Zero-admission lanes remain in the <a href=\"tables.html#continuity\">complete alternatives</a>. Bars: horizon-capped held minutes, not labor or CPU. Counts: admitted episodes / provider-session spans / compactions. Q21–Q24.",
            lanes.len()
        ),
    )
}

fn opening(height: f64, title: &str, name: &str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{WIDTH}\" height=\"{height}\" viewBox=\"0 0 {WIDTH} {height}\" role=\"img\" aria-labelledby=\"{name}-title\"><title id=\"{name}-title\">{}</title><style>{SVG_FONT_FACE}text{{font:13px LMSans,sans-serif;fill:#242d31}}</style>",
        escape(title)
    )
}
fn figure(public: &Path, name: &str, mut svg: String, caption: &str) -> Result<String> {
    super::super::privacy::scan(&svg)?;
    std::fs::write(public.join("assets").join(format!("{name}.svg")), &svg)?;
    if let Some(start) = svg.find(SVG_FONT_FACE) {
        svg.replace_range(start..start + SVG_FONT_FACE.len(), "");
    }
    Ok(format!(
        "<figure id=\"fig-{name}\" class=\"family-{}\"><div class=\"figure-label\">{}</div>{svg}<figcaption>{caption}</figcaption></figure>",
        family(name).to_lowercase(),
        figure_label(name)
    ))
}

fn family(name: &str) -> &'static str {
    ATLAS
        .iter()
        .find(|row| row.0 == name)
        .map_or("Magnitude", |row| row.2)
}

fn figure_label(name: &str) -> String {
    ATLAS.iter().position(|row| row.0 == name).map_or_else(
        || "Magnitude · wall exposure".to_owned(),
        |index| format!("{:02} · {} · {}", index + 1, ATLAS[index].2, ATLAS[index].1),
    )
}
fn short(label: &str) -> String {
    label
        .replace("Before blocking-policy integration", "Pre-policy")
        .replace("After blocking-policy integration", "Policy to refill")
        .replace("After materialization integration", "Post-refill")
}
