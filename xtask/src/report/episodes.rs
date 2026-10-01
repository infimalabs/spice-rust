//! Claim-lane episodes with a common follow-up and explicit missing coverage.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wire::{AnyFact, Envelope, FactKind, Instant, Seq};

use super::Result;
use super::reduce::{Observation, Study};

const FOLLOW_UP_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Serialize, Deserialize)]
pub(super) struct Episode {
    pub task: String,
    pub claim: Seq,
    pub phase: String,
    pub lane: String,
    pub occupant: String,
    pub admitted: Instant,
    pub until: Instant,
    pub coverage_broken: bool,
    pub transition_broken: bool,
    pub ended: Option<Instant>,
    pub terminal_seq: Option<Seq>,
    pub outcome: String,
    pub eligible: bool,
    pub right_censored: bool,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Outcomes {
    pub follow_up_hours: usize,
    pub admission_start: Instant,
    pub admission_end_ms: i64,
    pub episodes: Vec<Episode>,
    pub by_phase: BTreeMap<String, BTreeMap<String, usize>>,
    pub eligible: usize,
    pub right_censored: usize,
    pub missing_history: usize,
    pub waiting_minutes: Vec<f64>,
    pub held_minutes: Vec<f64>,
    pub invalid_clock_intervals: usize,
}

enum Event<'a> {
    Fact(&'a Envelope<AnyFact>),
    End(&'a str, &'a Observation),
}

impl Event<'_> {
    fn seq(&self) -> Seq {
        match self {
            Self::Fact(fact) => fact.seq,
            Self::End(_, observation) => observation.seq,
        }
    }
}

pub(super) fn measure(study: &Study) -> Result<Outcomes> {
    let start = study.earliest_delivery;
    let end_ms = study.manifest.horizon_at.get().as_millisecond() - FOLLOW_UP_MS;
    let mut events = study.lifecycle.iter().map(Event::Fact).collect::<Vec<_>>();
    for (birth, task) in &study.tasks {
        events.extend(
            task.phase_ends
                .iter()
                .map(|observation| Event::End(birth, observation)),
        );
        events.extend(
            task.withdrawals
                .iter()
                .map(|observation| Event::End(birth, observation)),
        );
    }
    events.sort_unstable_by_key(Event::seq);
    let mut active = BTreeMap::<String, Episode>::new();
    let mut finished = Vec::new();
    for event in events {
        match event {
            Event::End(birth, observation) => {
                let outcome = match observation.kind.as_str() {
                    "task::done" => "phase completed",
                    "task::deleted" => "deleted",
                    "task::closed" => "operator closed",
                    _ => return Err("unknown claimed-phase terminal observation".into()),
                };
                if let Some(episode) = active.remove(birth) {
                    let outcome =
                        if observation.kind == "task::done" && observation.phase != episode.phase {
                            "missing history"
                        } else {
                            outcome
                        };
                    finish(
                        episode,
                        observation.at,
                        observation.seq,
                        outcome,
                        &mut finished,
                    );
                }
            }
            Event::Fact(fact) => transition(fact, study, &mut active, &mut finished)?,
        }
    }
    finished.extend(active.into_values());
    let mut result = Outcomes {
        follow_up_hours: 24,
        admission_start: start,
        admission_end_ms: end_ms,
        episodes: finished,
        by_phase: BTreeMap::new(),
        eligible: 0,
        right_censored: 0,
        missing_history: 0,
        waiting_minutes: Vec::new(),
        held_minutes: Vec::new(),
        invalid_clock_intervals: 0,
    };
    for episode in &mut result.episodes {
        if episode.phase == "review" || episode.admitted < start {
            continue;
        }
        let admitted_ms = episode.admitted.get().as_millisecond();
        if admitted_ms > end_ms {
            episode.right_censored = true;
            result.right_censored += 1;
            continue;
        }
        episode.eligible = true;
        result.eligible += 1;
        let deadline = admitted_ms + FOLLOW_UP_MS;
        let terminal_ms = episode.ended.map(|at| at.get().as_millisecond());
        if episode.transition_broken || terminal_ms.is_some_and(|at| at < admitted_ms) {
            episode.outcome = "missing history".to_owned();
            result.invalid_clock_intervals += 1;
        } else if terminal_ms.is_some_and(|at| at <= deadline) {
            // An end already judged unwitnessed bounds no held interval.
            if episode.outcome != "missing history" {
                result
                    .held_minutes
                    .push((terminal_ms.expect("terminal exists") - admitted_ms) as f64 / 60_000.0);
            }
        } else if !episode.coverage_broken && episode.until.get().as_millisecond() >= deadline {
            episode.outcome = "still held at 24h".to_owned();
        } else {
            episode.outcome = "missing history".to_owned();
        }
        if episode.outcome == "missing history" {
            result.missing_history += 1;
        }
        *result
            .by_phase
            .entry(episode.phase.clone())
            .or_default()
            .entry(episode.outcome.clone())
            .or_default() += 1;
        let task = &study.tasks[&episode.task];
        if task
            .first_claim
            .as_ref()
            .is_some_and(|claim| claim.seq == episode.claim)
        {
            let waiting = admitted_ms - task.admission.at.get().as_millisecond();
            if waiting >= 0 {
                result.waiting_minutes.push(waiting as f64 / 60_000.0);
            } else {
                result.invalid_clock_intervals += 1;
            }
        }
    }
    result.waiting_minutes.sort_by(f64::total_cmp);
    result.held_minutes.sort_by(f64::total_cmp);
    Ok(result)
}

