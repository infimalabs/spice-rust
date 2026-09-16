//! oh-my-pi's current RPC launch contract.
//!
//! omp is driven over `--mode rpc`: one JSON command per line on stdin, one
//! JSON frame per line on stdout. The argument vector fixes the model,
//! thinking level, and system prompt; the conversation itself is opened by
//! the request bytes the supervisor writes once stdin exists, and steering
//! is interjected the same way for as long as the child lives.

use std::ffi::OsString;

use listen::Dialect;
use serde::Serialize;
use sill::Running;

use crate::claude::SKILL_SYSTEM_PROMPT_PREAMBLE;
use crate::launch::Launch;
use crate::{ContractError, LaunchError, Process, Record, RecordStream};

/// Human-facing vendor spelling carried by every refusal.
const VENDOR: &str = "oh-my-pi";

/// Request identity of the `get_state` command that opens every launch.
///
/// Its reply is the session declaration: the decoder reads the session
/// identity from it, so the supervisor asks before anything else.
pub const STATE_REQUEST_ID: &str = "spice-state";

/// Request identity of the prompt that starts the first turn.
pub const PROMPT_REQUEST_ID: &str = "spice-prompt";

/// Prefix of the request identities that carry interjected steering.
pub const STEER_REQUEST_ID_PREFIX: &str = "spice-steer-";

/// Build the exact current omp invocation without performing I/O.
///
/// # Errors
///
/// Returns a path-specific refusal for unsupported cross-vendor fields, or the
/// common argument-vector refusal when the binary is empty.
pub fn record(binary: impl Into<OsString>, launch: &Launch) -> Result<Record, ContractError> {
    if launch.personality.is_some() {
        return Err(ContractError::Unsupported {
            path: "vendor.omp.personality",
            vendor: VENDOR,
        });
    }
    if launch.fast_mode.is_some() {
        return Err(ContractError::Unsupported {
            path: "vendor.omp.fast_mode",
            vendor: VENDOR,
        });
    }
    if launch.model_context_window.is_some() {
        return Err(ContractError::Unsupported {
            path: "vendor.omp.model_context_window",
            vendor: VENDOR,
        });
    }
    let mut arguments = vec![
        OsString::from("--mode"),
        OsString::from("rpc"),
        // The lane is the approval authority, exactly as the other dialects
        // bypass their own permission prompts; a prompt no operator can see
        // would otherwise hold the turn forever.
        OsString::from("--approval-mode"),
        OsString::from("yolo"),
        // Title generation spends a model call on a session no operator scrolls.
        OsString::from("--no-title"),
        OsString::from("--model"),
        OsString::from(launch.model.as_str()),
        OsString::from("--thinking"),
        OsString::from(launch.effort.as_str()),
        OsString::from("--append-system-prompt"),
        OsString::from(system_prompt(launch)),
    ];
    if let Some(session) = launch.resume.session_id() {
        arguments.push(OsString::from("--resume"));
        arguments.push(OsString::from(session));
    }
    Ok(Record::new(
        binary,
        arguments,
        Dialect::Omp,
        RecordStream::Stdout,
        launch.resume.clone(),
    )?
    .with_opening(opening_request(launch)))
}

/// The skill preamble, the start prompt, and this worktree's steering anchor.
///
/// Steering reaches omp as an interjected user message rather than on tool
/// stderr, so the anchor names that channel; the token is otherwise the same
/// one Claude's prompt carries.
#[must_use]
pub fn system_prompt(launch: &Launch) -> String {
    let mut prompt = format!("{SKILL_SYSTEM_PROMPT_PREAMBLE}\n\n{}", launch.prompt);
    if let Some(key) = &launch.steering_key {
        prompt.push_str("\n\n");
        prompt.push_str(&key.interjection_line());
    }
    prompt
}

/// The request lines that open a launch: a state query, then the first turn.
///
/// omp writes its `ready` frame before reading stdin, so both requests may be
/// queued before the child has answered anything. The state reply declares
/// the session; the prompt starts the turn with the same composed system
/// prompt the arguments carry, so the agent re-grounds in the skill.
#[must_use]
pub fn opening_request(launch: &Launch) -> Vec<u8> {
    let mut bytes = request_line(&Request {
        id: STATE_REQUEST_ID.to_owned(),
        kind: "get_state",
        message: None,
        streaming_behavior: None,
    });
    bytes.extend(request_line(&Request {
        id: PROMPT_REQUEST_ID.to_owned(),
        kind: "prompt",
        message: Some(system_prompt(launch)),
        streaming_behavior: None,
    }));
    bytes
}

/// One interjected steering message, numbered by its place in the launch.
///
/// While omp is streaming, the message queues through its steering queue and
/// is delivered after the current tool call; while it is idle, the same
/// request starts a new turn. Either way the agent sees it as a user message.
#[must_use]
pub fn steer_request(sequence: u64, message: &str) -> Vec<u8> {
    request_line(&Request {
        id: format!("{STEER_REQUEST_ID_PREFIX}{sequence}"),
        kind: "prompt",
        message: Some(message.to_owned()),
        streaming_behavior: Some("steer"),
    })
}

/// Launch omp through the sole `sill` process door.
///
/// The returned child has a piped stdin the caller must feed with
/// [`Record::opening`]; nothing happens until it does.
///
/// # Errors
///
/// Returns [`LaunchError`] when the contract is invalid or `sill` cannot
/// create the child.
pub fn launch(
    binary: impl Into<OsString>,
    launch: &Launch,
    process: &Process,
) -> Result<Running, LaunchError> {
    let record = record(binary, launch).map_err(LaunchError::Contract)?;
    sill::spawn(record.spec(process)).map_err(LaunchError::Spawn)
}

fn request_line(request: &Request) -> Vec<u8> {
    let mut line = serde_json::to_vec(request).expect("an RPC request is representable as JSON");
    line.push(b'\n');
    line
}

/// omp reads these key-for-key, so the type declares its own order.
#[derive(Serialize)]
struct Request {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(rename = "streamingBehavior", skip_serializing_if = "Option::is_none")]
    streaming_behavior: Option<&'static str>,
}
