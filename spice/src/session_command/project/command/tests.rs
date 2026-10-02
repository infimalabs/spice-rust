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

    /// One newest-first page of `session commands` over this transcript.
    fn report(&self, limit: usize, failed: bool, summary: bool) -> Report {
        let path = self.0.to_string_lossy().into_owned();
        let limit = limit.to_string();
        let mut argv = vec!["spice", "session", "commands", &path, "--limit", &limit];
        argv.extend(failed.then_some("--failed"));
        argv.extend(summary.then_some("--summary"));
        let matches = crate::cli::command()
            .try_get_matches_from(argv)
            .expect("commands syntax parses");
        let leaf = crate::cli::selected_matches(&matches);
        let page = Page::from_matches(leaf, &["session", "commands"]).expect("bounded page");
        commands(
            &[ResolvedSource {
                path: self.0.clone(),
                vendor: Vendor::inferred(listen::Dialect::Omp),
                dialect: Some(listen::Dialect::Omp),
                thread: None,
            }],
            leaf,
            page,
        )
        .expect("native command report")
    }

    fn commands(&self, limit: usize, failed: bool) -> Vec<String> {
        let Report::Commands { page, .. } = self.report(limit, failed, false) else {
            panic!("command report");
        };
        page.rows.into_iter().map(|row| row.command.text).collect()
    }
}

impl Drop for Transcript {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn record(second: u32, message: Value) -> Value {
    json!({
        "type": "message",
        "id": format!("entry-{second}"),
        "timestamp": format!("2026-09-19T02:13:{second:02}.000Z"),
        "message": message,
    })
}

fn call(second: u32, id: &str, command: &str) -> Value {
    record(
        second,
        json!({
            "role": "assistant",
            "content": [{
                "type": "toolCall", "id": id, "name": "bash", "arguments": {"command": command}
            }],
            "stopReason": "toolUse",
        }),
    )
}

fn result(second: u32, id: &str, output: &str, error: Option<bool>) -> Value {
    let mut message = json!({
        "role": "toolResult",
        "toolCallId": id,
        "toolName": "bash",
        "content": [{"type": "text", "text": output}],
    });
    if let Some(error) = error {
        message["isError"] = json!(error);
    }
    record(second, message)
}

/// A bash result that states its exit code structurally, as omp does.
fn exited(second: u32, id: &str, code: i32) -> Value {
    let mut value = result(second, id, "failed", Some(true));
    value["message"]["details"] = json!({"exitCode": code});
    value
}

#[test]
fn explicit_completion_without_exit_code_is_not_missing_history() {
    let transcript = Transcript::new(&[
        call(1, "old", "old"),
        exited(2, "old", 7),
        call(3, "success", "success"),
        result(4, "success", "done", Some(false)),
        call(5, "failure", "failure"),
        result(6, "failure", "tool refused", Some(true)),
        call(7, "unreported", "unreported"),
        result(8, "unreported", "unreported", None),
        call(9, "running", "running"),
        result(10, "running", "Command still running", Some(false)),
        call(11, "pending", "pending"),
    ]);
    let Report::Commands { page, .. } = transcript.report(5, false, false) else {
        panic!("command report");
    };
    assert_eq!(
        page.rows
            .iter()
            .map(|row| row.command.text.as_str())
            .collect::<Vec<_>>(),
        ["failure", "success", "old"]
    );
    assert_eq!(
        page.rows
            .iter()
            .map(|row| row.exit_code)
            .collect::<Vec<_>>(),
        [None, None, Some(7)]
    );
    assert!(page.rows.iter().all(|row| row.status == "completed"));
    assert_eq!(transcript.commands(1, true), ["old"]);
    let Report::Commands {
        audit: Some(audit), ..
    } = transcript.report(1, false, true)
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
    assert_eq!(transcript.commands(1, false), ["new"]);
    assert_eq!(transcript.commands(2, false), ["new", "old"]);
}
