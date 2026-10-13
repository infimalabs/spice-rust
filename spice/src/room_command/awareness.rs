//! `spice room awareness HANDLE`: which lanes were told about one Design.
//!
//! Each row is one admitted `room::noticed` record, oldest first: the lane and
//! occupant told, the claimed task and claim it served, and the terms that
//! matched. Rows page whole in journal order; a page that withholds any
//! continues at the first withheld one. Reading changes nothing.

use std::fmt;

use clap::ArgMatches;
use serde::Serialize;
use spice::repository::Repository;
use wire::output::Bounded;
use wire::rooms::DesignNoticed;
use wire::{CanonicalUuid, FaultClass, Outcome, Reply, ReplyPosition, Seq, TaskHandle, TaskStamp};

use super::{Failure, current_handle, discover, fault, store, task_invalid};
use crate::read_page::Page;

/// What a continuation's `--after` names.
const CURSOR: &str = "an awareness record of this Design";

/// One page of the lanes told about one Design.
#[derive(Debug, Serialize)]
pub(crate) struct DesignAwareness {
    design: TaskHandle,
    anchor: TaskStamp,
    page: Bounded<Noticed>,
}

/// One admitted awareness record.
#[derive(Debug, Serialize)]
pub(crate) struct Noticed {
    seq: Seq,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    task: TaskHandle,
    claim: Seq,
    terms: Vec<String>,
}

/// Read one bounded page of a Design's awareness records from the working
/// repository.
pub(crate) fn run(
    matches: &ArgMatches,
    contract: &crate::cli::Contract,
) -> Outcome<DesignAwareness> {
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
) -> Outcome<DesignAwareness> {
    let opened = || -> Result<_, Failure> {
        let projects = spice::config::capture_repository_scoped(repository, &[], None)
            .map_err(|error| {
                fault(
                    FaultClass::Environment,
                    "room::configuration-invalid",
                    error,
                )
            })?
            .resolved()
            .root()
            .tasks()
            .project_policy();
        let (_, journal) = super::open(repository)?;
        Ok((projects, journal))
    };
    match opened() {
        Ok((projects, journal)) => answer(&journal, &projects, matches, contract.path),
        Err(failure) => failure.outcome(),
    }
}

/// One page of the awareness records of the Design `matches` names.
pub(crate) fn answer(
    journal: &journal::JournalReader,
    projects: &board::ProjectPolicy,
    matches: &ArgMatches,
    path: &[&str],
) -> Outcome<DesignAwareness> {
    match awareness(journal, projects, matches, path) {
        Ok((position, body)) => Outcome::Reply(Reply { position, body }),
        Err(failure) => failure.outcome(),
    }
}

fn awareness(
    journal: &journal::JournalReader,
    projects: &board::ProjectPolicy,
    matches: &ArgMatches,
    path: &[&str],
) -> Result<(ReplyPosition, DesignAwareness), Failure> {
    let page = Page::from_matches(matches, path)?;
    let named = matches
        .get_one::<String>("HANDLE")
        .expect("generated room parser requires HANDLE");
    let named = named
        .parse::<TaskHandle>()
        .map_err(|error| task_invalid(named, &error))?;
    let anchor = named
        .born()
        .map_err(|error| task_invalid(named.as_str(), &error))?;
    let (cursor, design, rows) = journal
        .read_with(|read| -> Result<_, Failure> {
            let design = current_handle(read, &anchor, projects)?;
            let cursor = read.cursor().map_err(store)?.ok_or_else(|| {
                fault(
                    FaultClass::Store,
                    "room::cursor-missing",
                    "the journal names a task but no read position",
                )
            })?;
            let mut rows = Vec::new();
            let mut after = None;
            loop {
                let fetched = read
                    .selected_fact_page(
                        &[wire::FactKind::DesignNoticed],
                        after,
                        Some(cursor),
                        journal::FactReadBudget::RETAINED,
                    )
                    .map_err(store)?;
                for envelope in fetched.rows {
                    let Some(body) = envelope.fact.read::<DesignNoticed>() else {
                        continue;
                    };
                    if body.design != anchor {
                        continue;
                    }
                    rows.push(Noticed {
                        seq: envelope.seq,
                        lane: body.lane,
                        occupant: body.occupant,
                        task: current_handle(read, &body.task, projects)?,
                        claim: Seq::new(body.claim.get()).ok_or_else(|| {
                            fault(
                                FaultClass::Store,
                                "room::awareness-claim-invalid",
                                "an awareness record names claim position zero",
                            )
                        })?,
                        terms: body.terms,
                    });
                }
                match fetched.continuation {
                    Some(next) => after = Some(next),
                    None => break,
                }
            }
            Ok((cursor, design, rows))
        })
        .map_err(|error| match error {
            journal::ReadError::Journal(error) => store(error),
            journal::ReadError::Answer(failure) => failure,
        })?;
    if let Some(notice) = crate::board_command::rename_notice(&named, &design) {
        eprintln!("{notice}");
    }
    let page = page.listed(rows, |row| row.seq.to_string(), CURSOR)?;
    Ok((
        ReplyPosition::Snapshot(cursor),
        DesignAwareness {
            design,
            anchor,
            page,
        },
    ))
}

impl fmt::Display for DesignAwareness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "design {} awareness (background notices; no attention or acknowledgment)",
            self.design
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

impl fmt::Display for Noticed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} lane {} occupant {} task {} claim {} terms {}",
            self.seq,
            self.lane,
            self.occupant,
            self.task,
            self.claim,
            self.terms.join(", ")
        )
    }
}
