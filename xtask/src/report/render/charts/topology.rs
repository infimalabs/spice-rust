//! Typed relationship topology; layout never substitutes one edge meaning for another.

use super::{Publication, Result, escape, figure, opening};
use std::path::Path;

pub(in super::super) fn campaign(p: &Publication, public: &Path, name: &str) -> Result<String> {
    let nodes = &p.campaign.nodes;
    let count = nodes.len();
    let step = 310.0 / count.max(1) as f64;
    let height = 410.0;
    let mut svg = opening(
        height,
        "The same campaign nodes in three typed adjacency maps",
        name,
    );
    // Every cell is an exact directed edge, not an inferred connection or weighted flow.
    for (panel, layer) in ["origin", "blocking dependency", "correction dependency"]
        .into_iter()
        .enumerate()
    {
        let left = 40.0 + panel as f64 * 216.0;
        let cell = 172.0 / count.max(1) as f64;
        svg.push_str(&format!(
            "<text x=\"{left}\" y=\"18\">{}</text>",
            escape(layer)
        ));
        for (index, node) in nodes.iter().enumerate() {
            let y = 68.0 + index as f64 * step;
            svg.push_str(&format!("<text x=\"{}\" y=\"{}\" text-anchor=\"end\" style=\"font-size:10px\">{}</text><text transform=\"translate({:.2} 58) rotate(-60)\" style=\"font-size:9px\">{}</text><path d=\"M{left} {}h172\" stroke=\"#e3e5e7\"/>",left-4.0,y+12.0,node.task,left+index as f64*cell+cell/2.0,node.task,y+step));
            for (target, destination) in nodes.iter().enumerate() {
                if p.campaign.edges.iter().any(|edge| {
                    edge.from == node.task && edge.to == destination.task && edge.layer == layer
                }) {
                    svg.push_str(&format!("<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" fill=\"#128c68\"><title>{} → {} · {layer}</title></rect>",left+target as f64*cell+1.0,y+3.0,(cell-2.0).max(1.0),step-5.0,node.task,destination.task));
                }
            }
        }
        svg.push_str(&format!(
            "<text x=\"{left}\" y=\"402\" style=\"font-size:11px\">Row → column · exact edge</text>"
        ));
    }
    svg.push_str("</svg>");
    figure(
        public,
        name,
        svg,
        "Q28–Q29 · Identical node order in all three panels. A filled cell means row → column. Origin points child → filing parent; dependencies point dependent → prerequisite. Correction requests are a fourth recorded layer in the alternatives, not substituted for correction dependencies. An empty cell means no displayed edge. <a href=\"tables.html#campaign\">Every node, edge, author and frontier</a>.",
    )
}

pub(in super::super) fn git(p: &Publication, public: &Path, name: &str) -> Result<String> {
    let height = 50.0 + p.git.cases.len() as f64 * 72.0;
    let mut svg = opening(
        height,
        "Curated implementation heads and actual two-parent merges",
        name,
    );
    svg.push_str("<text x=\"0\" y=\"18\">Case</text><text x=\"130\" y=\"18\">First parent / integrated line</text><text x=\"490\" y=\"18\">Contribution</text>");
    for (index, case) in p.git.cases.iter().enumerate() {
        let y = 45.0 + index as f64 * 72.0;
        let first = case
            .merge_parents
            .first()
            .ok_or("curated merge lacks first parent")?;
        svg.push_str(&format!("<text x=\"0\" y=\"{}\">{}</text><path d=\"M130 {y}H405M270 {}Q270 {y} 405 {y}\" stroke=\"#128c68\" stroke-width=\"2\" fill=\"none\"/><circle cx=\"130\" cy=\"{y}\" r=\"5\" fill=\"#128c68\"/><circle cx=\"405\" cy=\"{y}\" r=\"5\" fill=\"#128c68\"/><circle cx=\"270\" cy=\"{}\" r=\"5\" fill=\"#128c68\"/><text x=\"130\" y=\"{}\">{first}</text><text x=\"405\" y=\"{}\">{}</text><text x=\"285\" y=\"{}\">{} · implementation</text><text x=\"490\" y=\"{}\">{} commit(s)</text>",y+4.0,case.evidence,y+28.0,y+28.0,y-10.0,y-10.0,case.integrated,y+32.0,case.implementation_head,y+4.0,case.contribution_commits.map_or_else(||"Unknown".to_owned(),|count|count.to_string())));
    }
    svg.push_str("</svg>");
    figure(
        public,
        name,
        svg,
        "Q25–Q27 · Each row is an independent exact-object join, not a continuous trunk history or time axis. The implementation head is the recorded second parent and an ancestor of its integrated merge. Contribution counts use first-parent-to-second-parent ranges. <a href=\"tables.html#git\">All curated object joins</a>.",
    )
}

