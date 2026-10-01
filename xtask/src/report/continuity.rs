//! Continuity follows recorded lane/provider identity, never occupant counts.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use super::Result;
use super::episodes::Outcomes;
use super::publication::{Aliases, MINUTE_MS, Quantity, day, quantity};
use super::reduce::Study;

#[derive(Serialize, Deserialize)]
pub(super) struct SessionSpan {
    pub id: String,
    pub lane: String,
    pub first_day: String,
    pub last_day: String,
    pub configurations: Vec<String>,
    pub declarations: usize,
    pub compactions: usize,
    pub starts: usize,
    pub exits: usize,
}

#[derive(Serialize, Deserialize)]
pub(super) struct LaneExposure {
    pub lane: String,
    pub held_minutes: f64,
    pub admitted_episodes: usize,
    pub provider_sessions: usize,
    pub compactions: usize,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Continuity {
    pub sessions: Vec<SessionSpan>,
    pub lanes: Vec<LaneExposure>,
    pub events: BTreeMap<String, usize>,
    pub unattributed_events: usize,
    pub retunes: usize,
    pub unknown_declarations: usize,
}

struct Span {
    first: wire::Instant,
    last: wire::Instant,
    configurations: std::collections::BTreeSet<String>,
    declarations: usize,
    compactions: usize,
    starts: usize,
    exits: usize,
}

type SessionKey = (String, String, String);

/// Provider-session spans folded from ordered lane facts.
#[derive(Default)]
struct Ledger {
    spans: BTreeMap<SessionKey, Span>,
    occupants: BTreeMap<String, SessionKey>,
    current: BTreeMap<String, SessionKey>,
    configurations: BTreeMap<String, String>,
    counts: BTreeMap<String, usize>,
    unknown: usize,
    unknown_declarations: usize,
    retunes: usize,
}

impl Ledger {
    fn configuration(
        &mut self,
        vendor: &str,
        body: &serde_json::Map<String, Value>,
    ) -> Result<String> {
        let config = serde_json::to_string(&(vendor, body.get("model"), body.get("effort")))?;
        let next = format!("C{:03}", self.configurations.len() + 1);
        Ok(self.configurations.entry(config).or_insert(next).clone())
    }

    fn fold(&mut self, fact: &wire::Envelope<wire::AnyFact>, window: wire::Instant) -> Result<()> {
        let kind = fact.fact.kind().as_str();
        if !kind.starts_with("lane::") {
            return Ok(());
        }
        let body = fact.fact.body();
        let in_window = fact.at >= window;
        if in_window {
            *self.counts.entry(kind.to_owned()).or_default() += 1;
        }
        let Some(lane) = string(body, "lane") else {
            self.unknown += usize::from(in_window);
            return Ok(());
        };
        if kind == "lane::declared" {
            return self.declare(fact, lane, in_window);
        }
        let key = match string(body, "occupant") {
            Some(occupant) => self.occupants.get(occupant),
            None if body.get("occupant").is_none() => self.current.get(lane),
            None => None,
        };
        let Some(key) = key.cloned() else {
            self.unknown += usize::from(in_window);
            return Ok(());
        };
        let config = if kind == "lane::retuned" {
            self.retunes += usize::from(in_window);
            Some(self.configuration(&key.1, body)?)
        } else {
            None
        };
        let span = self
            .spans
            .get_mut(&key)
            .ok_or("provider-session span disappeared")?;
        span.last = fact.at;
        match kind {
            "lane::compacted" => span.compactions += 1,
            "lane::started" => span.starts += 1,
            "lane::exited" => span.exits += 1,
            _ => {}
        }
        span.configurations.extend(config);
        Ok(())
    }

