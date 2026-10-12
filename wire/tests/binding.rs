//! The body-and-answer binding carried by a command kind.
//!
//! WIRE states that a command's kind selects the body DTO, witness schema,
//! transition, and reply DTO together. The witness half is proven next door in
//! `registry.rs`; this file proves the other two. Exhaustiveness itself is
//! enforced by a match and cannot be asserted from a test, so what is asserted
//! here is that every kind resolves to exactly one body and one answer, that
//! every declared shape is actually reachable and decodable, that a body
//! spelled for the wrong kind is refused rather than truncated, and that each
//! WIRE spelling re-encodes unchanged.

use std::collections::BTreeSet;

use wire::intent::{
    AnswerShape, BoardStatement, BoardTask, CommandAnswer, Intent, Positioned, ProjectVisibility,
    TaskDetailAnswer, TaskDetailClosure, TaskDetailNote, TaskGuidance, TaskRelationEdge,
    TaskRelationFrontier, TaskRelationKind, TaskRelationNode, TaskRelationNodeRef,
    TaskRelationTruncation, TaskRelationsAnswer, TaskTimelineAnswer, TaskTimelineEntry,
    TaskTimelineEvent,
};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, ClaimHead, CommandKind, Instant, LaneResidency,
    LeaseHead, Phase, ResidencyHead, ResidentState, Seq, TaskHandle, TeamSubscription,
    WorkflowHead,
};

