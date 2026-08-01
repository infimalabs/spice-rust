use std::ffi::{OsStr, OsString};

#[cfg(not(target_arch = "wasm32"))]
use std::{
    io::{self, Write},
    path::Path,
    process::ExitStatus,
};

use wire::{Fault, FaultClass, Outcome, Refusal};

mod cli;
mod render;

use render::Form;

const REPLY_EXIT: i32 = 0;
const FAULT_EXIT: i32 = 1;
const REFUSAL_EXIT: i32 = 2;

fn main() {
    let matches = cli::command().get_matches();
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(command) = cli::run_command(&matches) {
            run(command);
        }
    }

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

/// Emits any pending direction and hands the caller's command through.
#[cfg(not(target_arch = "wasm32"))]
fn run(command: Vec<OsString>) -> ! {
    let (program, arguments) = command
        .split_first()
        .expect("clap requires the run command operand");

    if let Some(rendering) = bootstrap_environment(steer::hook::INBOX) {
        let mut stderr = io::stderr().lock();
        let _ = steer::run::emit(Path::new(&rendering), &mut stderr);
    }

    match steer::run::hand(program, arguments) {
        Ok(status) => terminate_child(status),
        Err(error) => {
            let mut stderr = io::stderr().lock();
            let _ = writeln!(stderr, "spice run: {error}");
            let code = match error.kind() {
                io::ErrorKind::NotFound => 127,
                io::ErrorKind::PermissionDenied => 126,
                _ => FAULT_EXIT,
            };
            terminate_code(code);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn terminate_child(status: ExitStatus) -> ! {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        if let Some(signal) = status.signal() {
            terminate_code(128 + signal);
        }
    }

    terminate_code(status.code().unwrap_or(FAULT_EXIT));
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
pub fn terminate<B>(outcome: &Outcome<B>) -> ! {
    terminate_code(exit_status(outcome));
}

#[expect(
    clippy::disallowed_methods,
    reason = "the binary rim alone terminates the process"
)]
fn terminate_code(code: i32) -> ! {
    std::process::exit(code);
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
