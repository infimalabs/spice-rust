use super::*;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

struct Transcript(PathBuf);

impl Transcript {
    fn new(records: &[Value]) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("fixture clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "spice-command-history-{}-{nonce}.jsonl",
            std::process::id()
        ));
        let text = records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&path, format!("{text}\n")).expect("native transcript");
        Self(path)
    }

    fn report(&self, limit: usize, newest_first: bool, failed: bool, summary: bool) -> Report {
        commands(
            &[ResolvedSource {
                path: self.0.clone(),
                vendor: Vendor::inferred(listen::Dialect::Claude),
                dialect: Some(listen::Dialect::Claude),
                thread: None,
            }],
            CommandOptions {
                query: CommandQuery::new(false, failed, false, false, summary, limit)
                    .expect("bounded query"),
                max_text: 100,
                newest_first,
            },
        )
        .expect("native command report")
    }
}

impl Drop for Transcript {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn record(second: u32, role: &str, content: Value) -> Value {
    json!({
        "type": role,
        "uuid": format!("00000000-0000-4000-8000-{second:012}"),
        "timestamp": format!("2026-09-19T02:13:{second:02}.000Z"),
        "message": {"type": "message", "role": role, "content": [content]}
    })
}

fn call(second: u32, id: &str, command: &str) -> Value {
    record(
        second,
        "assistant",
        json!({
            "type": "tool_use", "id": id, "name": "Bash", "input": {"command": command}
        }),
    )
}

fn result(second: u32, id: &str, output: &str, error: Option<bool>) -> Value {
    let mut block = json!({"type": "tool_result", "tool_use_id": id, "content": output});
    if let Some(error) = error {
        block["is_error"] = json!(error);
    }
    record(second, "user", block)
}

#[test]
fn explicit_completion_without_exit_code_is_not_missing_history() {
    let transcript = Transcript::new(&[
        call(1, "old", "old"),
        result(2, "old", "Process exited with code 7", Some(true)),
        call(3, "success", "success"),
        result(4, "success", "done", Some(false)),
        call(5, "failure", "failure"),
        result(6, "failure", "tool refused", Some(true)),
        call(7, "unreported", "unreported"),
        result(8, "unreported", "unreported", None),
        call(9, "running", "running"),
        result(
            10,
            "running",
            "Process running with session ID 42",
            Some(false),
        ),
        call(11, "pending", "pending"),
    ]);
    let Report::Commands { rows, .. } = transcript.report(5, true, false, false) else {
        panic!("command report");
    };
    assert_eq!(
        rows.iter()
            .map(|row| row.command.as_str())
            .collect::<Vec<_>>(),
        ["failure", "success", "old"]
    );
    assert_eq!(
        rows.iter().map(|row| row.exit_code).collect::<Vec<_>>(),
        [None, None, Some(7)]
    );
    assert!(rows.iter().all(|row| row.status == "completed"));
    let Report::Commands { rows, .. } = transcript.report(1, true, true, false) else {
        panic!("filtered command report");
    };
    assert_eq!(
        rows.iter()
            .map(|row| row.command.as_str())
            .collect::<Vec<_>>(),
        ["old"]
    );
    let Report::Commands {
        audit: Some(audit), ..
    } = transcript.report(1, false, false, true)
    else {
        panic!("command audit");
    };
    assert_eq!((audit.population, audit.matched, audit.failed), (3, 3, 1));
}

#[test]
fn newest_invocation_survives_an_older_invocation_finishing_last() {
    let transcript = Transcript::new(&[
        call(1, "old", "old"),
        call(2, "new", "new"),
        result(3, "new", "Process exited with code 0", Some(false)),
        result(4, "old", "Process exited with code 0", Some(false)),
    ]);
    let Report::Commands { rows, .. } = transcript.report(1, true, false, false) else {
        panic!("bounded command report");
    };
    assert_eq!(
        rows.iter()
            .map(|row| row.command.as_str())
            .collect::<Vec<_>>(),
        ["new"]
    );
    let Report::Commands { rows, .. } = transcript.report(2, false, false, false) else {
        panic!("chronological command report");
    };
    assert_eq!(
        rows.iter()
            .map(|row| row.command.as_str())
            .collect::<Vec<_>>(),
        ["old", "new"]
    );
}
