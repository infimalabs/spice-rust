//! `spice room inbox`: open operator items in key order.
//!
//! Each item is the receipt the operator owes one publication, pending or
//! lapsed, shown with the publication's text from its owning source and, for
//! an ASK, its question result. Items page whole in key order; a page that
//! withholds any continues at the first withheld item. Reading settles
//! nothing.

use std::fmt;
use std::path::Path;

use clap::ArgMatches;
use serde::Serialize;
use spice::repository::Repository;
use wire::output::Bounded;
use wire::rooms::{RoomAuthorityStamp, RoomDestination, RoomPublicationKind, RoomReceiptRef};
use wire::{FaultClass, Outcome, Reply, ReplyPosition};

use super::conversation::{ItemView, Party, ResultView, item_view, result_view};
use super::{
    Body, Content, Failure, conversation_body, discover, fault, open, project_body, store,
    write_content,
};
use crate::read_page::Page;

/// What a continuation's `--after` names.
const CURSOR: &str = "an operator item key";

/// One page of open operator items.
#[derive(Debug, Serialize)]
pub(crate) struct RoomInbox {
    page: Bounded<InboxItem>,
}

/// One open operator item with what it is a receipt for.
#[derive(Debug, Serialize)]
pub(crate) struct InboxItem {
    receipt: ItemView,
    verb: &'static str,
    author: RoomDestination,
    content: Content,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<ResultView>,
}

/// One item as the journal records it, before its source is read.
struct Recorded {
    receipt: ItemView,
    verb: &'static str,
    author: RoomDestination,
    body: Body,
    result: Option<ResultView>,
}

/// Read one bounded page of open operator items from the working repository.
pub(crate) fn run(matches: &ArgMatches, contract: &crate::cli::Contract) -> Outcome<RoomInbox> {
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
) -> Outcome<RoomInbox> {
    match open(repository) {
        Ok((state, journal)) => answer(&journal, state.tape(), matches, contract.path),
        Err(failure) => failure.outcome(),
    }
}

/// One page of open operator items, read from `journal` and the lane tapes
/// under `tape`.
pub(crate) fn answer(
    journal: &journal::JournalReader,
    tape: &Path,
    matches: &ArgMatches,
    path: &[&str],
) -> Outcome<RoomInbox> {
    match inbox(journal, tape, matches, path) {
        Ok((position, body)) => Outcome::Reply(Reply { position, body }),
        Err(failure) => failure.outcome(),
    }
}

fn inbox(
    journal: &journal::JournalReader,
    tape: &Path,
    matches: &ArgMatches,
    path: &[&str],
) -> Result<(ReplyPosition, RoomInbox), Failure> {
    let page = Page::from_matches(matches, path)?;
    let from = page
        .after()
        .map(|after| {
            after
                .parse::<RoomAuthorityStamp>()
                .map(RoomReceiptRef::new)
                .map_err(|_| page.cursor_refusal(CURSOR))
        })
        .transpose()?;
    let retain = page.retain();
    let (cursor, population, recorded) = journal
        .read_with(|read| -> Result<_, Failure> {
            let cursor = read.cursor().map_err(store)?.ok_or_else(|| {
                fault(
                    FaultClass::Store,
                    "room::cursor-missing",
                    "the admitted journal names no read position",
                )
            })?;
            let (items, population) = read
                .open_room_items_page(from.as_ref(), retain)
                .map_err(store)?;
            let recorded = items
                .into_iter()
                .map(|(record, head)| {
                    let publication = read
                        .room_conversation(record.publication())
                        .map_err(store)?
                        .map(|(_, publication)| publication)
                        .ok_or_else(|| {
                            fault(
                                FaultClass::Store,
                                "room::item-publication-missing",
                                format!(
                                    "operator item {} names unadmitted publication {}",
                                    record.receipt(),
                                    record.publication()
                                ),
                            )
                        })?;
                    let (verb, result) = match publication.kind() {
                        RoomPublicationKind::Msg {} => ("msg", None),
                        RoomPublicationKind::Ask { result, .. } => ("ask", Some(result)),
                        RoomPublicationKind::Ans { result, .. } => ("ans", Some(result)),
                    };
                    let result = match result {
                        Some(result) => read
                            .question_result(result)
                            .map_err(store)?
                            .map(|(record, head)| result_view(read, record, head))
                            .transpose()?,
                        None => None,
                    };
                    Ok(Recorded {
                        receipt: item_view(read, record, head)?,
                        verb,
                        author: publication.author().clone(),
                        body: conversation_body(read, &publication)?,
                        result,
                    })
                })
                .collect::<Result<Vec<_>, Failure>>()?;
            Ok((cursor, population, recorded))
        })
        .map_err(|error| match error {
            journal::ReadError::Journal(error) => store(error),
            journal::ReadError::Answer(failure) => failure,
        })?;
    let rows = recorded
        .into_iter()
        .map(|recorded| {
            let key = recorded.receipt.item().to_string();
            let (content, _) = project_body(recorded.body, tape);
            (
                InboxItem {
                    receipt: recorded.receipt,
                    verb: recorded.verb,
                    author: recorded.author,
                    content,
                    result: recorded.result,
                },
                key,
            )
        })
        .collect();
    let page = page.shown(rows, population)?;
    Ok((ReplyPosition::Snapshot(cursor), RoomInbox { page }))
}

impl fmt::Display for RoomInbox {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("room inbox")?;
        if self.page.rows.is_empty() && self.page.omission.is_none() {
            formatter.write_str("\nno open operator items")?;
        }
        for item in &self.page.rows {
            write!(formatter, "\n{item}")?;
        }
        match &self.page.omission {
            Some(omission) => write!(formatter, "\n{omission}"),
            None => Ok(()),
        }
    }
}

impl fmt::Display for InboxItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}\n  recorded {} from {}",
            self.receipt,
            self.verb,
            Party(&self.author)
        )?;
        if let Some(result) = &self.result {
            write!(formatter, "\n  {result}")?;
        }
        write_content(formatter, &self.content)
    }
}
