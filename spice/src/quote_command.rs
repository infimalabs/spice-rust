//! `spice quote`: one quote target resolved by the resident's quote authority.

use std::fmt;

use clap::ArgMatches;
use serde::Serialize;
use spice::repository::Repository;
use wire::intent::{CommandAnswer, QuoteRead};
use wire::principal::PrincipalDirectory;
use wire::{Fault, FaultClass, Outcome, QuoteResolution, Refusal, Reply};

use crate::cli::Contract;
use crate::command_client::{self, SelectedCommand};

/// A resolved quote: its recorded author, a human beside their principal's
/// recorded name, then the exact source text.
#[derive(Serialize)]
#[serde(transparent)]
pub(crate) struct QuoteView {
    resolution: QuoteResolution,
    #[serde(skip)]
    principals: Option<PrincipalDirectory>,
}

impl fmt::Display for QuoteView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match spice::quote::render(&self.resolution, self.principals.as_ref()) {
            Ok(text) | Err(text) => formatter.write_str(&text),
        }
    }
}

/// Resolve the named target; a target that names no quotable source refuses.
pub(crate) fn run(matches: &ArgMatches, contract: &Contract) -> Outcome<QuoteView> {
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => return fault("quote::cwd-unavailable", &error.to_string()),
    };
    let repository = match Repository::discover(&current) {
        Ok(repository) => repository,
        Err(error) => return fault(spice::repository::ResolveError::REASON, &error.to_string()),
    };
    let state_paths = match repository.admit_state() {
        Ok(state_paths) => state_paths,
        Err(error) => return crate::state_failure(&error).outcome(),
    };
    let Some(target) = matches.get_one::<String>("TARGET") else {
        return fault("quote::target-missing", "the parser admitted no TARGET");
    };
    let body = QuoteRead {
        target: target.clone(),
    };
    match command_client::execute(
        state_paths.serve(),
        contract,
        SelectedCommand::QuoteRead { body: &body },
    ) {
        Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Quote(resolution),
        }) => match *resolution {
            QuoteResolution::Unavailable { target, reason } => {
                let refusal = spice::quote::render(
                    &QuoteResolution::Unavailable {
                        target: target.clone(),
                        reason,
                    },
                    None,
                )
                .err()
                .unwrap_or_default();
                Outcome::Refusal(Refusal::bare(
                    wire::RefusalDiagnostic::new(
                        "quote::unavailable".to_owned(),
                        Some("target".to_owned()),
                        None,
                        Some(target),
                    )
                    .with_detail(&refusal),
                ))
            }
            resolution => match spice::quote::author_names(state_paths.journal(), &resolution) {
                Ok(principals) => Outcome::Reply(Reply {
                    position,
                    body: QuoteView {
                        resolution,
                        principals,
                    },
                }),
                Err(detail) => fault("quote::principal-names-unreadable", &detail),
            },
        },
        Outcome::Reply(_) => Outcome::Fault(Fault {
            class: FaultClass::Protocol,
            reason: "quote::answer-invalid".to_owned(),
            detail: "resident quote reply did not carry a quote resolution".to_owned(),
        }),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

fn fault<B>(reason: &str, detail: &str) -> Outcome<B> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}
