#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bus::{Bus, CommitOutcome};
use journal::{FactCause, JournalWrite};
use serde::de::DeserializeOwned;
use wire::body::{TasteCleared, TasteConfirmed};
use wire::intent::{BoardAnswer, CommandAnswer, TaskAdd, TaskAnswer};
use wire::{Actor, FactKind, Instant, Outcome, Producer, ReplyPosition};

#[path = "support/readiness.rs"]
mod readiness;

use readiness::Readiness;

const READY: Duration = Duration::from_secs(10);
const REFUSAL_EXIT: i32 = 2;
const ADD_TITLE: &str = concat!(
    "Propose tracked taste addition: Repeated alternate execution branches hide the current ",
    "path"
);
static NEXT: AtomicU64 = AtomicU64::new(0);

const POLICY: &str = r#"
[tasks]
stems = ["taste"]

[taste.zeta]
words = ["wait*", "sleep*"]
message = "Observe the real completion event."

[taste.alpha]
words = ["alias*"]
message = "Keep one current name."
"#;

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
        fs::write(root.join("spice.toml"), POLICY).expect("tracked taste policy");
        let runtime = root.join(".git/.spice-rust");
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
            .args(["--json", "up"])
            .current_dir(&fixture.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("resident starts");
        let mut resident = Self { child: Some(child) };
        match readiness::wait_until_ready(
            resident.child.as_mut().expect("resident child"),
            &fixture.root.join(".git"),
            READY,
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

    let added = text_reply::<TaskAnswer>(&fixture.invoke(&[
        "taste",
        "add",
        "Repeated alternate execution branches hide the current path",
    ]));
    let dropped = json_reply::<TaskAnswer>(&fixture.invoke(&["--json", "taste", "drop", "alpha"]));
    assert!(matches!(dropped.position, ReplyPosition::Committed(_)));

    let before_unknown = fixture.cursor();
    let unknown = fixture.invoke(&["--json", "taste", "drop", "missing"]);
    assert_eq!(unknown.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) =
        wire::decode::<Outcome<serde_json::Value>>(&unknown.stderr).expect("refusal is typed")
    else {
        panic!("unknown taste did not refuse")
    };
    assert_eq!(refusal.reason, "taste::unknown");
    assert_eq!(fixture.cursor(), before_unknown);

    let current_board = board(&fixture);
    assert_review_plan(&current_board, &added, ADD_TITLE);
    assert_review_plan(
        &current_board,
        &dropped.body,
        "Propose tracked taste retirement: alpha",
    );

    resident.stop(&fixture);
    let restarted = Resident::start(&fixture);
    assert_eq!(
        text_reply::<serde_json::Value>(&fixture.invoke(&["taste"])),
        text
    );
    let restarted_board = board(&fixture);
    assert_review_plan(&restarted_board, &added, ADD_TITLE);
    assert_review_plan(
        &restarted_board,
        &dropped.body,
        "Propose tracked taste retirement: alpha",
    );
    restarted.stop(&fixture);

    let reader = journal::JournalReader::open(&fixture.journal).expect("final journal opens");
    let facts = reader.fact_snapshot().expect("final fact snapshot");
    assert_eq!(kind_count(&facts, FactKind::TaskAdded), 2);
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
    }));
}

fn assert_report(fixture: &Fixture, report: &serde_json::Value) {
    assert_eq!(report["provenance"]["source"], "tracked");
    assert_eq!(report["provenance"]["present"], true);
    assert_eq!(
        report["provenance"]["location"],
        fixture.root.join("spice.toml").display().to_string()
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
        serde_json::json!(["wait*", "sleep*"])
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

fn assert_review_plan(board: &BoardAnswer, answer: &TaskAnswer, title: &str) {
    let task = board
        .tasks
        .iter()
        .find(|task| task.statement.body.handle == answer.task)
        .expect("planned task is on the current board");
    assert_eq!(task.presentation_title(), title);
    assert_eq!(task.statement.body.project, "taste.policy");
    assert!(!task.visibility.is_hidden());
    assert_eq!(
        task.workflow
            .body
            .flow
            .iter()
            .map(wire::Phase::as_str)
            .collect::<Vec<_>>(),
        ["todo", "review"]
    );
    assert_eq!(task.workflow.body.phase.as_str(), "todo");
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