/// Every command body, spelled as WIRE spells it, paired with the kind whose
/// binding must decode it.
const BODIES: &[(CommandKind, &str)] = &[
    (CommandKind::BoardRead, r#"{ "scope": "all" }"#),
    (
        CommandKind::ReportRead,
        r#"{"version":1,"upper":42,"kinds":["task::phased"]}"#,
    ),
    (CommandKind::TaskDetail, r#"{ "task": "1kKRRsDP" }"#),
    (
        CommandKind::TaskRelations,
        r#"{
  "roots": ["1kKRRsDP"],
  "relations": ["dependency", "review"],
  "depth": 2,
  "node_limit": 32
}"#,
    ),
    (
        CommandKind::TaskTimeline,
        r#"{
  "lanes": ["6f5ae476-efa6-471b-9bed-0f6994c3dcfa"],
  "before": 42,
  "limit": 50
}"#,
    ),
    (CommandKind::BoardGraph, r#"{ "view": "dependencies" }"#),
    (CommandKind::BoardHandout, r#"{ "output": "-" }"#),
    (CommandKind::DocLedger, r#"{ "task": "SPICE-1kKRRsDP" }"#),
    (CommandKind::GlobalSettingsRead, r#"{}"#),
    (CommandKind::GlobalSettingsSet, r#"{ "fast_mode": true }"#),
    (CommandKind::VendorCatalog, r#"{}"#),
    (
        CommandKind::TaskAdd,
        r#"{
  "title": "Bind every command kind to its body and reply DTO",
  "project": "wire",
  "acceptance": ["every kind names one body and one reply"],
  "origin": "task:WIRE-parent",
  "depends": ["SPICE-1kKRRsDP"]
}"#,
    ),
    (
        CommandKind::TaskOopsAdd,
        r#"{
  "title": "Create one system-owned hidden task",
  "description": "The server fixes this route to .oops"
}"#,
    ),
    (
        CommandKind::TasteProposalAdd,
        r#"{
  "finding": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "occurrence": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  "recurrence": 3,
  "title": "Review one mined taste candidate",
  "description": "Finding identity: aaaa",
  "acceptance": ["operator reviews cited evidence"],
  "origin": "ack:1kMine01",
  "depends": ["TASTE-1kKRRsDP"]
}"#,
    ),
    (
        CommandKind::TaskEdit,
        r#"{ "task": "SPICE-1kKRRsDP", "title": "a replacement statement" }"#,
    ),
    (
        CommandKind::TaskDelete,
        r#"{ "task": "SPICE-1kKRRsDP", "reason": "filed against the wrong surface" }"#,
    ),
    (
        CommandKind::TaskClose,
        r#"{ "task": "SPICE-1kKRRsDP", "validation": "verified in the browser", "force_claimed": true }"#,
    ),
    (
        CommandKind::TaskNote,
        r#"{ "task": "SPICE-1kKRRsDP", "note": "the registry row is exhaustive" }"#,
    ),
    (
        CommandKind::TaskDefer,
        r#"{ "task": "SPICE-1kKRRsDP", "wait": "2026-07-31T05:49:09.000Z" }"#,
    ),
    (CommandKind::TaskWake, r#"{ "task": "SPICE-1kKRRsDP" }"#),
    (
        CommandKind::TaskDepends,
        r#"{ "task": "SPICE-1kKRRsDP", "after": ["SPICE-1kKRRsDQ"], "remove": true }"#,
    ),
    (
        CommandKind::TaskPhase,
        r#"{ "task": "SPICE-1kKRRsDP", "phase": "verify" }"#,
    ),
    (
        CommandKind::TaskDone,
        r#"{ "task": "SPICE-1kKRRsDP", "validation": "cargo test --workspace" }"#,
    ),
    (
        CommandKind::TaskReview,
        r#"{ "task": "SPICE-1kKRRsDP", "finding": "accept" }"#,
    ),
    (
        CommandKind::TaskAssign,
        r#"{
  "task": "SPICE-1kKRRsDP",
  "reviewer": "lane:018f82d4-6f59-7d06-92e3-bce9131163c2"
}"#,
    ),
    (CommandKind::ClaimWin, r#"{ "rule": "next" }"#),
    (
        CommandKind::ClaimCarry,
        r#"{
  "task": "SPICE-1kKRRsDP",
  "successor": "lane:018f82d4-6f59-7d06-92e3-bce9131163c2"
}"#,
    ),
    (
        CommandKind::ClaimRenew,
        r#"{ "task": "SPICE-1kKRRsDP", "lease_seconds": 900 }"#,
    ),
    (CommandKind::ClaimFree, r#"{ "task": "SPICE-1kKRRsDP" }"#),
    (CommandKind::ClaimLapse, r#"{ "task": "SPICE-1kKRRsDP" }"#),
    (
        CommandKind::DocApply,
        r##"{
  "preview": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "document": "# Task\n"
}"##,
    ),
    (
        CommandKind::SteerSend,
        r##"{
  "fanout": "018f82d4-6f59-7d06-92e3-bce9131163c1",
  "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
  "message": "drain the queue"
}"##,
    ),
    (CommandKind::SteerInbox, r#"{}"#),
    (
        CommandKind::QuoteRead,
        r#"{ "target": "spice://quote/v1/steer/1kQnrzNZ/6ac2a419aad5b1cb8a9b632aed2429f5f7bd479de18dd1bb1bb031643450b335" }"#,
    ),
    (
        CommandKind::SteerHistory,
        r#"{
  "window": { "kind": "anchor", "identity": "018f82d4-6f59-7d06-92e3-bce9131163c4" },
  "query": "parser",
  "lanes": ["018f82d4-6f59-7d06-92e3-bce9131163c2"],
  "dispositions": ["live", "canceled"],
  "authored_before": "2026-08-01T06:07:19.000Z",
  "limit": 20
}"#,
    ),
    (
        CommandKind::SteerResend,
        r#"{ "directive": "018f82d4-6f59-7d06-92e3-bce9131163c4" }"#,
    ),
    (
        CommandKind::SteerRecover,
        r#"{ "directive": "018f82d4-6f59-7d06-92e3-bce9131163c4" }"#,
    ),
    (
        CommandKind::SteerCancel,
        r#"{ "directive": "018f82d4-6f59-7d06-92e3-bce9131163c4" }"#,
    ),
    (
        CommandKind::SteerDispose,
        r#"{
  "token": "1kJJP87B",
  "disposition": "acked"
}"#,
    ),
    (
        CommandKind::ConsentDecide,
        r#"{ "request": "018f82d4-6f59-7d06-92e3-bce9131163c4", "answer": "asked" }"#,
    ),
    (CommandKind::ConsentRead, r#"{}"#),
    (CommandKind::ConsentPolicy, r#"{}"#),
    (
        CommandKind::TrustPend,
        r#"{ "digest": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" }"#,
    ),
    (
        CommandKind::TrustGrant,
        r#"{ "digest": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" }"#,
    ),
    (
        CommandKind::TrustRevoke,
        r#"{ "digest": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" }"#,
    ),
    (CommandKind::LaneRead, r#"{}"#),
    (
        CommandKind::LaneSeat,
        r#"{
  "topology_revision": 9,
  "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
  "team": "018f82d4-6f59-7d06-92e3-bce9131163c2",
  "worktree": "worktrees/h",
  "life": "drive",
  "vendor": "omp"
}"#,
    ),
    (
        CommandKind::LaneState,
        r#"{
  "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
  "action": "wake",
  "worktree": "/worktrees/successor"
}"#,
    ),
    (
        CommandKind::LaneRenewal,
        r#"{
  "topology_revision": 9,
  "team": "018f82d4-6f59-7d06-92e3-bce9131163c2",
  "lane": "018f82d4-6f59-7d06-92e3-bce9131163c3",
  "requested": true
}"#,
    ),
    (
        CommandKind::LaneRetune,
        r#"{
  "topology_revision": 9,
  "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
  "model": "claude-opus-5-5"
}"#,
    ),
    (
        CommandKind::LaneEffortRetune,
        r#"{
  "topology_revision": 9,
  "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
  "effort": "high"
}"#,
    ),
    (
        CommandKind::LaneFastRetune,
        r#"{
  "topology_revision": 9,
  "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
  "fast_mode": true
}"#,
    ),
    (
        CommandKind::LaneArrange,
        r#"{
  "topology_revision": 9,
  "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
  "before": "018f82d4-6f59-7d06-92e3-bce9131163c3"
}"#,
    ),
    (
        CommandKind::TeamConfigure,
        r#"{
  "topology_revision": 9,
  "team": "018f82d4-6f59-7d06-92e3-bce9131163c2",
  "life": "drain"
}"#,
    ),
    (
        CommandKind::TeamArrange,
        r#"{
  "topology_revision": 9,
  "teams": [
    {
      "team": "018f82d4-6f59-7d06-92e3-bce9131163c2",
      "lanes": [
        "018f82d4-6f59-7d06-92e3-bce9131163c3",
        "018f82d4-6f59-7d06-92e3-bce9131163c4"
      ]
    },
    {
      "team": "018f82d4-6f59-7d06-92e3-bce9131163c5",
      "lanes": ["018f82d4-6f59-7d06-92e3-bce9131163c6"]
    }
  ]
}"#,
    ),
    (
        CommandKind::LaneRecall,
        r#"{ "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2" }"#,
    ),
    (
        CommandKind::TasteRecord,
        r#"{
  "name": "polling",
  "digest": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
  "locus": "wire/src/intent.rs:1"
}"#,
    ),
    (
        CommandKind::CanaryRecognize,
        r#"{ "key": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd" }"#,
    ),
    (
        CommandKind::VendorRecognize,
        r#"{ "key": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd" }"#,
    ),
    (
        CommandKind::CommitRecognize,
        r#"{ "key": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd" }"#,
    ),
    (
        CommandKind::ProofRecognize,
        r#"{ "key": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd" }"#,
    ),
    (
        CommandKind::ReleasePublish,
        r#"{
  "key": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
  "manifest": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
  "notes": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
}"#,
    ),
    (CommandKind::UpChange, r#"{ "state": "quiescing" }"#),
];

