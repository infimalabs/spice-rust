//! Ordered task and phase observations; all exact identities remain private.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufRead as _, BufReader, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use wire::{FactKind, Instant, Seq, StreamFact};

use super::{Result, SnapshotManifest, hash_file};

const COHORT_SIZE: usize = 1500;

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Observation {
    pub seq: Seq,
    pub at: Instant,
    pub kind: String,
    pub phase: String,
    pub actor: Value,
    pub cause: Option<Seq>,
    pub git: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Review {
    pub observation: Observation,
    pub verdict: String,
    pub author: Option<Value>,
    pub correction_births: Vec<String>,
    pub parent_statement_changed: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Task {
    pub birth: String,
    pub project: String,
    pub admission: Observation,
    pub flow: Vec<String>,
    pub phase: String,
    pub deleted: bool,
    pub closure: Option<Observation>,
    pub reviews: Vec<Review>,
    pub phase_ends: Vec<Observation>,
    pub withdrawals: Vec<Observation>,
    pub first_claim: Option<Observation>,
    pub claims: Vec<Observation>,
    pub origin: Option<Seq>,
    pub creation: Option<Value>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Study {
    pub version: u32,
    pub manifest: SnapshotManifest,
    pub selected: Vec<String>,
    pub tasks: BTreeMap<String, Task>,
    pub selected_fingerprint: String,
    pub routes: BTreeMap<String, usize>,
    pub projects: BTreeMap<String, usize>,
    pub earliest_delivery: Instant,
    pub latest_delivery: Instant,
    pub lifecycle: Vec<StreamFact>,
}

/// The independent board capture: a `spice board --output FILE` export, or the
/// reply envelope an earlier `spice board --json` printed.
#[derive(Deserialize)]
#[serde(untagged)]
enum Oracle {
    Board(OracleBody),
    Reply { cursor: Seq, body: OracleBody },
}

impl Oracle {
    /// The captured board, when every cursor it names is `upper`.
    fn at(self, upper: Seq) -> Option<OracleBody> {
        let (cursor, body) = match self {
            Self::Board(body) => (body.cursor, body),
            Self::Reply { cursor, body } => (cursor, body),
        };
        (cursor == upper && body.cursor == upper).then_some(body)
    }
}

#[derive(Deserialize)]
struct OracleBody {
    cursor: Seq,
    tasks: Vec<OracleTask>,
}

#[derive(Deserialize)]
struct OracleTask {
    statement: Positioned<OracleStatement>,
    workflow: Positioned<OracleWorkflow>,
}

#[derive(Deserialize)]
struct Positioned<T> {
    position: Seq,
    body: T,
}

#[derive(Deserialize)]
struct OracleStatement {
    born: String,
}

#[derive(Deserialize)]
struct OracleWorkflow {
    status: String,
}

pub(super) fn analyze(directory: &Path, output: &mut dyn Write) -> Result<()> {
    let manifest: SnapshotManifest =
        serde_json::from_reader(File::open(directory.join("snapshot-manifest.json"))?)?;
    if manifest.version != super::SNAPSHOT_VERSION
        || hash_file(&directory.join("facts.ndjson"))? != manifest.facts_sha256
        || hash_file(&directory.join("independent-board-oracle.json"))? != manifest.oracle_sha256
    {
        return Err("snapshot version or frozen evidence digest mismatch".into());
    }
    let mut history = History::default();
    let mut previous = None;
    let mut fact_count = 0;
    let mut line = String::new();
    let mut input = BufReader::new(File::open(directory.join("facts.ndjson"))?);
    while input.read_line(&mut line)? != 0 {
        let fact: StreamFact = serde_json::from_str(&line)?;
        if fact.seq() > manifest.upper || previous.is_some_and(|seq| fact.seq() <= seq) {
            return Err("snapshot is not a strictly ordered historical prefix".into());
        }
        previous = Some(fact.seq());
        fact_count += 1;
        history.push(fact)?;
        line.clear();
    }
    if fact_count != manifest.fact_count {
        return Err("snapshot fact count changed".into());
    }
    let History {
        tasks, lifecycle, ..
    } = history;
    let mut completed = tasks
        .values()
        .filter(|task| !task.deleted && task.closure.is_some())
        .collect::<Vec<_>>();
    completed.sort_by(|left, right| {
        left.closure
            .as_ref()
            .expect("completed")
            .seq
            .cmp(&right.closure.as_ref().expect("completed").seq)
            .then_with(|| left.birth.cmp(&right.birth))
    });
    if completed.len() < COHORT_SIZE {
        return Err(format!(
            "only {} reconstructed public completions; need 1500",
            completed.len()
        )
        .into());
    }
    let selected = completed[completed.len() - COHORT_SIZE..]
        .iter()
        .map(|task| task.birth.clone())
        .collect::<Vec<_>>();
    reconcile(directory, &manifest, &completed, &selected)?;
    let earliest_delivery = completed[completed.len() - COHORT_SIZE]
        .closure
        .as_ref()
        .expect("completed")
        .at;
    let latest_delivery = completed
        .last()
        .expect("population checked")
        .closure
        .as_ref()
        .expect("completed")
        .at;
    let mut routes = BTreeMap::new();
    let mut projects = BTreeMap::new();
    for task in &completed[completed.len() - COHORT_SIZE..] {
        *routes.entry(route(task).to_owned()).or_default() += 1;
        *projects.entry(task.project.clone()).or_default() += 1;
    }
    routes.entry("operator closure".to_owned()).or_default();
    let mut ordered_births = selected.clone();
    ordered_births.sort();
    let selected_fingerprint =
        format!("{:x}", Sha256::digest(ordered_births.join("\n").as_bytes()));
    let study = Study {
        version: 1,
        manifest,
        selected,
        tasks,
        selected_fingerprint,
        routes,
        projects,
        earliest_delivery,
        latest_delivery,
        lifecycle,
    };
    let outcomes = super::episodes::measure(&study)?;
    super::publication::derived_json(&directory.join("phase-outcomes-private.json"), &outcomes)?;
    super::publication::derived_json(&directory.join("study-private.json"), &study)?;
    super::publication::publish(&study, &outcomes, directory)?;
    writeln!(
        output,
        "reconciled 1500 distinct deliveries; selected fingerprint {}",
        study.selected_fingerprint
    )?;
    Ok(())
}

/// Task histories and non-task lifecycle facts folded in sequence order.
#[derive(Default)]
struct History {
    tasks: BTreeMap<String, Task>,
    reviews: BTreeMap<Seq, (String, usize)>,
    lifecycle: Vec<StreamFact>,
}

impl History {
    fn push(&mut self, fact: StreamFact) -> Result<()> {
        fold_task(&fact, &mut self.tasks, &mut self.reviews)?;
        if !matches!(
            fact.kind(),
            FactKind::TaskAdded
                | FactKind::TaskPhased
                | FactKind::TaskDone
                | FactKind::TaskReviewed
                | FactKind::TaskAssigned
                | FactKind::TaskDeferred
                | FactKind::TaskWoken
                | FactKind::TaskEdited
                | FactKind::TaskDeleted
                | FactKind::TaskClosed
                | FactKind::TaskConcluded
        ) {
            self.lifecycle.push(fact);
        }
        Ok(())
    }
}

fn fold_task(
    fact: &StreamFact,
    tasks: &mut BTreeMap<String, Task>,
    reviews: &mut BTreeMap<Seq, (String, usize)>,
) -> Result<()> {
    let body = fact.body();
    let Some(birth) = body.get("task").and_then(Value::as_str) else {
        return Ok(());
    };
    if fact.kind() == FactKind::TaskAdded {
        return fold_admission(fact, birth, tasks, reviews);
    }
    let Some(task) = tasks.get_mut(birth) else {
        if fact.task().is_some() {
            return Err(format!("task history has no admission: {birth} at {}", fact.seq()).into());
        }
        return Ok(());
    };
    match fact.kind() {
        FactKind::TaskPhased => {
            task.phase = text(body, "phase")?.to_owned();
            task.closure = None;
        }
        FactKind::TaskDone => {
            let done = observation(fact, &task.phase)?;
            task.phase_ends.push(done);
            if task.flow.last() == Some(&task.phase) {
                task.closure = Some(observation(fact, &task.phase)?);
            }
        }
        FactKind::TaskReviewed => {
            let verdict = text(body, "verdict")?.to_owned();
            let author = task
                .phase_ends
                .iter()
                .rev()
                .find(|done| done.phase != "review")
                .map(|done| done.actor.clone());
            // A finding is a historical terminal candidate, not an asserted reopening.
            // Continuation and a causal parent statement revision invalidate it before selection.
            task.closure = Some(observation(fact, &task.phase)?);
            let index = task.reviews.len();
            task.reviews.push(Review {
                observation: observation(fact, &task.phase)?,
                verdict,
                author,
                correction_births: Vec::new(),
                parent_statement_changed: false,
            });
            reviews.insert(fact.seq(), (birth.to_owned(), index));
        }
        FactKind::TaskEdited => {
            if let Some((parent, index)) = fact.cause().and_then(|cause| reviews.get(&cause))
                && parent == birth
                && task.reviews[*index].verdict == "finding"
            {
                task.reviews[*index].parent_statement_changed = true;
                task.closure = None;
            }
        }
        FactKind::TaskAssigned => {
            task.closure = None;
        }
        FactKind::ClaimWon => {
            let phase = body
                .get("cell")
                .and_then(|cell| cell.get("phase"))
                .and_then(Value::as_str)
                .unwrap_or(&task.phase);
            let claim = observation(fact, phase)?;
            if task.first_claim.is_none() {
                task.first_claim = Some(observation(fact, phase)?);
            }
            task.claims.push(claim);
            task.closure = None;
        }
        FactKind::TaskDeleted | FactKind::TaskClosed | FactKind::TaskConcluded => {
            fold_settlement(fact, task)?;
        }
        _ => {}
    }
    Ok(())
}

/// Fold one operator settlement: withdrawal, closure, or Design conclusion,
/// where a collapsed conclusion counts as a withdrawal.
fn fold_settlement(fact: &StreamFact, task: &mut Task) -> Result<()> {
    let body = fact.body();
    match fact.kind() {
        FactKind::TaskDeleted => {
            task.withdrawals.push(observation(fact, &task.phase)?);
            task.deleted = true;
            task.closure = None;
        }
        FactKind::TaskClosed => {
            task.withdrawals
                .push(observation(fact, text(body, "phase")?)?);
            task.closure = Some(observation(fact, text(body, "phase")?)?);
        }
        FactKind::TaskConcluded => {
            let collapsed = body
                .get("conclusion")
                .and_then(|conclusion| conclusion.get("outcome"))
                .and_then(Value::as_str)
                == Some("collapsed");
            if collapsed {
                task.withdrawals.push(observation(fact, &task.phase)?);
            }
            task.closure = Some(observation(fact, &task.phase)?);
        }
        _ => {}
    }
    Ok(())
}

fn fold_admission(
    fact: &StreamFact,
    birth: &str,
    tasks: &mut BTreeMap<String, Task>,
    reviews: &BTreeMap<Seq, (String, usize)>,
) -> Result<()> {
    let body = fact.body();
    let phase = text(body, "phase")?.to_owned();
    let task = Task {
        birth: birth.to_owned(),
        project: text(body, "project")?.to_owned(),
        admission: observation(fact, &phase)?,
        flow: body
            .get("flow")
            .and_then(Value::as_array)
            .ok_or("task admission has no flow")?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "phase is not a string".into())
            })
            .collect::<Result<_>>()?,
        phase,
        deleted: false,
        closure: None,
        reviews: Vec::new(),
        phase_ends: Vec::new(),
        withdrawals: Vec::new(),
        first_claim: None,
        claims: Vec::new(),
        origin: fact.cause(),
        creation: body.get("creation").cloned(),
    };
    if tasks.insert(birth.to_owned(), task).is_some() {
        return Err("duplicate task birth".into());
    }
    if let Some((parent, index)) = fact.cause().and_then(|cause| reviews.get(&cause)) {
        tasks
            .get_mut(parent)
            .ok_or("correction parent absent")?
            .reviews[*index]
            .correction_births
            .push(birth.to_owned());
    }
    Ok(())
}

