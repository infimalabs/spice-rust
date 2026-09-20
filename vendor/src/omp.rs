//! oh-my-pi's current RPC launch contract.
//!
//! omp is driven over `--mode rpc`: one JSON command per line on stdin, one
//! JSON frame per line on stdout. The argument vector fixes the model,
//! thinking level, service tier, and system prompt; the conversation itself is opened by
//! the request bytes the supervisor writes once stdin exists, and steering
//! is interjected the same way for as long as the child lives.

pub mod frame;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
/// Every tool that would start a process is left off: `bash`, `eval`, and
/// `hub` would run commands the lane cannot see or outlive, so the lane
/// registers its own [`SHELL_TOOL`] instead and executes every command
/// itself. One agent inhabits one worktree, so omp's `task` sub-agent tool
/// is left off as well, exactly as Claude's `Task` is denied. omp's `todo`
/// tool is left off for a different reason: the occupant's plan is the task
/// it holds, whose phases and acceptance the board already carries where
/// every other lane and the operator can read them, and a private second
/// copy is a plan no reviewer can check and one that drifts from the row it
/// shadows. The rest is omp's own roster for a session with no MCP server,
/// so a tool omp adds later joins only by being named.
pub const TOOLS: &str = "read,write,edit,glob,grep,web_search";

/// [`TOOLS`] plus omp's language-server tool, for a lane whose repository
/// policy admits it.
///
/// The tool starts language servers, which is why it is named only on that
/// explicit disposition rather than sitting in the roster above. The servers
/// stay inside the occupant's own process tree: [`config_overlay`] declines
/// omp's shared broker, and resident launch composition separately disables
/// the external lspmux route through its child environment.
/// What each server is and how it runs is the worktree's own business, read
/// from the repository's `.omp/lsp.json`; this crate names no server.
pub const LSP_TOOLS: &str = "read,write,edit,glob,grep,web_search,lsp";

/// Request identity of the `set_host_tools` command that registers the
/// lane's shell ahead of the first turn.
pub const TOOLS_REQUEST_ID: &str = "spice-tools";

/// Name of the host tool the lane registers in place of omp's `bash`.
///
/// omp refuses a host tool named after any tool it ships, so the lane's shell
/// carries its own name; the model reaches it exactly as it would `bash`.
pub const SHELL_TOOL: &str = "shell";

/// Name of the host tool through which the model follows the commands the
/// lane backgrounded.
pub const JOB_TOOL: &str = "job";

/// Seconds a shell command may run when the agent names no deadline.
pub const SHELL_TIMEOUT_DEFAULT: u64 = 300;

/// Seconds a shell command runs in the foreground before the lane keeps it
/// as a job and answers with what it has said so far: omp's own
/// auto-background threshold, so the model's habit carries over.
pub const SHELL_FOREGROUND: u64 = 60;

/// Tightest deadline the agent may ask for, in seconds; zero asks for none.
pub const SHELL_TIMEOUT_MIN: u64 = 1;

/// Longest deadline the agent may ask for, in seconds; longer work belongs
/// to a process the lane keeps rather than one command.
pub const SHELL_TIMEOUT_MAX: u64 = 3600;

/// What the model reads about the lane's shell, in place of omp's `bash`.
const SHELL_DESCRIPTION: &str = "Runs one shell command in the worktree and returns its \
output. This is the only way to execute anything: the lane runs the command in its own \
environment and ends it at the deadline. Chain order-dependent commands with `&&`. The \
native wrapper bounds inline output and supplies a readable full-log path when needed; \
do not append `tail` just to limit output. A command still running after 60 seconds, or \
when a message arrives for you, is kept as a \
job: you get its output so far at once, it keeps running, and its result is delivered \
automatically when it ends.";

/// What the model reads about the command parameter.
const SHELL_COMMAND_DESCRIPTION: &str = "command to execute";

/// What the model reads about the lane's job tool.
const JOB_DESCRIPTION: &str = "Follows the commands the shell kept as jobs. A job's result \
is delivered automatically when it ends, so you never need to poll: use `list` to see every \
job, `logs` to read what one has said so far, `stop` to end one, and `wait` only when you \
are blocked on one. A wait returns when the job ends, when the timeout elapses, or when a \
message arrives for you; the job keeps running.";

/// What the model reads about the job operation parameter.
const JOB_OP_DESCRIPTION: &str = "what to do: list, logs, wait, or stop";