#[test]
fn generic_hidden_creation_discriminant_stays_retired() {
    assert!(
        "task::system-add".parse::<CommandKind>().is_err(),
        "the retired generic hidden-creation discriminant still crosses the wire",
    );
}

#[path = "binding/answers.rs"]
mod answers;
use answers::ANSWERS;
#[test]
fn every_command_kind_has_a_body_spelling() {
    let spelled: BTreeSet<_> = BODIES.iter().map(|(kind, _)| *kind).collect();
    for kind in CommandKind::ALL {
        assert!(spelled.contains(kind), "{kind} has no body fixture");
    }
    assert_eq!(
        spelled.len(),
        BODIES.len(),
        "a kind is spelled twice in the fixtures",
    );
}

#[test]
fn every_body_decodes_as_the_shape_its_kind_binds() {
    for (kind, body) in BODIES {
        let shape = kind.binding().body;
        let intent = shape
            .decode(body.as_bytes())
            .unwrap_or_else(|error| panic!("{kind} body does not decode as {shape:?}: {error}"));
        assert_eq!(
            intent.shape(),
            shape,
            "{kind} decoded into a shape its binding does not name",
        );
    }
}

#[test]
fn every_body_re_encodes_unchanged() {
    for (kind, body) in BODIES {
        let intent = kind
            .binding()
            .body
            .decode(body.as_bytes())
            .expect("the fixture decodes");
        let spelled: serde_json::Value =
            serde_json::from_str(body).expect("the fixture is well-formed JSON");
        let round_tripped = serde_json::to_value(&intent).expect("an intent encodes");
        assert_eq!(
            round_tripped, spelled,
            "{kind} did not re-encode to the spelling it was decoded from",
        );
    }
}

#[test]
fn every_answer_decodes_as_the_shape_that_names_its_dto() {
    for (shape, body) in ANSWERS {
        let answer = shape
            .decode(body.as_bytes())
            .unwrap_or_else(|error| panic!("{shape:?} answer does not decode: {error}"));
        assert_eq!(answer.shape(), *shape);
    }
}

