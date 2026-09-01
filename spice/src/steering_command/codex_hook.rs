use std::ffi::{OsStr, OsString};
use std::io::Read;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const PRE_TOOL_USE: &str = vendor::PRE_TOOL_HOOK_EVENT;
const POST_TOOL_USE: &str = vendor::POST_TOOL_HOOK_EVENT;
const BASH_TOOL: &str = "Bash";
const ALLOW: &str = "allow";

#[derive(Debug, Deserialize)]
struct HookInput {
    hook_event_name: String,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_input: Option<Value>,
}

/// Work selected from one vendor hook invocation.
pub(crate) enum Invocation {
    /// Post-tool steering delivery retains its existing resident round trip.
    Steering,
    /// A Codex shell command may be replaced through RTK.
    Rewrite(RewriteRequest),
    /// An event outside the two installed hook contracts needs no output.
    Ignore,
}

/// Canonical Codex Bash input admitted for RTK selection.
pub(crate) struct RewriteRequest {
    command: String,
}

/// Bytes and advisory stderr produced by one pre-tool invocation.
pub(crate) struct Reply {
    document: Vec<u8>,
    diagnostic: Option<String>,
}

impl Reply {
    pub(crate) fn document(&self) -> &[u8] {
        &self.document
    }

    pub(crate) fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }
}

#[derive(Serialize)]
struct HookOutput {
    #[serde(rename = "hookSpecificOutput")]
    hook_specific_output: HookSpecificOutput,
}

#[derive(Serialize)]
struct HookSpecificOutput {
    #[serde(rename = "hookEventName")]
    hook_event_name: &'static str,
    #[serde(rename = "permissionDecision", skip_serializing_if = "Option::is_none")]
    permission_decision: Option<&'static str>,
    #[serde(rename = "updatedInput", skip_serializing_if = "Option::is_none")]
    updated_input: Option<Value>,
}

struct Selection {
    command: String,
    diagnostic: Option<String>,
}

/// Read and classify the event Codex or Claude wrote to hook stdin.
pub(crate) fn read(reader: &mut impl Read, harness: &str) -> Result<Invocation, String> {
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| format!("hook input unavailable: {error}"))?;
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Invocation::Steering);
    }
    let input: HookInput = serde_json::from_slice(&bytes)
        .map_err(|error| format!("hook input is not valid JSON: {error}"))?;
    match input.hook_event_name.as_str() {
        POST_TOOL_USE => Ok(Invocation::Steering),
        PRE_TOOL_USE if harness != "codex" => Err(format!(
            "unsupported pre-tool rewrite harness {harness:?}; expected \"codex\""
        )),
        PRE_TOOL_USE => {
            let tool_name = input
                .tool_name
                .ok_or_else(|| "Codex pre-tool hook input is missing tool_name".to_owned())?;
            if tool_name != BASH_TOOL {
                return Ok(Invocation::Ignore);
            }
            let command = input
                .tool_input
                .and_then(|value| {
                    value
                        .get("command")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .ok_or_else(|| {
                    "Codex Bash pre-tool hook input is missing tool_input.command".to_owned()
                })?;
            Ok(Invocation::Rewrite(RewriteRequest { command }))
        }
        _ => Ok(Invocation::Ignore),
    }
}

impl RewriteRequest {
    /// Ask RTK once and encode a valid Codex pre-tool answer.
    pub(crate) fn answer(self, executable: Option<&OsStr>) -> Result<Reply, String> {
        self.answer_with(executable, |command, executable| {
            let native = [OsString::from("sh"), OsString::from("-lc"), command.into()];
            let decision = spice::rtk::rewrite(&native, executable);
            let selected = decision
                .command()
                .get(2)
                .and_then(|word| word.to_str())
                .unwrap_or(command)
                .to_owned();
            Selection {
                command: selected,
                diagnostic: decision.diagnostic().map(str::to_owned),
            }
        })
    }

    fn answer_with(
        self,
        executable: Option<&OsStr>,
        select: impl FnOnce(&str, &OsStr) -> Selection,
    ) -> Result<Reply, String> {
        let selection = executable.map_or_else(
            || Selection {
                command: self.command.clone(),
                diagnostic: None,
            },
            |executable| select(&self.command, executable),
        );
        let updated_input =
            (selection.command != self.command).then(|| json!({ "command": selection.command }));
        let permission_decision = updated_input.as_ref().map(|_| ALLOW);
        let mut document = wire::encode(&HookOutput {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: PRE_TOOL_USE,
                permission_decision,
                updated_input,
            },
        })
        .map_err(|error| error.to_string())?;
        document.push(b'\n');
        Ok(Reply {
            document,
            diagnostic: selection.diagnostic,
        })
    }
}

