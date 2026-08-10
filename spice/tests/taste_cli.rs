#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use bus::{Bus, CommitOutcome};
use journal::{FactCause, JournalWrite};
use serde::de::DeserializeOwned;
use wire::body::{
    LaneSeated, SteerAcked, SteerDeclined, SteerEscalated, SteerSent, TasteCleared, TasteConfirmed,
    TasteReminded, TranscriptLocus,
};
use wire::intent::{BoardAnswer, CommandAnswer, TaskAdd, TaskAnswer, TextAnswer};
use wire::{
    Actor, CanonicalUuid, Digest, Domain, FactKind, Instant, Outcome, Producer, ReplyPosition,
};

#[path = "support/readiness.rs"]
mod readiness;
#[path = "support/staged_app.rs"]
mod staged_app;

use readiness::Readiness;

const REFUSAL_EXIT: i32 = 2;
const CURATION_ORIGIN: &str = "taste:curation";
const ADD_TITLE: &str = concat!(
    "Propose tracked taste addition: Repeated alternate execution branches hide the current ",
    "path"
);
static NEXT: AtomicU64 = AtomicU64::new(0);

const POLICY: &str = r#"
[tasks]
stems = ["taste"]

[policy]
taste_mining_threshold = 3

[taste.zeta]
words = ["wait*", "sleep*"]
message = "Observe the real completion event."

[taste.alpha]
words = ["alias*"]
message = "Keep one current name."
"#;

/// Stable steering keys the fire fixtures spell, each eight alphanumeric characters.
const FIRST_DIRECTIVE: &str = "1kKmT001";
const RENEWED_DIRECTIVE: &str = "1kKmT003";
const ALPHA_DIRECTIVE: &str = "1kKmT004";
const UNSEATED_DIRECTIVE: &str = "1kKmT005";
/// Prose the `zeta` bag selects, authored once and written again after
/// compaction so one unchanged identity spans two epochs.
const REPEATED_PROSE: &str = "we will wait for the build and sleep between checks";

struct Fixture {
    root: PathBuf,
    journal: PathBuf,
    canvas: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let requested = std::env::temp_dir().join(format!(
            "spice-taste-process-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(requested.join(".git")).expect("fixture Git directory");
        let root = fs::canonicalize(requested).expect("fixture root canonicalizes");
        staged_app::seed(&root);
        fs::write(root.join("spice-rust.toml"), POLICY).expect("tracked taste policy");
        let runtime = root.join(".git/.spice-rs");
        let fixture = Self {
            journal: runtime.join("data/spicejournal.sqlite3"),
            canvas: runtime.join("canvas"),
            root,
        };
        seed_judgments(&fixture);
        fixture
    }

    fn invoke(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(arguments)
            .current_dir(&self.root)
            .output()
            .expect("spice taste command launches")
    }

    fn cursor(&self) -> wire::Seq {
        journal::JournalReader::open(&self.journal)
            .expect("journal reader opens")
            .cursor()
            .expect("journal cursor reads")
            .expect("resident has established authority")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("fixture cleanup");
    }
}

struct Resident {
    child: Option<Child>,
}

impl Resident {
    fn start(fixture: &Fixture) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_spice"))
            // A fixture resident asks the kernel for a free port: the fixed default
            // belongs to the operator's one resident, and concurrent fixtures on a
            // shared machine cannot all have it.
            .args(["--json", "up", "--port", "0"])
            .current_dir(&fixture.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("resident starts");
        let mut resident = Self { child: Some(child) };
        let watched = fixture.canvas.parent().expect("canvas has runtime parent");
        fs::create_dir_all(watched).expect("runtime directory");
        match readiness::wait_until_ready(
            resident.child.as_mut().expect("resident child"),
            watched,
            readiness::ARRIVAL_HORIZON,
            "resident discovery",
            || fixture.canvas.is_file(),
        ) {
            Readiness::Ready => resident,
            Readiness::Exited => panic!("resident exited before readiness"),
        }
    }

