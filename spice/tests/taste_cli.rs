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

#[path = "taste_cli/mining_fixture.rs"]
mod mining_fixture;
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
words = ["wait", "sleep"]
message = "Observe the real completion event."

[taste.alpha]
words = ["alias"]
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
            // The suite exercises operator-context commands; a lane identity
            // inherited from an agent shell would flip discovery into lane
            // context and miss the fixture resident.
            .env_remove(spice::agent_launch::SPICE_RS_LANE)
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
            // The fixture resident serves this root in operator context, never
            // as a seat of the launching agent shell.
            .env_remove(spice::agent_launch::SPICE_RS_LANE)
            // A fixture resident asks the kernel for a free port: the fixed default
            // belongs to the operator's one resident, and concurrent fixtures on a
            // shared machine cannot all have it.
            .args(["--json", "up", "--foreground", "--port", "0"])
            .current_dir(&fixture.root)
            .arg(spice::install::TEST_CANVAS_STAGE_ARGUMENT)
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
            Readiness::Exited => {
                let output = resident
                    .child
                    .take()
                    .expect("exited resident child")
                    .wait_with_output()
                    .expect("exited resident output");
                panic!("resident exited before readiness: {}", spoken(&output));
            }
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
    assert_review_plan(&current_board, &added, ADD_TITLE, &causal, false);
    assert_review_plan(
        &current_board,
        &dropped.body,
        "Propose tracked taste retirement: alpha",
        &causal,
        true,
    );

    resident.stop(&fixture);
    let restarted = Resident::start(&fixture);
    assert_eq!(
        text_reply::<serde_json::Value>(&fixture.invoke(&["taste"])),
        text
    );
    let restarted_board = board(&fixture);
    assert_review_plan(&restarted_board, &added, ADD_TITLE, &causal, false);
    assert_review_plan(
        &restarted_board,
        &dropped.body,
        "Propose tracked taste retirement: alpha",
        &causal,
        true,
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
fn disabled_scoped_rows_remain_visible_and_drop_preserves_the_complete_row() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join("spice-rust.toml"),
        format!(
            concat!(
                "{}\n",
                "[taste.retired]\n",
                "words = [\"branch\"]\n",
                "message = \"Keep one current path.\"\n",
                "enabled = false\n",
                "scopes = {{ paths = [\"src/**\"], vendors = [\"codex\"], phases = [\"pre-commit-success\"] }}\n",
            ),
            POLICY,
        ),
    )
    .expect("disabled scoped taste policy");
    let resident = Resident::start(&fixture);

    let report = text_reply::<serde_json::Value>(&fixture.invoke(&["taste"]));
    let row = report["tastes"]
        .as_array()
        .expect("taste rows")
        .iter()
        .find(|row| row["name"] == "retired")
        .expect("disabled row remains enumerable");
    assert_eq!(row["enabled"], false);
    assert_eq!(
        row["scopes"],
        serde_json::json!({
            "paths": ["src/**"],
            "extensions": [],
            "vendors": ["codex"],
            "models": [],
            "phases": ["pre-commit-success"],
        })
    );

    let dropped = fixture.invoke(&["taste", "drop", "retired", "--origin", CURATION_ORIGIN]);
    assert!(dropped.status.success(), "{}", spoken(&dropped));
    let proposal = task_add_requests(&fixture)
        .into_iter()
        .find(|plan| plan.title == "Propose tracked taste retirement: retired")
        .expect("disabled retirement proposal is durable");
    let description = proposal
        .description
        .expect("proposal carries the tracked row");
    assert!(description.contains("enabled = false"), "{description}");
    assert!(
        description.contains(
            "scopes = { paths = [\"src/**\"], vendors = [\"codex\"], phases = [\"pre-commit-success\"] }"
        ),
        "{description}",
    );

    resident.stop(&fixture);
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
        true,
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
    assert_eq!(
        added
            .task
            .born()
            .expect("current task stamp")
            .as_str()
            .len(),
        wire::TaskStamp::WIDTH,
    );

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
    mining_fixture::assert_recurring_operator_corrections();
}

#[test]
fn unavailable_mining_evidence_faults_without_filing_or_editing_policy() {
    let fixture = Fixture::new();
    let unavailable = mining_fixture::seed_unavailable_placeholder(&fixture);
    let tracked_before = fs::read_to_string(fixture.root.join("spice-rust.toml")).unwrap();
    let resident = Resident::start(&fixture);
    mining_fixture::rewrite_unavailable_ack(&fixture, unavailable);

    let output = fixture.invoke(&["--json", "taste", "mine"]);
    assert!(!output.status.success(), "{}", spoken(&output));
    let Outcome::Fault(fault) =
        wire::decode::<Outcome<serde_json::Value>>(&output.stderr).expect("fault is typed")
    else {
        panic!("unavailable evidence did not fault: {}", spoken(&output))
    };
    assert_eq!(fault.reason, "taste::mining-evidence-invalid");
    assert!(
        fault
            .detail
            .contains("unavailable operator steering directive")
    );
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
        0
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("spice-rust.toml")).unwrap(),
        tracked_before
    );
    mining_fixture::restore_unavailable_placeholder(&fixture, unavailable);
    resident.stop(&fixture);
}

