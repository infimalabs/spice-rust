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
        "A clean ending can carry corrections",
        "Flow",
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

const OUTCOME_WINDOWS: [&str; 3] = [
    "Before blocking-policy integration",
    "After blocking-policy integration",
    "After materialization integration",
];
const OUTCOME_WINDOW_LABELS: [&str; 3] = ["Pre-policy", "Policy to refill", "Post-refill"];
const OUTCOME_PHASES: [&str; 3] = ["implementation", "plan", "todo"];
const OUTCOME_NAMES: [&str; 5] = [
    "phase completed",
    "released",
    "lapsed",
    "deleted",
    "derived transfer",
];
const OUTCOME_COLORS: [&str; 5] = ["#2a78d6", "#c05629", "#6554a4", "#737329", "#b34d7a"];
const OUTCOME_SCALE: f64 = 0.075;
const MIN_OUTCOME_LABEL_THICKNESS: f64 = 11.0;
type OutcomeWindow = [[f64; 5]; 3];
type OutcomeCounts = [OutcomeWindow; 3];

pub(super) fn outcomes(publication: &Publication, public: &Path, name: &str) -> Result<String> {
    let record = publication
        .quantities
        .iter()
        .find(|quantity| quantity.id == "Q06")
        .ok_or("phase outcome quantity missing")?;
    let counts = outcome_counts(record)?;
    let mut svg = opening(
        370.0,
        "Window, phase and first outcome within 24 hours",
        name,
    );
    svg.push_str("<text x=\"0\" y=\"17\">Exact aggregate alluvial · one common thickness scale · 2,194 eligible admissions</text>");
    let mut panel_y = 43.0;
    for (window, window_counts) in counts.iter().enumerate() {
        panel_y = outcome_panel(&mut svg, window, window_counts, panel_y);
    }
    svg.push_str("<text x=\"0\" y=\"363\" style=\"font-size:10px\">I implementation · P plan · T todo · C completed · R released · L lapsed · D deleted · X derived transfer</text></svg>");
    figure(
        public,
        name,
        svg,
        "Q06 · Ribbon thickness uses one scale across all three windows and all 2,194 observed episodes. Left stacking preserves phase mix; right stacking preserves the exact first boundary inside 24 hours. This is aggregate flow, not individual paths or elapsed time. Late admissions are separately right-censored in Q05. <a href=\"tables.html#Q06\">Every window × phase × outcome count</a>.",
    )
}

fn outcome_counts(record: &Quantity) -> Result<OutcomeCounts> {
    let mut counts = [[[0.0_f64; 5]; 3]; 3];
    for row in &record.values {
        let mut parts = row.label.split(" / ");
        let window = parts.next().ok_or("outcome row lacks a window")?;
        let phase = parts.next().ok_or("outcome row lacks a phase")?;
        let outcome = parts.next().ok_or("outcome row lacks an outcome")?;
        if parts.next().is_some() {
            return Err("outcome row has an unexpected label shape".into());
        }
        let window = OUTCOME_WINDOWS
            .iter()
            .position(|candidate| *candidate == window)
            .ok_or("outcome row has an unknown window")?;
        let phase = OUTCOME_PHASES
            .iter()
            .position(|candidate| *candidate == phase)
            .ok_or("outcome row has an unknown phase")?;
        let outcome = OUTCOME_NAMES
            .iter()
            .position(|candidate| *candidate == outcome)
            .ok_or("outcome row has an unknown outcome")?;
        counts[window][phase][outcome] += row.value;
    }
    let total = counts.iter().flatten().flatten().copied().sum::<f64>();
    if total != record.observed as f64 || record.observed != record.denominator {
        return Err("Q06 alluvial does not cover its full observed denominator".into());
    }
    Ok(counts)
}