    fn declare(
        &mut self,
        fact: &wire::Envelope<wire::AnyFact>,
        lane: &str,
        in_window: bool,
    ) -> Result<()> {
        let body = fact.fact.body();
        let (Some(vendor), Some(identity)) = (string(body, "vendor"), string(body, "identity"))
        else {
            self.unknown_declarations += usize::from(in_window);
            return Ok(());
        };
        let key = (lane.to_owned(), vendor.to_owned(), identity.to_owned());
        let config = self.configuration(vendor, body)?;
        let span = self.spans.entry(key.clone()).or_insert_with(|| Span {
            first: fact.at,
            last: fact.at,
            configurations: std::collections::BTreeSet::new(),
            declarations: 0,
            compactions: 0,
            starts: 0,
            exits: 0,
        });
        span.last = fact.at;
        span.declarations += 1;
        span.configurations.insert(config);
        if let Some(occupant) = string(body, "occupant") {
            self.occupants.insert(occupant.to_owned(), key.clone());
        }
        self.current.insert(lane.to_owned(), key);
        Ok(())
    }
}

impl LaneExposure {
    fn empty(lane: String) -> Self {
        Self {
            lane,
            held_minutes: 0.0,
            admitted_episodes: 0,
            provider_sessions: 0,
            compactions: 0,
        }
    }
}

pub(super) fn observe(study: &Study, outcomes: &Outcomes, aliases: &Aliases) -> Result<Continuity> {
    let mut ledger = Ledger::default();
    for fact in &study.lifecycle {
        ledger.fold(fact, study.earliest_delivery)?;
    }
    let mut lane_rows = BTreeMap::<String, LaneExposure>::new();
    let mut sessions = Vec::new();
    for (key, span) in ledger
        .spans
        .iter()
        .filter(|(_, span)| span.last >= study.earliest_delivery)
    {
        let lane = aliases
            .lanes
            .get(&key.0)
            .ok_or("continuity lane lacks publication alias")?
            .clone();
        let row = lane_rows
            .entry(lane.clone())
            .or_insert_with(|| LaneExposure::empty(lane.clone()));
        row.provider_sessions += 1;
        row.compactions += span.compactions;
        sessions.push(SessionSpan {
            id: format!("S{:03}", sessions.len() + 1),
            lane,
            first_day: day(&span.first.to_string()),
            last_day: day(&span.last.to_string()),
            configurations: span.configurations.iter().cloned().collect(),
            declarations: span.declarations,
            compactions: span.compactions,
            starts: span.starts,
            exits: span.exits,
        });
    }
    expose(study, outcomes, aliases, &mut lane_rows)?;
    Ok(Continuity {
        sessions,
        lanes: lane_rows.into_values().collect(),
        events: ledger.counts,
        unattributed_events: ledger.unknown,
        retunes: ledger.retunes,
        unknown_declarations: ledger.unknown_declarations,
    })
}

/// Add admitted claim episodes and their horizon-capped held time per lane.
fn expose(
    study: &Study,
    outcomes: &Outcomes,
    aliases: &Aliases,
    lane_rows: &mut BTreeMap<String, LaneExposure>,
) -> Result<()> {
    for episode in outcomes
        .episodes
        .iter()
        .filter(|episode| episode.admitted >= study.earliest_delivery)
    {
        let lane = aliases
            .lanes
            .get(&episode.lane)
            .ok_or("claim lane lacks publication alias")?
            .clone();
        let row = lane_rows
            .entry(lane.clone())
            .or_insert_with(|| LaneExposure::empty(lane));
        row.admitted_episodes += 1;
        if !episode.transition_broken {
            let end = episode
                .ended
                .unwrap_or(study.manifest.horizon_at)
                .min(study.manifest.horizon_at);
            let duration = end.get().as_millisecond() - episode.admitted.get().as_millisecond();
            if duration >= 0 {
                row.held_minutes += duration as f64 / MINUTE_MS;
            }
        }
    }
    Ok(())
}

pub(super) fn quantities(continuity: &Continuity) -> Vec<Quantity> {
    vec![
        quantity(
            "Q21",
            "Observed continuity identities",
            "Lane and provider-session spans intersecting the delivery window",
            "identities",
            "Provider session identity is the recorded (lane, vendor, identity) tuple. Declarations, starts and occupants are not worker or session counts.",
            continuity.sessions.len(),
            [
                (
                    "Lane identities with recorded exposure or declarations".to_owned(),
                    continuity.lanes.len() as f64,
                ),
                (
                    "Distinct provider-session spans".to_owned(),
                    continuity.sessions.len() as f64,
                ),
            ],
        ),
        quantity(
            "Q22",
            "Lifecycle event coverage",
            "Recorded lane events inside the delivery window",
            "events",
            "Event-time declarations and retunes are the only effective-configuration observations. Events without a resolvable session attribution remain unknown.",
            continuity.events.values().sum(),
            continuity
                .events
                .iter()
                .map(|(kind, count)| (kind.clone(), *count as f64)),
        ),
        quantity(
            "Q23",
            "Continuity attribution limits",
            "Recorded continuity observations",
            "observations",
            "Unattributed lifecycle events and declarations without provider identity are distinct coverage limitations; configuration aliases do not imply equivalent models.",
            continuity.events.values().sum(),
            [
                (
                    "Unattributed lifecycle events".to_owned(),
                    continuity.unattributed_events as f64,
                ),
                (
                    "Declarations missing provider identity".to_owned(),
                    continuity.unknown_declarations as f64,
                ),
                (
                    "Observed in-window retunes".to_owned(),
                    continuity.retunes as f64,
                ),
            ],
        ),
        quantity(
            "Q24",
            "Lane-held wall exposure",
            "Non-review claim episodes admitted inside the delivery window",
            "minutes",
            "Recorded admission to terminal boundary or horizon. Episode overlaps and missing transitions prevent interpreting these minutes as labor or CPU utilization.",
            continuity
                .lanes
                .iter()
                .map(|lane| lane.admitted_episodes)
                .sum(),
            continuity
                .lanes
                .iter()
                .map(|lane| (lane.lane.clone(), lane.held_minutes)),
        ),
    ]
}

fn string<'a>(body: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    body.get(key).and_then(Value::as_str)
}