#[test]
fn every_answer_re_encodes_unchanged() {
    for (shape, body) in ANSWERS {
        let answer = shape.decode(body.as_bytes()).expect("the fixture decodes");
        let spelled: serde_json::Value =
            serde_json::from_str(body).expect("the fixture is well-formed JSON");
        let round_tripped = serde_json::to_value(&answer).expect("an answer encodes");
        assert_eq!(
            round_tripped, spelled,
            "{shape:?} did not re-encode to the spelling it was decoded from",
        );
    }
}

#[test]
fn every_answer_decodes_alike_from_its_bytes_and_its_parsed_value() {
    for (shape, body) in ANSWERS {
        let from_bytes = shape.decode(body.as_bytes()).expect("the fixture decodes");
        let value: serde_json::Value =
            serde_json::from_str(body).expect("the fixture is well-formed JSON");
        let from_value = shape
            .decode_value(value)
            .unwrap_or_else(|error| panic!("{shape:?} parsed answer does not decode: {error}"));
        assert_eq!(
            from_value, from_bytes,
            "{shape:?} decodes differently from a parsed value"
        );
    }
}

#[test]
fn task_learning_is_only_an_optional_retained_answer_shape() {
    let retained = r#"{
        "task": "SPICE-1kKRRsDP",
        "status": "done",
        "learning": {
            "stored": 1,
            "candidates": 2,
            "skipped": ["rejected"]
        }
    }"#;
    let answer = AnswerShape::Task
        .decode(retained.as_bytes())
        .expect("retained task learning decodes");
    let CommandAnswer::Task(answer) = &answer else {
        panic!("task answer decoded as another shape: {answer:?}");
    };
    let learning = answer
        .learning
        .as_ref()
        .expect("retained answer carries its compatibility report");
    assert_eq!(learning.stored, 1);
    assert_eq!(learning.candidates, 2);
    assert_eq!(learning.skipped, ["rejected"]);
    assert_eq!(
        serde_json::to_value(answer).expect("retained answer re-encodes"),
        serde_json::from_str::<serde_json::Value>(retained).expect("retained fixture JSON")
    );

    let current = r#"{"task":"SPICE-1kKRRsDP","status":"done"}"#;
    let current = AnswerShape::Task
        .decode(current.as_bytes())
        .expect("current task answer decodes");
    let CommandAnswer::Task(current) = current else {
        panic!("current task answer decoded as another shape: {current:?}");
    };
    assert!(current.learning.is_none());
    assert!(
        !serde_json::to_value(current)
            .expect("current task answer encodes")
            .as_object()
            .expect("task answer encodes as an object")
            .contains_key("learning")
    );
}

/// The single board task row spelled in [`ANSWERS`], as parsed JSON.
fn spelled_board_task() -> serde_json::Value {
    let (_, body) = ANSWERS
        .iter()
        .find(|(shape, _)| *shape == AnswerShape::Board)
        .expect("the answer table spells a board body");
    let spelled: serde_json::Value =
        serde_json::from_str(body).expect("the board fixture is well-formed JSON");
    let mut tasks = spelled["tasks"]
        .as_array()
        .expect("the board body carries a task array")
        .clone();
    assert_eq!(tasks.len(), 1, "the board fixture spells exactly one task");
    tasks.remove(0)
}

#[test]
fn task_detail_answer_round_trips_attributed_note_identity() {
    let task: BoardTask =
        serde_json::from_value(spelled_board_task()).expect("board task fixture decodes");
    let expected = TaskDetailAnswer {
        task,
        media: vec![
            wire::Media::from_base64("capture.png", "image/png", "iVBORw0KGgo=")
                .expect("fixture media admits"),
        ],
        notes: vec![TaskDetailNote {
            position: seq(7),
            at: instant("2026-08-01T06:47:19.000Z"),
            author: Actor::Operator,
            text: "One durable observation.".to_owned(),
        }],
        closure: Some(TaskDetailClosure {
            position: seq(9),
            at: instant("2026-08-01T06:52:03.000Z"),
            author: Actor::Operator,
            validation: "Verified in the browser.".to_owned(),
            phase: "todo".parse().expect("fixture phase"),
            claimed: Some(Actor::Lane(
                "018f82d4-6f59-7d06-92e3-bce913116411"
                    .parse()
                    .expect("fixture lane"),
            )),
        }),
    };
    let bytes = wire::encode(&expected).expect("task detail answer encodes");
    let CommandAnswer::TaskDetail(actual) = AnswerShape::TaskDetail
        .decode(&bytes)
        .expect("task detail answer decodes through its registered shape")
    else {
        panic!("task detail shape decoded another answer")
    };
    assert_eq!(*actual, expected);
}

