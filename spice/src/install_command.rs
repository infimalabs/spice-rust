//! Native rim for the digest-bound coexistence installer.

use std::path::PathBuf;

use serde::Serialize;
use spice::install::{InstallError, InstallPlan, InstallReceipt};
use spice::repository::Repository;
use spice::repository::StateError;
use wire::{Digest, Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition};

pub(crate) enum Answer {
    Plan(Outcome<InstallPreview>),
    Receipt(Outcome<InstallReceipt>),
}

/// Operator-facing preview carrying the exact token accepted by `--apply`.
#[derive(Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstallPreview {
    plan: InstallPlan,
    digest: Digest,
}

pub(crate) fn run(matches: &clap::ArgMatches) -> Answer {
    let request = if matches.get_flag("unapply") {
        Request::Unapply
    } else if let Some(digest) = matches.get_one::<String>("apply") {
        Request::Apply(digest.clone())
    } else {
        Request::Plan
    };
    let working = match std::env::current_dir() {
        Ok(working) => working,
        Err(error) => {
            return request.fault("install::working-directory-unavailable", error.to_string());
        }
    };
    let repository = match Repository::discover(&working) {
        Ok(repository) => repository,
        Err(error) => {
            return request.fault("install::repository-unresolved", error.to_string());
        }
    };

    match request {
        Request::Plan => {
            let executable = match executable() {
                Ok(executable) => executable,
                Err(outcome) => return Answer::Plan(outcome),
            };
            Answer::Plan(preview(spice::install::plan(
                &repository,
                Some(&executable),
            )))
        }
        Request::Apply(spelling) => {
            let digest = match spelling.parse::<Digest>() {
                Ok(digest) => digest,
                Err(error) => {
                    return Answer::Receipt(Outcome::Refusal(Refusal {
                        reason: "install::digest-refused".to_owned(),
                        witness: Some("install-plan".to_owned()),
                        expected: Some("canonical install plan digest".to_owned()),
                        observed: Some(spelling),
                        remedy: format!(
                            "run install without --apply and use its exact digest: {error}"
                        ),
                    }));
                }
            };
            let executable = match executable() {
                Ok(executable) => executable,
                Err(outcome) => return Answer::Receipt(outcome),
            };
            Answer::Receipt(result(spice::install::apply(
                &repository,
                Some(&executable),
                &digest,
            )))
        }
        Request::Unapply => Answer::Receipt(result(spice::install::unapply(&repository))),
    }
}

fn preview(result: Result<InstallPlan, InstallError>) -> Outcome<InstallPreview> {
    result
        .and_then(|plan| {
            let digest = plan.digest()?;
            Ok(InstallPreview { plan, digest })
        })
        .map_or_else(failure, |body| {
            Outcome::Reply(Reply {
                position: ReplyPosition::Direct {},
                body,
            })
        })
}

enum Request {
    Plan,
    Apply(String),
    Unapply,
}

impl Request {
    fn fault(self, reason: &str, detail: String) -> Answer {
        let fault = Fault {
            class: FaultClass::Environment,
            reason: reason.to_owned(),
            detail,
        };
        match self {
            Self::Plan => Answer::Plan(Outcome::Fault(fault)),
            Self::Apply(_) | Self::Unapply => Answer::Receipt(Outcome::Fault(fault)),
        }
    }
}

fn executable<B>() -> Result<PathBuf, Outcome<B>> {
    std::env::current_exe().map_err(|error| {
        Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "install::executable-unresolved".to_owned(),
            detail: error.to_string(),
        })
    })
}

fn result<B>(result: Result<B, InstallError>) -> Outcome<B> {
    match result {
        Ok(body) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body,
        }),
        Err(error) => failure(error),
    }
}

fn failure<B>(error: InstallError) -> Outcome<B> {
    let detail = error.to_string();
    let refusal_reason = install_refusal_reason(&error);
    let refusal_remedy = install_refusal_remedy(&error);
    match error {
        InstallError::PathShape(_)
        | InstallError::PathOutsideRoot(_)
        | InstallError::ParentChanged(_)
        | InstallError::ExclusionsNotUtf8
        | InstallError::DivergentExclusions
        | InstallError::BinaryNotRegular(_)
        | InstallError::BinaryEmpty(_)
        | InstallError::BinaryContextChanged
        | InstallError::DigestMismatch { .. }
        | InstallError::LegacyRecord { .. }
        | InstallError::RecordMissing
        | InstallError::ApplyInProgress
        | InstallError::UnapplyInProgress
        | InstallError::Drift { .. } => Outcome::Refusal(Refusal {
            reason: refusal_reason.to_owned(),
            witness: Some("install-plan".to_owned()),
            expected: Some("the exact current coexistence install state".to_owned()),
            observed: Some(detail),
            remedy: refusal_remedy.to_owned(),
        }),
        InstallError::State(
            StateError::Linked { .. }
            | StateError::NotDirectory { .. }
            | StateError::NotRegularFile { .. },
        ) => Outcome::Refusal(Refusal {
            reason: refusal_reason.to_owned(),
            witness: Some("install-state".to_owned()),
            expected: Some("unlinked Rust-owned repository state".to_owned()),
            observed: Some(detail),
            remedy: "repair the named Rust state component before retrying".to_owned(),
        }),
        InstallError::Repository(_)
        | InstallError::State(StateError::Filesystem { .. })
        | InstallError::Io { .. } => Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "install::environment-failed".to_owned(),
            detail,
        }),
        InstallError::Encode(_)
        | InstallError::Decode(_)
        | InstallError::InvalidRecord(_)
        | InstallError::Interrupted { .. } => Outcome::Fault(Fault {
            class: FaultClass::Store,
            reason: "install::state-failed".to_owned(),
            detail,
        }),
    }
}

fn install_refusal_reason(error: &InstallError) -> &'static str {
    match error {
        InstallError::DigestMismatch { .. } => "install::digest-moved",
        InstallError::LegacyRecord { .. } => "install::release-owned-state",
        InstallError::RecordMissing => "install::record-missing",
        InstallError::ApplyInProgress | InstallError::UnapplyInProgress => {
            "install::transition-incomplete"
        }
        InstallError::Drift { .. } => "install::drift",
        InstallError::PathOutsideRoot(_) => "install::path-outside-root",
        InstallError::ParentChanged(_) => "install::parent-changed",
        InstallError::State(_) => "install::state-refused",
        _ => "install::input-refused",
    }
}

fn install_refusal_remedy(error: &InstallError) -> &'static str {
    match error {
        InstallError::PathOutsideRoot(_) => {
            "repair the named symlink or record path so it stays within its permitted root"
        }
        InstallError::ParentChanged(_) => {
            "rerun install preview against the current parent generation before applying"
        }
        _ => "inspect the current plan and repair the named state before retrying",
    }
}
