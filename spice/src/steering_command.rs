//! CLI acquisition for the resident steering command family.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use spice::repository::Repository;
use wire::intent::{CommandAnswer, InboxAnswer, LaneInbox};
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

const HOOK_DELIVERY_LOCK: &str = "steering-delivery";

/// Durable equivalent of spice-z's `MaximReminderGate` publish boundary.
///
/// The predecessor keeps a process-local content-key-to-compaction-index map in
/// the one watchdog stdout thread. Rust hook commands are separate processes,
/// so this record and its kernel lock preserve the same content/epoch invariant
/// across invocations. Directive keys remain transport identities and are
/// deliberately absent from the rendered-content digest.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HookDeliveryState {
    lanes: Vec<LaneDeliveryState>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LaneDeliveryState {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    compaction: u64,
    contents: Vec<String>,
}

pub(crate) fn hook_document(
    answer: &CommandAnswer,
    harness: &str,
) -> Result<Option<Vec<u8>>, String> {
    validate_hook_answer(answer, harness)?;
    let CommandAnswer::Inbox(inbox) = answer else {
        unreachable!("validated hook answer is an inbox");
    };
    if inbox.lanes.iter().all(|lane| lane.pending.is_empty()) {
        return Ok(None);
    }
    let current = std::env::current_dir()
        .map_err(|error| format!("inbox hook working directory unavailable: {error}"))?;
    let repository = Repository::discover(&current)
        .map_err(|error| format!("inbox hook repository unavailable: {error}"))?;
    let delivery = repository
        .admit_steering_delivery()
        .map_err(|error| format!("inbox hook state unavailable: {error}"))?;
    let lock_parent = delivery
        .lock()
        .parent()
        .expect("the admitted hook lock has a parent");
    fs::create_dir_all(lock_parent)
        .map_err(|error| format!("inbox hook lock directory unavailable: {error}"))?;
    hook_document_at(answer, harness, delivery.state(), delivery.lock())
}

fn hook_document_at(
    answer: &CommandAnswer,
    harness: &str,
    state_path: &Path,
    lock_path: &Path,
) -> Result<Option<Vec<u8>>, String> {
    validate_hook_answer(answer, harness)?;
    let CommandAnswer::Inbox(inbox) = answer else {
        unreachable!("validated hook answer is an inbox");
    };
    if inbox.lanes.iter().all(|lane| lane.pending.is_empty()) {
        return Ok(None);
    }
    let name = sill::LockName::new(HOOK_DELIVERY_LOCK).expect("fixed hook lock name is valid");
    let _possession = sill::LockFile::new(name, lock_path)
        .acquire()
        .map_err(|error| format!("inbox hook delivery lock unavailable: {error}"))?;
    let mut state = read_hook_delivery_state(state_path)?;
    let (filtered, changed) = filter_hook_inbox(inbox, &mut state)?;
    let document = filtered
        .as_ref()
        .map(render_hook_document)
        .transpose()?
        .flatten();
    if changed {
        write_hook_delivery_state(state_path, &state)?;
    }
    Ok(document)
}

fn validate_hook_answer(answer: &CommandAnswer, harness: &str) -> Result<(), String> {
    if spice::extension::vendor(harness).is_none() {
        let expected = spice::extension::registry()
            .map_err(|error| error.to_string())?
            .vendors()
            .into_iter()
            .map(|vendor| vendor.name())
            .collect::<Vec<_>>()
            .join(" or ");
        return Err(format!(
            "unsupported hook harness {harness:?}; expected {expected}"
        ));
    }
    let CommandAnswer::Inbox(_) = answer else {
        return Err("inbox hook received a non-inbox command answer".to_owned());
    };
    Ok(())
}

fn render_hook_document(inbox: &InboxAnswer) -> Result<Option<Vec<u8>>, String> {
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
                "\n\nlane={} key={} stage={}\n{}",
                lane.lane, directive.key, directive.stage, directive.text
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

fn read_hook_delivery_state(path: &Path) -> Result<HookDeliveryState, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(HookDeliveryState::default());
        }
        Err(error) => return Err(format!("inbox hook delivery state unreadable: {error}")),
    };
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("inbox hook delivery state invalid: {error}"))
}