/// Prove that both producer branches satisfy the independent Codex consumer contract.
pub(crate) fn validate_rtk_hook_contract() -> Result<(), String> {
    const ORIGINAL: &str = "git status";
    const REWRITTEN: &str = "rtk git status";

    let changed = RewriteRequest {
        command: ORIGINAL.to_owned(),
    }
    .answer_with(Some(OsStr::new("rtk")), |_, _| Selection {
        command: REWRITTEN.to_owned(),
        diagnostic: None,
    })?;
    match vendor::codex::decode_rtk_pre_tool_output(changed.document())? {
        vendor::codex::RtkPreToolDecision::Rewritten(command) if command == REWRITTEN => {}
        decision => {
            return Err(format!(
                "Codex changed-command contract selected {decision:?}; expected {REWRITTEN:?}"
            ));
        }
    }

    let unchanged = RewriteRequest {
        command: ORIGINAL.to_owned(),
    }
    .answer(None)?;
    match vendor::codex::decode_rtk_pre_tool_output(unchanged.document())? {
        vendor::codex::RtkPreToolDecision::Unchanged => Ok(()),
        decision => Err(format!(
            "Codex unchanged-command contract selected {decision:?}; expected unchanged"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation(input: &str, harness: &str) -> Result<Invocation, String> {
        read(&mut input.as_bytes(), harness)
    }

    fn pre_tool(command: &str) -> String {
        json!({
            "hook_event_name": PRE_TOOL_USE,
            "tool_name": BASH_TOOL,
            "tool_input": { "command": command }
        })
        .to_string()
    }

    #[test]
    fn post_tool_use_remains_the_distinct_steering_path() {
        assert!(matches!(
            invocation(r#"{"hook_event_name":"PostToolUse"}"#, "codex"),
            Ok(Invocation::Steering)
        ));
    }

    #[test]
    fn codex_bash_rewrite_updates_only_the_canonical_command_input() {
        let Invocation::Rewrite(request) =
            invocation(&pre_tool("git status"), "codex").expect("valid pre-tool input")
        else {
            panic!("Codex Bash input selects rewriting");
        };
        let reply = request
            .answer_with(Some(OsStr::new("/opt/rtk")), |command, executable| {
                assert_eq!(command, "git status");
                assert_eq!(executable, OsStr::new("/opt/rtk"));
                Selection {
                    command: "rtk git status".to_owned(),
                    diagnostic: None,
                }
            })
            .expect("hook answer encodes");
        let document: Value = serde_json::from_slice(reply.document()).expect("answer is JSON");
        assert_eq!(
            document,
            json!({
                "hookSpecificOutput": {
                    "hookEventName": PRE_TOOL_USE,
                    "permissionDecision": "allow",
                    "updatedInput": { "command": "rtk git status" }
                }
            })
        );
        assert_eq!(
            vendor::codex::decode_rtk_pre_tool_output(reply.document()),
            Ok(vendor::codex::RtkPreToolDecision::Rewritten(
                "rtk git status".to_owned()
            ))
        );
        assert_eq!(reply.diagnostic(), None);
    }

    #[test]
    fn unchanged_selection_emits_no_updated_input_and_preserves_diagnostics() {
        let Invocation::Rewrite(request) =
            invocation(&pre_tool("git status"), "codex").expect("valid pre-tool input")
        else {
            panic!("Codex Bash input selects rewriting");
        };
        let reply = request
            .answer_with(Some(OsStr::new("rtk")), |command, _| Selection {
                command: command.to_owned(),
                diagnostic: Some("empty RTK stdout at exit 9".to_owned()),
            })
            .expect("hook answer encodes");
        let document: Value = serde_json::from_slice(reply.document()).expect("answer is JSON");
        assert_eq!(
            document,
            json!({ "hookSpecificOutput": { "hookEventName": PRE_TOOL_USE } })
        );
        assert_eq!(
            vendor::codex::decode_rtk_pre_tool_output(reply.document()),
            Ok(vendor::codex::RtkPreToolDecision::Unchanged)
        );
        assert_eq!(reply.diagnostic(), Some("empty RTK stdout at exit 9"));
    }

    #[test]
    fn rendered_branches_satisfy_the_codex_consumer_contract() {
        validate_rtk_hook_contract().expect("Codex accepts both RTK response branches");
    }

    #[test]
    fn pre_tool_rewrite_is_codex_only_and_non_bash_tools_are_ignored() {
        assert!(invocation(&pre_tool("git status"), "claude").is_err());
        let input = json!({
            "hook_event_name": PRE_TOOL_USE,
            "tool_name": "Read",
            "tool_input": { "command": "not a shell command" }
        })
        .to_string();
        assert!(matches!(
            invocation(&input, "codex"),
            Ok(Invocation::Ignore)
        ));
    }
}
