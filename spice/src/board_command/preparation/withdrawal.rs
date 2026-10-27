//! The author's withdrawal of one exact undecided Design pass proposal.
//!
//! Withdrawal records its own immutable proposal head beside the proposal,
//! which stays exactly as it was recorded. The journal fold
//! repeats the author and decision rules, so a decision and a withdrawal
//! racing for one candidate cannot both land.

use wire::rooms::{CandidateRef, CandidateWithdraw, CandidateWithdrawn, PreparationDecided};

use super::*;

/// One candidate's withdrawal authority at the command barrier.
struct Observed {
    cursor: Option<Seq>,
    candidate: Option<Seq>,
    author: Option<Actor>,
    withdrawn: Option<(Seq, String)>,
    decided: bool,
}

pub(super) fn withdraw_candidate(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &CandidateWithdraw,
) -> Outcome<CommandAnswer> {
    let candidate = &request_body.withdrawal.candidate;
    let code = candidate.candidate.to_string();
    let read_candidate = candidate.clone();
    let observed = match bus.read(move |read| observe(read, &read_candidate)) {
        Ok(observed) => observed,
        Err(error) => return store_fault("preparation::authority-unreadable", &error.to_string()),
    };
    let Some(head) = observed.candidate else {
        return refusal(
            "preparation::candidate-missing",
            Some(code),
            "retain the exact candidate before withdrawing it",
        );
    };
    if observed.author.as_ref() != Some(command.actor()) {
        return refusal(
            "preparation::withdrawal-not-author",
            Some(code),
            "only the actor that retained this candidate withdraws it; the operator disposes of \
             it with `spice design decide`",
        );
    }
    let candidate_witness = Witness::Proposal {
        proposal: ProposalRef::Candidate {
            candidate: candidate.clone(),
        },
        head: Some(head),
    };
    if let Err(outcome) = require_witness(command, &candidate_witness) {
        return outcome;
    }
    let subject = ProposalRef::Withdrawal {
        candidate: candidate.clone(),
    };
    if let Some((withdrawn, reason)) = observed.withdrawn {
        return repeated(command, subject, withdrawn, observed.cursor, &code, &reason);
    }
    if observed.decided {
        return refusal(
            "preparation::candidate-decided",
            Some(code),
            "a decided candidate stays the operator's record; retain a new candidate version instead",
        );
    }
    if let Err(outcome) = require_proposal_absence(command, &subject) {
        return outcome;
    }
    record(
        bus,
        command,
        request_body,
        &candidate.controller,
        CandidateWithdrawn {
            withdrawal: request_body.withdrawal.clone(),
        },
        Vec::new(),
        format!("withdrew candidate {code}"),
    )
}

/// The author repeating a recorded withdrawal changes nothing and answers
/// with the reason that withdrawal recorded.
fn repeated(
    command: &wire::AdmittedCommand,
    subject: ProposalRef,
    withdrawn: Seq,
    cursor: Option<Seq>,
    code: &str,
    reason: &str,
) -> Outcome<CommandAnswer> {
    let witness = Witness::Proposal {
        proposal: subject,
        head: Some(withdrawn),
    };
    if let Err(outcome) = require_witness(command, &witness) {
        return outcome;
    }
    let Some(cursor) = cursor else {
        return store_fault(
            "preparation::cursor-missing",
            "the journal names a withdrawal but no read position",
        );
    };
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(cursor),
        body: CommandAnswer::Text(TextAnswer {
            text: format!(
                "candidate {code} is already withdrawn at {}: {reason}",
                wire::SequenceCode::new(withdrawn.get())
            ),
        }),
    })
}

fn observe(
    read: &journal::JournalRead<'_>,
    candidate: &CandidateRef,
) -> Result<Observed, GuardError> {
    let retained = ProposalRef::Candidate {
        candidate: candidate.clone(),
    };
    let withdrawn = read
        .proposal_head(&ProposalRef::Withdrawal {
            candidate: candidate.clone(),
        })?
        .map(|head| match head.body() {
            journal::ProposalAuthority::Withdrawal(withdrawal) => {
                Ok((head.position(), withdrawal.reason.clone()))
            }
            _ => Err(GuardError::Invalid(
                "withdrawal authority decoded as another proposal family".to_owned(),
            )),
        })
        .transpose()?;
    Ok(Observed {
        cursor: read.cursor()?,
        candidate: read.proposal_head(&retained)?.map(|head| head.position()),
        author: read.proposal_author(&retained)?,
        withdrawn,
        decided: decided(read, candidate)?,
    })
}

/// Whether any decision recorded under the candidate's controller disposes
/// of it, read through that controller's decision index page by page.
fn decided(read: &journal::JournalRead<'_>, candidate: &CandidateRef) -> Result<bool, GuardError> {
    let mut after = None;
    loop {
        let page = read.preparation_decisions(
            &candidate.controller,
            after,
            journal::FactReadBudget::RETAINED,
        )?;
        if page.rows.iter().any(|envelope| {
            envelope
                .fact
                .read::<PreparationDecided>()
                .is_some_and(|decided| decided.decision.outcome.candidate() == candidate)
        }) {
            return Ok(true);
        }
        match page.continuation {
            Some(position) => after = Some(position),
            None => return Ok(false),
        }
    }
}