fn transition(
    fact: &Envelope<AnyFact>,
    study: &Study,
    active: &mut BTreeMap<String, Episode>,
    finished: &mut Vec<Episode>,
) -> Result<()> {
    let body = fact.fact.body();
    let Some(task) = body.get("task").and_then(Value::as_str) else {
        return Ok(());
    };
    match fact.fact.kind() {
        FactKind::ClaimWon => {
            let lane = field(body, "lane")?;
            let occupant = field(body, "occupant")?;
            let phase = study
                .tasks
                .get(task)
                .and_then(|task| task.claims.iter().find(|claim| claim.seq == fact.seq))
                .map(|claim| claim.phase.as_str())
                .ok_or("claim has no ordered phase witness")?;
            if let Some(previous) = active.remove(task) {
                let outcome = if previous.lane != lane && previous.phase == phase {
                    "derived transfer"
                } else {
                    "missing history"
                };
                finish(previous, fact.at, fact.seq, outcome, finished);
            }
            if phase == "review" {
                return Ok(());
            }
            let until = field(body, "until")?.parse()?;
            active.insert(
                task.to_owned(),
                Episode {
                    task: task.to_owned(),
                    claim: fact.seq,
                    phase: phase.to_owned(),
                    lane: lane.to_owned(),
                    occupant: occupant.to_owned(),
                    admitted: fact.at,
                    until,
                    coverage_broken: false,
                    transition_broken: false,
                    ended: None,
                    terminal_seq: None,
                    outcome: "unsettled".to_owned(),
                    eligible: false,
                    right_censored: false,
                },
            );
        }
        FactKind::ClaimFreed => {
            if let Some(episode) = active.remove(task) {
                finish(episode, fact.at, fact.seq, "released", finished);
            }
        }
        FactKind::ClaimLapsed => {
            let occupant = field(body, "occupant")?;
            if active
                .get(task)
                .is_some_and(|episode| episode.occupant == occupant)
            {
                let episode = active.remove(task).expect("matched canonical occupant");
                finish(episode, fact.at, fact.seq, "lapsed", finished);
            } else if let Some(mut episode) = active.remove(task) {
                episode.coverage_broken = true;
                finish(episode, fact.at, fact.seq, "missing history", finished);
            }
        }
        FactKind::ClaimCarried | FactKind::ClaimRebound => {
            if let Some(episode) = active.get_mut(task) {
                if field(body, "lane")? != episode.lane
                    || field(body, "ancestor")? != episode.occupant
                {
                    episode.coverage_broken = true;
                    episode.transition_broken = true;
                }
                episode.occupant = field(body, "occupant")?.to_owned();
                extend_lease(fact, episode)?;
            }
        }
        FactKind::ClaimRenewed => {
            if let Some(episode) = active.get_mut(task) {
                extend_lease(fact, episode)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn extend_lease(fact: &Envelope<AnyFact>, episode: &mut Episode) -> Result<()> {
    if let Some(until) = fact.fact.body().get("until").and_then(Value::as_str) {
        if fact.at > episode.until {
            episode.coverage_broken = true;
        }
        let until: Instant = until.parse()?;
        if until < episode.until {
            episode.coverage_broken = true;
        }
        episode.until = until;
    }
    Ok(())
}

fn finish(mut episode: Episode, at: Instant, seq: Seq, outcome: &str, finished: &mut Vec<Episode>) {
    episode.ended = Some(at);
    episode.terminal_seq = Some(seq);
    episode.outcome = outcome.to_owned();
    finished.push(episode);
}

fn field<'a>(body: &'a serde_json::Map<String, Value>, key: &str) -> Result<&'a str> {
    body.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("canonical claim field {key} missing").into())
}