#[test]
fn task_relations_answer_round_trips_discriminated_nodes_and_frontier() {
    let task: wire::TaskStamp = "1kKRRsDP".parse().expect("fixture task stamp");
    let dependency: wire::TaskStamp = "1kKRRsDQ".parse().expect("fixture dependency stamp");
    let edge = TaskRelationEdge {
        relation: TaskRelationKind::Dependency,
        from: TaskRelationNodeRef::Task { task: task.clone() },
        to: TaskRelationNodeRef::Task {
            task: dependency.clone(),
        },
    };
    let expected = TaskRelationsAnswer {
        snapshot: seq(7),
        roots: vec![task.clone()],
        nodes: vec![TaskRelationNode::Task {
            task,
            handle: "SPICE-1kKRRsDP".parse().expect("fixture task handle"),
            title: "Inspect bounded relationships".to_owned(),
            phase: "todo".parse().expect("fixture phase"),
            status: "pending".to_owned(),
        }],
        edges: Vec::new(),
        truncation: Some(TaskRelationTruncation {
            frontier: vec![TaskRelationFrontier { depth: 1, edge }],
            omitted: 2,
        }),
    };
    let bytes = wire::encode(&expected).expect("task relations answer encodes");
    let CommandAnswer::TaskRelations(actual) = AnswerShape::TaskRelations
        .decode(&bytes)
        .expect("task relations answer decodes through its registered shape")
    else {
        panic!("task relations shape decoded another answer")
    };
    assert_eq!(*actual, expected);
}

#[test]
fn task_timeline_answer_round_trips_payload_and_bare_events() {
    let lane: CanonicalUuid = "6f5ae476-efa6-471b-9bed-0f6994c3dcfa"
        .parse()
        .expect("fixture lane");
    let entry = |position, event| TaskTimelineEntry {
        position: seq(position),
        at: instant("2026-08-01T06:47:19.000Z"),
        lane,
        task: "1kKRRsDP".parse().expect("fixture task stamp"),
        handle: "SPICE-1kKRRsDP".parse().expect("fixture task handle"),
        title: "Page team transitions".to_owned(),
        event,
    };
    let expected = TaskTimelineAnswer {
        snapshot: seq(9),
        lanes: vec![lane],
        entries: vec![
            entry(
                9,
                TaskTimelineEvent::Phased {
                    phase: "review".parse().expect("fixture phase"),
                },
            ),
            entry(8, TaskTimelineEvent::Claimed {}),
        ],
        next: Some(seq(8)),
    };
    let bytes = wire::encode(&expected).expect("task timeline answer encodes");
    let CommandAnswer::TaskTimeline(actual) = AnswerShape::TaskTimeline
        .decode(&bytes)
        .expect("task timeline answer decodes through its registered shape")
    else {
        panic!("task timeline shape decoded another answer")
    };
    assert_eq!(*actual, expected);
}

/// Every path at which two encoded rows disagree, named so a failure is legible.
///
/// `assert_eq!` on two `serde_json::Value`s prints both rows whole and leaves
/// the reader to find the one key among dozens that moved. Naming the paths is
/// what makes this single guard a separate witness for each field it covers
/// rather than one undifferentiated failure standing in for all of them.
fn disagreeing_paths(left: &serde_json::Value, right: &serde_json::Value) -> Vec<String> {
    let mut found = Vec::new();
    collect_disagreements(left, right, String::new(), &mut found);
    found
}

fn collect_disagreements(
    left: &serde_json::Value,
    right: &serde_json::Value,
    path: String,
    into: &mut Vec<String>,
) {
    match (left, right) {
        (serde_json::Value::Object(left), serde_json::Value::Object(right)) => {
            let keys: BTreeSet<&String> = left.keys().chain(right.keys()).collect();
            for key in keys {
                let nested = format!("{path}/{key}");
                match (left.get(key), right.get(key)) {
                    (Some(left), Some(right)) => collect_disagreements(left, right, nested, into),
                    _ => into.push(nested),
                }
            }
        }
        (serde_json::Value::Array(left), serde_json::Value::Array(right))
            if left.len() == right.len() =>
        {
            for (index, (left, right)) in left.iter().zip(right).enumerate() {
                collect_disagreements(left, right, format!("{path}/{index}"), into);
            }
        }
        _ if left != right => into.push(path),
        _ => {}
    }
}

fn seq(position: u64) -> Seq {
    Seq::new(position).expect("a fact position")
}

fn handle(spelled: &str) -> TaskHandle {
    spelled.parse().expect("a task handle")
}

fn uuid(spelled: &str) -> CanonicalUuid {
    spelled.parse().expect("a canonical uuid")
}

fn actor(spelled: &str) -> Actor {
    spelled.parse().expect("an actor")
}

