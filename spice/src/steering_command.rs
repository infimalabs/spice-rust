//! CLI acquisition for the resident steering command family.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::ArgMatches;
use spice::repository::Repository;
use wire::intent::CommandAnswer;
use wire::{CanonicalUuid, Disposition, Fault, FaultClass, Outcome, Refusal};

use crate::command_client::{SelectedCommand, execute_steering};

#[cfg(test)]
mod tests;

#[derive(serde::Serialize)]
struct HookOutput {
    #[serde(rename = "hookSpecificOutput")]
    hook_specific_output: HookSpecificOutput,
}

#[derive(serde::Serialize)]
struct HookSpecificOutput {
    #[serde(rename = "hookEventName")]
    hook_event_name: &'static str,
    #[serde(rename = "additionalContext")]
    additional_context: String,
}

pub(crate) fn hook_document(
    answer: &CommandAnswer,
    harness: &str,
) -> Result<Option<Vec<u8>>, String> {
    if !matches!(harness, "claude" | "codex") {
        return Err(format!(
            "unsupported hook harness {harness:?}; expected claude or codex"
        ));
    }
    let CommandAnswer::Inbox(inbox) = answer else {
        return Err("inbox hook received a non-inbox command answer".to_owned());
    };
    if inbox.lanes.iter().all(|lane| lane.pending.is_empty()) {
        return Ok(None);
    }
    let executable = std::env::current_exe()
        .map(PathBuf::into_os_string)
        .map_err(|error| format!("inbox hook executable unavailable: {error}"))?;
    let executable = executable.to_string_lossy();
    let mut context = format!(
        concat!(
            "Rust Spice steering is pending. Address every key immediately before other work. ",
            "Begin assistant prose with `ACK <key>:` or `NACK <key>:`, then retire it after ",
            "that prose with `{} ack <key>` or `{} ack <key> --decline`."
        ),
        executable, executable,
    );
    for lane in &inbox.lanes {
        for directive in &lane.pending {
            context.push_str(&format!(
                "\n\nlane={} key={}\n{}",
                lane.lane, directive.key, directive.text
            ));
        }
    }
    let mut document = wire::encode(&HookOutput {
        hook_specific_output: HookSpecificOutput {
            hook_event_name: "PostToolUse",
            additional_context: context,
        },
    })
    .map_err(|error| error.to_string())?;
    document.push(b'\n');
    Ok(Some(document))
}

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
    // `SelectedCommand::SteerAck` borrows its key, so the key is parsed here
    // and outlives the selection that names it.
    let key = match acknowledgment_key(matches, contract) {
        Ok(key) => key,
        Err(outcome) => return outcome,
    };
    let (selected, lane_context) = match select(matches, contract, lane_environment, key.as_ref()) {
        Ok(selection) => selection,
        Err(outcome) => return outcome,
    };
    execute_steering(state.canvas(), contract, selected, lane_context)
}

/// Parse the key `ack` borrows, and nothing at all for every other contract.
fn acknowledgment_key(
    matches: &ArgMatches,
    contract: &crate::cli::Contract,
) -> Result<Option<steer::Key>, Outcome<CommandAnswer>> {
    if !matches!(contract.path, ["ack"]) {
        return Ok(None);
    }
    let text = matches
        .get_one::<String>("KEY")
        .expect("clap requires an acknowledgment key");
    text.parse::<steer::Key>()
        .map(Some)
        .map_err(|error| refusal("steer::key-invalid", Some(text.clone()), &error.to_string()))
}

/// Choose the command one steering contract names, with its lane context.
///
/// Every arm answers the same pair [`run`] would have built inline, so the
/// operand refusals each path can raise stay in their original order.
fn select<'a>(
    matches: &'a ArgMatches,
    contract: &crate::cli::Contract,
    lane_environment: Option<OsString>,
    key: Option<&'a steer::Key>,
) -> Result<(SelectedCommand<'a>, Option<CanonicalUuid>), Outcome<CommandAnswer>> {
    match contract.path {
        ["steer"] => {
            let lane = uuid_operand(matches, "LANE", "steer::lane-invalid")?;
            let message = matches
                .get_one::<String>("TEXT")
                .expect("clap requires steering text");
            Ok((SelectedCommand::SteerSend { lane, message }, None))
        }
        ["steer", "cancel"] => {
            let directive = uuid_operand(matches, "ID", "steer::directive-invalid")?;
            Ok((SelectedCommand::SteerCancel { directive }, None))
        }
        ["steer", "recover"] => {
            let directive = uuid_operand(matches, "ID", "steer::directive-invalid")?;
            Ok((SelectedCommand::SteerRecover { directive }, None))
        }
        ["inbox"] => {
            let lane = optional_uuid_operand(matches, "LANE", "steer::lane-invalid")?;
            Ok((
                SelectedCommand::SteerInbox { lane },
                lane_context(lane_environment)?,
            ))
        }
        ["hook"] => {
            let Some(lane) = lane_context(lane_environment)? else {
                return Err(refusal(
                    "steer::lane-context-absent",
                    None,
                    "hook requires the SPICE_LANE identity of an activated lane shell",
                ));
            };
            Ok((SelectedCommand::SteerInbox { lane: None }, Some(lane)))
        }
        ["ack"] => {
            let key = key.expect("the ack contract parsed its key above");
            let disposition = if matches.get_flag("decline") {
                Disposition::Declined
            } else {
                Disposition::Acked
            };
            Ok((
                SelectedCommand::SteerAck { key, disposition },
                lane_context(lane_environment)?,
            ))
        }
        _ => Err(environment_fault(
            "steer::dispatch-invalid",
            "the steering adapter received another CLI contract",
        )),
    }
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
