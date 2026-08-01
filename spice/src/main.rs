use std::ffi::{OsStr, OsString};

use wire::{Outcome, Refusal};

mod cli;

const REPLY_EXIT: i32 = 0;
const FAULT_EXIT: i32 = 1;
const REFUSAL_EXIT: i32 = 2;

fn main() {
    let matches = cli::command().get_matches();
    let contract = cli::selected_contract(&matches)
        .expect("the generated parser returns only registered command leaves");
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
    if let Outcome::Refusal(refusal) = &outcome {
        eprintln!("spice: {}; {}", refusal.reason, refusal.remedy);
    }
    terminate(&outcome);
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

    use super::{FAULT_EXIT, REFUSAL_EXIT, REPLY_EXIT, exit_status};

    #[test]
    fn typed_outcomes_map_to_the_public_exit_contract() {
        let reply = Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(Seq::new(1).expect("non-zero position")),
            body: (),
        });
        let refusal: Outcome<()> = Outcome::Refusal(Refusal {
            reason: "command::refused".to_owned(),
            witness: None,
            expected: None,
            observed: None,
            remedy: "change the request".to_owned(),
        });
        let fault: Outcome<()> = Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "environment::unavailable".to_owned(),
            detail: "required service is unavailable".to_owned(),
        });

        assert_eq!(exit_status(&reply), REPLY_EXIT);
        assert_eq!(exit_status(&refusal), REFUSAL_EXIT);
        assert_eq!(exit_status(&fault), FAULT_EXIT);
        assert_eq!((REPLY_EXIT, REFUSAL_EXIT, FAULT_EXIT), (0, 2, 1));
    }
}