fn phase(spelled: &str) -> Phase {
    spelled.parse().expect("a phase")
}

fn instant(spelled: &str) -> Instant {
    spelled.parse().expect("an instant")
}

/// The board task row, built field by field, must encode to the spelled row.
///
/// This is the guard the JSON table cannot be: a struct literal naming every
/// field is exhaustive at compile time, so a field added to [`BoardTask`] or to
/// anything it reaches fails this crate's build whether it is optional or
/// required. Asserting the literal encodes to the spelling in [`ANSWERS`] then
/// pins the two together in the other direction, which is what a round trip
/// cannot do: an optional deleted from the JSON decodes to `None` and
/// re-encodes skipped, so the round trip compares two equally diminished
/// values and passes, while this comparison sees the key the literal supplies
/// and the JSON no longer does.
#[test]
fn the_board_task_row_is_the_struct_this_crate_declares() {
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce9131163c3");
    let task = BoardTask {
        visibility: ProjectVisibility::Public,
        origin_thread: "operator".to_owned(),
        creator_team: None,
        presentation_title: "Bind a board task".to_owned(),
        statement: Positioned {
            position: seq(2),
            body: BoardStatement {
                preparation: None,
                intent: None,
                handle: handle("SPICE-1kKRRsDP"),
                born: "1kKRRsDP".parse().expect("a task stamp"),
                queue_origin: seq(2),
                description: Some(
                    "One task spelled through every field the shape carries.".to_owned(),
                ),
                acceptance: vec![
                    "the row decodes".to_owned(),
                    "the row re-encodes unchanged".to_owned(),
                ],
                requirements: wire::EnvironmentRequirements::default(),
                project: "wire.binding".to_owned(),
                tags: vec!["binding".to_owned()],
                dependencies: vec!["1kKRRsDQ".parse().expect("a task stamp")],
                provenance: "task:1kKRRsDP".parse().expect("an origin"),
                creation: wire::body::TaskCreationContext::unavailable(
                    wire::body::TaskCreationUnavailable::OperatorActor,
                ),
                revision: seq(2),
            },
        },
        workflow: Positioned {
            position: seq(3),
            body: WorkflowHead {
                flow: vec![phase("todo"), phase("review")],
                phase: phase("todo"),
                status: "pending".to_owned(),
                review: "unassigned".to_owned(),
                assignment: Some(actor("operator")),
                deferral: Some(instant("2026-08-01T07:07:19.000Z")),
                completion: Some(instant("2026-08-01T08:07:19.000Z")),
                generation: 0,
            },
        },
        claim: Some(Positioned {
            position: seq(4),
            body: ClaimHead {
                actor: actor("operator"),
                fact: seq(4),
                claimed: seq(4),
                cell: wire::ClaimCell {
                    project: "wire.binding".to_owned(),
                    phase: phase("todo"),
                },
                lane: uuid("018f82d4-6f59-7d06-92e3-bce9131163c5"),
                occupant: uuid("018f82d4-6f59-7d06-92e3-bce9131163c6"),
                epoch,
                recovery: None,
                disposition: ClaimDisposition::Active,
                acknowledged: 0,
            },
        }),
        lease: Some(Positioned {
            position: seq(5),
            body: LeaseHead {
                start: instant("2026-08-01T06:07:19.000Z"),
                renewal: instant("2026-08-01T06:37:19.000Z"),
                maximum: instant("2026-08-01T09:07:19.000Z"),
                expiry: instant("2026-08-01T07:07:19.000Z"),
                epoch,
            },
        }),
        explicit_release: false,
        claim_context: Some(wire::intent::TaskClaimContext {
            claimed: seq(4),
            lane: uuid("018f82d4-6f59-7d06-92e3-bce9131163c5"),
            occupant: uuid("018f82d4-6f59-7d06-92e3-bce9131163c6"),
            epoch,
            source: wire::body::SourceBoundary::Proven {
                heads: vec![wire::TranscriptSourceHead {
                    lane: uuid("018f82d4-6f59-7d06-92e3-bce9131163c5"),
                    vendor: "omp".parse().expect("a vendor"),
                    stream: "session".to_owned(),
                    source: uuid("018f82d4-6f59-7d06-92e3-bce9131163c7"),
                    next: wire::body::TranscriptPosition {
                        member: 3,
                        offset: 44,
                    },
                }],
            },
            recovery: wire::intent::TaskTranscriptRecovery::Transcript {
                session: "thread-123".to_owned(),
            },
        }),
        git_context: Some(wire::intent::TaskGitContext {
            fact: seq(6),
            kind: wire::intent::TaskGitContextKind::Completion,
            boundary: wire::body::GitBoundary {
                head: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
                integrated: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
                upstream: Some("origin/main".to_owned()),
                upstream_head: Some("cccccccccccccccccccccccccccccccccccccccc".to_owned()),
                local_commits: 2,
            },
        }),
        parked_commits: None,
    };

    let encoded = serde_json::to_value(&task).expect("the constructed board task encodes");
    let spelled = spelled_board_task();
    assert_eq!(
        encoded,
        spelled,
        "the constructed board task and the row spelled in ANSWERS disagree at {}",
        disagreeing_paths(&encoded, &spelled).join(", "),
    );
}

