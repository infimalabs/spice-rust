//! `spice room result RESULT`: one question's result at its current head.
//!
//! Names the parties, deadline, state, who closed it and whether that was an
//! operator override, the answer's return receipt, and the question and
//! answer text from their owning sources. Reading settles nothing.

use std::fmt;
use std::path::Path;

use clap::ArgMatches;
use serde::Serialize;
use spice::repository::Repository;
use wire::rooms::{QuestionResultRef, RoomAuthorityStamp};
use wire::{FaultClass, Outcome, Reply, ReplyPosition};

use super::conversation::{ResultView, result_view};
use super::{
    Body, Content, Failure, conversation_body, discover, fault, open, project_body, store,
    write_content,
};

/// One question result with its question and answer text.
#[derive(Debug, Serialize)]
pub(crate) struct RoomResult {
    result: ResultView,
    question: Content,
    #[serde(skip_serializing_if = "Option::is_none")]
    answer: Option<Content>,
}

/// Read one question result from the working repository.
pub(crate) fn run(matches: &ArgMatches, contract: &crate::cli::Contract) -> Outcome<RoomResult> {
    match discover() {
        Ok(repository) => run_at(&repository, matches, contract),
        Err(failure) => failure.outcome(),
    }
}

/// [`run`] against `repository`.
pub(crate) fn run_at(
    repository: &Repository,
    matches: &ArgMatches,
    _contract: &crate::cli::Contract,
) -> Outcome<RoomResult> {
    match open(repository) {
        Ok((state, journal)) => answer(&journal, state.tape(), matches),
        Err(failure) => failure.outcome(),
    }
}

/// The result `matches` names, read from `journal` and the lane tapes under
/// `tape`.
pub(crate) fn answer(
    journal: &journal::JournalReader,
    tape: &Path,
    matches: &ArgMatches,
) -> Outcome<RoomResult> {
    match result(journal, tape, matches) {
        Ok((position, body)) => Outcome::Reply(Reply { position, body }),
        Err(failure) => failure.outcome(),
    }
}

fn result(
    journal: &journal::JournalReader,
    tape: &Path,
    matches: &ArgMatches,
) -> Result<(ReplyPosition, RoomResult), Failure> {
    let named = matches
        .get_one::<String>("RESULT")
        .expect("generated room parser requires RESULT");
    let result = named
        .parse::<RoomAuthorityStamp>()
        .map(QuestionResultRef::new)
        .map_err(|error| unknown(named, Some(error.to_string())))?;
    let (cursor, view, question, answer) = journal
        .read_with(|read| -> Result<_, Failure> {
            let cursor = read.cursor().map_err(store)?.ok_or_else(|| {
                fault(
                    FaultClass::Store,
                    "room::cursor-missing",
                    "the admitted journal names no read position",
                )
            })?;
            let (record, head) = read
                .question_result(&result)
                .map_err(store)?
                .ok_or_else(|| unknown(named, None))?;
            let view = result_view(read, record, head)?;
            let body = |stamp: &RoomAuthorityStamp| -> Result<Option<Body>, Failure> {
                read.room_conversation(stamp)
                    .map_err(store)?
                    .map(|(_, publication)| conversation_body(read, &publication))
                    .transpose()
            };
            let question = body(view.question())?.ok_or_else(|| {
                fault(
                    FaultClass::Store,
                    "room::result-question-missing",
                    format!(
                        "result {named} names unadmitted question {}",
                        view.question()
                    ),
                )
            })?;
            let answer = match view.answer() {
                Some(answer) => body(answer)?,
                None => None,
            };
            Ok((cursor, view, question, answer))
        })
        .map_err(|error| match error {
            journal::ReadError::Journal(error) => store(error),
            journal::ReadError::Answer(failure) => failure,
        })?;
    Ok((
        ReplyPosition::Snapshot(cursor),
        RoomResult {
            result: view,
            question: project_body(question, tape).0,
            answer: answer.map(|answer| project_body(answer, tape).0),
        },
    ))
}

fn unknown(named: &str, observed: Option<String>) -> Failure {
    Failure::Refusal(crate::read_page::refusal(
        "room",
        "result-unknown",
        observed.or_else(|| Some(format!("no question result {named}"))),
        "name a result handle an ASK minted; `spice room inbox` and `spice room history \
         HANDLE` name each open result",
    ))
}

impl fmt::Display for RoomResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}\n  question", self.result)?;
        write_content(formatter, &self.question)?;
        if let Some(answer) = &self.answer {
            formatter.write_str("\n  answer")?;
            write_content(formatter, answer)?;
        }
        Ok(())
    }
}
