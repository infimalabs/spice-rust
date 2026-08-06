#![cfg(unix)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use spice::placeholder::PlaceholderReason;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
struct Case {
    path: &'static str,
    arguments: &'static [&'static str],
    output: OutputKind,
    expected: ExpectedOutcome,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum OutputKind {
    Document,
    ChildStream,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ExpectedOutcome {
    Generic,
    TaskMigration,
}

const UUID: &str = "00000000-0000-4000-8000-000000000001";
const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

const CASES: &[Case] = &[
    case("demo", &[]),
    case("watch", &["watch", "missing"]),
    case("retro", &["retro", "missing"]),
    case("session", &["session"]),
    case("session briefing", &["session", "briefing"]),
    case("session commands", &["session", "commands"]),
    case("session commits", &["session", "commits"]),
    case("session compactions", &["session", "compactions"]),
    case("session messages", &["session", "messages"]),
    case("session phases", &["session", "phases"]),
    case("session slices", &["session", "slices"]),
    case("session summary", &["session", "summary"]),
    case("session sweep", &["session", "sweep"]),
    case(
        "session thread",
        &["session", "thread", "00000000-0000-4000-8000-000000000002"],
    ),
    case("session timeline", &["session", "timeline"]),
    case("session tokens", &["session", "tokens"]),
    case("session turns", &["session", "turns"]),
    case("doctor", &["doctor"]),
    case("version", &["version"]),
    case("install", &["install"]),
    case("lock", &["lock"]),
    case(
        "lock run",
        &["lock", "run", "readiness", "--", "/bin/echo", "locked"],
    ),
    case("restore verify", &["restore", "verify", "missing"]),
    case("dev pre-commit", &["dev", "pre-commit"]),
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
    case("task status", &["task", "status"]),
    case("task next", &["task", "next"]),
    case("task reclaim", &["task", "reclaim"]),
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
    case(
        "task artifact add",
        &["task", "artifact", "add", "READY-12345678", "missing"],
    ),
    case(
        "task artifact list",
        &["task", "artifact", "list", "READY-12345678"],
    ),
    case("task artifact prune", &["task", "artifact", "prune"]),
    case(
        "task artifact show",
        &["task", "artifact", "show", "READY-12345678", "A1"],
    ),
    case("doc", &["doc", "missing"]),
    case("doc ledger", &["doc", "ledger", "READY-12345678"]),
    case("lane", &["lane"]),
    case("lane add", &["lane", "add"]),
    case("lane drain", &["lane", "drain", UUID]),
    case("lane wake", &["lane", "wake", UUID]),
    case("lane import", &["lane", "import", UUID, "."]),
    case("steer", &["steer", UUID, "readiness direction"]),
    case("steer cancel", &["steer", "cancel", UUID]),
    case("steer recover", &["steer", "recover", UUID]),
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
        expected: ExpectedOutcome::Generic,
    },
    case("ack", &["ack", "ready"]),
    case("config", &["config"]),
    Case {
        path: "config migrate-tasks",
        arguments: &["config", "migrate-tasks", "task-export.json"],
        output: OutputKind::Document,
        expected: ExpectedOutcome::TaskMigration,
    },
];

const fn case(path: &'static str, arguments: &'static [&'static str]) -> Case {
    Case {
        path,
        arguments,
        output: OutputKind::Document,
        expected: ExpectedOutcome::Generic,
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "spice-landing-process-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).expect("create isolated landing fixture");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove isolated landing fixture");
    }
}