fn observation(fact: &StreamFact, phase: &str) -> Result<Observation> {
    Ok(Observation {
        seq: fact.seq(),
        at: *fact.at(),
        kind: fact.kind().as_str().to_owned(),
        phase: phase.to_owned(),
        actor: serde_json::to_value(fact.actor())?,
        cause: fact.cause(),
        git: fact.body().get("git").cloned(),
    })
}

fn text<'a>(body: &'a serde_json::Map<String, Value>, field: &str) -> Result<&'a str> {
    body.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("required report field {field} absent").into())
}

fn route(task: &Task) -> &'static str {
    let closure = task
        .closure
        .as_ref()
        .expect("route only for completed task");
    match closure.kind.as_str() {
        "task::closed" => "operator closure",
        "task::reviewed" => {
            if task
                .reviews
                .last()
                .is_some_and(|review| review.verdict == "finding")
            {
                "historically terminal finding"
            } else {
                "clean review"
            }
        }
        "task::done" if closure.phase == "review" => "legacy review-phase done",
        "task::done" => "ordinary flow-ending done",
        _ => "unclassified closure",
    }
}

fn reconcile(
    directory: &Path,
    manifest: &SnapshotManifest,
    completed: &[&Task],
    selected: &[String],
) -> Result<()> {
    let oracle: Oracle =
        serde_json::from_reader(File::open(directory.join("independent-board-oracle.json"))?)?;
    let Some(oracle) = oracle.at(manifest.upper) else {
        return Err("independent oracle has a different barrier".into());
    };
    let mut observed = oracle
        .tasks
        .iter()
        .filter(|task| task.workflow.body.status == "completed")
        .collect::<Vec<_>>();
    observed.sort_by(|left, right| {
        left.workflow
            .position
            .cmp(&right.workflow.position)
            .then_with(|| left.statement.body.born.cmp(&right.statement.body.born))
    });
    if observed.len() < COHORT_SIZE {
        return Err("oracle has fewer than 1500 public completed tasks".into());
    }
    let expected = observed[observed.len() - COHORT_SIZE..]
        .iter()
        .map(|task| task.statement.body.born.as_str())
        .collect::<BTreeSet<_>>();
    let actual = selected.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let expected_all = observed
        .iter()
        .map(|task| task.statement.body.born.as_str())
        .collect::<BTreeSet<_>>();
    let actual_all = completed
        .iter()
        .map(|task| task.birth.as_str())
        .collect::<BTreeSet<_>>();
    let discrepancy = serde_json::json!({
        "upper": manifest.upper, "oracle_sha256": manifest.oracle_sha256,
        "reconstructed_completed": actual_all.len(), "oracle_completed": expected_all.len(),
        "selected": actual.len(), "missing_selected": expected.difference(&actual).collect::<Vec<_>>(),
        "extra_selected": actual.difference(&expected).collect::<Vec<_>>(),
        "missing_completed": expected_all.difference(&actual_all).collect::<Vec<_>>(),
        "extra_completed": actual_all.difference(&expected_all).collect::<Vec<_>>(),
    });
    super::publication::derived_json(&directory.join("cohort-reconciliation.json"), &discrepancy)?;
    if expected != actual || expected_all != actual_all || actual.len() != COHORT_SIZE {
        return Err("historical reduction disagrees with the independent board oracle; see cohort-reconciliation.json".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
