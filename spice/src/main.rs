use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc;

use listen::{RetroSelector, StreamOutcome, SurfaceError, Watch};
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
    let leaf = cli::selected_matches(&matches);
    match contract.path {
        ["watch"] => run_watch(leaf, form),
        ["retro"] => run_retro(leaf, form),
        _ => {}
    }

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

fn run_retro(matches: &clap::ArgMatches, form: Form) -> ! {
    let selector = match matches.get_one::<String>("LANE|LOCUS|PATH") {
        Some(value) => match RetroSelector::parse(value) {
            Ok(selector) => selector,
            Err(error) => {
                let refusal: Outcome<()> = Outcome::Refusal(Refusal {
                    reason: "retro::selector".to_owned(),
                    witness: None,
                    expected: Some("LANE, M:O:L, or PATH".to_owned()),
                    observed: Some(value.clone()),
                    remedy: error.to_string(),
                });
                deliver(&refusal, form);
            }
        },
        None => RetroSelector::Path(PathBuf::from(".")),
    };
    match listen::retro(".", selector) {
        Ok(outcome) => deliver_stream_once(&outcome, form),
        Err(error) => deliver_fault(surface_fault(error), form),
    }
}

fn run_watch(matches: &clap::ArgMatches, form: Form) -> ! {
    let root = matches
        .get_one::<String>("ROOT")
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    let once = matches.get_flag("once");
    let mut watch = Watch::new(&root);

    if once || !is_real_directory(&root) {
        match poll_and_emit(&mut watch, form) {
            Ok(()) => terminate_stream(),
            Err(fault) => deliver_fault(fault, form),
        }
    }

    follow(&root, &mut watch, form)
}

fn is_real_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir())
}

#[cfg(not(target_arch = "wasm32"))]
fn follow(root: &Path, watch: &mut Watch, form: Form) -> ! {
    use notify::{RecursiveMode, Watcher};

    let (sender, receiver) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = match notify::recommended_watcher(move |event| {
        let _ = sender.send(event);
    }) {
        Ok(watcher) => watcher,
        Err(error) => deliver_fault(watch_fault("create filesystem watcher", error), form),
    };
    if let Err(error) = watcher.watch(root, RecursiveMode::Recursive) {
        deliver_fault(watch_fault("arm filesystem watcher", error), form);
    }
    if let Err(fault) = poll_and_emit(watch, form) {
        deliver_fault(fault, form);
    }

    loop {
        match receiver.recv() {
            Ok(Ok(_)) => {
                if let Err(fault) = poll_and_emit(watch, form) {
                    deliver_fault(fault, form);
                }
            }
            Ok(Err(error)) => {
                deliver_fault(watch_fault("receive filesystem notification", error), form);
            }
            Err(error) => {
                deliver_fault(
                    Fault {
                        class: FaultClass::Environment,
                        reason: "watch::notification".to_owned(),
                        detail: format!("filesystem notification channel disconnected: {error}"),
                    },
                    form,
                );
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn follow(_root: &Path, _watch: &mut Watch, form: Form) -> ! {
    deliver_fault(
        Fault {
            class: FaultClass::Environment,
            reason: "watch::unavailable".to_owned(),
            detail: "filesystem notifications are unavailable on wasm32".to_owned(),
        },
        form,
    )
}

fn poll_and_emit(watch: &mut Watch, form: Form) -> Result<(), Fault> {
    let outcome = watch.poll().map_err(surface_fault)?;
    if !outcome.is_empty() {
        render::render_stream(&outcome, form)
            .map_err(render_fault)?
            .speak();
    }
    Ok(())
}

fn deliver_stream_once(outcome: &StreamOutcome, form: Form) -> ! {
    match render::render_stream(outcome, form) {
        Ok(rendering) => {
            rendering.speak();
            terminate_stream();
        }
        Err(error) => deliver_fault(render_fault(error), form),
    }
}

fn surface_fault(error: SurfaceError) -> Fault {
    Fault {
        class: FaultClass::Environment,
        reason: "listen::surface".to_owned(),
        detail: error.to_string(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn watch_fault(operation: &str, error: notify::Error) -> Fault {
    Fault {
        class: FaultClass::Environment,
        reason: "watch::notification".to_owned(),
        detail: format!("failed to {operation}: {error}"),
    }
}

fn render_fault(error: String) -> Fault {
    Fault {
        class: FaultClass::Protocol,
        reason: "render::unencodable".to_owned(),
        detail: error,
    }
}

fn deliver_fault(fault: Fault, form: Form) -> ! {
    deliver(&Outcome::Fault(fault), form)
}

#[expect(
    clippy::disallowed_methods,
    reason = "the binary rim alone maps a successful stream to its process exit"
)]
fn terminate_stream() -> ! {
    std::process::exit(REPLY_EXIT);
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
            let fault: Outcome<()> = Outcome::Fault(render_fault(error));
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
