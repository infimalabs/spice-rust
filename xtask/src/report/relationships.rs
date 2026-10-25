//! Frozen board relationships: origin, blocking and correction are separate layers.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::Path;

use super::Result;
use super::publication::{Aliases, Quantity, domain};
use super::reduce::Study;

const CAMPAIGN_ROOT: &str = "dSBzcF2SZ";
const MAX_NODES: usize = 16;

/// The independent board capture: a `spice board --output FILE` export, or the
/// reply envelope an earlier `spice board --json` printed.
#[derive(Deserialize)]
#[serde(untagged)]
enum Oracle {
    Board(Board),
    Reply { cursor: wire::Seq, body: Board },
}
#[derive(Deserialize)]
struct Board {
    cursor: wire::Seq,
    tasks: Vec<Row>,
}
#[derive(Deserialize)]
struct Row {
    statement: Statement,
}
#[derive(Deserialize)]
struct Statement {
    position: wire::Seq,
    body: Body,
}
#[derive(Deserialize)]
struct Body {
    born: String,
    project: String,
    provenance: String,
    dependencies: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Node {
    pub task: String,
    pub domain: String,
    pub selected: bool,
    pub authors: Vec<String>,
}
#[derive(Serialize, Deserialize)]
pub(super) struct Edge {
    pub from: String,
    pub to: String,
    pub layer: String,
}
#[derive(Serialize, Deserialize)]
pub(super) struct Campaign {
    pub root: String,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub omitted_public_nodes: usize,
    pub external_edges: usize,
    pub unavailable_frontier_edges: usize,
    pub relationship_source: String,
}

pub(super) fn observe(study: &Study, directory: &Path, aliases: &Aliases) -> Result<Campaign> {
    let public = public_statements(study, directory)?;
    let members = campaign_members(&public)?;
    let displayed = displayed_frontier(&members, aliases);
    let mut edges = Edges {
        displayed: &displayed,
        public: &public,
        kept: BTreeSet::new(),
        external: 0,
        unavailable: 0,
    };
    for birth in &displayed {
        edges.task(study, birth, &public[birth]);
    }
    let selected = study.selected.iter().collect::<BTreeSet<_>>();
    let nodes = displayed
        .iter()
        .map(|birth| {
            let task = study
                .tasks
                .get(birth)
                .ok_or("campaign has no admitted task history")?;
            Ok(Node {
                task: aliases.tasks[birth].clone(),
                domain: domain(&public[birth].project).to_owned(),
                selected: selected.contains(birth),
                authors: task
                    .phase_ends
                    .iter()
                    .filter_map(|observation| aliases.actor(&observation.actor))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Campaign { root: aliases.tasks[CAMPAIGN_ROOT].clone(), nodes,
        edges: edges.kept.into_iter().map(|(from, to, layer)| Edge { from: aliases.tasks[&from].clone(), to: aliases.tasks[&to].clone(), layer: layer.to_owned() }).collect(),
        omitted_public_nodes: members.len() - displayed.len(), external_edges: edges.external, unavailable_frontier_edges: edges.unavailable,
        relationship_source: "Origin and dependencies: independent public board statement at the frozen horizon. Correction requests and phase authors: ordered admitted journal observations. Statement revisions are not inferred from current heads.".to_owned() })
}

/// Public statements from the independent oracle pinned at the frozen horizon.
fn public_statements(study: &Study, directory: &Path) -> Result<BTreeMap<String, Body>> {
    let oracle: Oracle =
        serde_json::from_reader(File::open(directory.join("independent-board-oracle.json"))?)?;
    let (cursor, board) = match oracle {
        Oracle::Board(board) => (board.cursor, board),
        Oracle::Reply { cursor, body } => (cursor, body),
    };
    if cursor != study.manifest.upper || board.cursor != study.manifest.upper {
        return Err("relationship oracle does not name the frozen horizon".into());
    }
    let mut public = BTreeMap::new();
    for row in board.tasks {
        if row.statement.position > study.manifest.upper {
            return Err("relationship statement is above horizon".into());
        }
        if public
            .insert(row.statement.body.born.clone(), row.statement.body)
            .is_some()
        {
            return Err("relationship oracle repeats a public birth".into());
        }
    }
    Ok(public)
}

/// The campaign root and every public statement descended from it by origin.
fn campaign_members(public: &BTreeMap<String, Body>) -> Result<BTreeSet<String>> {
    if !public.contains_key(CAMPAIGN_ROOT) {
        return Err("curated campaign root is outside public oracle".into());
    }
    let mut members = BTreeSet::from([CAMPAIGN_ROOT.to_owned()]);
    let mut frontier = vec![CAMPAIGN_ROOT.to_owned()];
    while let Some(parent) = frontier.pop() {
        for row in public
            .values()
            .filter(|row| row.provenance.strip_prefix("task:") == Some(parent.as_str()))
        {
            if members.insert(row.born.clone()) {
                frontier.push(row.born.clone());
            }
        }
    }
    Ok(members)
}

/// At most `MAX_NODES` published members, always including the root.
fn displayed_frontier(members: &BTreeSet<String>, aliases: &Aliases) -> BTreeSet<String> {
    let mut displayed = members
        .iter()
        .filter(|birth| aliases.tasks.contains_key(*birth))
        .take(MAX_NODES)
        .cloned()
        .collect::<BTreeSet<_>>();
    if !displayed.contains(CAMPAIGN_ROOT) {
        if displayed.len() == MAX_NODES {
            displayed.pop_last();
        }
        displayed.insert(CAMPAIGN_ROOT.to_owned());
    }
    displayed
}

struct Edges<'a> {
    displayed: &'a BTreeSet<String>,
    public: &'a BTreeMap<String, Body>,
    kept: BTreeSet<(String, String, &'static str)>,
    external: usize,
    unavailable: usize,
}

impl Edges<'_> {
    fn task(&mut self, study: &Study, birth: &str, row: &Body) {
        if let Some(parent) = row.provenance.strip_prefix("task:") {
            self.add(birth, parent, "origin");
        }
        let task = study.tasks.get(birth);
        for target in &row.dependencies {
            let correction = task.is_some_and(|task| {
                task.reviews
                    .iter()
                    .any(|review| review.correction_births.contains(target))
            });
            let layer = if correction {
                "correction dependency"
            } else {
                "blocking dependency"
            };
            self.add(birth, target, layer);
        }
        for review in task.iter().flat_map(|task| &task.reviews) {
            for child in &review.correction_births {
                self.add(birth, child, "correction request");
            }
        }
    }

    fn add(&mut self, from: &str, to: &str, layer: &'static str) {
        if self.displayed.contains(to) {
            self.kept.insert((from.to_owned(), to.to_owned(), layer));
        } else if self.public.contains_key(to) {
            self.external += 1;
        } else {
            self.unavailable += 1;
        }
    }
}

pub(super) fn quantity(campaign: &Campaign) -> Quantity {
    super::publication::quantity(
        "Q28",
        "Bounded campaign node frontier",
        "The curated public origin campaign",
        "nodes",
        "The node bound is editorial, not a delivery-population cutoff. Complete displayed-node alternatives accompany the figure; omitted public campaign nodes are explicit.",
        campaign.nodes.len() + campaign.omitted_public_nodes,
        [
            ("Displayed nodes".to_owned(), campaign.nodes.len() as f64),
            (
                "Omitted public campaign nodes".to_owned(),
                campaign.omitted_public_nodes as f64,
            ),
        ],
    )
}