    fn stop(mut self, fixture: &Fixture) {
        let down = fixture.invoke(&["--json", "down"]);
        assert!(down.status.success(), "down failed: {}", spoken(&down));
        let output = self
            .child
            .take()
            .expect("resident child")
            .wait_with_output()
            .expect("resident exits");
        assert!(
            output.status.success(),
            "resident failed: {}",
            spoken(&output)
        );
    }
}

impl Drop for Resident {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn taste_reporting_and_review_plans_survive_restart() {
    let fixture = Fixture::new();
    let resident = Resident::start(&fixture);

    let text = text_reply::<serde_json::Value>(&fixture.invoke(&["taste"]));
    assert_report(&fixture, &text);
    let json = json_reply::<serde_json::Value>(&fixture.invoke(&["taste", "--json"]));
    assert!(matches!(json.position, ReplyPosition::Snapshot(_)));
    assert_eq!(json.body, text);

    let before_ungrounded = fixture.cursor();
    let ungrounded = fixture.invoke(&[
        "--json",
        "taste",
        "add",
        "Repeated alternate execution branches hide the current path",
    ]);
    assert_eq!(ungrounded.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) = wire::decode::<Outcome<serde_json::Value>>(&ungrounded.stderr)
        .expect("ungrounded refusal is typed")
    else {
        panic!("ungrounded taste filing did not refuse")
    };
    assert_eq!(refusal.reason(), "task::origin-required");
    assert!(refusal.explanation().contains("--origin"));
    assert_eq!(fixture.cursor(), before_ungrounded);

    let added = text_reply::<TaskAnswer>(&fixture.invoke(&[
        "taste",
        "add",
        "Repeated alternate execution branches hide the current path",
        "--origin",
        CURATION_ORIGIN,
    ]));
    let dropped = json_reply::<TaskAnswer>(&fixture.invoke(&[
        "--json",
        "taste",
        "drop",
        "alpha",
        "--origin",
        CURATION_ORIGIN,
    ]));
    assert!(matches!(dropped.position, ReplyPosition::Committed(_)));

    let before_unknown = fixture.cursor();
    let unknown = fixture.invoke(&["--json", "taste", "drop", "missing"]);
    assert_eq!(unknown.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) =
        wire::decode::<Outcome<serde_json::Value>>(&unknown.stderr).expect("refusal is typed")
    else {
        panic!("unknown taste did not refuse")
    };
    assert_eq!(refusal.reason(), "taste::unknown");
    assert_eq!(fixture.cursor(), before_unknown);

    let causal: wire::Origin = CURATION_ORIGIN.parse().expect("curation origin");
    let current_board = board(&fixture);
    assert_review_plan(&current_board, &added, ADD_TITLE, &causal);
    assert_review_plan(
        &current_board,
        &dropped.body,
        "Propose tracked taste retirement: alpha",
        &causal,
    );

    resident.stop(&fixture);
    let restarted = Resident::start(&fixture);
    assert_eq!(
        text_reply::<serde_json::Value>(&fixture.invoke(&["taste"])),
        text
    );
    let restarted_board = board(&fixture);
    assert_review_plan(&restarted_board, &added, ADD_TITLE, &causal);
    assert_review_plan(
        &restarted_board,
        &dropped.body,
        "Propose tracked taste retirement: alpha",
        &causal,
    );
    restarted.stop(&fixture);