fn assert_report(fixture: &Fixture, report: &serde_json::Value) {
    assert_eq!(report["provenance"]["source"], "tracked");
    assert_eq!(report["provenance"]["present"], true);
    assert_eq!(
        report["provenance"]["location"],
        fixture.root.join("spice-rust.toml").display().to_string()
    );
    assert_eq!(report["provenance"]["digest"], report["digest"]);
    assert_eq!(
        report["tastes"]
            .as_array()
            .expect("taste rows")
            .iter()
            .map(|row| row["name"].as_str().expect("taste name"))
            .collect::<Vec<_>>(),
        ["zeta", "alpha"]
    );
    assert_eq!(
        report["tastes"][0]["words"],
        serde_json::json!(["wait", "sleep"])
    );
    assert_eq!(report["tastes"][0]["judgments"][0]["verdict"], "confirmed");
    assert_eq!(report["tastes"][0]["judgments"][0]["locus"], "tape:zeta");
    assert_eq!(report["tastes"][1]["judgments"][0]["verdict"], "cleared");
    assert_eq!(report["tastes"][1]["judgments"][0]["locus"], "tape:alpha");
    assert_eq!(
        report["tastes"][0]["judgments"][0]["at"],
        "2030-04-12T18:44:09.271Z"
    );
    assert!(
        report["tastes"]
            .as_array()
            .expect("taste rows")
            .iter()
            .all(|row| row["name"] != "retired")
    );
}

fn assert_review_plan(
    board: &BoardAnswer,
    answer: &TaskAnswer,
    title: &str,
    origin: &wire::Origin,
    taste_review: bool,
) {
    let task = board
        .tasks
        .iter()
        .find(|task| task.statement.body.handle == answer.task)
        .expect("planned task is on the current board");
    assert_eq!(task.presentation_title(), title);
    assert_eq!(task.statement.body.project, "taste.policy");
    assert_eq!(&task.statement.body.provenance, origin);
    assert!(!task.visibility.is_hidden());
    let expected_flow = if taste_review {
        vec!["plan", "todo", "review"]
    } else {
        vec!["todo", "review"]
    };
    assert_eq!(
        task.workflow
            .body
            .flow
            .iter()
            .map(wire::Phase::as_str)
            .collect::<Vec<_>>(),
        expected_flow,
    );
    assert_eq!(
        task.workflow.body.phase.as_str(),
        if taste_review { "plan" } else { "todo" },
    );
    assert_eq!(
        task.statement
            .body
            .tags
            .iter()
            .any(|tag| tag == "taste-review:required"),
        taste_review,
    );
    assert_eq!(task.workflow.body.status, "pending");
}

fn board(fixture: &Fixture) -> BoardAnswer {
    let output = fixture.invoke(&["board"]);
    assert!(output.status.success(), "board failed: {}", spoken(&output));
    let answer = wire::intent::AnswerShape::Board
        .decode(&output.stdout)
        .expect("board text body is typed");
    let CommandAnswer::Board(board) = answer else {
        panic!("board command returned another answer shape")
    };
    *board
}

