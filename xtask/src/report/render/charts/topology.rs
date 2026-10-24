//! Typed relationship topology; layout never substitutes one edge meaning for another.

use super::{Publication, Result, escape, figure, opening};
use std::path::Path;

pub(in super::super) fn campaign(p: &Publication, public: &Path, name: &str) -> Result<String> {
    let positions = campaign_positions(p)?;
    let mut svg = opening(
        470.0,
        "Origin skeleton with blocking and correction relationships",
        name,
    );
    svg.push_str(&format!(
        "<defs>\
         <marker id=\"{name}-origin\" viewBox=\"0 0 8 8\" refX=\"8\" refY=\"4\" markerWidth=\"5\" markerHeight=\"5\" orient=\"auto\"><path d=\"M0 0L8 4L0 8\" fill=\"#128c68\"/></marker>\
         <marker id=\"{name}-blocking\" viewBox=\"0 0 8 8\" refX=\"8\" refY=\"4\" markerWidth=\"5\" markerHeight=\"5\" orient=\"auto\"><path d=\"M0 0L8 4L0 8\" fill=\"#2a78d6\"/></marker>\
         <marker id=\"{name}-correction\" viewBox=\"0 0 8 8\" refX=\"8\" refY=\"4\" markerWidth=\"5\" markerHeight=\"5\" orient=\"auto\"><path d=\"M0 0L8 4L0 8\" fill=\"#c05629\"/></marker>\
         </defs>\
         <path d=\"M0 14H28\" stroke=\"#128c68\" marker-end=\"url(#{name}-origin)\"/><text x=\"36\" y=\"18\" style=\"font-size:10px\">origin</text>\
         <path d=\"M104 14H132\" stroke=\"#2a78d6\" stroke-dasharray=\"5 4\" marker-end=\"url(#{name}-blocking)\"/><text x=\"140\" y=\"18\" style=\"font-size:10px\">blocking</text>\
         <path d=\"M230 10H258\" stroke=\"#c05629\" stroke-width=\"2.4\" marker-end=\"url(#{name}-correction)\"/><path d=\"M230 18H258\" stroke=\"#c05629\" stroke-width=\"2.4\" stroke-dasharray=\"2 3\"/><text x=\"266\" y=\"18\" style=\"font-size:10px\">correction dependency / request</text>"
    ));
    campaign_edges(&mut svg, p, &positions, name)?;
    campaign_nodes(&mut svg, p, &positions);
    svg.push_str("<text x=\"0\" y=\"466\" style=\"font-size:10px\">Node fill: blue distribution · violet validation · orange appliance/policy · dashed node outside selected cohort</text></svg>");
    figure(
        public,
        name,
        svg,
        "Q28–Q29 · The green origin skeleton gives filing depth. Blue dashed arrows are blocking dependencies. Orange solid and dotted arrows preserve correction dependency and correction request as separate recorded relations. Arrow direction is always from the relationship's source to destination; line shape is not duration or volume. Node labels below are phase-author lanes, not owners. <a href=\"tables.html#campaign\">Every node, typed edge and frontier</a>.",
    )
}

fn campaign_positions(p: &Publication) -> Result<std::collections::BTreeMap<&str, (f64, f64)>> {
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
                    65.0 + depth as f64 / maximum as f64 * 515.0,
                    56.0 + (index as f64 + 0.5) / tier.len() as f64 * 355.0,
                ),
            );
        }
    }
    if positions.len() != nodes.len() {
        return Err("displayed campaign nodes do not form a rooted origin topology".into());
    }
    Ok(positions)
}

fn campaign_edges(
    svg: &mut String,
    p: &Publication,
    positions: &std::collections::BTreeMap<&str, (f64, f64)>,
    name: &str,
) -> Result<()> {
    for edge in &p.campaign.edges {
        let (from_x, from_y) = positions[edge.from.as_str()];
        let (to_x, to_y) = positions[edge.to.as_str()];
        let direction = if to_x >= from_x { 1.0 } else { -1.0 };
        let from_edge = from_x + direction * 38.0;
        let to_edge = to_x - direction * 38.0;
        let middle = (from_edge + to_edge) / 2.0;
        let (stroke, width, dash, marker, bend) = match edge.layer.as_str() {
            "origin" => ("#128c68", 1.2, "", "origin", 0.0),
            "blocking dependency" => ("#2a78d6", 1.5, "5 4", "blocking", -7.0),
            "correction dependency" => ("#c05629", 2.4, "", "correction", -11.0),
            "correction request" => ("#c05629", 2.4, "2 3", "correction", 11.0),
            _ => return Err("campaign contains an unsupported relationship layer".into()),
        };
        svg.push_str(&format!(
            "<path d=\"M{from_edge:.2} {from_y:.2}C{middle:.2} {:.2} {middle:.2} {:.2} {to_edge:.2} {to_y:.2}\" \
             fill=\"none\" stroke=\"{stroke}\" stroke-width=\"{width}\" stroke-dasharray=\"{dash}\" \
             stroke-opacity=\".78\" marker-end=\"url(#{name}-{marker})\">\
             <title>{} → {} · {}</title></path>",
            from_y + bend,
            to_y + bend,
            edge.from,
            edge.to,
            escape(&edge.layer)
        ));
    }
    Ok(())
}

fn campaign_nodes(
    svg: &mut String,
    p: &Publication,
    positions: &std::collections::BTreeMap<&str, (f64, f64)>,
) {
    for node in &p.campaign.nodes {
        let (x, y) = positions[node.task.as_str()];
        let (fill, stroke) = match node.domain.as_str() {
            "Distribution" => ("#e6f0fb", "#2a78d6"),
            "Validation" => ("#efecf7", "#6554a4"),
            "Appliance and policy" => ("#f7e8e1", "#c05629"),
            _ => ("#e5f3ed", "#128c68"),
        };
        let joined_authors;
        let (title_authors, label_authors): (&str, &str) = if node.authors.is_empty() {
            ("none", "no phase author")
        } else {
            joined_authors = node.authors.join(", ");
            (&joined_authors, &joined_authors)
        };
        svg.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"76\" height=\"28\" rx=\"4\" fill=\"{}\" stroke=\"{stroke}\" stroke-width=\"{}\" stroke-dasharray=\"{}\">\
             <title>{} · {} · {} · phase authors {title_authors}</title></rect>\
             <text x=\"{x}\" y=\"{}\" text-anchor=\"middle\">{}</text>\
             <text x=\"{x}\" y=\"{}\" text-anchor=\"middle\" style=\"font-size:9px\">{label_authors}</text>",
            x - 38.0,
            y - 14.0,
            if node.selected { fill } else { "#faf7ef" },
            if node.task == p.campaign.root { 2.8 } else { 1.3 },
            if node.selected { "" } else { "4 3" },
            node.task,
            escape(&node.domain),
            if node.selected { "selected" } else { "context" },
            y + 4.0,
            node.task,
            y + 25.0,
        ));
    }
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