#[test]
fn a_claim_overlap_is_visible_while_the_legacy_empty_shape_stays_stable() {
    let body = r#"{
  "task": "FIDELIT-1kKRSfyV",
  "holder": "operator",
  "warnings": [{
    "task": "MOSAIC-1kKQTw1V",
    "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
    "surface": ["mosaic", "watchband"]
  }]
}"#;
    let answer = AnswerShape::Claim
        .decode(body.as_bytes())
        .expect("claim warning decodes");
    let CommandAnswer::Claim(claim) = &answer else {
        panic!("claim binding returned another answer: {answer:?}");
    };
    assert_eq!(claim.warnings.len(), 1);
    assert_eq!(claim.warnings[0].task.as_str(), "MOSAIC-1kKQTw1V");
    assert_eq!(
        claim.warnings[0].lane.to_string(),
        "018f82d4-6f59-7d06-92e3-bce9131163c2"
    );
    assert_eq!(claim.warnings[0].surface, ["mosaic", "watchband"]);

    let spelled: serde_json::Value = serde_json::from_str(body).expect("fixture JSON");
    assert_eq!(
        serde_json::to_value(answer).expect("warning answer encodes"),
        spelled
    );
}

#[test]
fn the_answer_union_reports_the_shape_it_carries() {
    let answer = CommandAnswer::Taste(wire::intent::TasteAnswer {
        name: "polling".parse().expect("a name"),
        confirmed: true,
    });
    assert_eq!(answer.shape(), AnswerShape::Taste);
}

#[test]
fn a_body_spelled_for_another_kind_is_refused() {
    // `task::done` names a validation; the shape bound to `task::wake` names a
    // task alone. Bodies deny unknown fields, so the extra key is a refusal
    // rather than a silent truncation into a valid-looking wake.
    let done = r#"{ "task": "SPICE-1kKRRsDP", "validation": "cargo test --workspace" }"#;
    let shape = CommandKind::TaskWake.binding().body;
    assert!(
        shape.decode(done.as_bytes()).is_err(),
        "a done body decoded as the shape bound to task::wake",
    );

    // The reverse direction fails on the missing field rather than an extra.
    let bare = r#"{ "task": "SPICE-1kKRRsDP" }"#;
    let shape = CommandKind::TaskDone.binding().body;
    assert!(
        shape.decode(bare.as_bytes()).is_err(),
        "a body missing its required validation decoded as task::done",
    );
}

#[test]
fn release_publication_requires_an_explicit_current_or_historical_note_state() {
    let shape = CommandKind::ReleasePublish.binding().body;
    let missing = r#"{
        "key": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        "manifest": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
    }"#;
    assert!(
        shape.decode(missing.as_bytes()).is_err(),
        "unmigrated publication intent must not enter the current decoder"
    );

    let historical = r#"{
        "key": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        "manifest": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        "notes": null
    }"#;
    let Intent::ReleasePublish(publish) = shape
        .decode(historical.as_bytes())
        .expect("migrated historical absence decodes")
    else {
        panic!("release body decoded as another intent")
    };
    assert_eq!(publish.notes, wire::ReleaseNotesIdentity::HistoricalAbsent);
}

#[test]
fn team_lifetimes_close_over_creator_team_scope() {
    use wire::intent::TeamLife;

    let team = uuid("018f82d4-6f59-7d06-92e3-bce913116301");
    let peer = uuid("018f82d4-6f59-7d06-92e3-bce913116302");
    assert!(!TeamLife::Steer.admits_creator_team(Some(team), team));
    assert!(TeamLife::Drive.admits_creator_team(Some(team), team));
    assert!(!TeamLife::Drive.admits_creator_team(Some(peer), team));
    assert!(!TeamLife::Drive.admits_creator_team(None, team));
    assert!(TeamLife::Drain.admits_creator_team(None, team));
    for policy in [TeamLife::Steer, TeamLife::Drive, TeamLife::Drain] {
        assert_eq!(TeamLife::from_wire(policy.as_str()), Some(policy));
    }
}

