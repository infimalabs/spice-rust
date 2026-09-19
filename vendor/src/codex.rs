//! Codex's current exec launch contract.
//!
//! The launch selects Codex's JSONL stdout so the supervisor receives the
//! `thread.started` declaration and every subsequent record through its sole
//! selected-stream capture path.

use std::ffi::OsString;

use listen::Dialect;
use serde::Deserialize;
use sill::Running;

use crate::launch::{
    Launch, McpServer, POST_TOOL_HOOK_EVENT, POST_TOOL_HOOK_STATUS_MESSAGE,
    POST_TOOL_HOOK_TIMEOUT_SECONDS, PRE_TOOL_HOOK_EVENT, PRE_TOOL_HOOK_STATUS_MESSAGE,
    PRE_TOOL_HOOK_TIMEOUT_SECONDS, SteeringHook,
};
use crate::{ContractError, LaunchError, Process, Record, RecordStream};

/// Total matcher for every Codex tool completion.
///
/// Tool names are not a closed vocabulary. Enumerating them makes any newly
/// named tool a boundary after which steering never arrives, so every
/// completion is deliberately a steering checkpoint.
pub const POST_TOOL_HOOK_MATCHER: &str = "*";

/// Canonical Codex hook name for shell-like tool commands.
pub const PRE_TOOL_HOOK_MATCHER: &str = "^Bash$";

