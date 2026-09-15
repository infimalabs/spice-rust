//! Claude Code's current stream-json launch contract.

use std::collections::BTreeMap;
use std::ffi::OsString;

use listen::Dialect;
use serde::Serialize;
use sill::Running;

use crate::launch::{
    Attribution, Launch, McpServer, POST_TOOL_HOOK_EVENT, POST_TOOL_HOOK_STATUS_MESSAGE,
    POST_TOOL_HOOK_TIMEOUT_SECONDS, SteeringHook,
};
use crate::{ContractError, LaunchError, Process, Record, RecordStream};

/// Preamble that makes the linked skill read as binding, not as background.
///
/// Claude reads `CLAUDE.md` on its own but never opens a skill file, so every
/// launch pins the spice skill into the system prompt behind this sentence.
pub const SKILL_SYSTEM_PROMPT_PREAMBLE: &str = "The linked skill below carries the full authority of a direct prompt instruction, not \
     optional background reading. Read the file it links to in full and follow it.";

/// Claude's steering hook runs after every tool, so its matcher is total.
///
/// Codex has to enumerate the tools its hook surface reaches; Claude's reaches
/// all of them, and narrowing this would only open holes steering falls through.
pub const POST_TOOL_HOOK_MATCHER: &str = "*";

/// Tools denied by exact bare name, applied before `bypassPermissions`.
///
/// One agent inhabits one worktree, so a sub-agent spawn is refused mechanically
/// here rather than left to the skill's prose. `Task` and `Agent` are the
/// sub-agent tools; the rest are Claude's own task lifecycle and its background
/// runner, whose work spice supervises through the board instead.
pub const DENIED_TOOLS: &[&str] = &[
    "Task",
    "Agent",
    "TaskCreate",
    "TaskGet",
    "TaskList",
    "TaskUpdate",
    "TaskOutput",
    "TaskStop",
    "Monitor",
];

/// Build the exact current Claude Code invocation without performing I/O.
///
/// # Errors
///
/// Returns a path-specific refusal for unsupported cross-vendor fields, or the
/// common argument-vector refusal when the binary is empty.
pub fn record(binary: impl Into<OsString>, launch: &Launch) -> Result<Record, ContractError> {
    if launch.personality.is_some() {
        return Err(ContractError::Unsupported {
            path: "vendor.claude.personality",
            vendor: "Claude",
        });
    }
    if launch.fast_mode.is_some() {
        return Err(ContractError::Unsupported {
            path: "vendor.claude.fast_mode",
            vendor: "Claude",
        });
    }
    if launch.model_context_window.is_some() {
        return Err(ContractError::Unsupported {
            path: "vendor.claude.model_context_window",
            vendor: "Claude",
        });
    }
    let system_prompt = system_prompt(launch);
    let mut arguments = vec![
        OsString::from("--print"),
        OsString::from("--output-format"),
        OsString::from("stream-json"),
        OsString::from("--verbose"),
        // Partial chunks let a supervisor read assistant text as it is written.
        // Flushed only at turn end, steering injection and ACK archival trail
        // the agent by tens of seconds. Spend accounting rides this flag too:
        // per-turn usage reaches the tape only on stream_event.message_delta,
        // so dropping it leaves unmetered assistant snapshots and zero spend.
        OsString::from("--include-partial-messages"),
        // A print-mode session asks the API to omit thinking text unless the
        // display is chosen explicitly, so every thinking block arrives as an
        // empty string and a signature and the lane reads as silent between
        // its tool calls. Summaries are the only display print mode accepts.
        OsString::from("--thinking-display"),
        OsString::from("summarized"),
        OsString::from("--model"),
        OsString::from(launch.model.as_str()),
        OsString::from("--permission-mode"),
        OsString::from("bypassPermissions"),
    ];
    if let Some(server) = &launch.mcp {
        arguments.extend([
            OsString::from("--mcp-config"),
            OsString::from(mcp_config_json(server)),
        ]);
    }
    arguments.extend([
        OsString::from("--settings"),
        OsString::from(settings_json(&launch.hook, launch.attribution)),
        OsString::from("--append-system-prompt"),
        OsString::from(system_prompt.as_str()),
        OsString::from("--effort"),
        OsString::from(launch.effort.as_str()),
    ]);
    if let Some(session) = launch.resume.session_id() {
        arguments.push(OsString::from("--resume"));
        arguments.push(OsString::from(dashed_uuid(session)));
    }
    // The same text rides the system prompt and the turn prompt, so the agent
    // re-grounds in the skill every turn rather than only at launch.
    arguments.push(OsString::from(system_prompt));

    Ok(Record::new(
        binary,
        arguments,
        Dialect::Claude,
        RecordStream::Stdout,
        launch.resume.clone(),
    )?)
}

