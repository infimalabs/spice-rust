//! oh-my-pi's current RPC launch contract.
//!
//! omp is driven over `--mode rpc`: one JSON command per line on stdin, one
//! JSON frame per line on stdout. The argument vector fixes the model,
//! thinking level, service tier, and system prompt; the conversation itself is opened by
//! the request bytes the supervisor writes once stdin exists, and steering
//! is interjected the same way for as long as the child lives.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

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

/// Request identity of the interrupt a drain sends.
pub const ABORT_REQUEST_ID: &str = "spice-abort";

/// Environment name carrying the session identity the lane pinned at launch.
///
/// omp exports nothing of its own to the tools it runs, so the lane mints the
/// identity before launch and carries it under this name; `spice session`
/// inside the occupant's tools reads it back the way it reads Claude's and
/// Codex's native variables.
pub const SESSION_ENVIRONMENT: &str = wire::SpiceVariable::Session.as_str();

/// The tools a lane's occupant runs with, spelled as omp's `--tools` roster.
///
/// One agent inhabits one worktree, so omp's `task` sub-agent tool is left
/// off the roster mechanically rather than by the skill's prose, exactly as
/// Claude's `Task` is denied; the rest is omp's own roster for a session
/// with no MCP server, so a tool omp adds later joins only by being named.
pub const TOOLS: &str = "read,write,edit,bash,glob,grep,eval,todo,hub,web_search";

/// Width of the fixed title slot that opens every omp session file.
const TITLE_SLOT_BYTES: usize = 256;

/// Session header version omp writes today.
const SESSION_VERSION: u8 = 3;

/// Build the exact current omp invocation without performing I/O.
///
/// # Errors
///
/// Returns a path-specific refusal for unsupported cross-vendor fields or a
/// fast-mode disposition left implicit, or the common argument-vector refusal
/// when the binary is empty.
pub fn record(binary: impl Into<OsString>, launch: &Launch) -> Result<Record, ContractError> {
    if launch.personality.is_some() {
        return Err(ContractError::Unsupported {
            path: "vendor.omp.personality",
            vendor: VENDOR,
        });
    }
    if launch.model_context_window.is_some() {
        return Err(ContractError::Unsupported {
            path: "vendor.omp.model_context_window",
            vendor: VENDOR,
        });
    }
    let fast_mode = launch.fast_mode.ok_or(ContractError::Missing {
        path: "vendor.omp.fast_mode",
    })?;
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
        // Fast mode is the priority tier on every provider family omp serves;
        // an explicit `none` keeps the lane's disposition ahead of omp's own
        // settings when it is off.
        OsString::from("--service-tier"),
        OsString::from(if fast_mode { "priority" } else { "none" }),
        OsString::from("--tools"),
        OsString::from(TOOLS),
        OsString::from("--append-system-prompt"),
        OsString::from(system_prompt(launch)),
    ];
    if let Some(session) = launch.resume.session_id() {
        arguments.push(OsString::from("--resume"));
        arguments.push(OsString::from(session));
    }
    let environment = launch
        .resume
        .session_id()
        .map(|session| (OsString::from(SESSION_ENVIRONMENT), OsString::from(session)));
    Ok(Record::new(
        binary,
        arguments,
        Dialect::Omp,
        RecordStream::Stdout,
        launch.resume.clone(),
    )?
    .with_opening(opening_request(launch))
    .with_environment(environment))
}

/// The session file a lane writes before launch so the identity is known.
///
/// omp names a session only once it starts, so a fresh lane writes the file
/// omp would have written, where omp keeps this working directory's sessions,
/// and resumes it by identity: an empty fixed-width title slot, then the
/// version-3 header. `home`, `temp`, and `cwd` are the canonical directories
/// omp encodes the working directory against; `at` is the ISO-8601 instant,
/// to the millisecond, that names the file and stamps the header.
#[must_use]
pub fn session_file(
    sessions: &Path,
    home: &Path,
    temp: &Path,
    cwd: &Path,
    identity: &str,
    at: &str,
) -> (PathBuf, String) {
    let path = sessions
        .join(session_directory(home, temp, cwd))
        .join(format!("{}_{identity}.jsonl", at.replace([':', '.'], "-")));
    let header = SessionHeader {
        kind: "session",
        version: SESSION_VERSION,
        id: identity,
        timestamp: at,
        cwd: &cwd.to_string_lossy(),
    };
    (path, format!("{}{}\n", title_slot(at), json(&header)))
}