/// One definition of teamed governs rendering, placement, and allocation.
///
/// A lane seated alone under its own identity is a defined agent, not a team:
/// it keeps its durable policy but allocates as steer until placement moves it
/// onto a team, where the configured lifetime resumes.
#[test]
fn a_lane_seated_alone_under_its_own_identity_is_not_on_a_team() {
    use wire::intent::TeamLife;

    let lane = uuid("018f82d4-6f59-7d06-92e3-bce913116301");
    let peer = uuid("018f82d4-6f59-7d06-92e3-bce913116302");
    let team = uuid("018f82d4-6f59-7d06-92e3-bce913116303");
    assert!(
        !wire::head::teamed(lane, [lane]),
        "a default seat is not a team"
    );
    assert!(
        !wire::head::teamed(lane, []),
        "a team without members is not one"
    );
    assert!(
        wire::head::teamed(team, [lane]),
        "one agent placed on a team is on it"
    );
    assert!(
        wire::head::teamed(lane, [lane, peer]),
        "two agents under either identity are a team"
    );
    assert!(wire::head::teamed(lane, [peer, lane]));

    for policy in [TeamLife::Steer, TeamLife::Drive, TeamLife::Drain] {
        let configured = TeamSubscription {
            team: lane,
            policy,
            fast_mode: false,
        };
        assert_eq!(configured.effective([lane]).policy, TeamLife::Steer);
        assert_eq!(configured.effective([lane, peer]).policy, policy);
        assert_eq!(
            TeamSubscription {
                team,
                policy,
                fast_mode: false,
            }
            .effective([lane])
            .policy,
            policy
        );
    }

    let mut residency = ResidencyHead {
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce913116310"),
        state: ResidentState::Seated,
        topology_revision: 0,
        lanes: vec![residency_row(lane, lane), residency_row(peer, peer)],
        subscriptions: Vec::new(),
        renewals: Vec::new(),
    };
    assert_eq!(residency.team_members(lane).collect::<Vec<_>>(), [lane]);
    residency.lanes[1].team = lane;
    assert_eq!(
        residency.team_members(lane).collect::<Vec<_>>(),
        [lane, peer]
    );
    assert_eq!(residency.team_members(peer).count(), 0);
}

fn residency_row(lane: CanonicalUuid, team: CanonicalUuid) -> LaneResidency {
    LaneResidency {
        lane,
        team,
        name: None,
        worktree: "/fixture".to_owned(),
        life: "drive".to_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        model: None,
        effort: None,
        fast_mode: false,
        occupant: None,
        lineage: Vec::new(),
        capabilities: Vec::new(),
        session: None,
        disposition: "vacant".to_owned(),
    }
}

#[test]
fn fast_mode_is_on_when_any_of_lane_team_or_global_enables_it() {
    let lane = uuid("018f82d4-6f59-7d06-92e3-bce913116301");
    let peer = uuid("018f82d4-6f59-7d06-92e3-bce913116302");
    let team = uuid("018f82d4-6f59-7d06-92e3-bce913116303");
    let other = uuid("018f82d4-6f59-7d06-92e3-bce913116304");
    let subscription = |team, fast_mode| TeamSubscription {
        team,
        policy: wire::intent::TeamLife::Drive,
        fast_mode,
    };
    for lane_flag in [false, true] {
        for team_flag in [false, true] {
            for global in [false, true] {
                let mut row = residency_row(lane, team);
                row.fast_mode = lane_flag;
                let residency = ResidencyHead {
                    epoch: uuid("018f82d4-6f59-7d06-92e3-bce913116310"),
                    state: ResidentState::Seated,
                    topology_revision: 0,
                    lanes: vec![row.clone(), residency_row(peer, other)],
                    subscriptions: vec![subscription(team, team_flag), subscription(other, true)],
                    renewals: Vec::new(),
                };
                assert_eq!(
                    residency.fast_mode(&row, global),
                    lane_flag || team_flag || global,
                    "lane={lane_flag} team={team_flag} global={global}"
                );
            }
        }
    }
}

#[test]
fn task_guidance_has_one_exact_actionable_spelling() {
    let guidance = TaskGuidance::PlanAdvanceOwnership;
    let encoded = wire::encode(&guidance).expect("task guidance encodes");
    assert_eq!(encoded, format!("\"{}\"", guidance.as_str()).as_bytes());
    assert_eq!(
        wire::decode::<TaskGuidance>(&encoded).expect("task guidance decodes"),
        guidance
    );
    assert!(
        wire::decode::<TaskGuidance>(br#""same-task-next-phase""#).is_err(),
        "task guidance must refuse promises outside its closed vocabulary"
    );
}