    let reader = journal::JournalReader::open(&fixture.journal).expect("final journal opens");
    let facts = reader.fact_snapshot().expect("final fact snapshot");
    assert_eq!(kind_count(&facts, FactKind::TaskAdded), 2);
    assert!(
        facts
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == FactKind::TaskAdded)
            .all(|fact| fact.origin == causal)
    );
    reader.verify().expect("final authority verifies");
    let plans = task_add_requests(&fixture);
    assert_eq!(plans.len(), 2);
    let addition = plans
        .iter()
        .find(|plan| plan.title.starts_with("Propose tracked taste addition:"))
        .expect("addition receipt is durable");
    assert!(addition.description.as_deref().is_some_and(|description| {
        description.contains("Repeated alternate execution branches hide the current path")
    }));
    let retirement = plans
        .iter()
        .find(|plan| plan.title == "Propose tracked taste retirement: alpha")
        .expect("retirement receipt is durable");
    let retirement_body = retirement
        .description
        .as_deref()
        .expect("retirement carries its evidence");
    assert!(retirement_body.contains("[taste.alpha]"));
    assert!(retirement_body.contains("tape:alpha"));
    assert!(plans.iter().all(|plan| {
        plan.project.as_deref() == Some("taste.policy")
            && plan.flow.as_deref() == Some("todo,review")
            && plan.acceptance.len() == 2
            && plan.origin.as_ref() == Some(&causal)
    }));
}

#[test]
fn live_fires_count_each_stable_directive_once_and_keep_one_durable_vendor() {
    let fixture = Fixture::new();
    seed_fires(&fixture);
    let resident = Resident::start(&fixture);

    let before = fixture.cursor();
    let report = text_reply::<serde_json::Value>(&fixture.invoke(&["taste"]));
    // Deriving the view appended nothing, so the report persists no counter.
    assert_eq!(fixture.cursor(), before);

    let zeta = &report["tastes"][0];
    assert_eq!(zeta["name"], "zeta");
    let fires = zeta["fires"].as_array().expect("zeta fires");
    assert_eq!(fires.len(), 2, "{zeta}");
    // Newest first. Escalation retained the first reminder's identity, so its
    // acknowledgment is the standing that publication carries directly.
    assert_eq!(fires[0]["key"], RENEWED_DIRECTIVE);
    assert_eq!(fires[0]["standing"], "declined");
    assert_eq!(fires[0]["compaction"], 20);
    assert_eq!(fires[1]["key"], FIRST_DIRECTIVE);
    assert_eq!(fires[1]["standing"], "acked");
    assert_eq!(fires[1]["compaction"], 10);
    assert!(fires.iter().all(|fire| fire["vendor"] == "claude"));
    // Two occupant generations, one seat: a renewal cannot split the vendor.
    assert_ne!(fires[0]["occupant"], fires[1]["occupant"]);
    assert_eq!(
        zeta["vendors"],
        serde_json::json!([{"vendor": "claude", "fires": 2, "acked": 1, "declined": 1}])
    );
    assert_eq!(
        zeta["recurrence"],
        serde_json::json!([{
            "content": fires[0]["content"],
            "epochs": 2,
            "latest": fires[0]["position"],
        }])
    );
    // The pre-reminder curation evidence is still readable beside the fires.
    assert_eq!(zeta["judgments"][0]["verdict"], "confirmed");
    assert_eq!(zeta["judgments"][0]["locus"], "tape:zeta");

    let alpha = &report["tastes"][1];
    assert_eq!(alpha["name"], "alpha");
    assert_eq!(alpha["fires"].as_array().expect("alpha fires").len(), 1);
    assert_eq!(alpha["fires"][0]["standing"], "live");
    assert_eq!(
        alpha["vendors"],
        serde_json::json!([{"vendor": "claude", "fires": 1, "acked": 0, "declined": 0}])
    );
    assert_eq!(alpha["recurrence"], serde_json::json!([]));
    assert_eq!(alpha["judgments"][0]["verdict"], "cleared");
    assert_eq!(alpha["judgments"][0]["locus"], "tape:alpha");

    let dropped = json_reply::<TaskAnswer>(&fixture.invoke(&[
        "--json",
        "taste",
        "drop",
        "zeta",
        "--origin",
        CURATION_ORIGIN,
    ]));
    let causal = CURATION_ORIGIN.parse().expect("curation origin");
    assert_review_plan(
        &board(&fixture),
        &dropped.body,
        "Propose tracked taste retirement: zeta",
        &causal,
    );
    let retirement = task_add_requests(&fixture)
        .into_iter()
        .find(|plan| plan.title == "Propose tracked taste retirement: zeta")
        .expect("retirement receipt is durable");
    assert_eq!(retirement.origin, Some(causal));
    let cited = retirement
        .description
        .as_deref()
        .expect("retirement carries its evidence");
    // The reminder's own locus, not the transcript locus the decline was read
    // at: the operator narrows a bag by rereading the prose that fired it.
    assert!(
        cited.contains("- tape 1+40 event 0 line 20 column 1 on claude"),
        "{cited}"
    );
    assert!(!cited.contains("line 11 column 1"), "{cited}");
    assert!(
        cited.contains("Historical cleared loci:\n- none"),
        "{cited}"
    );
    resident.stop(&fixture);
}

#[test]
fn a_reminder_naming_an_unseated_lane_refuses_rather_than_disappearing() {
    let fixture = Fixture::new();
    seed_unseated_reminder(&fixture);
    let resident = Resident::start(&fixture);

    let output = fixture.invoke(&["--json", "taste"]);
    assert_eq!(
        output.status.code(),
        Some(REFUSAL_EXIT),
        "{}",
        spoken(&output)
    );
    let Outcome::Refusal(refusal) =
        wire::decode::<Outcome<serde_json::Value>>(&output.stderr).expect("refusal is typed")
    else {
        panic!("an unseated reminder did not refuse")
    };
    assert_eq!(refusal.reason(), "taste::lane-unseated");
    assert_eq!(
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed),
        Some(uuid(0x20).to_string().as_str())
    );
    // The whole report is withheld: an unjoinable fire cannot be dropped from a
    // row that still renders as if it were the current answer.
    assert!(output.stdout.is_empty(), "{}", spoken(&output));
    resident.stop(&fixture);
}

