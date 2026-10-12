//! Shadow relevance observations of settled room questions.
//!
//! Every question result asked in a task room is observed exactly once, after
//! it settles. The observation shares the result's identity, cites the ASK's
//! journal fact and the fact that answered or closed it, declares the result
//! head's position as its horizon, and classifies the settlement. An answer
//! is unknown: settlement alone cannot show whether it carried evidence or only
//! agreed, so it never claims progress. A decline is declined; a deadline that
//! passed unanswered is a missed opportunity; a withdrawn question is stale
//! activity; none of those is progress. Observing writes only
//! `room::observed`, so no delivery, wake, claim, task, room, group, view,
//! review, or authorization changes.

use wire::rooms::{
    QuestionResultRecord, QuestionResultState, RoomDestination, RoomProgressStanding,
    RoomRelevanceClass, RoomRelevanceObservation, RoomRelevanceObserved, RoomSource,
    RoomSourceUnavailable, RoomSourceView,
};
use wire::{Actor, CanonicalUuid, Producer, Seq, SequenceCode};

use super::*;

/// One settled result and the journal positions of the publications it
/// names, as one read saw them.
struct Settled {
    record: QuestionResultRecord,
    state: QuestionResultState,
    settled: Seq,
    asked: Option<Seq>,
    answered: Option<Seq>,
}

/// Record one shadow observation for each settled, unobserved question
/// result in a task room. A refused or failed observation is reported and
/// retried on the next commit; it never fails the caller.
pub(in crate::resident) fn reconcile(
    bus: &Bus,
    epoch: CanonicalUuid,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let settled = bus
        .read(|read| {
            read.unobserved_question_results()?
                .into_iter()
                .map(|(record, head)| {
                    let asked = read
                        .room_conversation(record.question())?
                        .map(|(seq, _)| seq);
                    let state = head.body().state().clone();
                    let answered = match &state {
                        QuestionResultState::Answered { answer, .. } => {
                            read.room_conversation(answer)?.map(|(seq, _)| seq)
                        }
                        _ => None,
                    };
                    Ok(Settled {
                        record,
                        state,
                        settled: head.position(),
                        asked,
                        answered,
                    })
                })
                .collect::<Result<Vec<_>, journal::GuardError>>()
        })
        .map_err(|error| store_fault("room::relevance-unreadable", &error.to_string()))?;
    for settled in settled {
        let Some(observation) = observe(&settled) else {
            continue;
        };
        if let Err(error) = commit(bus, epoch, observation) {
            let _ = writeln!(
                diagnostic,
                "room relevance observation of result {} was not recorded: {error:?}",
                settled.record.result()
            );
        }
    }
    Ok(())
}

fn commit(
    bus: &Bus,
    epoch: CanonicalUuid,
    observation: RoomRelevanceObservation,
) -> CommandResult<()> {
    let at = Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("room::relevance-clock-invalid", &error.to_string()))?;
    let origin = format!("resident:{epoch}")
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("resident::origin-invalid", &error.to_string()))?;
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::ResidentRecognition);
    write
        .fact(RoomRelevanceObserved { observation }, FactCause::Root)
        .map_err(|error| store_fault("room::relevance-build", &error.to_string()))?;
    match bus
        .commit(write)
        .map_err(|error| store_fault("room::relevance-commit", &error.to_string()))?
    {
        CommitOutcome::Committed(_) => Ok(()),
        CommitOutcome::Refused(refusal) => {
            Err(store_fault("room::relevance-refused", &refusal.to_string()))
        }
        CommitOutcome::Fault(fault) => {
            Err(store_fault("room::relevance-fault", &fault.to_string()))
        }
    }
}

/// The shadow observation of one settled result, absent while it is pending
/// or when it was asked outside any task room.
fn observe(settled: &Settled) -> Option<RoomRelevanceObservation> {
    let record = &settled.record;
    let room = record.context()?.clone();
    let result = record.result();
    let question = record.question();
    let requester = party(record.requester());
    let respondent = party(record.respondent());
    let settling = RoomSource::Journal {
        fact: SequenceCode::new(settled.settled.get()),
    };
    let (classification, progress, closing, explanation, uncertainty) = match &settled.state {
        QuestionResultState::Pending {} => return None,
        QuestionResultState::Answered { answer, .. } => (
            RoomRelevanceClass::Unknown,
            RoomProgressStanding::Unknown,
            settled
                .answered
                .map_or(settling, |seq| RoomSource::Journal {
                    fact: SequenceCode::new(seq.get()),
                }),
            format!(
                "{respondent} answered question {question} with {answer}, settling result \
                 {result} that {requester} asked in this room"
            ),
            "the settlement shows an answer, not its content: whether it carried evidence \
             advancing an unresolved criterion or only agreed or acknowledged is unassessed"
                .to_owned(),
        ),
        QuestionResultState::Declined { .. } => (
            RoomRelevanceClass::Declined,
            RoomProgressStanding::NoProgress,
            settling,
            format!(
                "{respondent} declined question {question} from {requester}; result {result} \
                 closed without an answer"
            ),
            String::new(),
        ),
        QuestionResultState::TimedOut {} => (
            RoomRelevanceClass::MissedOpportunity,
            RoomProgressStanding::NoProgress,
            settling,
            format!(
                "question {question} from {requester} reached its deadline {} with no answer \
                 from {respondent}; result {result} timed out",
                record.deadline()
            ),
            String::new(),
        ),
        QuestionResultState::Cancelled { .. } => (
            RoomRelevanceClass::StaleActivity,
            RoomProgressStanding::NoProgress,
            settling,
            format!(
                "question {question} to {respondent} was withdrawn before an answer; result \
                 {result} leaves no live duty"
            ),
            "the withdrawal may follow progress made elsewhere, which this result does not show"
                .to_owned(),
        ),
    };
    let asked = match settled.asked {
        Some(seq) => RoomSourceView::Available {
            source: RoomSource::Journal {
                fact: SequenceCode::new(seq.get()),
            },
        },
        None => RoomSourceView::Unavailable {
            source: record.source().clone(),
            unavailable: RoomSourceUnavailable::Missing {},
        },
    };
    Some(RoomRelevanceObservation {
        observation: result.authority().clone(),
        room,
        through: SequenceCode::new(settled.settled.get()),
        sources: vec![asked, RoomSourceView::Available { source: closing }],
        classification,
        progress,
        explanation,
        exceptions: String::new(),
        uncertainty,
    })
}

fn party(destination: &RoomDestination) -> String {
    match destination {
        RoomDestination::Operator {} => "the operator".to_owned(),
        RoomDestination::Lane { lane } => format!("lane {lane}"),
        RoomDestination::Task { task } => format!("room {task}"),
    }
}

#[cfg(test)]
mod tests;