fn outcome_panel(svg: &mut String, window: usize, counts: &OutcomeWindow, panel_y: f64) -> f64 {
    let phase_totals = counts.map(|outcomes| outcomes.into_iter().sum::<f64>());
    let mut outcome_totals = [0.0_f64; 5];
    for outcomes in counts {
        for (index, count) in outcomes.iter().enumerate() {
            outcome_totals[index] += count;
        }
    }
    let panel_total = phase_totals.iter().sum::<f64>();
    svg.push_str(&format!(
        "<text x=\"0\" y=\"{panel_y}\" style=\"font-weight:700\">{} · n={}</text>\
         <text x=\"0\" y=\"{}\" style=\"font-size:10px\">phase I {} · P {} · T {}</text>\
         <text x=\"240\" y=\"{}\" style=\"font-size:10px\">outcome C {} · R {} · L {} · D {} · X {}</text>",
        OUTCOME_WINDOW_LABELS[window],
        number(panel_total),
        panel_y + 13.0,
        number(phase_totals[0]),
        number(phase_totals[1]),
        number(phase_totals[2]),
        panel_y + 13.0,
        number(outcome_totals[0]),
        number(outcome_totals[1]),
        number(outcome_totals[2]),
        number(outcome_totals[3]),
        number(outcome_totals[4])
    ));
    let top = panel_y + 20.0;
    let mut left = [[0.0_f64; 5]; 3];
    let mut cursor = top;
    for (phase, outcomes) in counts.iter().enumerate() {
        for (outcome, count) in outcomes.iter().enumerate() {
            left[phase][outcome] = cursor;
            cursor += count * OUTCOME_SCALE;
        }
    }
    let mut right = [[0.0_f64; 5]; 3];
    cursor = top;
    for outcome in 0..OUTCOME_NAMES.len() {
        for phase in 0..OUTCOME_PHASES.len() {
            right[phase][outcome] = cursor;
            cursor += counts[phase][outcome] * OUTCOME_SCALE;
        }
    }
    for phase in 0..OUTCOME_PHASES.len() {
        for outcome in 0..OUTCOME_NAMES.len() {
            let count = counts[phase][outcome];
            if count == 0.0 {
                continue;
            }
            let thickness = count * OUTCOME_SCALE;
            let left_top = left[phase][outcome];
            let right_top = right[phase][outcome];
            svg.push_str(&format!(
                "<path d=\"M110 {left_top:.3}C270 {left_top:.3} 390 {right_top:.3} 550 {right_top:.3}\
                 L550 {:.3}C390 {:.3} 270 {:.3} 110 {:.3}Z\" fill=\"{}\" fill-opacity=\".84\">\
                 <title>{} / {} / {}: {}</title></path>",
                right_top + thickness,
                right_top + thickness,
                left_top + thickness,
                left_top + thickness,
                OUTCOME_COLORS[outcome],
                OUTCOME_WINDOW_LABELS[window],
                OUTCOME_PHASES[phase],
                OUTCOME_NAMES[outcome],
                number(count)
            ));
        }
    }
    let mut phase_cursor = top;
    for (phase, count) in phase_totals.iter().enumerate() {
        let thickness = count * OUTCOME_SCALE;
        svg.push_str(&format!(
            "<path d=\"M106 {phase_cursor:.3}H110V{:.3}H106\" fill=\"none\" stroke=\"#242d31\"/>",
            phase_cursor + thickness
        ));
        if thickness >= MIN_OUTCOME_LABEL_THICKNESS {
            svg.push_str(&format!(
                "<text x=\"102\" y=\"{:.3}\" text-anchor=\"end\" style=\"font-size:10px\">{}</text>",
                phase_cursor + thickness / 2.0 + 3.0,
                ["impl", "plan", "todo"][phase]
            ));
        }
        phase_cursor += thickness;
    }
    let mut outcome_cursor = top;
    for (outcome, count) in outcome_totals.iter().enumerate() {
        let thickness = count * OUTCOME_SCALE;
        svg.push_str(&format!(
            "<path d=\"M550 {outcome_cursor:.3}H554V{:.3}H550\" fill=\"none\" stroke=\"{}\" stroke-width=\"1.5\"/>",
            outcome_cursor + thickness,
            OUTCOME_COLORS[outcome]
        ));
        outcome_cursor += thickness;
    }
    top + panel_total * OUTCOME_SCALE + 16.0
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