#[test]
fn native_task_add_carries_authored_context_through_client_add() {
    let fixture = Fixture::new();
    let resident = Resident::start(&fixture);
    let added = text_reply::<TaskAnswer>(&fixture.invoke(&[
        "task",
        "add",
        "Carry authored CLI context",
        "--project",
        "task.cli",
        "--description",
        "The created task must retain this operator handoff.",
        "--acceptance",
        "Description reaches the authoritative statement",
        "--acceptance",
        "Every acceptance criterion survives the client seam",
        "--origin",
        "ack:1kLGp2NJ",
    ]));

    let current = board(&fixture);
    let statement = &current
        .tasks
        .iter()
        .find(|task| task.statement.body.handle == added.task)
        .expect("created task is visible on the authoritative board")
        .statement
        .body;
    assert_eq!(
        statement.description.as_deref(),
        Some("The created task must retain this operator handoff.")
    );
    assert_eq!(
        statement.acceptance,
        [
            "Description reaches the authoritative statement",
            "Every acceptance criterion survives the client seam",
        ]
    );
    assert_eq!(statement.provenance.to_string(), "ack:1kLGp2NJ");

    let requests = task_add_requests(&fixture);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].description, statement.description);
    assert_eq!(requests[0].acceptance, statement.acceptance);
    assert_eq!(requests[0].origin, Some(statement.provenance.clone()));

    let listed = text_reply::<TextAnswer>(&fixture.invoke(&[
        "task",
        "list",
        "--project",
        "project:task.cli",
        "--status",
        "pending",
        "--limit",
        "1",
    ]));
    assert!(
        listed
            .text
            .starts_with("scope explicit-project project:task.cli\n")
    );
    assert!(listed.text.contains(added.task.as_str()), "{}", listed.text);
    assert!(
        listed.text.contains("Carry authored CLI context"),
        "{}",
        listed.text
    );

    let status = text_reply::<TextAnswer>(&fixture.invoke(&["task", "status"]));
    assert!(status.text.contains(concat!(
        "filter ( project:agent.operator.task or origin_thread.is:operator or ",
        "( project:serve or project:task or project:taste ) )"
    )));
    assert!(
        status
            .text
            .contains("public task project depth 2..3 dotted segments")
    );
    resident.stop(&fixture);
}

