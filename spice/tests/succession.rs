#![cfg(unix)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
struct Case {
    path: &'static str,
    arguments: &'static [&'static str],
    output: OutputKind,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum OutputKind {
    Document,
    ChildStream,
}

const UUID: &str = "00000000-0000-4000-8000-000000000001";
const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

const CASES: &[Case] = &[
    case("watch", &["watch", "missing"]),
    case("retro", &["retro", "missing"]),
    case("doctor", &["doctor"]),
    case("version", &["version"]),
    case("install", &["install"]),
    case("lock", &["lock"]),
    case(
        "lock run",
        &["lock", "run", "readiness", "--", "/bin/echo", "locked"],
    ),
    case("restore verify", &["restore", "verify", "missing"]),
    case("gate", &["gate"]),
    case("gate camp", &["gate", "camp"]),
    case("gate kill", &["gate", "kill"]),
    case("gate receipt", &["gate", "receipt"]),
    case("proof", &["proof"]),
    case("proof replay", &["proof", "replay"]),
    case("release notes", &["release", "notes", "missing"]),
    case("up", &["up"]),
    case("down", &["down"]),
    case("board", &["board"]),
    case("board graph", &["board", "graph", "all"]),
    case(
        "board handout",
        &["board", "handout", "readiness-handout.json"],
    ),
    case("task next", &["task", "next"]),
    case("task add", &["task", "add", "readiness task"]),
    case("task capture", &["task", "capture"]),
    case("task done", &["task", "done"]),
    case("task review", &["task", "review", "READY-12345678"]),
    case(
        "task note",
        &["task", "note", "READY-12345678", "readiness note"],
    ),
    case(
        "task defer",
        &["task", "defer", "READY-12345678", "2026-08-02T00:00:00Z"],
    ),
    case("task wake", &["task", "wake", "READY-12345678"]),
    case("task free", &["task", "free", "READY-12345678"]),
    case("task oops", &["task", "oops", "readiness discovery"]),
    case(
        "task depends",
        &["task", "depends", "READY-12345678", "AFTER-12345678"],
    ),
    case("doc", &["doc", "missing"]),
    case("doc ledger", &["doc", "ledger", "READY-12345678"]),
    case("lane", &["lane"]),
    case("lane add", &["lane", "add"]),
    case("lane drain", &["lane", "drain", UUID]),
    case("lane wake", &["lane", "wake", UUID]),
    case("steer", &["steer", UUID, "readiness direction"]),
    case("steer cancel", &["steer", "cancel", UUID]),
    case("inbox", &["inbox"]),
    case("recall", &["recall"]),
    case("consent", &["consent"]),
    case("consent allow", &["consent", "allow", UUID]),
    case("consent deny", &["consent", "deny", UUID]),
    case("consent policy", &["consent", "policy"]),
    case("taste", &["taste"]),
    case("taste add", &["taste", "add", "readiness taste"]),
    case("taste drop", &["taste", "drop", "readiness"]),
    case("trust", &["trust"]),
    case("trust grant", &["trust", "grant", DIGEST]),
    case("trust revoke", &["trust", "revoke", DIGEST]),
    case("backup", &["backup", "readiness-backup"]),
    case("restore", &["restore", "missing"]),
    case("journal", &["journal"]),
    case("journal replay", &["journal", "replay"]),
    case("journal reindex", &["journal", "reindex"]),
    case("journal verify", &["journal", "verify"]),
    case("fold", &["fold"]),
    case("thread", &["thread", "1"]),
    case("canvas", &["canvas"]),
    case("release publish", &["release", "publish"]),
    Case {
        path: "run",
        arguments: &["run", "--", "/bin/echo", "run"],
        output: OutputKind::ChildStream,
    },
    case("ack", &["ack", "ready"]),
    case("config", &["config"]),
];

const fn case(path: &'static str, arguments: &'static [&'static str]) -> Case {
    Case {
        path,
        arguments,
        output: OutputKind::Document,
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "spice-succession-process-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).expect("create isolated succession fixture");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove isolated succession fixture");
    }
}

#[test]
fn every_registered_leaf_reaches_process_dispatch_in_text_and_json() {
    let registered = landed_ledger_paths();
    let exercised = CASES.iter().map(|case| case.path).collect::<BTreeSet<_>>();
    assert_eq!(registered.len(), 64);
    assert_eq!(CASES.len(), 64);
    assert_eq!(exercised.len(), CASES.len(), "process case repeated");
    assert_eq!(
        exercised, registered,
        "process fixture and registry drifted"
    );

    let scratch = Scratch::new();
    for case in CASES {
        let text = invoke(scratch.path(), case, false);
        let json = invoke(scratch.path(), case, true);
        assert_eq!(text.status.code(), json.status.code(), "{}", case.path);
        assert_exit(case, &text, "text");
        assert_exit(case, &json, "JSON");
        assert_one_stream(&text, case.path);
        assert_one_stream(&json, case.path);
        assert_no_placeholder(&text, case.path);
        assert_no_placeholder(&json, case.path);

        if case.output == OutputKind::Document {
            let document = if json.status.success() {
                &json.stdout
            } else {
                &json.stderr
            };
            serde_json::from_slice::<serde_json::Value>(document).unwrap_or_else(|error| {
                panic!("{} JSON outcome did not decode: {error}", case.path)
            });
        }
    }
}

fn assert_exit(case: &Case, output: &Output, form: &str) {
    match case.output {
        OutputKind::Document => assert!(
            matches!(output.status.code(), Some(0..=2)),
            "{} {form} used undeclared exit {:?}",
            case.path,
            output.status.code(),
        ),
        OutputKind::ChildStream => assert_eq!(
            output.status.code(),
            Some(0),
            "{} {form} did not preserve the selected child exit",
            case.path,
        ),
    }
}

fn landed_ledger_paths() -> BTreeSet<&'static str> {
    include_str!("../../PORTING.md")
        .split_once("### Landed")
        .expect("the ledger names landed Rust leaves")
        .1
        .split_once("## Installed Python paths")
        .expect("the landed inventory has a finite boundary")
        .0
        .lines()
        .filter_map(|line| line.strip_prefix("- `"))
        .filter_map(|line| line.split_once('`').map(|(path, _)| path))
        .collect()
}

fn invoke(root: &Path, case: &Case, json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
    if json {
        command.arg("--json");
    }
    command
        .args(case.arguments)
        .current_dir(root)
        .env_remove(steer::hook::INBOX)
        .output()
        .unwrap_or_else(|error| panic!("{} process did not launch: {error}", case.path))
}

fn assert_one_stream(output: &Output, path: &str) {
    let (spoken, silent) = if output.status.success() {
        (&output.stdout, &output.stderr)
    } else {
        (&output.stderr, &output.stdout)
    };
    assert!(!spoken.is_empty(), "{path} emitted no process outcome");
    assert!(silent.is_empty(), "{path} mixed stdout and stderr");
}

fn assert_no_placeholder(output: &Output, path: &str) {
    for placeholder in [
        "release::not-landed",
        "release::dispatch-gap",
        "release::succession-unaccounted",
        "command::handler-not-landed",
    ] {
        assert!(
            !output
                .stdout
                .windows(placeholder.len())
                .any(|bytes| bytes == placeholder.as_bytes())
                && !output
                    .stderr
                    .windows(placeholder.len())
                    .any(|bytes| bytes == placeholder.as_bytes()),
            "{path} reached placeholder {placeholder}",
        );
    }
}
