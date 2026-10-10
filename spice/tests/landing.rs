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
    HookSilent,
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
    case(
        "session context",
        &[
            "session", "context", "retained", "--lane", UUID, "--vendor", "omp", "--stream",
            "session", "--source", UUID, "--member", "1", "--offset", "0",
        ],
    ),
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
    case("rtk gain", &["rtk", "gain", UUID]),
    case("install", &["install"]),
    case("install symbols", &["install", "symbols"]),
    case("dev cargo", &["dev", "cargo", "--", "--version"]),
    case("dev xtask", &["dev", "xtask", "--", "help"]),
    case("dev cargo-store", &["dev", "cargo-store"]),
    case("dev report-page", &["dev", "report-page", "{}"]),
    case("lock", &["lock"]),
    case(
        "lock run",
        &["lock", "run", "readiness", "--", "/bin/echo", "locked"],
    ),
    case("restore verify", &["restore", "verify", "missing"]),
    case("gate", &["gate"]),
    case("gate camp", &["gate", "camp"]),
    case(
        "gate commit-msg",
        &["gate", "commit-msg", "missing-message"],
    ),
    case("gate prove", &["gate", "prove"]),
    case("gate kill", &["gate", "kill"]),
    case("gate pre-commit", &["gate", "pre-commit"]),
    case("gate selection", &["gate", "selection"]),
    Case {
        path: "gate reference-transaction",
        arguments: &["gate", "reference-transaction", "committed"],
        output: OutputKind::HookSilent,
        expected: ExpectedOutcome::Generic,
    },
    case("proof", &["proof"]),
    case("proof replay", &["proof", "replay"]),
    case("release notes", &["release", "notes"]),
    case("release seal", &["release", "seal"]),
    case("up", &["up"]),
    case("down", &["down"]),
    case("board", &["board"]),
    case("board graph", &["board", "graph", "all"]),
    case(
        "board handout",
        &["board", "handout", "readiness-handout.json"],
    ),
    case("task list", &["task", "list"]),
    case("task status", &["task", "status"]),
    case("task show", &["task", "show", "READY-12345678"]),
    case("task next", &["task", "next"]),
    case("task recover", &["task", "recover", "READY-12345678"]),
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
        &[
            "task",
            "depends",
            "READY-12345678",
            "AFTER-12345678",
            "--remove",
        ],
    ),
    case(
        "task delete",
        &[
            "task",
            "delete",
            "READY-12345678",
            "filed against the wrong surface",
        ],
    ),
    case(
        "task modify",
        &[
            "task",
            "modify",
            "READY-12345678",
            "--description",
            "the meaning the filing missed",
            "--acceptance",
            "a clause a reviewer can check",
            "--acceptance",
            "and a second the first one missed",
            "--priority",
            "high",
            "--project",
            "board.test",
        ],
    ),
    case(
        "task require",
        &["task", "require", "READY-12345678", "--clear"],
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
    case("lane list", &["lane", "list"]),
    case("lane drain", &["lane", "drain", UUID]),
    case("lane wake", &["lane", "wake", UUID]),
    case("lane model", &["lane", "model", UUID, "claude-opus-5-5"]),
    case("lane effort", &["lane", "effort", UUID, "high"]),
    case("lane fast", &["lane", "fast", UUID, "on"]),
    case("lane remove", &["lane", "remove", UUID]),
    case("lane import", &["lane", "import", UUID, "."]),
    case("lane purge", &["lane", "purge"]),
    case("steer", &["steer", UUID, "readiness direction"]),
    case("steer cancel", &["steer", "cancel", UUID]),
    case("steer resend", &["steer", "resend", UUID]),
    case("steer recover", &["steer", "recover", UUID]),
    case("tell", &["tell", UUID, "the parser moved"]),
    case("inbox", &["inbox"]),
    case("recall", &["recall"]),
    case("consent", &["consent"]),
    case("consent allow", &["consent", "allow", UUID]),
    case("consent deny", &["consent", "deny", UUID]),
    case("consent policy", &["consent", "policy"]),
    case("taste", &["taste"]),
    case("taste add", &["taste", "add", "readiness taste"]),
    case("taste drop", &["taste", "drop", "readiness"]),
    case("taste mine", &["taste", "mine"]),
    case("trust", &["trust"]),
    case("trust pend", &["trust", "pend"]),
    case("trust grant", &["trust", "grant", DIGEST]),
    case("trust revoke", &["trust", "revoke", DIGEST]),
    case("backup", &["backup", "readiness-backup"]),
    case("restore", &["restore", "missing"]),
    case("journal", &["journal"]),
    case("journal reindex", &["journal", "reindex"]),
    case("journal verify", &["journal", "verify"]),
    case("thread", &["thread", "1"]),
    case("release publish", &["release", "publish"]),
    Case {
        path: "run",
        arguments: &["run", "--", "/bin/echo", "run"],
        output: OutputKind::ChildStream,
        expected: ExpectedOutcome::Generic,
    },
    case("config", &["config"]),
    Case {
        path: "config migrate-tasks",
        arguments: &["config", "migrate-tasks", "task-export.json"],
        output: OutputKind::Document,
        expected: ExpectedOutcome::TaskMigration,
    },
];

