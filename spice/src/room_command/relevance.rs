//! `spice room relevance HANDLE`: one room's shadow relevance observations.
//!
//! Each observation names its journal horizon, its classification and
//! progress standing, the exact available or unavailable sources it cites,
//! its explanation, exceptions, and uncertainty, and the operator's
//! assessment when one exists. Aggregate metrics cover the whole room at the
//! read position, whatever page is shown. Observations page whole in identity
//! order; a page that withholds any continues at the first withheld one.
//! Reading changes no observation, assessment, room, task, delivery, or
//! authorization.

use std::fmt;

use clap::ArgMatches;
use serde::Serialize;
use spice::repository::Repository;
use wire::output::Bounded;
use wire::rooms::{
    RoomAuthorityStamp, RoomRelevanceAssessment, RoomRelevanceObservation, RoomSourceView,
};
use wire::{FaultClass, Outcome, Reply, ReplyPosition, Seq, TaskHandle, TaskStamp};

use super::{Failure, Source, Unavailable, current_handle, discover, fault, store};
use crate::read_page::Page;

/// What a continuation's `--after` names.
const CURSOR: &str = "an observation of this room";

/// One page of a room's shadow observations with whole-room metrics.
#[derive(Debug, Serialize)]
pub(crate) struct RoomRelevance {
    room: TaskHandle,
    anchor: TaskStamp,
    metrics: Metrics,
    page: Bounded<Observed>,
}

/// Whole-room classification and assessment counters.
#[derive(Debug, Serialize)]
struct Metrics {
    observations: usize,
    relevant_contributions: usize,
    declines: usize,
    duplicates: usize,
    stale_activity: usize,
    missed_opportunities: usize,
    non_evidence_signals: usize,
    unknown_observations: usize,
    progress: usize,
    confirmed: usize,
    false_positives: usize,
    unknown_assessments: usize,
    unassessed: usize,
}

/// One observation at its admitted position and its assessment, if any.
#[derive(Debug, Serialize)]
pub(crate) struct Observed {
    seq: Seq,
    observation: RoomRelevanceObservation,
    #[serde(skip_serializing_if = "Option::is_none")]
    assessment: Option<Assessed>,
}

#[derive(Debug, Serialize)]
struct Assessed {
    seq: Seq,
    #[serde(flatten)]
    assessment: RoomRelevanceAssessment,
}

/// Read one bounded page of a room's observations from the working repository.
pub(crate) fn run(matches: &ArgMatches, contract: &crate::cli::Contract) -> Outcome<RoomRelevance> {
    match discover() {
        Ok(repository) => run_at(&repository, matches, contract),
        Err(failure) => failure.outcome(),
    }
}

/// [`run`] against `repository`.
pub(crate) fn run_at(
    repository: &Repository,
    matches: &ArgMatches,
    contract: &crate::cli::Contract,
) -> Outcome<RoomRelevance> {
    match super::open(repository) {
        Ok((_, journal)) => answer(&journal, matches, contract.path),
        Err(failure) => failure.outcome(),
    }
}

/// One page of the observations of the room `matches` names.
pub(crate) fn answer(
    journal: &journal::JournalReader,
    matches: &ArgMatches,
    path: &[&str],
) -> Outcome<RoomRelevance> {
    match relevance(journal, matches, path) {
        Ok((position, body)) => Outcome::Reply(Reply { position, body }),
        Err(failure) => failure.outcome(),
    }
}