/// What the model reads about the job identity parameter.
const JOB_ID_DESCRIPTION: &str = "the job, as the shell named it; required except for list";

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
    let lsp = launch.lsp.ok_or(ContractError::Missing {
        path: "vendor.omp.lsp",
    })?;
    let config_overlay = launch
        .config_overlay
        .as_ref()
        .ok_or(ContractError::Missing {
            path: "vendor.omp.config_overlay",
        })?;
    let mut arguments = vec![
        // RPC mode already disables titles: generating one would spend a model
        // call on a session no operator scrolls.
        OsString::from("--mode"),
        OsString::from("rpc"),
        // The compaction threshold has no flag and no RPC command; it rides a
        // per-run overlay the lane writes, exactly as Codex takes its own
        // limit as a `--config` assignment.
        OsString::from("--config"),
        config_overlay.clone().into_os_string(),
        // The lane is the approval authority, exactly as the other dialects
        // bypass their own permission prompts; a prompt no operator can see
        // would otherwise hold the turn forever.
        OsString::from("--approval-mode"),
        OsString::from("yolo"),
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
        OsString::from(if lsp { LSP_TOOLS } else { TOOLS }),
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

/// The configuration overlay a lane writes before launch, stating the
/// compaction threshold and the language-server disposition this seat runs.
///
/// omp reads `compaction.thresholdTokens` ahead of every percentage or
/// reserve it would otherwise derive, clamped beneath the model's window.
///
/// The `lsp` block is stated either way rather than left to omp's defaults.
/// A declined lane turns the integration off outright, because omp otherwise
/// starts a language server behind the lane's back on the first write to a
/// file some server claims, whether or not the tool is in the roster. An
/// admitted lane declines omp's shared broker. The resident must also disable
/// external lspmux routing in the child environment; that route is independent
/// of this overlay. Which servers run stays the worktree's own business.
#[must_use]
pub fn config_overlay(auto_compact_threshold_tokens: u64, lsp: bool) -> String {
    let mut overlay = format!("compaction:\n  thresholdTokens: {auto_compact_threshold_tokens}\n");
    overlay.push_str(if lsp {
        "lsp:\n  enabled: true\n  shared: false\n"
    } else {
        "lsp:\n  enabled: false\n"
    });
    overlay
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
/// omp writes its `ready` frame before reading stdin, so every request may be
/// queued before the child has answered anything. The lane's shell is
/// registered first so the first turn's schema carries it; the state reply
/// declares the session; the prompt starts the turn with the same composed
/// system prompt the arguments carry, so the agent re-grounds in the skill.
#[must_use]
pub fn opening_request(launch: &Launch) -> Vec<u8> {
    let mut bytes = host_tools_request();
    bytes.extend(request_line(&Request {
        id: STATE_REQUEST_ID.to_owned(),
        kind: "get_state",
        message: None,
        streaming_behavior: None,
    }));
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

/// The `set_host_tools` command that registers the lane's shell.
///
/// The shell is essential, so it sits in the model's top-level schema rather
/// than behind discovery, and its parameters are the two omp's own `bash`
/// makes load-bearing: the command and its deadline. The job tool follows
/// what the shell kept running.
#[must_use]
pub fn host_tools_request() -> Vec<u8> {
    let deadline = serde_json::json!({
        "type": "integer",
        "minimum": 0,
        "maximum": SHELL_TIMEOUT_MAX,
        "description": format!(
            "timeout in seconds; 0 disables the command deadline; nonzero \
             values are clamped to {SHELL_TIMEOUT_MIN}-{SHELL_TIMEOUT_MAX}"
        ),
    });
    let wait = serde_json::json!({
        "type": "integer",
        "minimum": SHELL_TIMEOUT_MIN,
        "maximum": SHELL_TIMEOUT_MAX,
        "description": format!(
            "seconds to wait; omitted means {SHELL_FOREGROUND}, and \
             values are clamped to {SHELL_TIMEOUT_MIN}-{SHELL_TIMEOUT_MAX}"
        ),
    });
    request_line(&serde_json::json!({
        "id": TOOLS_REQUEST_ID,
        "type": "set_host_tools",
        "tools": [{
            "name": SHELL_TOOL,
            "label": "Shell",
            "description": SHELL_DESCRIPTION,
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": SHELL_COMMAND_DESCRIPTION},
                    "timeout": deadline,
                },
                "required": ["command"],
                "additionalProperties": false,
            },
            "loadMode": "essential",
        }, {
            "name": JOB_TOOL,
            "label": "Job",
            "description": JOB_DESCRIPTION,
            "parameters": {
                "type": "object",
                "properties": {
                    "op": {
                        "type": "string",
                        "enum": ["list", "logs", "wait", "stop"],
                        "description": JOB_OP_DESCRIPTION,
                    },
                    "id": {"type": "string", "description": JOB_ID_DESCRIPTION},
                    "timeout": wait,
                },
                "required": ["op"],
                "additionalProperties": false,
            },
            "loadMode": "essential",
        }],
    }))
}

/// What the model asks of one job.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobOp {
    List,
    Logs,
    Wait,
    Stop,
}

