//! Explicit live proof for Codex's automatic-compaction continuation contract.
//!
//! This is ignored because it makes three authenticated model calls and writes
//! a real Codex transcript. Run it only when live model use is intended:
//! `spice dev cargo -- test --locked -p vendor --test codex_compaction_live -- --ignored --nocapture`.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use vendor::codex_auto_compact_token_limit;

const MODEL: &str = "gpt-5.6-sol";
const CONTEXT_WINDOW: u64 = 32_768;
const AUTO_COMPACT_LIMIT: u64 = 31_129;

#[test]
#[ignore = "slow:vendor:codex-live-compaction"]
fn headless_codex_compacts_then_resumes_without_an_overflow_loop() {
    assert_eq!(
        codex_auto_compact_token_limit(CONTEXT_WINDOW),
        AUTO_COMPACT_LIMIT
    );
    let hook_capture = std::env::temp_dir().join(format!(
        "spice-codex-compaction-hook-{}.json",
        std::process::id()
    ));

    let first = invoke(None, &sized_prompt("FIRST_OK", 1_200), &hook_capture);
    let thread = thread_id(&first);
    assert_eq!(last_agent_message(&first).as_deref(), Some("FIRST_OK"));
    let transcript = captured_transcript(&hook_capture);
    assert!(
        transcript
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains(&thread)),
        "the hook and JSON event stream must identify the same thread"
    );
    assert_eq!(
        compaction_count(&transcript),
        0,
        "first turn stays below the boundary"
    );

    let second = invoke(
        Some(&thread),
        &sized_prompt("SECOND_OK", 2_500),
        &hook_capture,
    );
    assert_eq!(last_agent_message(&second).as_deref(), Some("SECOND_OK"));
    assert!(
        compaction_count(&transcript) >= 1,
        "the accumulated second turn must cross the automatic-compaction boundary"
    );

    let third = invoke(
        Some(&thread),
        "Reply exactly CONTINUED_OK. Do not use tools.",
        &hook_capture,
    );
    assert_eq!(last_agent_message(&third).as_deref(), Some("CONTINUED_OK"));
    fs::remove_file(hook_capture).expect("live proof removes its temporary hook capture");
}

fn invoke(thread: Option<&str>, prompt: &str, hook_capture: &Path) -> Vec<Value> {
    let hook_command = format!("sh -c 'cat > {}'", hook_capture.display());
    let hook_override = format!(
        "hooks.PostToolUse=[{{matcher=\"*\",hooks=[{{type=\"command\",command={}}}]}}]",
        serde_json::to_string(&hook_command).expect("hook command is a TOML string")
    );
    let mut command = Command::new("codex");
    command.args([
        "exec",
        "--strict-config",
        "--ignore-user-config",
        "--skip-git-repo-check",
        "--model",
        MODEL,
        "--json",
        "--config",
        &format!("model_context_window={CONTEXT_WINDOW}"),
        "--config",
        &format!("model_auto_compact_token_limit={AUTO_COMPACT_LIMIT}"),
        "--config",
        &hook_override,
        "--dangerously-bypass-approvals-and-sandbox",
        "--dangerously-bypass-hook-trust",
    ]);
    if let Some(thread) = thread {
        command.args(["resume", thread]);
    }
    let output = command.arg(prompt).output().expect("Codex CLI launches");
    assert_success(&output);
    output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("Codex --json emits JSONL"))
        .collect()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "Codex exited {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn thread_id(records: &[Value]) -> String {
    records
        .iter()
        .find_map(|record| {
            (record["type"] == "thread.started")
                .then(|| record["thread_id"].as_str())
                .flatten()
        })
        .expect("fresh Codex output declares its thread")
        .to_owned()
}

fn last_agent_message(records: &[Value]) -> Option<String> {
    records.iter().rev().find_map(|record| {
        (record["type"] == "item.completed" && record["item"]["type"] == "agent_message")
            .then(|| {
                record["item"]["text"]
                    .as_str()
                    .map(str::trim)
                    .map(str::to_owned)
            })
            .flatten()
    })
}

fn sized_prompt(marker: &str, rows: usize) -> String {
    let mut prompt = format!(
        "Use the shell tool to run `pwd` once. Remember the following distinct labels for this turn. Then reply exactly {marker}.\n"
    );
    for index in 0..rows {
        writeln!(prompt, "boundary-label-{marker}-{index:05}").expect("string write succeeds");
    }
    prompt
}

fn captured_transcript(hook_capture: &Path) -> PathBuf {
    let hook: Value = serde_json::from_slice(
        &fs::read(hook_capture).expect("PostToolUse hook captured its input"),
    )
    .expect("PostToolUse hook input is JSON");
    hook["transcript_path"]
        .as_str()
        .map(PathBuf::from)
        .expect("PostToolUse hook input carries transcript_path")
}

fn compaction_count(path: &Path) -> usize {
    fs::read_to_string(path)
        .expect("persisted Codex rollout reads")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("rollout remains JSONL"))
        .filter(|record| record["type"] == "compacted")
        .count()
}
