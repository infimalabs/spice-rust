use std::ffi::{OsStr, OsString};

use wire::{Fault, FaultClass, Outcome, Refusal};

mod cli;
mod render;

use render::Form;

const REPLY_EXIT: i32 = 0;
const FAULT_EXIT: i32 = 1;
const REFUSAL_EXIT: i32 = 2;

fn main() {
    let matches = cli::command().get_matches();
    let contract = cli::selected_contract(&matches)
        .expect("the generated parser returns only registered command leaves");
    let form = if cli::json_selected(&matches) {
        Form::Json
    } else {
        Form::Text
    };
    let command = contract.path.join(" ");
    let outcome: Outcome<()> = Outcome::Refusal(Refusal {
        reason: "release::not-landed".to_owned(),
        witness: None,
        expected: None,
        observed: Some(command),
        remedy: format!(
            "finish the {} battery before Rust release one admits this leaf",
            contract.battery
        ),
    });
    deliver(&outcome, form);
}

/// Renders one outcome in the selected form and exits on it.
///
/// An outcome that cannot be encoded is a defect in this binary's own DTOs
/// rather than anything the caller did, so it becomes a fault with its own
/// exit code. That fault is rendered as text, which is total, so this reaches
/// an exit in one step and never asks the encoder a second time.
fn deliver(outcome: &Outcome<()>, form: Form) -> ! {
    match render::render(outcome, form) {
        Ok(rendering) => {
            rendering.speak();
            terminate(outcome);
        }
        Err(error) => {
            let fault: Outcome<()> = Outcome::Fault(Fault {
                class: FaultClass::Protocol,
                reason: "render::unencodable".to_owned(),
                detail: error,
            });
            render::render(&fault, Form::Text)
                .expect("the text rendering of a fault encodes nothing")
                .speak();
            terminate(&fault);
        }
    }
}

/// Acquire one ambient bootstrap value before typed configuration crosses
/// into the library graph.
#[expect(
    clippy::disallowed_methods,
    reason = "the binary rim owns ambient bootstrap acquisition"
)]
pub fn bootstrap_environment(name: impl AsRef<OsStr>) -> Option<OsString> {
    std::env::var_os(name)
}

/// Terminate the process after the command result has returned to the binary
/// rim.
#[expect(
    clippy::disallowed_methods,
    reason = "the binary rim alone maps typed outcomes to process exits"
)]
pub fn terminate<B>(outcome: &Outcome<B>) -> ! {
    std::process::exit(exit_status(outcome));
}

const fn exit_status<B>(outcome: &Outcome<B>) -> i32 {
    match outcome {
        Outcome::Reply(_) => REPLY_EXIT,
        Outcome::Refusal(_) => REFUSAL_EXIT,
        Outcome::Fault(_) => FAULT_EXIT,
    }
}

#[cfg(test)]
mod tests {
    use wire::{Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition, Seq};

    use super::render::{self, Stream};
    use super::{FAULT_EXIT, REFUSAL_EXIT, REPLY_EXIT, exit_status};

    fn reply() -> Outcome<()> {
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(Seq::new(1).expect("non-zero position")),
            body: (),
        })
    }

    fn refusal() -> Outcome<()> {
        Outcome::Refusal(Refusal {
            reason: "command::refused".to_owned(),
            witness: None,
            expected: None,
            observed: None,
            remedy: "change the request".to_owned(),
        })
    }

    fn fault() -> Outcome<()> {
        Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "environment::unavailable".to_owned(),
            detail: "required service is unavailable".to_owned(),
        })
    }

    #[test]
    fn typed_outcomes_map_to_the_public_exit_contract() {
        assert_eq!(exit_status(&reply()), REPLY_EXIT);
        assert_eq!(exit_status(&refusal()), REFUSAL_EXIT);
        assert_eq!(exit_status(&fault()), FAULT_EXIT);
        assert_eq!((REPLY_EXIT, REFUSAL_EXIT, FAULT_EXIT), (0, 2, 1));
    }

    /// The stream an outcome speaks on and the exit it carries read the same.
    ///
    /// These are two independent classifications of one enum, written in two
    /// modules, and nothing but this makes them agree. A caller redirecting by
    /// stream and a caller branching on the code are asking the same question,
    /// so an outcome that answered on standard output while exiting nonzero
    /// would strand whichever of them guessed wrong — and a variant added to
    /// one match without the other is exactly how that happens.
    #[test]
    fn the_stream_an_outcome_speaks_on_agrees_with_the_exit_it_carries() {
        for outcome in [reply(), refusal(), fault()] {
            assert_eq!(
                render::stream(&outcome) == Stream::Out,
                exit_status(&outcome) == REPLY_EXIT,
                "{outcome:?} must read the same either way",
            );
        }
    }
}