fn relevance(
    journal: &journal::JournalReader,
    matches: &ArgMatches,
    path: &[&str],
) -> Result<(ReplyPosition, RoomRelevance), Failure> {
    let page = Page::from_matches(matches, path)?;
    let (named, anchor) = super::named_room(matches)?;
    let from = page
        .after()
        .map(|after| {
            after
                .parse::<RoomAuthorityStamp>()
                .map_err(|_| page.cursor_refusal(CURSOR))
        })
        .transpose()?;
    let retain = page.retain();
    let (cursor, room, metrics, population, records) = journal
        .read_with(|read| -> Result<_, Failure> {
            let room = current_handle(read, &anchor)?;
            let cursor = read.cursor().map_err(store)?.ok_or_else(|| {
                fault(
                    FaultClass::Store,
                    "room::cursor-missing",
                    "the journal names a task but no read position",
                )
            })?;
            let mut records = Vec::with_capacity(retain);
            let mut population = 0;
            if let Some(from) = &from {
                let first = read
                    .room_relevance_record(from)
                    .map_err(store)?
                    .filter(|record| record.observation.body().room == anchor)
                    .ok_or_else(|| Failure::Refusal(page.cursor_refusal(CURSOR)))?;
                records.push(first);
                population += 1;
            }
            population += read
                .room_relevance_count(&anchor, from.as_ref())
                .map_err(store)?;
            let budget = journal::FactReadBudget::new(
                retain.clamp(1, journal::FactReadBudget::RETAINED.rows()),
                journal::FactReadBudget::RETAINED.decoded_bytes(),
            )
            .map_err(store)?;
            let mut after = from;
            while records.len() < retain.min(population) {
                let fetched = read
                    .room_relevance_page(&anchor, after.as_ref(), budget)
                    .map_err(store)?;
                let wanted = retain - records.len();
                records.extend(fetched.rows.into_iter().take(wanted));
                match fetched.continuation {
                    Some(next) => after = Some(next),
                    None => break,
                }
            }
            let metrics = read.room_relevance_metrics(&anchor).map_err(store)?;
            Ok((cursor, room, metrics, population, records))
        })
        .map_err(|error| match error {
            journal::ReadError::Journal(error) => store(error),
            journal::ReadError::Answer(failure) => failure,
        })?;
    if let Some(notice) = crate::board_command::rename_notice(&named, &room) {
        eprintln!("{notice}");
    }
    let rows = records
        .into_iter()
        .map(|record| {
            let key = record.observation.body().observation.to_string();
            (
                Observed {
                    seq: record.observation.position(),
                    observation: record.observation.into_body(),
                    assessment: record.assessment.map(|head| Assessed {
                        seq: head.position(),
                        assessment: head.into_body(),
                    }),
                },
                key,
            )
        })
        .collect();
    let page = page.shown(rows, population)?;
    Ok((
        ReplyPosition::Snapshot(cursor),
        RoomRelevance {
            room,
            anchor,
            metrics: Metrics::from(metrics),
            page,
        },
    ))
}

impl From<journal::RoomRelevanceMetrics> for Metrics {
    fn from(metrics: journal::RoomRelevanceMetrics) -> Self {
        Self {
            observations: metrics.observations,
            relevant_contributions: metrics.relevant_contributions,
            declines: metrics.declines,
            duplicates: metrics.duplicates,
            stale_activity: metrics.stale_activity,
            missed_opportunities: metrics.missed_opportunities,
            non_evidence_signals: metrics.non_evidence_signals,
            unknown_observations: metrics.unknown_observations,
            progress: metrics.progress,
            confirmed: metrics.confirmed,
            false_positives: metrics.false_positives,
            unknown_assessments: metrics.unknown_assessments,
            unassessed: metrics.unassessed,
        }
    }
}

impl fmt::Display for RoomRelevance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let metrics = &self.metrics;
        write!(
            formatter,
            "room {} relevance (shadow; changes no authority)\n  observations {} \
             relevant-contribution {} declined {} duplicate {} stale-activity {} \
             missed-opportunity {} non-evidence {} unknown {} progress {}\n  assessments \
             confirmed {} false-positive {} unknown {} unassessed {}",
            self.room,
            metrics.observations,
            metrics.relevant_contributions,
            metrics.declines,
            metrics.duplicates,
            metrics.stale_activity,
            metrics.missed_opportunities,
            metrics.non_evidence_signals,
            metrics.unknown_observations,
            metrics.progress,
            metrics.confirmed,
            metrics.false_positives,
            metrics.unknown_assessments,
            metrics.unassessed,
        )?;
        for row in &self.page.rows {
            write!(formatter, "\n{row}")?;
        }
        match &self.page.omission {
            Some(omission) => write!(formatter, "\n{omission}"),
            None => Ok(()),
        }
    }
}

impl fmt::Display for Observed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let observation = &self.observation;
        write!(
            formatter,
            "{} {} {} through {} at {}\n  {}",
            observation.observation,
            observation.classification.name(),
            observation.progress.name(),
            observation.through,
            self.seq,
            observation.explanation
        )?;
        for view in &observation.sources {
            match view {
                RoomSourceView::Available { source } => {
                    write!(formatter, "\n  source {}", Source(source))?;
                }
                RoomSourceView::Unavailable {
                    source,
                    unavailable,
                } => write!(
                    formatter,
                    "\n  source {} unavailable {}",
                    Source(source),
                    Unavailable(unavailable)
                )?,
            }
        }
        if !observation.exceptions.is_empty() {
            write!(formatter, "\n  exceptions {}", observation.exceptions)?;
        }
        if !observation.uncertainty.is_empty() {
            write!(formatter, "\n  uncertainty {}", observation.uncertainty)?;
        }
        match &self.assessment {
            Some(assessed) => write!(
                formatter,
                "\n  assessed {} at {}: {}",
                assessed.assessment.verdict.name(),
                assessed.seq,
                assessed.assessment.explanation
            ),
            None => write!(
                formatter,
                "\n  unassessed; `spice room assess {} confirmed|false-positive|unknown TEXT`",
                observation.observation
            ),
        }
    }
}