/// Every rendered path a list registers more than once, in the order the repeats fall.
///
/// A merge that folds two edits of either the case array or the contract registry
/// can keep both copies of an entry, and the panic is the only thing the next lane
/// reads. How far the distinct set fell short of the list is a subtraction they
/// then have to solve; the paths themselves are the answer, so those are what this
/// hands back. Every repeat is collected rather than the first, because one fold
/// can duplicate more than one entry and a lane that fixes only the named one
/// comes straight back.
fn repeated_paths<T>(items: &[T], render: impl Fn(&T) -> String) -> Vec<String> {
    let mut seen = BTreeSet::new();
    items
        .iter()
        .filter_map(|item| {
            let path = render(item);
            (!seen.insert(path.clone())).then_some(path)
        })
        .collect()
}

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
    let repeated_contracts =
        repeated_paths(spice::cli::contracts(), |contract| contract.path.join(" "));
    assert!(
        repeated_contracts.is_empty(),
        "contract path repeated: {}",
        repeated_contracts.join(", ")
    );
    let registered = landed_contract_paths();
    let repeated = repeated_paths(CASES, |case| case.path.to_owned());
    assert!(
        repeated.is_empty(),
        "process case repeated: {}",
        repeated.join(", ")
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
        if case.output != OutputKind::HookSilent {
            assert_one_stream(&text, case.path);
            assert_one_stream(&json, case.path);
        } else {
            assert!(text.stdout.is_empty() && text.stderr.is_empty());
            assert!(json.stdout.is_empty() && json.stderr.is_empty());
        }
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

#[test]
fn a_repeated_case_is_named_rather_than_counted() {
    // The defect this guards is a merge folding two edits of the array and
    // keeping both copies of an entry, so the fixture duplicates two different
    // paths rather than one: a count only says how many entries went missing,
    // and a refusal that stops at the first repeat sends the next lane back for
    // the second. Both duplicates have to come out by name, in the order a
    // reader scanning down the array would meet them.
    let folded = [
        case("task status", &["task", "status"]),
        case("task next", &["task", "next"]),
        case("task status", &["task", "status"]),
        case("task add", &["task", "add", "readiness task"]),
        case("task next", &["task", "next"]),
    ];
    assert_eq!(
        repeated_paths(&folded, |case| case.path.to_owned()),
        ["task status", "task next"]
    );

    // The shipped array reaches the same refusal here in milliseconds, where the
    // dispatch test only reaches it after standing up a scratch root.
    let shipped = repeated_paths(CASES, |case| case.path.to_owned());
    assert!(
        shipped.is_empty(),
        "process case repeated: {}",
        shipped.join(", ")
    );
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
        OutputKind::HookSilent => assert_eq!(
            output.status.code(),
            Some(0),
            "{} {form} did not preserve the inert hook exit",
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
        .env_remove("SPICE_AGENT_VENDOR") // env-policy: allow
        .env(spice::cargo_isolation::BOOTSTRAP_HOME_VARIABLE, root)
        .env(
            spice::cargo_isolation::NATIVE_CARGO_VARIABLE,
            fixture_native_tool("cargo"),
        )
        .env(
            spice::cargo_isolation::NATIVE_RUSTC_VARIABLE,
            fixture_native_tool("rustc"),
        )
        .env(
            spice::cargo_isolation::NATIVE_RUSTDOC_VARIABLE,
            fixture_native_tool("rustdoc"),
        )
        .output()
        .unwrap_or_else(|error| panic!("{} process did not launch: {error}", case.path))
}

fn fixture_native_tool(name: &str) -> PathBuf {
    let tool = PathBuf::from(env!("CARGO"))
        .parent()
        .expect("Cargo test executable has one toolchain directory")
        .join(name);
    assert!(tool.is_file(), "native fixture tool exists: {tool:?}");
    tool
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
