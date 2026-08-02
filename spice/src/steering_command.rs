//! CLI acquisition for the resident steering command family.

use std::ffi::OsString;

use clap::ArgMatches;
use spice::repository::Repository;
use wire::intent::CommandAnswer;
use wire::{CanonicalUuid, Disposition, Fault, FaultClass, Outcome, Refusal};

use crate::command_client::{SelectedCommand, execute_steering};

pub(crate) fn run(
    matches: &ArgMatches,
    contract: &crate::cli::Contract,
    lane_environment: Option<OsString>,
) -> Outcome<CommandAnswer> {
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => return environment_fault("steer::cwd-unavailable", &error.to_string()),
    };
    let repository = match Repository::discover(&current) {
        Ok(repository) => repository,
        Err(error) => return environment_fault("steer::git-unavailable", &error.to_string()),
    };
    let state = match repository.admit_state() {
        Ok(state) => state,
        Err(error) => return crate::state_failure(&error).outcome(),
    };
    let (selected, lane_context) = match contract.path {
        ["steer"] => {
            let lane = match uuid_operand(matches, "LANE", "steer::lane-invalid") {
                Ok(lane) => lane,
                Err(outcome) => return outcome,
            };
            let message = matches
                .get_one::<String>("TEXT")
                .expect("clap requires steering text");
            (SelectedCommand::SteerSend { lane, message }, None)
        }
        ["steer", "cancel"] => {
            let directive = match uuid_operand(matches, "ID", "steer::directive-invalid") {
                Ok(directive) => directive,
                Err(outcome) => return outcome,
            };
            (SelectedCommand::SteerCancel { directive }, None)
        }
        ["steer", "recover"] => {
            let directive = match uuid_operand(matches, "ID", "steer::directive-invalid") {
                Ok(directive) => directive,
                Err(outcome) => return outcome,
            };
            (SelectedCommand::SteerRecover { directive }, None)
        }
        ["inbox"] => {
            let lane = match optional_uuid_operand(matches, "LANE", "steer::lane-invalid") {
                Ok(lane) => lane,
                Err(outcome) => return outcome,
            };
            let lane_context = match lane_context(lane_environment) {
                Ok(lane) => lane,
                Err(outcome) => return outcome,
            };
            (SelectedCommand::SteerInbox { lane }, lane_context)
        }
        ["ack"] => {
            let text = matches
                .get_one::<String>("KEY")
                .expect("clap requires an acknowledgment key");
            let key = match text.parse::<steer::Key>() {
                Ok(key) => key,
                Err(error) => {
                    return refusal("steer::key-invalid", Some(text.clone()), &error.to_string());
                }
            };
            let disposition = if matches.get_flag("decline") {
                Disposition::Declined
            } else {
                Disposition::Acked
            };
            let lane_context = match lane_context(lane_environment) {
                Ok(lane) => lane,
                Err(outcome) => return outcome,
            };
            return execute_steering(
                state.canvas(),
                contract,
                SelectedCommand::SteerAck {
                    key: &key,
                    disposition,
                },
                lane_context,
            );
        }
        _ => {
            return environment_fault(
                "steer::dispatch-invalid",
                "the steering adapter received another CLI contract",
            );
        }
    };
    execute_steering(state.canvas(), contract, selected, lane_context)
}

fn lane_context(value: Option<OsString>) -> Result<Option<CanonicalUuid>, Outcome<CommandAnswer>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let text = value.into_string().map_err(|value| {
        refusal(
            "steer::lane-context-invalid",
            Some(value.to_string_lossy().into_owned()),
            "SPICE_LANE must name one canonical lane UUID",
        )
    })?;
    text.parse().map(Some).map_err(|_| {
        refusal(
            "steer::lane-context-invalid",
            Some(text),
            "SPICE_LANE must name one canonical lane UUID",
        )
    })
}

fn uuid_operand(
    matches: &ArgMatches,
    name: &str,
    reason: &str,
) -> Result<CanonicalUuid, Outcome<CommandAnswer>> {
    let text = matches
        .get_one::<String>(name)
        .expect("clap requires the registered UUID operand");
    text.parse().map_err(|_| {
        refusal(
            reason,
            Some(text.clone()),
            "supply the canonical lower-case UUID printed by the resident",
        )
    })
}

fn optional_uuid_operand(
    matches: &ArgMatches,
    name: &str,
    reason: &str,
) -> Result<Option<CanonicalUuid>, Outcome<CommandAnswer>> {
    matches
        .get_one::<String>(name)
        .map(|_| uuid_operand(matches, name, reason))
        .transpose()
}

fn refusal(reason: &str, observed: Option<String>, remedy: &str) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("steering-operand".to_owned()),
        expected: None,
        observed,
        remedy: remedy.to_owned(),
    })
}

fn environment_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}