pub(in super::super) fn origins(p: &Publication, public: &Path, name: &str) -> Result<String> {
    let nodes = &p.campaign.nodes;
    let mut depths = std::collections::BTreeMap::new();
    depths.insert(p.campaign.root.as_str(), 0_usize);
    for _ in 0..nodes.len() {
        for edge in p
            .campaign
            .edges
            .iter()
            .filter(|edge| edge.layer == "origin")
        {
            if let Some(depth) = depths.get(edge.to.as_str()).copied() {
                depths.entry(edge.from.as_str()).or_insert(depth + 1);
            }
        }
    }
    let maximum = depths.values().copied().max().unwrap_or(0).max(1);
    let mut positions = std::collections::BTreeMap::new();
    for depth in 0..=maximum {
        let tier = nodes
            .iter()
            .filter(|node| depths.get(node.task.as_str()) == Some(&depth))
            .collect::<Vec<_>>();
        for (index, node) in tier.iter().enumerate() {
            positions.insert(
                node.task.as_str(),
                (
                    80.0 + depth as f64 / maximum as f64 * 490.0,
                    35.0 + (index as f64 + 0.5) / tier.len() as f64 * 390.0,
                ),
            );
        }
    }
    if positions.len() != nodes.len() {
        return Err("displayed origin nodes do not form a rooted campaign".into());
    }
    let mut svg = opening(
        465.0,
        "The campaign origin tree and its recorded phase authors",
        name,
    );
    svg.push_str(&format!("<defs><marker id=\"{name}-arrow\" viewBox=\"0 0 8 8\" refX=\"8\" refY=\"4\" markerWidth=\"5\" markerHeight=\"5\" orient=\"auto\"><path d=\"M0 0L8 4L0 8\" fill=\"#128c68\"/></marker></defs><text x=\"0\" y=\"18\">Origin · child → filing parent</text>"));
    for edge in p
        .campaign
        .edges
        .iter()
        .filter(|edge| edge.layer == "origin")
    {
        let (from_x, from_y) = positions[edge.from.as_str()];
        let (to_x, to_y) = positions[edge.to.as_str()];
        let middle = (from_x + to_x) / 2.0;
        svg.push_str(&format!("<path d=\"M{} {from_y}C{middle} {from_y} {middle} {to_y} {} {to_y}\" fill=\"none\" stroke=\"#128c68\" stroke-opacity=\".65\" stroke-width=\"1.5\" marker-end=\"url(#{name}-arrow)\"/>",from_x-35.0,to_x+35.0));
    }
    for node in nodes {
        let (x, y) = positions[node.task.as_str()];
        svg.push_str(&format!("<rect x=\"{}\" y=\"{}\" width=\"70\" height=\"24\" rx=\"3\" fill=\"{}\" stroke=\"#128c68\"/><text x=\"{x}\" y=\"{}\" text-anchor=\"middle\">{}</text><text x=\"{x}\" y=\"{}\" text-anchor=\"middle\" style=\"font-size:11px\">{}</text>",x-35.0,y-12.0,if node.selected {"#e5f3ed"}else{"#fff"},y+4.0,node.task,y+24.0,if node.authors.is_empty() {"no phase author".to_owned()}else{node.authors.join(", ")}));
    }
    svg.push_str("</svg>");
    figure(
        public,
        name,
        svg,
        "Q28 · Filled nodes are selected deliveries; the open node is context outside selection. Labels below nodes are recorded phase-author lanes, not owners. Layout shows origin depth, not duration or magnitude. Six public campaign nodes are omitted by the display bound. <a href=\"tables.html#campaign\">Complete displayed node support and edge frontier</a>.",
    )
}