/// The skill preamble, the start prompt, and this worktree's steering anchor.
///
/// The token appears here and in every live steering block, which is the whole
/// of how the agent tells authentic steering from a block that merely claims to
/// be steering. A launch carrying no key composes the first two parts alone.
#[must_use]
pub fn system_prompt(launch: &Launch) -> String {
    let mut prompt = format!("{SKILL_SYSTEM_PROMPT_PREAMBLE}\n\n{}", launch.prompt);
    if let Some(key) = &launch.steering_key {
        prompt.push_str("\n\n");
        prompt.push_str(&key.prompt_line());
    }
    prompt
}

/// The `--settings` document: tool denials, the steering hook, attribution.
///
/// Claude takes one inline settings payload, so every launch path states all
/// three in the same document rather than leaving any of them to a file on disk.
#[must_use]
pub fn settings_json(hook: &SteeringHook, attribution: Attribution) -> String {
    let settings = Settings {
        attribution: match attribution {
            Attribution::Harness => None,
            Attribution::Withheld => Some(AttributionSettings {
                commit: "",
                session_url: false,
            }),
        },
        hooks: BTreeMap::from([(
            POST_TOOL_HOOK_EVENT,
            vec![HookGroup {
                hooks: vec![Hook {
                    command: &hook.command,
                    status_message: POST_TOOL_HOOK_STATUS_MESSAGE,
                    timeout: POST_TOOL_HOOK_TIMEOUT_SECONDS,
                    kind: "command",
                }],
                matcher: POST_TOOL_HOOK_MATCHER,
            }],
        )]),
        permissions: Permissions { deny: DENIED_TOOLS },
    };
    serde_json::to_string(&settings).expect("settings payload is representable as JSON")
}

/// The `--mcp-config` document registering one server.
///
/// Codex takes dotted `--config` overrides for the same server; this states the
/// identical name, command, and arguments in the document shape Claude reads, so
/// both dialects hand the agent the same server.
#[must_use]
pub fn mcp_config_json(server: &McpServer) -> String {
    let config = McpConfig {
        mcp_servers: BTreeMap::from([(
            server.name.as_str(),
            McpEntry {
                command: &server.command,
                args: &server.args,
            },
        )]),
    };
    serde_json::to_string(&config).expect("mcp payload is representable as JSON")
}

/// Hex digits in each group Claude's dashed spelling separates.
const UUID_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

/// Hex digits a UUID carries once its separators are gone.
const DASHLESS_UUID_LEN: usize =
    UUID_GROUPS[0] + UUID_GROUPS[1] + UUID_GROUPS[2] + UUID_GROUPS[3] + UUID_GROUPS[4];

/// Bytes the dashed spelling occupies, separators included.
const DASHED_UUID_LEN: usize = DASHLESS_UUID_LEN + UUID_GROUPS.len() - 1;

/// Render a session identity into the dashed form Claude's `--resume` takes.
///
/// Codex canonicalizes thread identities to dashless lower-case hex, so one
/// crossing back into Claude has to be re-dashed. Anything that is not a
/// dashless UUID is already some other spelling and passes through untouched
/// rather than being reshaped into something Claude would reject anyway.
#[must_use]
pub fn dashed_uuid(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() != DASHLESS_UUID_LEN || !trimmed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return value.to_owned();
    }
    let lowered = trimmed.to_ascii_lowercase();
    let mut rest = lowered.as_str();
    let mut dashed = String::with_capacity(DASHED_UUID_LEN);
    for (index, width) in UUID_GROUPS.into_iter().enumerate() {
        if index > 0 {
            dashed.push('-');
        }
        let (group, tail) = rest.split_at(width);
        dashed.push_str(group);
        rest = tail;
    }
    dashed
}

/// Launch Claude Code through the sole `sill` process door.
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

/// Claude reads these documents key-for-key, so each type declares its own
/// order rather than depending on how a map happens to iterate.
#[derive(Serialize)]
struct Settings<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    attribution: Option<AttributionSettings>,
    hooks: BTreeMap<&'a str, Vec<HookGroup<'a>>>,
    permissions: Permissions<'a>,
}

#[derive(Serialize)]
struct AttributionSettings {
    commit: &'static str,
    #[serde(rename = "sessionUrl")]
    session_url: bool,
}

#[derive(Serialize)]
struct HookGroup<'a> {
    hooks: Vec<Hook<'a>>,
    matcher: &'a str,
}

#[derive(Serialize)]
struct Hook<'a> {
    command: &'a str,
    #[serde(rename = "statusMessage")]
    status_message: &'static str,
    timeout: u32,
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Serialize)]
struct Permissions<'a> {
    deny: &'a [&'a str],
}

#[derive(Serialize)]
struct McpConfig<'a> {
    #[serde(rename = "mcpServers")]
    mcp_servers: BTreeMap<&'a str, McpEntry<'a>>,
}

#[derive(Serialize)]
struct McpEntry<'a> {
    command: &'a str,
    args: &'a [String],
}