/// Consumer-side interpretation of Spice's Codex RTK pre-tool response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RtkPreToolDecision {
    /// Codex should execute the original tool input.
    Unchanged,
    /// Codex accepted the replacement shell command.
    Rewritten(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RtkHookOutput {
    #[serde(rename = "hookSpecificOutput")]
    hook_specific_output: RtkHookSpecificOutput,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RtkHookSpecificOutput {
    #[serde(rename = "hookEventName")]
    hook_event_name: String,
    #[serde(rename = "permissionDecision", default)]
    permission_decision: Option<PermissionDecision>,
    #[serde(rename = "updatedInput", default)]
    updated_input: Option<RtkUpdatedInput>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum PermissionDecision {
    Allow,
    Deny,
    Ask,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RtkUpdatedInput {
    command: String,
}

/// Decode the Codex `PreToolUse` subset emitted by Spice's RTK hook.
///
/// This is deliberately independent of the hook producer. It mirrors Codex's
/// current requirement that `updatedInput` is accepted only together with
/// `permissionDecision: "allow"`.
///
/// # Errors
///
/// Returns a contract error when the document is malformed, names another
/// hook event, or combines the permission decision and replacement input in a
/// form Codex does not accept for this route.
pub fn decode_rtk_pre_tool_output(bytes: &[u8]) -> Result<RtkPreToolDecision, String> {
    let output: RtkHookOutput = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid Codex pre-tool hook output: {error}"))?;
    let specific = output.hook_specific_output;
    if specific.hook_event_name != PRE_TOOL_HOOK_EVENT {
        return Err(format!(
            "Codex RTK hook returned event {:?}; expected {PRE_TOOL_HOOK_EVENT:?}",
            specific.hook_event_name
        ));
    }
    match (specific.permission_decision, specific.updated_input) {
        (None, None) => Ok(RtkPreToolDecision::Unchanged),
        (Some(PermissionDecision::Allow), Some(input)) => {
            Ok(RtkPreToolDecision::Rewritten(input.command))
        }
        (None, Some(_)) => Err(
            "Codex PreToolUse hook returned updatedInput without permissionDecision:allow"
                .to_owned(),
        ),
        (Some(PermissionDecision::Allow), None) => Err(
            "Codex PreToolUse hook returned permissionDecision:allow without updatedInput"
                .to_owned(),
        ),
        (Some(PermissionDecision::Deny | PermissionDecision::Ask), _) => {
            Err("Codex RTK hook returned a non-allow permission decision".to_owned())
        }
    }
}

/// Build the exact current Codex invocation without performing I/O.
///
/// # Errors
///
/// Returns a path-specific refusal when fast mode or the model context window
/// is not explicit, or the common argument-vector refusal when the binary is
/// empty.
pub fn record(binary: impl Into<OsString>, launch: &Launch) -> Result<Record, ContractError> {
    let fast_mode = launch.fast_mode.ok_or(ContractError::Missing {
        path: "vendor.codex.fast_mode",
    })?;
    if launch.model_context_window.is_none() {
        return Err(ContractError::Missing {
            path: "vendor.codex.model_context_window",
        });
    }
    if launch.auto_compact_window_tokens.is_none() {
        return Err(ContractError::Missing {
            path: "vendor.codex.auto_compact_window_tokens",
        });
    }
    if launch.config_overlay.is_some() {
        return Err(ContractError::Unsupported {
            path: "vendor.codex.config_overlay",
            vendor: "Codex",
        });
    }
    if launch.lsp.is_some() {
        return Err(ContractError::Unsupported {
            path: "vendor.codex.lsp",
            vendor: "Codex",
        });
    }
    let mut arguments = vec![
        OsString::from("exec"),
        OsString::from("--cd"),
        launch.cwd.clone().into_os_string(),
        OsString::from("--model"),
        OsString::from(launch.model.as_str()),
        OsString::from("--json"),
    ];
    for override_ in config_overrides(launch) {
        arguments.push(OsString::from("--config"));
        arguments.push(OsString::from(override_));
    }
    arguments.extend([
        OsString::from(if fast_mode { "--enable" } else { "--disable" }),
        OsString::from("fast_mode"),
        // A supervised lane owns its own boundary: the sandbox would refuse the
        // worktree writes the work consists of, and an approval prompt has no
        // operator in front of it to answer. Trust for the steering hook is
        // stated here for the same reason -- the hook is spice's own.
        OsString::from("--sandbox"),
        OsString::from("danger-full-access"),
        OsString::from("--dangerously-bypass-approvals-and-sandbox"),
        OsString::from("--dangerously-bypass-hook-trust"),
    ]);
    // `resume` is a subcommand of `exec`, so it follows every flag the exec
    // itself takes and carries the thread ahead of the prompt.
    if let Some(session) = launch.resume.session_id() {
        arguments.push(OsString::from("resume"));
        arguments.push(OsString::from(session));
    }
    arguments.push(OsString::from(launch.prompt.as_str()));

    Ok(Record::new(
        binary,
        arguments,
        Dialect::Codex,
        RecordStream::Stdout,
        launch.resume.clone(),
    )?)
}

/// Every dotted override this launch states, in the order Codex reads them.
///
/// Codex configures from TOML fragments rather than Claude's JSON documents, so
/// the steering hook, the reasoning effort, the MCP server, and any personality
/// all arrive as `--config` assignments.
#[must_use]
pub fn config_overrides(launch: &Launch) -> Vec<String> {
    let mut overrides = vec![
        pre_tool_hook_override(&launch.hook),
        post_tool_hook_override(&launch.hook),
        format!("model_reasoning_effort=\"{}\"", launch.effort.as_str()),
    ];
    if let Some(context_window) = launch.model_context_window {
        overrides.push(format!("model_context_window={context_window}"));
    }
    if let Some(window) = launch.auto_compact_window_tokens {
        overrides.push(format!(
            "model_auto_compact_token_limit={}",
            crate::codex_auto_compact_token_limit(window)
        ));
    }
    if let Some(server) = &launch.mcp {
        overrides.extend(mcp_overrides(server));
    }
    if let Some(personality) = &launch.personality {
        overrides.push(format!("personality={}", toml_string(personality)));
    }
    overrides
}

/// The dotted override installing dynamic RTK dispatch before shell execution.
#[must_use]
pub fn pre_tool_hook_override(hook: &SteeringHook) -> String {
    format!(
        "hooks.{PRE_TOOL_HOOK_EVENT}=[{{matcher={matcher},hooks=[{{type={kind},command={command},\
         timeout={PRE_TOOL_HOOK_TIMEOUT_SECONDS},statusMessage={status}}}]}}]",
        matcher = toml_string(PRE_TOOL_HOOK_MATCHER),
        kind = toml_string("command"),
        command = toml_string(&hook.command),
        status = toml_string(PRE_TOOL_HOOK_STATUS_MESSAGE),
    )
}

/// The dotted override installing the steering hook.
///
/// This is Codex's spelling of the same hook Claude takes inside `--settings`:
/// one `PostToolUse` group, one command, the same timeout and status message.
#[must_use]
pub fn post_tool_hook_override(hook: &SteeringHook) -> String {
    format!(
        "hooks.{POST_TOOL_HOOK_EVENT}=[{{matcher={matcher},hooks=[{{type={kind},command={command},\
         timeout={POST_TOOL_HOOK_TIMEOUT_SECONDS},statusMessage={status}}}]}}]",
        matcher = toml_string(POST_TOOL_HOOK_MATCHER),
        kind = toml_string("command"),
        command = toml_string(&hook.command),
        status = toml_string(POST_TOOL_HOOK_STATUS_MESSAGE),
    )
}

/// The dotted overrides registering one MCP server.
#[must_use]
pub fn mcp_overrides(server: &McpServer) -> Vec<String> {
    let args = server.args.iter().map(String::as_str).collect::<Vec<_>>();
    vec![
        format!(
            "mcp_servers.{}.command={}",
            server.name,
            toml_string(&server.command)
        ),
        format!(
            "mcp_servers.{}.args=[{}]",
            server.name,
            args.iter()
                .map(|argument| toml_string(argument))
                .collect::<Vec<_>>()
                .join(",")
        ),
    ]
}

/// Quote one value as a TOML basic string.
///
/// TOML's basic string is JSON's string, so a JSON encoding of the value is
/// already correctly escaped for the position it lands in.
fn toml_string(value: &str) -> String {
    serde_json::to_string(value).expect("a string is representable as JSON")
}

/// Launch Codex through the sole `sill` process door.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtk_decoder_requires_allow_for_a_replacement_input() {
        let previous = br#"{
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "updatedInput": { "command": "rtk git status" }
            }
        }"#;

        let error = decode_rtk_pre_tool_output(previous)
            .expect_err("updatedInput without an allow decision is rejected");
        assert!(
            error.contains("updatedInput without permissionDecision:allow"),
            "unexpected contract error: {error}"
        );
    }

    #[test]
    fn rtk_decoder_accepts_the_allow_and_replacement_pair() {
        let accepted = br#"{
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "updatedInput": { "command": "rtk git status" }
            }
        }"#;

        assert_eq!(
            decode_rtk_pre_tool_output(accepted),
            Ok(RtkPreToolDecision::Rewritten("rtk git status".to_owned()))
        );
    }
}