/// One request on the lane's jobs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobCall {
    pub op: JobOp,
    /// The job named, absent only for a listing.
    pub id: Option<String>,
    /// How long a wait may last before it answers with the job still running.
    pub timeout: Duration,
}

impl JobCall {
    /// Read a call from the arguments a `host_tool_call` frame carried.
    ///
    /// # Errors
    ///
    /// Returns the refusal text when the operation is unknown or the job
    /// it needs is unnamed.
    pub fn parse(arguments: &serde_json::Map<String, serde_json::Value>) -> Result<Self, String> {
        let op = match arguments.get("op").and_then(serde_json::Value::as_str) {
            Some("list") => JobOp::List,
            Some("logs") => JobOp::Logs,
            Some("wait") => JobOp::Wait,
            Some("stop") => JobOp::Stop,
            Some(other) => return Err(format!("job has no op {other}")),
            None => return Err("job needs an op".to_owned()),
        };
        let id = arguments
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        if op != JobOp::List && id.is_none() {
            return Err(format!("job {} needs an id", op.name()));
        }
        let timeout = arguments
            .get("timeout")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(SHELL_FOREGROUND)
            .clamp(SHELL_TIMEOUT_MIN, SHELL_TIMEOUT_MAX);
        Ok(Self {
            op,
            id,
            timeout: Duration::from_secs(timeout),
        })
    }
}

impl JobOp {
    /// The operation as the model spells it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Logs => "logs",
            Self::Wait => "wait",
            Self::Stop => "stop",
        }
    }
}

/// One shell command the agent asked the lane to run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellCall {
    /// The command line, run by the lane's shell.
    pub command: String,
    /// How long it may run before the lane ends it; `None` lifts the deadline.
    pub timeout: Option<Duration>,
    /// What omp notes about the call ahead of what the command says: today,
    /// a deadline granted other than the one asked.
    pub notices: Vec<String>,
}

impl ShellCall {
    /// Read a call from the arguments a `host_tool_call` frame carried.
    ///
    /// The deadline is read exactly as omp reads its own `bash` deadline, so
    /// a model's habit from one carries to the other: omitted means the
    /// default, zero lifts it, and any other value is clamped to the range
    /// with omp's own notice when that changes it.
    ///
    /// # Errors
    ///
    /// Returns the refusal text when the arguments name no command.
    pub fn parse(arguments: &serde_json::Map<String, serde_json::Value>) -> Result<Self, String> {
        let command = arguments
            .get("command")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "shell needs a command".to_owned())?;
        let asked = arguments.get("timeout").and_then(serde_json::Value::as_u64);
        let (timeout, notices) = match asked {
            None => (Some(SHELL_TIMEOUT_DEFAULT), Vec::new()),
            Some(0) => (None, Vec::new()),
            Some(asked) => {
                let granted = asked.clamp(SHELL_TIMEOUT_MIN, SHELL_TIMEOUT_MAX);
                let clamped = (granted != asked).then(|| {
                    format!(
                        "Timeout clamped to {granted}s (requested {asked}s; allowed range \
                         {SHELL_TIMEOUT_MIN}-{SHELL_TIMEOUT_MAX}s)."
                    )
                });
                (Some(granted), clamped.into_iter().collect())
            }
        };
        Ok(Self {
            command: command.to_owned(),
            timeout: timeout.map(Duration::from_secs),
            notices,
        })
    }
}

/// What omp tells the model when a background job ends, around the job's
/// own result: the notice its `bash` jobs are delivered in.
#[must_use]
pub fn job_delivery(job: &str, result: &str) -> String {
    format!(
        "<system-notice>\nBackground job {job} has completed. Resume your work using the \
         result below.\n{result}\n</system-notice>"
    )
}

/// The answer to one host tool call: what the command said and how it ended.
///
/// The exit code rides the result's details, where omp's own `bash` keeps
/// it, so the transcript reads the same whichever ran the command; a failed
/// command is a tool error to the model, exactly as it is under `bash`.
#[must_use]
pub fn host_tool_result(id: &str, output: &str, exit_code: Option<i32>, failed: bool) -> Vec<u8> {
    let mut result = serde_json::json!({
        "content": [{"type": "text", "text": output}],
        "details": {"exitCode": exit_code},
    });
    if failed {
        result["isError"] = serde_json::Value::Bool(true);
    }
    request_line(&serde_json::json!({
        "type": "host_tool_result",
        "id": id,
        "result": result,
    }))
}

/// The answer to a host tool call the lane could not run at all: a tool it
/// never registered, or arguments that name no command.
#[must_use]
pub fn host_tool_refused(id: &str, reason: &str) -> Vec<u8> {
    request_line(&serde_json::json!({
        "type": "host_tool_result",
        "id": id,
        "result": {"content": [{"type": "text", "text": reason}]},
        "isError": true,
    }))
}
