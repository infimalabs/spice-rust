//! Deterministic, directly labeled vectors with complete semantic alternatives.

use super::super::publication::{Publication, Quantity};
use super::{Result, escape, number};
use std::path::Path;

const WIDTH: f64 = 660.0;
const ROW: f64 = 25.0;
const TOP: f64 = 34.0;
const LABEL: f64 = 280.0;
const VALUE: f64 = 58.0;
const COLORS: [&str; 6] = [
    "#255b60", "#963f2d", "#5a627b", "#696c34", "#6e525e", "#4b6475",
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
        svg.push_str(&format!("<text x=\"0\" y=\"{}\">{}</text><rect x=\"{LABEL}\" y=\"{y}\" width=\"{width:.3}\" height=\"17\" fill=\"{}\"/><text x=\"{}\" y=\"{}\" text-anchor=\"end\">{}</text>",y+13.0,escape(&short(&row.label)),COLORS[index%COLORS.len()],WIDTH-2.0,y+13.0,number(row.value)));
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
    let mut svg = opening(
        TOP + groups.len() as f64 * (OUTCOME_ROW + OUTCOME_ROW) + OUTCOME_ROW,
        "Phase outcomes within 24 hours",
        name,
    );
    svg.push_str(&format!("<defs><pattern id=\"{name}-release\" width=\"7\" height=\"7\" patternUnits=\"userSpaceOnUse\"><rect width=\"7\" height=\"7\" fill=\"#963f2d\"/><path d=\"M-2 2L2-2M0 7L7 0M5 9L9 5\" stroke=\"#faf7ef\" stroke-width=\"1\"/></pattern><pattern id=\"{name}-lapse\" width=\"7\" height=\"7\" patternUnits=\"userSpaceOnUse\"><rect width=\"7\" height=\"7\" fill=\"#5a627b\"/><circle cx=\"3\" cy=\"3\" r=\"1\" fill=\"#faf7ef\"/></pattern></defs><text x=\"0\" y=\"18\">First boundary in 24h · counts remain in each group's denominator</text>"));
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
        "Solid teal: completed. Diagonal stripe: released. Dots: lapsed. Olive: deleted or transferred. Labels give counts where space permits; every count, including narrow segments, is in <a href=\"tables.html#Q06\">Q06</a>. Equal-width rows show within-group shares, not throughput. <a href=\"tables.html#Q07\">Q07 exposure</a>.",
    )
}

pub(super) fn campaign(publication: &Publication, public: &Path, name: &str) -> Result<String> {
    const LINE: f64 = 16.0;
    let campaign = &publication.campaign;
    let mut rows = Vec::with_capacity(campaign.nodes.len());
    for node in &campaign.nodes {
        let columns = ["origin", "blocking dependency", "correction dependency"].map(|layer| {
            campaign
                .edges
                .iter()
                .filter(|edge| edge.from == node.task && edge.layer == layer)
                .map(|edge| edge.to.as_str())
                .collect::<Vec<_>>()
        });
        let lines = columns
            .iter()
            .map(|targets| targets.len().div_ceil(2))
            .max()
            .unwrap_or(1)
            .max(1);
        rows.push((node, columns, (lines as f64 * LINE + 9.0).max(ROW)));
    }
    let height = TOP + rows.iter().map(|(_, _, height)| height).sum::<f64>() + ROW;
    let mut svg = opening(height, "Aligned campaign relationship layers", name);
    svg.push_str("<text x=\"0\" y=\"18\">Task / recorded phase authors</text><text x=\"300\" y=\"18\">Origin</text><text x=\"410\" y=\"18\">Blocking</text><text x=\"520\" y=\"18\">Correction</text>");
    let mut y = TOP;
    for (node, columns, height) in rows {
        svg.push_str(&format!(
            "<text x=\"0\" y=\"{}\">{} / {}</text>",
            y + 13.0,
            node.task,
            if node.authors.is_empty() {
                "no phase author".to_owned()
            } else {
                node.authors.join(", ")
            }
        ));
        for (x, targets) in [300, 410, 520].into_iter().zip(columns) {
            if targets.is_empty() {
                svg.push_str(&format!("<text x=\"{x}\" y=\"{}\">—</text>", y + 13.0));
            }
            for (index, chunk) in targets.chunks(2).enumerate() {
                svg.push_str(&format!(
                    "<text x=\"{x}\" y=\"{}\">{}</text>",
                    y + 13.0 + index as f64 * LINE,
                    chunk.join(", ")
                ));
            }
        }
        svg.push_str(&format!(
            "<path d=\"M0 {}H660\" stroke=\"#bcc3be\" stroke-width=\"0.5\"/>",
            y + height - 3.0
        ));
        y += height;
    }
    svg.push_str("</svg>");
    figure(
        public,
        name,
        svg,
        "Each column names the destination of that row's typed relationship. Origin points from child to filing parent; blocking and correction dependencies point from dependent to prerequisite. Correction requests are separately listed in the alternatives. A dash means no displayed relation, not no outside dependency. <a href=\"tables.html#campaign\">Complete node/edge alternatives and Q28–Q29 frontier</a>.",
    )
}

pub(super) fn continuity(publication: &Publication, public: &Path, name: &str) -> Result<String> {
    let lanes = publication
        .continuity
        .lanes
        .iter()
        .filter(|lane| lane.admitted_episodes > 0)
        .take(12)
        .collect::<Vec<_>>();
    let maximum = lanes
        .iter()
        .map(|lane| lane.held_minutes)
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let mut svg = opening(
        TOP + lanes.len() as f64 * ROW + ROW,
        "Lane-stable exposure and provider spans",
        name,
    );
    svg.push_str("<text x=\"0\" y=\"18\">Lane · episodes / provider spans / compactions · bar: held minutes</text>");
    for (index, lane) in lanes.iter().enumerate() {
        let y = TOP + index as f64 * ROW;
        svg.push_str(&format!("<text x=\"0\" y=\"{}\">{} · {} / {} / {}</text><rect x=\"{LABEL}\" y=\"{y}\" height=\"17\" width=\"{:.3}\" fill=\"#255b60\"/><text x=\"{}\" y=\"{}\" text-anchor=\"end\">{}</text>",y+13.0,lane.lane,lane.admitted_episodes,lane.provider_sessions,lane.compactions,lane.held_minutes/maximum*(WIDTH-LABEL-VALUE),WIDTH-2.0,y+13.0,number(lane.held_minutes)));
    }
    svg.push_str("</svg>");
    figure(
        public,
        name,
        svg,
        "Literal identity order, not a ranking. All lanes and session strips, including omitted lanes, are in the <a href=\"tables.html#continuity\">complete alternatives</a>. Held minutes are wall exposure, not labor or CPU. Q21–Q24.",
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
        "<figure>{svg}<figcaption>{caption}</figcaption></figure>"
    ))
}
fn short(label: &str) -> String {
    label
        .replace("Before blocking-policy integration", "Pre-policy")
        .replace("After blocking-policy integration", "Policy to refill")
        .replace("After materialization integration", "Post-refill")
}
