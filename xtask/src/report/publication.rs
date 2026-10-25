//! The publication allowlist. Private observations never serialize through this type.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::episodes::Outcomes;
use super::reduce::{Study, Task};
use super::{Result, hash_file};

pub(super) const VERSION: u32 = 1;
pub(super) const POLICY_BOUNDARY: u64 = 140_384;
pub(super) const MATERIALIZATION_BOUNDARY: u64 = 664_721;
pub(super) const FOLLOW_UP_MS: i64 = 24 * 60 * 60 * 1000;
pub(super) const MINUTE_MS: f64 = 60_000.0;

#[derive(Serialize, Deserialize)]
pub(super) struct Quantity {
    pub id: String,
    pub name: String,
    pub population: String,
    pub unit: String,
    pub definition: String,
    pub denominator: usize,
    pub observed: usize,
    pub missing: usize,
    pub censored: usize,
    pub exclusions: String,
    pub values: Vec<ValueRow>,
    pub group_samples: BTreeMap<String, usize>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct ValueRow {
    pub label: String,
    pub value: f64,
}

#[derive(Serialize, Deserialize)]
pub(super) struct PublicTask {
    pub id: String,
    pub domain: String,
    pub selected: bool,
    pub admitted_day: String,
    pub closed_day: Option<String>,
    pub closure_route: String,
    pub phase: String,
    pub filing_lane: Option<String>,
    pub phase_authors: Vec<String>,
    pub review_verdicts: Vec<String>,
    pub review_lanes: Vec<Option<String>>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Case {
    pub id: String,
    pub title: String,
    pub reading: String,
    pub limitation: String,
    pub steps: Vec<CaseStep>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct CaseStep {
    pub task: String,
    pub hour: String,
    pub action: String,
    pub lane: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Publication {
    pub version: u32,
    pub title: String,
    pub horizon_day: String,
    pub first_delivery_day: String,
    pub last_delivery_day: String,
    pub calendar_zone: String,
    pub timestamp_policy: String,
    pub quantities: Vec<Quantity>,
    pub tasks: Vec<PublicTask>,
    pub cases: Vec<Case>,
    pub continuity: super::continuity::Continuity,
    pub campaign: super::relationships::Campaign,
    pub git: super::git::Concordance,
    pub provenance: Vec<super::provenance::Rule>,
}

#[derive(Serialize)]
struct AnalysisManifest {
    version: u32,
    input_sha256: String,
    oracle_sha256: String,
    consumer_sha256: String,
    publication_sha256: String,
    outcomes_sha256: String,
    schema: u32,
    timezone: &'static str,
}

pub(super) struct Aliases {
    pub tasks: BTreeMap<String, String>,
    pub lanes: BTreeMap<String, String>,
}

impl Aliases {
    pub fn new(study: &Study) -> Self {
        let tasks = study
            .tasks
            .keys()
            .enumerate()
            .map(|(index, key)| (key.clone(), format!("T{:04}", index + 1)))
            .collect();
        let mut lanes = std::collections::BTreeSet::new();
        for task in study.tasks.values() {
            for observation in std::iter::once(&task.admission)
                .chain(&task.phase_ends)
                .chain(task.reviews.iter().map(|review| &review.observation))
            {
                if let Some(lane) = observation
                    .actor
                    .as_str()
                    .and_then(|actor| actor.strip_prefix("lane:"))
                {
                    lanes.insert(lane.to_owned());
                }
            }
        }
        for fact in &study.lifecycle {
            if let Some(lane) = fact.body().get("lane").and_then(Value::as_str) {
                lanes.insert(lane.to_owned());
            }
        }
        let lanes = lanes
            .into_iter()
            .enumerate()
            .map(|(index, key)| (key, format!("L{:03}", index + 1)))
            .collect();
        Self { tasks, lanes }
    }

    pub fn actor(&self, actor: &Value) -> Option<String> {
        actor
            .as_str()
            .and_then(|actor| actor.strip_prefix("lane:"))
            .and_then(|lane| self.lanes.get(lane))
            .cloned()
    }
}

pub(super) fn publish(study: &Study, outcomes: &Outcomes, directory: &Path) -> Result<()> {
    let aliases = Aliases::new(study);
    let continuity = super::continuity::observe(study, outcomes, &aliases)?;
    let campaign = super::relationships::observe(study, directory, &aliases)?;
    let git = super::git::observe(study, directory)?;
    let provenance = super::provenance::rules(directory)?;
    let mut quantities = super::metrics::measure(study, outcomes)?;
    quantities.extend(super::continuity::quantities(&continuity));
    quantities.extend(super::git::quantities(&git));
    quantities.push(super::relationships::quantity(&campaign));
    quantities.push(quantity("Q29", "Bounded campaign edge frontier", "Typed relationships from every displayed campaign node", "edges",
        "Origin, blocking dependency, correction dependency and correction request are separate layers. External-public and unavailable frontiers are counted, not expanded into private context.",
        campaign.edges.len() + campaign.external_edges + campaign.unavailable_frontier_edges,
        [("Displayed typed edges".to_owned(), campaign.edges.len() as f64), ("Edges to external public nodes".to_owned(), campaign.external_edges as f64),
            ("Unavailable/publication-inadmissible frontier edges".to_owned(), campaign.unavailable_frontier_edges as f64)]));
    let cases = cases(study, &aliases, directory)?;
    let selected = study
        .selected
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    let publication = Publication {
        version: VERSION,
        title: "Making work land".to_owned(),
        horizon_day: day(&study.manifest.horizon_at.to_string()),
        first_delivery_day: day(&study.earliest_delivery.to_string()),
        last_delivery_day: day(&study.latest_delivery.to_string()),
        calendar_zone: "UTC".to_owned(),
        timestamp_policy: "Overview instants: day; curated case instants: hour; precise durations only in aggregate distributions and the attributed recorded materialization experiment.".to_owned(),
        quantities,
        tasks: study.tasks.values().map(|task| public_task(task, selected.contains(&task.birth), &aliases)).collect(),
        cases, continuity, campaign, git, provenance,
    };
    super::privacy::validate(&publication, study)?;
    derived_json(&directory.join("publication-data.json"), &publication)?;
    derived_json(
        &directory.join("identity-map-private.json"),
        &serde_json::json!({
            "tasks": aliases.tasks, "lanes": aliases.lanes,
            "evidence": {"E01": ["dS8s5DQTD", "dS977gL0K"], "E02": ["1kQyz32R"],
                "E03": ["dSw0Rz0zJ"], "E04": ["dTv81GDMK"], "E05": ["1kSgr8J0"]}
        }),
    )?;
    derived_json(
        &directory.join("analysis-manifest.json"),
        &AnalysisManifest {
            version: VERSION,
            input_sha256: study.manifest.facts_sha256.clone(),
            oracle_sha256: study.manifest.oracle_sha256.clone(),
            consumer_sha256: hash_file(&std::env::current_exe()?)?,
            publication_sha256: hash_file(&directory.join("publication-data.json"))?,
            outcomes_sha256: hash_file(&directory.join("phase-outcomes-private.json"))?,
            schema: study.manifest.pin.schema,
            timezone: "UTC",
        },
    )
}

fn public_task(task: &Task, selected: bool, aliases: &Aliases) -> PublicTask {
    PublicTask {
        id: aliases.tasks[&task.birth].clone(),
        domain: domain(&task.project).to_owned(),
        selected,
        admitted_day: day(&task.admission.at.to_string()),
        closed_day: task
            .closure
            .as_ref()
            .map(|closure| day(&closure.at.to_string())),
        closure_route: route(task).to_owned(),
        phase: phase(&task.phase).to_owned(),
        filing_lane: aliases.actor(&task.admission.actor),
        phase_authors: task
            .phase_ends
            .iter()
            .filter_map(|done| aliases.actor(&done.actor))
            .collect(),
        review_verdicts: task
            .reviews
            .iter()
            .map(|review| review.verdict.clone())
            .collect(),
        review_lanes: task
            .reviews
            .iter()
            .map(|review| aliases.actor(&review.observation.actor))
            .collect(),
    }
}

fn cases(study: &Study, aliases: &Aliases, directory: &Path) -> Result<Vec<Case>> {
    let command: wire::intent::ReportFlowRecord = serde_json::from_reader(File::open(
        directory.join("correction-command-record.json"),
    )?)?;
    if command.requested_flows != [Some("todo".to_owned())] {
        return Err("curated correction case lacks its retained explicit todo operand".into());
    }
    let specs = [
        (
            "E01",
            "Correction, then parent re-review",
            "A finding produced a correction with an explicitly requested implementation-only flow. The correction completed; the parent was then reviewed clean. These are ordered observations, not first-pass success.",
            "The request operand is proved by a retained command record, not by the resulting journal flow. The same lane authored implementation and review; this is not independent review.",
            vec!["dS8s5DQTD", "dS977gL0K"],
        ),
        (
            "E02",
            "A historical review-phase done",
            "Implementation and a later review-phase done were recorded on the same lane, without a review verdict. This is a distinct closure route.",
            "Do not relabel the legacy route as clean review or ordinary no-review completion.",
            vec!["1kQyz32R"],
        ),
        (
            "E03",
            "Unchanged bytes, avoided recompilation",
            "One recorded experiment compared fresh materialization, unchanged refill, and a small changed refill. The mechanism preserves source identity while avoiding needless timestamp changes.",
            "Historical recorded measurements, not fresh benchmarks, an era average, or proof of a whole-fleet speedup.",
            vec!["dSw0Rz0zJ"],
        ),
        (
            "E04",
            "Cross-lane filing, same-lane fix and review",
            "A delivered-key recognition problem crossed from a filing lane to a different implementation lane. The implementation lane also recorded clean review.",
            "A completed fix is not proof that every operational footgun was resolved. Incomplete admitted work remains visible at the horizon.",
            vec!["dTv81GDMK"],
        ),
        (
            "E05",
            "Blocking-policy integration",
            "Implementation completion records an integrated Git boundary; a later clean review records the policy task's closure.",
            "Neither observation identifies when a running appliance activated the policy. Timeline labels mean integrated, not activated.",
            vec!["1kSgr8J0"],
        ),
    ];
    specs
        .into_iter()
        .map(|(id, title, reading, limitation, births)| {
            let mut steps = Vec::new();
            for birth in births {
                let task = study
                    .tasks
                    .get(birth)
                    .ok_or("curated case is absent from admitted history")?;
                for observation in std::iter::once(&task.admission)
                    .chain(&task.phase_ends)
                    .chain(task.reviews.iter().map(|review| &review.observation))
                {
                    let action = match observation.kind.as_str() {
                        "task::added" => "admitted",
                        "task::done" if observation.phase == "review" => "legacy review-phase done",
                        "task::done" => "phase done",
                        "task::reviewed" => task
                            .reviews
                            .iter()
                            .find(|review| review.observation.seq == observation.seq)
                            .map(|review| review.verdict.as_str())
                            .ok_or("review observation missing")?,
                        _ => return Err("unclassified curated observation".into()),
                    };
                    steps.push((
                        observation.seq,
                        CaseStep {
                            task: aliases.tasks[birth].clone(),
                            hour: hour(&observation.at.to_string()),
                            action: action.to_owned(),
                            lane: aliases.actor(&observation.actor),
                        },
                    ));
                }
            }
            steps.sort_by_key(|(seq, _)| *seq);
            Ok(Case {
                id: id.to_owned(),
                title: title.to_owned(),
                reading: reading.to_owned(),
                limitation: limitation.to_owned(),
                steps: steps.into_iter().map(|(_, step)| step).collect(),
            })
        })
        .collect()
}

pub(super) fn route(task: &Task) -> &'static str {
    if task.deleted {
        return "deleted";
    }
    let Some(closure) = &task.closure else {
        return "incomplete at horizon";
    };
    match closure.kind.as_str() {
        "task::closed" => "operator closure",
        "task::reviewed"
            if task
                .reviews
                .last()
                .is_some_and(|review| review.verdict == "finding") =>
        {
            "historically terminal finding"
        }
        "task::reviewed" => "clean review",
        "task::done" if closure.phase == "review" => "legacy review-phase done",
        "task::done" => "ordinary flow-ending done",
        _ => "unclassified closure",
    }
}

/// Fixed public categories recognize whole labels; every other marker is redacted.
pub(super) fn domain(project: &str) -> &'static str {
    match project {
        "app" | "canvas" => "Interface",
        "gate" | "proof" => "Validation",
        "task" | "board" | "scheduler" => "Work coordination",
        "journal" | "bus" | "wire" | "tape" => "Evidence and protocol",
        "lane" | "vendor" | "listen" | "steer" => "Operating continuity",
        "dist" => "Distribution",
        "spice" | "doctrine" | "rim" | "sill" | "fold" | "taste" => "Appliance and policy",
        _ => "Other admitted work",
    }
}

pub(super) fn phase(value: &str) -> &'static str {
    match value {
        "plan" => "plan",
        "todo" => "todo",
        "implementation" => "implementation",
        "review" => "review",
        _ => "unclassified phase",
    }
}

pub(super) fn day(instant: &str) -> String {
    instant.chars().take("YYYY-MM-DD".len()).collect()
}
pub(super) fn hour(instant: &str) -> String {
    format!(
        "{}:00 UTC",
        instant
            .chars()
            .take("YYYY-MM-DDTHH".len())
            .collect::<String>()
            .replace('T', " ")
    )
}

pub(super) fn derived_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = path.with_extension("json.new");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = BufWriter::new(options.open(&temporary)?);
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.flush()?;
    file.get_ref().sync_all()?;
    fs::rename(temporary, path)?;
    Ok(())
}

pub(super) fn quantity(
    id: &str,
    name: &str,
    population: &str,
    unit: &str,
    definition: &str,
    denominator: usize,
    rows: impl IntoIterator<Item = (String, f64)>,
) -> Quantity {
    Quantity {
        id: id.to_owned(),
        name: name.to_owned(),
        population: population.to_owned(),
        unit: unit.to_owned(),
        definition: definition.to_owned(),
        denominator,
        observed: denominator,
        missing: 0,
        censored: 0,
        exclusions: "None beyond the stated population.".to_owned(),
        group_samples: BTreeMap::new(),
        values: rows
            .into_iter()
            .map(|(label, value)| ValueRow { label, value })
            .collect(),
    }
}

#[cfg(test)]
mod tests;
