//! CLI acquisition for the resident steering command family.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use spice::repository::Repository;
use wire::intent::{CommandAnswer, InboxAnswer};
use wire::{CanonicalUuid, Disposition, Fault, FaultClass, Outcome, Refusal};

use crate::command_client::{SelectedCommand, execute_steering};

mod codex_hook;
mod media_projection;
#[cfg(test)]
mod tests;

pub(crate) use codex_hook::Invocation as HookInvocation;
pub(crate) use codex_hook::validate_rtk_hook_contract as validate_codex_rtk_hook_contract;

pub(crate) fn read_hook_invocation(
    reader: &mut impl io::Read,
    harness: &str,
) -> Result<HookInvocation, String> {
    codex_hook::read(reader, harness)
}

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

struct HookPublication {
    lanes: Vec<HookLanePublication>,
}

struct HookLanePublication {
    lane: CanonicalUuid,
    working_state: Option<String>,
    task_contract_notices: Vec<HookTaskContractNotice>,
    groups: Vec<HookDirectiveGroup>,
}

struct HookTaskContractNotice {
    task: wire::TaskStamp,
    edit: wire::Seq,
    claim: wire::Seq,
    stage: String,
    text: Option<String>,
    key: String,
}

struct HookDirectiveGroup {
    identity: String,
    stage: String,
    text: Option<String>,
    media: Vec<media_projection::Projection>,
    keys: Vec<String>,
}

#[derive(serde::Serialize)]
struct HookContentIdentity<'a> {
    lane: CanonicalUuid,
    directive: CanonicalUuid,
    occupant: CanonicalUuid,
    compaction: u64,
    stage: wire::body::SteeringStage,
    text: &'a str,
    media: &'a [wire::AgentMediaReference],
}

#[derive(serde::Serialize)]
struct HookTaskContractIdentity<'a> {
    lane: CanonicalUuid,
    directive: CanonicalUuid,
    occupant: CanonicalUuid,
    compaction: u64,
    task: &'a wire::TaskStamp,
    edit: wire::Seq,
    claim: wire::Seq,
    stage: wire::body::SteeringStage,
    text: &'a str,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    working_state: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    delivered_keys: Vec<DeliveredKey>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    projections: Vec<media_projection::Record>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveredKey {
    content: String,
    key: String,
}

pub(crate) fn hook_document(
    answer: &CommandAnswer,
    harness: &str,
    working_state: Option<&spice::session_meter::WorkingState>,
) -> Result<Option<Vec<u8>>, String> {
    validate_hook_answer(answer, harness)?;
    let CommandAnswer::Inbox(_) = answer else {
        unreachable!("validated hook answer is an inbox");
    };
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
    hook_document_at(
        answer,
        harness,
        delivery.state(),
        delivery.lock(),
        delivery.projections(),
        working_state,
    )
}