fn write_hook_delivery_state(path: &Path, state: &HookDeliveryState) -> Result<(), String> {
    let bytes = serde_json::to_vec(state)
        .map_err(|error| format!("inbox hook delivery state unencodable: {error}"))?;
    atomic::Destination::new(path)
        .map_err(|error| format!("inbox hook delivery state unavailable: {error}"))?
        .publish(|staged| staged.write_all(&bytes))
        .map_err(|error| format!("inbox hook delivery state unpublished: {error}"))
}

fn filter_hook_inbox(
    inbox: &InboxAnswer,
    state: &mut HookDeliveryState,
) -> Result<(Option<InboxAnswer>, bool), String> {
    let mut changed = false;
    let mut lanes = Vec::new();
    for lane in &inbox.lanes {
        if lane.pending.is_empty() {
            continue;
        }
        let occupant = lane.occupant.ok_or_else(|| {
            format!(
                "inbox hook lane {} has pending steering without an occupant",
                lane.lane
            )
        })?;
        let compaction = lane.compaction.ok_or_else(|| {
            format!(
                "inbox hook lane {} has pending steering without a compaction anchor",
                lane.lane
            )
        })?;
        let row = match state.lanes.iter().position(|row| row.lane == lane.lane) {
            Some(index) => &mut state.lanes[index],
            None => {
                state.lanes.push(LaneDeliveryState {
                    lane: lane.lane,
                    occupant,
                    compaction,
                    contents: Vec::new(),
                });
                changed = true;
                state.lanes.last_mut().expect("the lane row was appended")
            }
        };
        if compaction < row.compaction {
            continue;
        }
        if row.occupant != occupant || row.compaction < compaction {
            row.occupant = occupant;
            row.compaction = compaction;
            row.contents.clear();
            changed = true;
        }
        let mut filtered = LaneInbox {
            pending: Vec::new(),
            ..lane.clone()
        };
        for directive in &lane.pending {
            let identity = rendered_content_identity(lane.lane, directive);
            if !row.contents.contains(&identity) {
                row.contents.push(identity);
                filtered.pending.push(directive.clone());
                changed = true;
            }
        }
        if !filtered.pending.is_empty() {
            lanes.push(filtered);
        }
    }
    let filtered = (!lanes.is_empty()).then_some(InboxAnswer {
        epoch: inbox.epoch,
        lanes,
    });
    Ok((filtered, changed))
}

fn rendered_content_identity(
    lane: CanonicalUuid,
    directive: &wire::intent::PendingDirective,
) -> String {
    let rendered = format!("lane={lane} stage={}\n{}", directive.stage, directive.text);
    blake3::hash(rendered.as_bytes()).to_hex().to_string()
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
    let canvas = match repository.admit_resident_client() {
        Ok(canvas) => canvas,
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
    execute_steering(&canvas, contract, selected, lane_context)
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
        ["steer", "resend"] => {
            let directive = uuid_operand(matches, "ID", "steer::directive-invalid")?;
            Ok((SelectedCommand::SteerResend { directive }, None))
        }
        ["steer", "recover"] => {
            let directive = uuid_operand(matches, "ID", "steer::directive-invalid")?;
            Ok((SelectedCommand::SteerRecover { directive }, None))
        }
        ["steer", "repair"] => {
            let lane = uuid_operand(matches, "LANE", "steer::lane-invalid")?;
            Ok((SelectedCommand::SteerRepair { lane }, None))
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
                    &format!(
                        "hook requires the {} identity of an activated Rust lane shell",
                        steer::hook::LANE
                    ),
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
            &format!("{} must name one canonical lane UUID", steer::hook::LANE),
        )
    })?;
    text.parse().map(Some).map_err(|_| {
        refusal(
            "steer::lane-context-invalid",
            Some(text),
            &format!("{} must name one canonical lane UUID", steer::hook::LANE),
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

fn refusal(reason: &str, observed: Option<String>, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            reason.to_owned(),
            Some("steering-operand".to_owned()),
            None,
            observed,
        )
        .with_detail(detail),
    ))
}

fn environment_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}