fn seed_judgments(fixture: &Fixture) {
    fs::create_dir_all(fixture.journal.parent().expect("journal parent"))
        .expect("journal directory");
    let journal = journal::Journal::open(&fixture.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let mut write = JournalWrite::new(
        Actor::Spice,
        "taste:fixture".parse().expect("fixture origin"),
        "2030-04-12T18:44:09.271Z"
            .parse::<Instant>()
            .expect("fixture instant"),
        Producer::Judge,
    );
    write
        .fact(
            TasteConfirmed {
                taste: "zeta".to_owned(),
                locus: "tape:zeta".to_owned(),
            },
            FactCause::Root,
        )
        .expect("confirmed judgment");
    write
        .fact(
            TasteCleared {
                taste: "alpha".to_owned(),
                locus: "tape:alpha".to_owned(),
            },
            FactCause::Root,
        )
        .expect("cleared judgment");
    write
        .fact(
            TasteConfirmed {
                taste: "retired".to_owned(),
                locus: "tape:retired".to_owned(),
            },
            FactCause::Root,
        )
        .expect("retired judgment");
    assert!(matches!(
        bus.commit(write).expect("judgments reach writer"),
        CommitOutcome::Committed(_)
    ));
    bus.shutdown().expect("fixture bus stops");
}

/// Seed the live reminder evidence the report derives its fire rows from.
///
/// One lane is seated once with one vendor. `zeta` fires twice on the same
/// prose across two occupant generations and two compaction epochs; the first
/// directive escalates before it is acknowledged, and the second is declined.
/// `alpha` fires once and is left live.
fn seed_fires(fixture: &Fixture) {
    let bus = open_bus(fixture);
    commit(
        &bus,
        Producer::ResidentRecognition,
        "01",
        |write, origin, at| {
            write
                .fact(seated("claude"), FactCause::Root)
                .expect("lane seating");
            publish_reminder(
                write,
                reminded(FIRST_DIRECTIVE, "zeta", uuid(0x21), REPEATED_PROSE, 10),
                origin,
                at,
            );
        },
    );
    escalate_and_acknowledge(&bus);
    commit(
        &bus,
        Producer::ResidentRecognition,
        "04",
        |write, origin, at| {
            publish_reminder(
                write,
                reminded(RENEWED_DIRECTIVE, "zeta", uuid(0x22), REPEATED_PROSE, 20),
                origin.clone(),
                at,
            );
            publish_reminder(
                write,
                reminded(ALPHA_DIRECTIVE, "alpha", uuid(0x22), "keep the alias", 20),
                origin,
                at,
            );
        },
    );
    let renewed = bus
        .read(|read| read.steering_head(directive(RENEWED_DIRECTIVE)))
        .expect("renewed reminder head reads")
        .expect("renewed reminder head exists");
    commit(&bus, Producer::AckScanner, "05", move |write, _, at| {
        let body = SteerDeclined {
            directive: directive(RENEWED_DIRECTIVE),
            key: RENEWED_DIRECTIVE.to_owned(),
            lane: uuid(0x20),
            epoch: uuid(0x2f),
            locus: locus(11),
            reason: "the wait is the observed completion event".to_owned(),
        };
        let pending = write
            .fact(body.clone(), FactCause::Existing(renewed.body().created))
            .expect("the renewed reminder is declined");
        let next =
            steer::Ledger::transition_head(renewed.body(), steer::Steering::Declined(&body), at)
                .expect("declined head builds");
        write.steering_head(pending, move |_| next);
    });
    bus.shutdown().expect("fixture bus stops");
}

/// Escalate the first directive, then answer that same stable identity.
fn escalate_and_acknowledge(bus: &Bus) {
    let published = bus
        .read(|read| read.steering_head(directive(FIRST_DIRECTIVE)))
        .expect("published reminder head reads")
        .expect("published reminder head exists");
    commit(bus, Producer::BusTimer, "02", move |write, _, at| {
        let body = SteerEscalated {
            directive: directive(FIRST_DIRECTIVE),
            key: FIRST_DIRECTIVE.to_owned(),
            lane: uuid(0x20),
            stage: wire::body::SteeringStage::Reminded,
        };
        let pending = write
            .fact(body.clone(), FactCause::Existing(published.body().created))
            .expect("the first directive reaches reminded presentation");
        let next =
            steer::Ledger::transition_head(published.body(), steer::Steering::Escalated(&body), at)
                .expect("escalated head builds");
        write.steering_head(pending, move |_| next);
    });
    let reminded = bus
        .read(|read| read.steering_head(directive(FIRST_DIRECTIVE)))
        .expect("reminded head reads")
        .expect("reminded head exists");
    commit(bus, Producer::AckScanner, "03", move |write, _, at| {
        let body = SteerAcked {
            directive: directive(FIRST_DIRECTIVE),
            key: FIRST_DIRECTIVE.to_owned(),
            lane: uuid(0x20),
            epoch: uuid(0x2f),
            locus: locus(9),
        };
        let pending = write
            .fact(body.clone(), FactCause::Existing(reminded.body().created))
            .expect("the stable directive is acknowledged");
        let next =
            steer::Ledger::transition_head(reminded.body(), steer::Steering::Acked(&body), at)
                .expect("acknowledged head builds");
        write.steering_head(pending, move |_| next);
    });
}

/// Seed one reminder whose lane the durable stream never seated.
fn seed_unseated_reminder(fixture: &Fixture) {
    let bus = open_bus(fixture);
    commit(
        &bus,
        Producer::ResidentRecognition,
        "01",
        |write, origin, at| {
            publish_reminder(
                write,
                reminded(UNSEATED_DIRECTIVE, "zeta", uuid(0x21), REPEATED_PROSE, 10),
                origin,
                at,
            );
        },
    );
    bus.shutdown().expect("fixture bus stops");
}

fn open_bus(fixture: &Fixture) -> Bus {
    let journal = journal::Journal::open(&fixture.journal).expect("fixture journal opens");
    Bus::start(journal).expect("fixture bus starts")
}

/// Commit one batch of facts, which is why the producer is a parameter: a
/// registered body admits exactly one producer class, and these span three.
fn commit(
    bus: &Bus,
    producer: Producer,
    minute: &str,
    facts: impl FnOnce(&mut JournalWrite, wire::Origin, Instant),
) {
    let origin = "taste:fixture"
        .parse::<wire::Origin>()
        .expect("fixture origin");
    let at = format!("2030-04-12T19:{minute}:00.000Z")
        .parse::<Instant>()
        .expect("fixture instant");
    let mut write = JournalWrite::new(Actor::Spice, origin.clone(), at, producer);
    facts(&mut write, origin, at);
    assert!(matches!(
        bus.commit(write).expect("facts reach writer"),
        CommitOutcome::Committed(_)
    ));
}

fn publish_reminder(
    write: &mut JournalWrite,
    body: TasteReminded,
    origin: wire::Origin,
    at: Instant,
) {
    let pending = write
        .fact(body.clone(), FactCause::Root)
        .expect("taste reminder builds");
    write.steering_head(pending, move |seq| {
        steer::Ledger::reminded_head(&body, origin, at, seq).expect("reminder head builds")
    });
}

fn seated(vendor: &str) -> LaneSeated {
    LaneSeated {
        lane: uuid(0x20),
        lineage_id: uuid(0x20),
        capability: uuid(0x2e),
        name: Some("Ava".to_owned()),
        worktree: "/worktrees/lane-a".to_owned(),
        life: "drive".to_owned(),
        vendor: vendor.parse().expect("fixture vendor"),
    }
}

fn reminded(
    key: &str,
    taste: &str,
    occupant: CanonicalUuid,
    prose: &str,
    compaction: u64,
) -> TasteReminded {
    TasteReminded {
        directive: directive(key),
        key: key.to_owned(),
        lane: uuid(0x20),
        taste: taste.to_owned(),
        content: Digest::compute(Domain::TasteProse, prose.as_bytes()),
        occupant,
        compaction,
        locus: locus(compaction as u32),
        text: format!("[TASTE] {taste}"),
        expires: "2031-01-01T00:00:00.000Z".parse().expect("fixture expiry"),
    }
}

/// A distinct durable identity per key, so no two publications collide.
fn directive(key: &str) -> CanonicalUuid {
    let suffix = key.bytes().fold(0_u64, |value, byte| {
        value.wrapping_mul(131).wrapping_add(u64::from(byte))
    });
    format!("00000000-0000-4000-8000-{:012x}", suffix & 0xffff_ffff_ffff)
        .parse()
        .expect("fixture directive UUID")
}

fn uuid(tail: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{tail:02x}")
        .parse()
        .expect("fixture UUID")
}

fn locus(line: u32) -> TranscriptLocus {
    TranscriptLocus {
        source: None,
        member: 1,
        offset: 40,
        length: 80,
        event: 0,
        line,
        column: 1,
    }
}

fn mining_locus(source: u8, index: u32) -> TranscriptLocus {
    TranscriptLocus {
        source: Some(uuid(source)),
        member: 5,
        offset: u64::from(index) * 100,
        length: 90,
        event: index,
        line: index,
        column: 1,
    }
}

fn text_reply<T: DeserializeOwned>(output: &Output) -> T {
    assert!(
        output.status.success(),
        "command failed: {}",
        spoken(output)
    );
    wire::decode(&output.stdout).expect("text reply body is typed")
}

fn json_reply<T: DeserializeOwned>(output: &Output) -> wire::Reply<T> {
    assert!(
        output.status.success(),
        "command failed: {}",
        spoken(output)
    );
    let Outcome::Reply(reply) =
        wire::decode::<Outcome<T>>(&output.stdout).expect("JSON outcome is typed")
    else {
        panic!("JSON command did not reply")
    };
    reply
}

fn kind_count(snapshot: &journal::FactSnapshot, kind: FactKind) -> usize {
    snapshot
        .facts()
        .iter()
        .filter(|fact| fact.fact.kind() == kind)
        .count()
}

fn task_add_requests(fixture: &Fixture) -> Vec<TaskAdd> {
    let connection = rusqlite::Connection::open(&fixture.journal).expect("receipt store opens");
    let mut statement = connection
        .prepare(concat!(
            "SELECT request FROM command_receipts ",
            "WHERE command_kind = 'task::add' ORDER BY first_seq"
        ))
        .expect("receipt query prepares");
    let rows = statement
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .expect("receipt query runs");
    rows.map(|row| {
        let request = row.expect("receipt row reads");
        wire::decode::<wire::Command<TaskAdd>>(&request)
            .expect("task-add receipt decodes")
            .body
    })
    .collect()
}

fn spoken(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