#[test]
fn every_registered_leaf_reaches_process_dispatch_in_text_and_json() {
    let registered = landed_contract_paths();
    // The way this array acquires a repeat is a merge fold keeping both sides of
    // an entry neither side moved, so the panic is read by whoever pulled the
    // baseline rather than by whoever wrote the duplicate. Two counts tell that
    // reader a leaf is doubled but not which one, and the array is long enough
    // that finding it means scripting over the source; naming it here is the
    // difference between a one-line deletion and a detour.
    let mut seen = BTreeSet::new();
    let repeated = CASES
        .iter()
        .filter(|case| !seen.insert(case.path))
        .map(|case| case.path)
        .collect::<Vec<_>>();
    assert!(
        repeated.is_empty(),
        "process case repeated: {}",
        repeated.join(", "),
    );
    let exercised = CASES
        .iter()
        .map(|case| case.path.to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        exercised, registered,
        "process fixture and registry drifted"
    );

    let scratch = Scratch::new();
    write_task_export(scratch.path());
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
        assert_expected_outcome(case, &text, &json);

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

fn write_task_export(root: &Path) {
    fs::write(
        root.join("task-export.json"),
        concat!(
            "{\n",
            "  \"version\": 1,\n",
            "  \"release\": \"0.31.0\",\n",
            "  \"tasks\": {\"hidden_stems\": [\"sandbox\"]}\n",
            "}\n",
        ),
    )
    .expect("write explicit predecessor task export");
}

fn assert_expected_outcome(case: &Case, text: &Output, json: &Output) {
    if case.expected == ExpectedOutcome::Generic {
        return;
    }
    assert!(text.status.success(), "{} text: {text:?}", case.path);
    let text = String::from_utf8(text.stdout.clone()).expect("migration text is UTF-8");
    assert!(text.contains("source_release = \"0.31.0\""), "{text}");
    assert!(text.contains("\"maxim_proposal\""), "{text}");
    assert!(text.contains("\"sandbox\""), "{text}");

    let outcome: wire::Outcome<serde_json::Value> =
        wire::decode(&json.stdout).expect("migration JSON is a typed outcome");
    let wire::Outcome::Reply(reply) = outcome else {
        panic!("{} must return a typed reply: {outcome:?}", case.path);
    };
    assert_eq!(reply.body["source_release"], "0.31.0");
    assert_eq!(
        reply.body["tasks"]["hidden_stems"],
        serde_json::json!(["oops", "maxim_proposal", "sandbox"]),
    );
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

/// Every landed leaf, read from the registry that actually dispatches rather
/// than from a second copy of it, so this sweep measures the live surface.
fn landed_contract_paths() -> BTreeSet<String> {
    spice::cli::contracts()
        .iter()
        .filter(|contract| spice::cli::landing(contract.path) == spice::cli::Landing::Landed)
        .map(|contract| contract.path.join(" "))
        .collect()
}

fn invoke(root: &Path, case: &Case, json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
    if json {
        command.arg("--json");
    }
    if case.path == "demo" {
        command.args([
            "demo",
            "--seed-only",
            "--root",
            if json {
                "readiness-demo-json"
            } else {
                "readiness-demo-text"
            },
        ]);
    } else {
        command.args(case.arguments);
    }
    command
        .current_dir(root)
        .env_remove(steer::hook::INBOX)
        .env_remove("CODEX_THREAD_ID") // env-policy: allow
        .env_remove("CLAUDE_CODE_SESSION_ID") // env-policy: allow
        .env_remove("SPICE_AGENT_VENDOR") // env-policy: allow
        .env("CODEX_HOME", root.join("absent-codex-home"))
        .env("CLAUDE_CONFIG_DIR", root.join("absent-claude-home"))
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

/// The exact sentences the placeholder inventory stands for, written out.
///
/// The scan below reads the inventory rather than this list, so that it can
/// never search for a sentence the binary stopped emitting. That leaves one
/// thing unguarded: the inventory could be renamed, and the scan would follow
/// it without complaint. This is the inventory read back as text, so a changed
/// sentence fails here and is answered deliberately instead of travelling
/// silently to every operator surface.
#[test]
fn the_placeholder_inventory_carries_exactly_the_documented_sentences() {
    assert_eq!(
        PlaceholderReason::ALL.map(PlaceholderReason::as_str),
        [
            "release::not-landed",
            "release::dispatch-gap",
            "release::landing-unaccounted",
            "command::handler-not-landed",
        ]
    );
}

/// The browser's absence assertion follows the canonical placeholder sentence.
///
/// JavaScript cannot import the Rust inventory, so this source-boundary test
/// makes the relationship explicit in the suite that owns the inventory. A
/// renamed reason must fail here and name the browser fixture that still holds
/// the old spelling, rather than leaving the browser to search for dead text.
#[test]
fn the_browser_landing_gap_guard_reads_the_not_landed_sentence() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/browser/canvas.spec.mjs");
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read browser spec {}: {error}", path.display()));
    let reason = PlaceholderReason::NotLanded.as_str();
    let assertion = format!("assert.doesNotMatch(content, /{reason}/);");

    assert!(
        source.contains(&assertion),
        concat!(
            "{} must bind its landing-gap assertion to ",
            "PlaceholderReason::NotLanded; missing {:?}"
        ),
        path.display(),
        assertion
    );
    assert_eq!(
        source.matches(reason).count(),
        1,
        "{} must contain exactly one landing-gap search for {reason:?}",
        path.display()
    );
}

fn assert_no_placeholder(output: &Output, path: &str) {
    for reason in PlaceholderReason::ALL {
        let placeholder = reason.as_str();
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
