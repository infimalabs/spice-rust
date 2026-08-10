use std::path::PathBuf;

use clap::ArgMatches;
use spice::repository::Repository;
use wire::intent::{CommandAnswer, ConsentAnswer, ConsentPolicyAnswer, ConsentQueueAnswer};
use wire::{CanonicalUuid, Fault, FaultClass, Outcome, Refusal, Reply};

use crate::cli::Contract;
use crate::command_client::{self, SelectedCommand};

pub(crate) fn read(contract: &Contract) -> Outcome<ConsentQueueAnswer> {
    let canvas = match canvas_path() {
        Ok(canvas) => canvas,
        Err(outcome) => return outcome,
    };
    queue_outcome(command_client::execute(
        &canvas,
        contract,
        SelectedCommand::ConsentRead,
    ))
}

pub(crate) fn policy(contract: &Contract) -> Outcome<ConsentPolicyAnswer> {
    let canvas = match canvas_path() {
        Ok(canvas) => canvas,
        Err(outcome) => return outcome,
    };
    match command_client::execute(&canvas, contract, SelectedCommand::ConsentPolicy) {
        Outcome::Reply(reply) => match reply.body {
            CommandAnswer::ConsentPolicy(body) => Outcome::Reply(Reply {
                position: reply.position,
                body,
            }),
            answer => answer_fault(
                "consent::policy-answer",
                &format!("expected a consent policy answer, received {answer:?}"),
            ),
        },
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

pub(crate) fn decide(
    matches: &ArgMatches,
    contract: &Contract,
    answer: wire::Answer,
) -> Outcome<ConsentAnswer> {
    let request = match matches
        .get_one::<String>("ID")
        .expect("consent decisions require the generated ID operand")
        .parse::<CanonicalUuid>()
    {
        Ok(request) => request,
        Err(_) => {
            return Outcome::Refusal(Refusal::invocation(
                "consent::request-invalid",
                "spice consent allow ID and spice consent deny ID require the exact canonical UUID printed by spice consent",
            ));
        }
    };
    let canvas = match canvas_path() {
        Ok(canvas) => canvas,
        Err(outcome) => return outcome,
    };
    let read_contract = crate::cli::contracts()
        .iter()
        .find(|candidate| candidate.path == ["consent"])
        .expect("the consent queue contract is registered");
    let queue = match queue_outcome(command_client::execute(
        &canvas,
        read_contract,
        SelectedCommand::ConsentRead,
    )) {
        Outcome::Reply(reply) => reply.body,
        Outcome::Refusal(refusal) => return Outcome::Refusal(refusal),
        Outcome::Fault(fault) => return Outcome::Fault(fault),
    };
    let Some(pending) = queue
        .pending
        .iter()
        .find(|pending| pending.request == request)
    else {
        return Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "consent::request-not-held".to_owned(),
                Some(request.to_string()),
                Some("a request in the current consent queue".to_owned()),
                Some("absent or terminal".to_owned()),
            )
            .with_detail("re-read the consent queue and decide a request that is still pending"),
        ));
    };
    let expected = match consent::decision_witnesses(pending, answer) {
        Ok(expected) => expected,
        Err(refusal) => return Outcome::Refusal(refusal),
    };
    match command_client::execute(
        &canvas,
        contract,
        SelectedCommand::ConsentDecide {
            request,
            answer,
            expected,
        },
    ) {
        Outcome::Reply(reply) => match reply.body {
            CommandAnswer::Consent(body) => Outcome::Reply(Reply {
                position: reply.position,
                body,
            }),
            body => answer_fault(
                "consent::decision-answer",
                &format!("expected a consent decision answer, received {body:?}"),
            ),
        },
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

fn queue_outcome(outcome: Outcome<CommandAnswer>) -> Outcome<ConsentQueueAnswer> {
    match outcome {
        Outcome::Reply(reply) => match reply.body {
            CommandAnswer::ConsentQueue(body) => Outcome::Reply(Reply {
                position: reply.position,
                body,
            }),
            answer => answer_fault(
                "consent::queue-answer",
                &format!("expected a consent queue answer, received {answer:?}"),
            ),
        },
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

fn canvas_path<B>() -> Result<PathBuf, Outcome<B>> {
    let current = std::env::current_dir().map_err(|error| {
        Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "consent::cwd-unavailable".to_owned(),
            detail: error.to_string(),
        })
    })?;
    let repository = Repository::discover(current).map_err(|error| {
        Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "consent::git-unavailable".to_owned(),
            detail: error.to_string(),
        })
    })?;
    repository
        .admit_state()
        .map(|state| state.canvas().to_path_buf())
        .map_err(|error| crate::state_failure(&error).outcome())
}

fn answer_fault<B>(reason: &str, detail: &str) -> Outcome<B> {
    Outcome::Fault(Fault {
        class: FaultClass::Protocol,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}