fn hook_document_at(
    answer: &CommandAnswer,
    harness: &str,
    state_path: &Path,
    lock_path: &Path,
    projection_root: &Path,
    working_state: Option<&spice::session_meter::WorkingState>,
) -> Result<Option<Vec<u8>>, String> {
    validate_hook_answer(answer, harness)?;
    let CommandAnswer::Inbox(inbox) = answer else {
        unreachable!("validated hook answer is an inbox");
    };
    let name = sill::LockName::new(HOOK_DELIVERY_LOCK).expect("fixed hook lock name is valid");
    let _possession = sill::LockFile::new(name, lock_path)
        .acquire()
        .map_err(|error| format!("inbox hook delivery lock unavailable: {error}"))?;
    let mut state = read_hook_delivery_state(state_path)?;
    let working_status = working_state.and_then(spice::session_meter::ordinary_status);
    let (filtered, changed) = filter_hook_inbox(
        inbox,
        &mut state,
        projection_root,
        working_status.as_deref(),
    )?;
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

fn render_hook_document(publication: &HookPublication) -> Result<Option<Vec<u8>>, String> {
    let executable = std::env::current_exe()
        .map(PathBuf::into_os_string)
        .map_err(|error| format!("inbox hook executable unavailable: {error}"))?;
    let executable = executable.to_string_lossy();
    let mut context = String::new();
    let has_task_contract_notices = publication
        .lanes
        .iter()
        .any(|lane| !lane.task_contract_notices.is_empty());
    if publication
        .lanes
        .iter()
        .any(|lane| lane.working_state.is_some() || !lane.task_contract_notices.is_empty())
    {
        context.push_str("Rust Spice system status.");
    }
    if has_task_contract_notices {
        context.push_str(concat!(
            " Task-contract revision acknowledgment is required ",
            "before repository work continues. Begin assistant prose with `ACK <key> ",
            "[<key> ...]:`, then run every printed acknowledgment command."
        ));
    }
    for lane in &publication.lanes {
        if let Some(status) = &lane.working_state {
            context.push_str(&format!("\n\nlane={}\n{status}", lane.lane));
        }
    }
    if has_task_contract_notices {
        for lane in &publication.lanes {
            for notice in &lane.task_contract_notices {
                context.push_str(&format!(
                    "\n\nTASK CONTRACT REVISION lane={} task={} edit={} claim={} key={} stage={}\n",
                    lane.lane, notice.task, notice.edit, notice.claim, notice.key, notice.stage,
                ));
                match &notice.text {
                    Some(text) => context.push_str(text),
                    None => context.push_str(
                        "This exact task-contract revision was already shown in the current \
                         compaction epoch; its acknowledgment remains pending.",
                    ),
                }
                context.push_str(&format!(
                    "\nacknowledgment: `{executable} ack {}`",
                    notice.key
                ));
            }
        }
    }
    if publication.lanes.iter().any(|lane| !lane.groups.is_empty()) {
        if !context.is_empty() {
            context.push_str("\n\n");
        }
        context.push_str(concat!(
            "Rust Spice steering is pending. Address every key immediately before other work. ",
            "Begin assistant prose with `ACK <key> [<key> ...]:` or ",
            "`NACK <key> [<key> ...]:`, then retire every listed key after that prose by ",
            "running each printed `ack <key>` command; append `--decline` for a NACK."
        ));
    }
    for lane in &publication.lanes {
        for group in &lane.groups {
            let keys = if group.keys.len() == 1 {
                format!("key={}", group.keys[0])
            } else {
                format!("keys={}", group.keys.join(" "))
            };
            context.push_str(&format!(
                "\n\nlane={} {} stage={}\n",
                lane.lane, keys, group.stage,
            ));
            match &group.text {
                Some(text) => context.push_str(text),
                None => context.push_str(
                    "Same-content steering was already delivered in this compaction epoch. \
                     Acknowledge every newly listed key without replaying the directive text.",
                ),
            }
            for projection in &group.media {
                let reference =
                    wire::encode(projection.reference()).map_err(|error| error.to_string())?;
                context.push_str("\nmedia-reference=");
                context.push_str(
                    std::str::from_utf8(&reference)
                        .expect("an encoded media reference is UTF-8 JSON"),
                );
                let projection = projection.encoded()?;
                context.push_str("\nmedia-projection=");
                context.push_str(
                    std::str::from_utf8(&projection)
                        .expect("an encoded media projection is UTF-8 JSON"),
                );
            }
            for key in &group.keys {
                context.push_str(&format!("\ncommand: `{executable} ack {key}`"));
            }
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
    projection_root: &Path,
    working_status: Option<&str>,
) -> Result<(Option<HookPublication>, bool), String> {
    if working_status.is_some() && inbox.lanes.len() != 1 {
        return Err("working-state hook status requires one selected lane".to_owned());
    }
    let mut changed = false;
    let mut lanes = Vec::new();
    for lane in &inbox.lanes {
        let (filtered, lane_changed) =
            filter_hook_lane(lane, state, projection_root, working_status)?;
        changed |= lane_changed;
        if let Some(filtered) = filtered {
            lanes.push(filtered);
        }
    }
    let filtered = (!lanes.is_empty()).then_some(HookPublication { lanes });
    Ok((filtered, changed))
}

fn filter_hook_lane(
    lane: &wire::intent::LaneInbox,
    state: &mut HookDeliveryState,
    projection_root: &Path,
    working_status: Option<&str>,
) -> Result<(Option<HookLanePublication>, bool), String> {
    let existing = state.lanes.iter().position(|row| row.lane == lane.lane);
    if lane.pending.is_empty()
        && lane.task_contract_notices.is_empty()
        && working_status.is_none()
        && existing.is_none()
    {
        return Ok((None, false));
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
    if existing.is_some_and(|index| compaction < state.lanes[index].compaction) {
        return Ok((None, false));
    }
    let (row, mut changed) = delivery_row(state, lane.lane, occupant, compaction, existing);
    let before = row.projections.clone();
    let projections = media_projection::reconcile(
        lane.lane,
        &lane.pending,
        &mut row.projections,
        projection_root,
    )?;
    changed |= before != row.projections;
    let (filtered, directives_changed) = filter_hook_directives(
        lane,
        occupant,
        compaction,
        row,
        &projections,
        working_status,
    )?;
    Ok((filtered, changed || directives_changed))
}

fn delivery_row(
    state: &mut HookDeliveryState,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    compaction: u64,
    existing: Option<usize>,
) -> (&mut LaneDeliveryState, bool) {
    let mut changed = false;
    let row = match existing {
        Some(index) => &mut state.lanes[index],
        None => {
            state.lanes.push(LaneDeliveryState {
                lane,
                occupant,
                compaction,
                contents: Vec::new(),
                working_state: None,
                delivered_keys: Vec::new(),
                projections: Vec::new(),
            });
            changed = true;
            state.lanes.last_mut().expect("the lane row was appended")
        }
    };
    if row.occupant != occupant || row.compaction < compaction {
        row.occupant = occupant;
        row.compaction = compaction;
        row.contents.clear();
        row.working_state = None;
        row.delivered_keys.clear();
        changed = true;
    }
    (row, changed)
}

fn filter_hook_directives(
    lane: &wire::intent::LaneInbox,
    occupant: CanonicalUuid,
    compaction: u64,
    row: &mut LaneDeliveryState,
    projections: &std::collections::BTreeMap<CanonicalUuid, Vec<media_projection::Projection>>,
    working_status: Option<&str>,
) -> Result<(Option<HookLanePublication>, bool), String> {
    let mut changed = false;
    let working_state = (row.working_state.as_deref() != working_status)
        .then(|| working_status.map(str::to_owned))
        .flatten();
    if row.working_state.as_deref() != working_status {
        row.working_state = working_status.map(str::to_owned);
        changed = true;
    }
    let mut filtered = HookLanePublication {
        lane: lane.lane,
        working_state,
        task_contract_notices: Vec::new(),
        groups: Vec::new(),
    };
    for notice in &lane.task_contract_notices {
        let identity = rendered_task_contract_identity(lane.lane, occupant, compaction, notice)?;
        if row
            .delivered_keys
            .iter()
            .any(|delivered| delivered.content == identity && delivered.key == notice.key)
        {
            continue;
        }
        let content_was_delivered = row.contents.contains(&identity);
        filtered.task_contract_notices.push(HookTaskContractNotice {
            task: notice.task.clone(),
            edit: notice.edit,
            claim: notice.claim,
            stage: notice.stage.to_string(),
            text: (!content_was_delivered).then(|| notice.text.clone()),
            key: notice.key.clone(),
        });
        if !content_was_delivered {
            row.contents.push(identity.clone());
        }
        row.delivered_keys.push(DeliveredKey {
            content: identity,
            key: notice.key.clone(),
        });
        changed = true;
    }
    for directive in &lane.pending {
        let references =
            wire::media::agent_media_references(directive.directive, &directive.attachments)
                .map_err(|error| {
                    format!(
                        "inbox hook directive {} has invalid media authority: {error}",
                        directive.directive,
                    )
                })?;
        let media = projections
            .get(&directive.directive)
            .cloned()
            .unwrap_or_default();
        let text = wire::media::without_media_payloads(&directive.text, &directive.attachments);
        let identity = rendered_content_identity(
            lane.lane,
            occupant,
            compaction,
            directive,
            &text,
            &references,
        )?;
        if row
            .delivered_keys
            .iter()
            .any(|delivered| delivered.content == identity && delivered.key == directive.key)
        {
            continue;
        }
        let content_was_delivered = row.contents.contains(&identity);
        match filtered
            .groups
            .iter_mut()
            .find(|group| group.identity == identity)
        {
            Some(group) => group.keys.push(directive.key.clone()),
            None => filtered.groups.push(HookDirectiveGroup {
                identity: identity.clone(),
                stage: directive.stage.to_string(),
                text: (!content_was_delivered).then_some(text),
                media,
                keys: vec![directive.key.clone()],
            }),
        }
        if !content_was_delivered {
            row.contents.push(identity.clone());
        }
        row.delivered_keys.push(DeliveredKey {
            content: identity,
            key: directive.key.clone(),
        });
        changed = true;
    }
    Ok((
        (filtered.working_state.is_some()
            || !filtered.groups.is_empty()
            || !filtered.task_contract_notices.is_empty())
        .then_some(filtered),
        changed,
    ))
}

fn rendered_content_identity(
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    compaction: u64,
    directive: &wire::intent::PendingDirective,
    text: &str,
    media: &[wire::AgentMediaReference],
) -> Result<String, String> {
    let identity = HookContentIdentity {
        lane,
        directive: directive.directive,
        occupant,
        compaction,
        stage: directive.stage,
        text,
        media,
    };
    let encoded = wire::encode(&identity).map_err(|error| error.to_string())?;
    Ok(blake3::hash(&encoded).to_hex().to_string())
}

fn rendered_task_contract_identity(
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    compaction: u64,
    notice: &wire::intent::TaskContractNotice,
) -> Result<String, String> {
    let identity = HookTaskContractIdentity {
        lane,
        directive: notice.directive,
        occupant,
        compaction,
        task: &notice.task,
        edit: notice.edit,
        claim: notice.claim,
        stage: notice.stage,
        text: &notice.text,
    };
    let encoded = wire::encode(&identity).map_err(|error| error.to_string())?;
    Ok(blake3::hash(&encoded).to_hex().to_string())
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