#[test]
fn recurring_operator_corrections_file_one_hidden_review_and_reuse_it() {
    let fixture = Fixture::new();
    seed_mining_corrections(&fixture);
    let tracked_before = fs::read_to_string(fixture.root.join("spice-rust.toml")).unwrap();
    let resident = Resident::start(&fixture);

    let first = json_reply::<serde_json::Value>(&fixture.invoke(&["--json", "taste", "mine"]));
    assert_eq!(first.body["threshold"], 3);
    assert_eq!(first.body["scanned"], 8);
    assert_eq!(first.body["excluded"], 4);
    assert_eq!(first.body["qualifying"], 1);
    assert_eq!(first.body["findings"][0]["recurrence"], 3);
    assert_eq!(first.body["findings"][0]["standing"], "filed");
    assert_eq!(
        first.body["findings"][0]["message"],
        "Do not use grep for repository searches; use rg instead."
    );

    let reader = journal::JournalReader::open(&fixture.journal).unwrap();
    let facts = reader.fact_snapshot().unwrap();
    let proposals = facts
        .facts()
        .iter()
        .filter_map(|fact| fact.fact.read::<wire::body::TaskAdded>())
        .filter(|added| added.project.starts_with(".maxim_proposal.taste."))
        .collect::<Vec<_>>();
    assert_eq!(proposals.len(), 1);
    let description = proposals[0].description.as_deref().unwrap();
    assert!(description.contains("Recurrence count: 3"), "{description}");
    assert!(description.contains("\"Do not use grep for repository searches; use rg instead.\""));
    assert!(description.contains("(declined)"), "{description}");
    assert!(description.contains("NACK reason"), "{description}");
    assert_eq!(proposals[0].acceptance.len(), 3);
    drop(reader);

    let second = json_reply::<serde_json::Value>(&fixture.invoke(&["--json", "taste", "mine"]));
    assert_eq!(second.body["findings"][0]["standing"], "reused-active");
    let facts = journal::JournalReader::open(&fixture.journal)
        .unwrap()
        .fact_snapshot()
        .unwrap();
    assert_eq!(
        facts
            .facts()
            .iter()
            .filter_map(|fact| fact.fact.read::<wire::body::TaskAdded>())
            .filter(|added| added.project.starts_with(".maxim_proposal.taste."))
            .count(),
        1
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("spice-rust.toml")).unwrap(),
        tracked_before
    );
    let predecessor = first.body["findings"][0]["task"]
        .as_str()
        .unwrap()
        .parse::<wire::TaskHandle>()
        .unwrap();
    resident.stop(&fixture);

    complete_mined_proposal(&fixture, &predecessor);
    seed_later_mining_correction(&fixture);
    let restarted = Resident::start(&fixture);
    let later = json_reply::<serde_json::Value>(&fixture.invoke(&["--json", "taste", "mine"]));
    assert_eq!(later.body["findings"][0]["standing"], "filed");
    assert_eq!(later.body["findings"][0]["recurrence"], 4);
    let successor = later.body["findings"][0]["task"]
        .as_str()
        .unwrap()
        .parse::<wire::TaskHandle>()
        .unwrap();
    assert_ne!(successor, predecessor);
    let proposals = journal::JournalReader::open(&fixture.journal)
        .unwrap()
        .fact_snapshot()
        .unwrap()
        .facts()
        .iter()
        .filter_map(|fact| fact.fact.read::<wire::body::TaskAdded>())
        .filter(|added| added.project.starts_with(".maxim_proposal.taste."))
        .collect::<Vec<_>>();
    assert_eq!(proposals.len(), 2);
    let successor_add = proposals
        .iter()
        .find(|added| added.task == successor.born().unwrap())
        .unwrap();
    assert_eq!(successor_add.depends, [predecessor.born().unwrap()]);
    assert!(
        successor_add
            .description
            .as_deref()
            .unwrap()
            .contains("Recurrence count: 4")
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("spice-rust.toml")).unwrap(),
        tracked_before
    );
    restarted.stop(&fixture);
}

include!("taste_cli/support.rs");