/// omp's session directory for `cwd`: its home-relative or temp-relative
/// spelling under a scope prefix, or the whole absolute path bracketed.
fn session_directory(home: &Path, temp: &Path, cwd: &Path) -> String {
    let dashed = |path: &Path| path.to_string_lossy().replace(['/', '\\', ':'], "-");
    for (prefix, root) in [("-", home), ("-tmp", temp)] {
        if let Ok(relative) = cwd.strip_prefix(root) {
            let relative = dashed(relative);
            let separator = if prefix.ends_with('-') { "" } else { "-" };
            return if relative.is_empty() {
                prefix.to_owned()
            } else {
                format!("{prefix}{separator}{relative}")
            };
        }
    }
    format!("--{}--", dashed(cwd).trim_start_matches('-'))
}

/// An empty title padded to exactly [`TITLE_SLOT_BYTES`] with its newline.
fn title_slot(at: &str) -> String {
    let mut slot = TitleSlot {
        kind: "title",
        v: 1,
        title: "",
        updated_at: at,
        pad: String::new(),
    };
    slot.pad = " ".repeat(TITLE_SLOT_BYTES.saturating_sub(json(&slot).len() + 1));
    format!("{}\n", json(&slot))
}

fn json(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("a session record is representable as JSON")
}

/// omp rewrites this line in place, so it is written key-for-key as omp does.
#[derive(Serialize)]
struct TitleSlot<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    v: u8,
    title: &'a str,
    #[serde(rename = "updatedAt")]
    updated_at: &'a str,
    pad: String,
}

#[derive(Serialize)]
struct SessionHeader<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    version: u8,
    id: &'a str,
    timestamp: &'a str,
    cwd: &'a str,
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

/// The interrupt a drain sends before any signal.
///
/// omp stops the turn where it stands and records the assistant message as
/// aborted, so the session and the tape both say the turn was interrupted
/// instead of ending in a signal nothing recorded.
#[must_use]
pub fn abort_request() -> Vec<u8> {
    request_line(&Request {
        id: ABORT_REQUEST_ID.to_owned(),
        kind: "abort",
        message: None,
        streaming_behavior: None,
    })
}

/// The decline of a dialog nobody at this lane can compose an answer to.
///
/// omp holds a turn on a select, input, or editor request until it is
/// answered. Declining is what an operator's escape would say: nothing was
/// chosen or typed, so the agent carries on instead of waiting for an
/// operator who is not there.
#[must_use]
pub fn dialog_declined(id: &str) -> Vec<u8> {
    request_line(&DialogResponse {
        kind: "extension_ui_response",
        id,
        cancelled: Some(true),
        confirmed: None,
    })
}

/// The answer to a confirm dialog, once consent has ruled on it.
///
/// A confirm holds the turn exactly as any other dialog does, but it asks a
/// yes-or-no question, so a consent grant answers yes and a denial answers no.
#[must_use]
pub fn dialog_confirmed(id: &str, confirmed: bool) -> Vec<u8> {
    request_line(&DialogResponse {
        kind: "extension_ui_response",
        id,
        cancelled: None,
        confirmed: Some(confirmed),
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

fn request_line(request: &impl Serialize) -> Vec<u8> {
    let mut line = serde_json::to_vec(request).expect("an RPC request is representable as JSON");
    line.push(b'\n');
    line
}

/// omp reads these key-for-key, so the types declare their own order.
#[derive(Serialize)]
struct DialogResponse<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    cancelled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    confirmed: Option<bool>,
}

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
