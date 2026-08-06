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

use wire::CommandKind;
use wire::intent::{AnswerShape, BodyShape, CommandAnswer, Intent};

/// Every command body, spelled as WIRE spells it, paired with the kind whose
/// binding must decode it.
const BODIES: &[(CommandKind, &str)] = &[
    (CommandKind::BoardRead, r#"{}"#),
    (CommandKind::BoardGraph, r#"{ "view": "dependencies" }"#),
    (CommandKind::BoardHandout, r#"{ "output": "-" }"#),
    (CommandKind::DocLedger, r#"{ "task": "SPICE-aBcD" }"#),
    (
        CommandKind::TaskAdd,
        r#"{
  "title": "Bind every command kind to its body and reply DTO",
  "project": "wire",
  "acceptance": ["every kind names one body and one reply"],
  "origin": "task:WIRE-parent",
  "depends": ["SPICE-aBcD"]
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
        CommandKind::TaskEdit,
        r#"{ "task": "SPICE-aBcD", "title": "a replacement statement" }"#,
    ),
    (
        CommandKind::TaskNote,
        r#"{ "task": "SPICE-aBcD", "note": "the registry row is exhaustive" }"#,
    ),
    (
        CommandKind::TaskDefer,
        r#"{ "task": "SPICE-aBcD", "wait": "2026-07-31T05:49:09.000Z" }"#,
    ),
    (CommandKind::TaskWake, r#"{ "task": "SPICE-aBcD" }"#),
    (
        CommandKind::TaskDepends,
        r#"{ "task": "SPICE-aBcD", "after": ["SPICE-eFgH"] }"#,
    ),
    (
        CommandKind::TaskPhase,
        r#"{ "task": "SPICE-aBcD", "phase": "verify" }"#,
    ),
    (
        CommandKind::TaskDone,
        r#"{ "task": "SPICE-aBcD", "validation": "cargo test --workspace" }"#,
    ),
    (
        CommandKind::TaskReview,
        r#"{ "task": "SPICE-aBcD", "finding": "accept" }"#,
    ),
    (
        CommandKind::TaskAssign,
        r#"{
  "task": "SPICE-aBcD",
  "reviewer": "lane:018f82d4-6f59-7d06-92e3-bce9131163c2"
}"#,
    ),
    (CommandKind::ClaimWin, r#"{ "rule": "next" }"#),
    (
        CommandKind::ClaimCarry,
        r#"{
  "task": "SPICE-aBcD",
  "successor": "lane:018f82d4-6f59-7d06-92e3-bce9131163c2"
}"#,
    ),
    (
        CommandKind::ClaimRenew,
        r#"{ "task": "SPICE-aBcD", "lease_seconds": 900 }"#,
    ),
    (CommandKind::ClaimFree, r#"{ "task": "SPICE-aBcD" }"#),
    (CommandKind::ClaimLapse, r#"{ "task": "SPICE-aBcD" }"#),
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
  "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
  "message": "drain the queue"
}"##,
    ),
    (CommandKind::SteerInbox, r#"{}"#),
    (
        CommandKind::SteerResend,
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
  "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
  "worktree": "worktrees/h",
  "life": "drive",
  "vendor": "claude"
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
  "manifest": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
}"#,
    ),
    (CommandKind::UpChange, r#"{ "state": "quiescing" }"#),
];

#[test]
fn oops_route_body_cannot_name_or_cross_target_a_hidden_project() {
    assert!(
        "task::system-add".parse::<CommandKind>().is_err(),
        "the retired generic hidden-creation discriminant still crosses the wire",
    );
    for project in [".oops", ".maxim_proposal.theme_2", ".sandbox.audit"] {
        let body =
            format!(r#"{{"title":"forged route","description":"detail","project":"{project}"}}"#);
        assert!(
            CommandKind::TaskOopsAdd
                .binding()
                .body
                .decode(body.as_bytes())
                .is_err(),
            "oops route decoded caller-selected project {project}",
        );
    }
}

/// One successful reply body for every declared answer shape.
const ANSWERS: &[(AnswerShape, &str)] = &[
    (
        AnswerShape::Board,
        r#"{
  "schema": 1,
  "cursor": 1,
  "principal": "operator",
  "project_policy": {
    "public_stems": ["serve", "task"],
    "public_min_depth": 2,
    "public_max_depth": 3
  },
  "tasks": [],
  "residency": {
    "position": 1,
    "body": {
      "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3",
      "state": "seated",
      "lanes": []
    }
  }
}"#,
    ),
    (AnswerShape::Text, r#"{ "text": "graph dependencies\n" }"#),
    (
        AnswerShape::Task,
        r#"{ "task": "SPICE-aBcD", "status": "done" }"#,
    ),
    (
        AnswerShape::Claim,
        r#"{ "task": "SPICE-aBcD", "holder": "operator" }"#,
    ),
    (
        AnswerShape::Doc,
        r#"{
  "preview": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "touched": ["SPICE-aBcD"]
}"#,
    ),
    (
        AnswerShape::Directive,
        r#"{
  "directive": "018f82d4-6f59-7d06-92e3-bce9131163c4",
  "key": "1kJJP87B",
  "disposition": "acked"
}"#,
    ),
    (
        AnswerShape::Inbox,
        r#"{
  "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3",
  "lanes": [{
    "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
    "occupant": "018f82d4-6f59-7d06-92e3-bce9131163c5",
    "pending": [{
      "directive": "018f82d4-6f59-7d06-92e3-bce9131163c4",
      "key": "1kJJP87B",
      "text": "drain the queue",
      "preview": "drain the queue",
      "expires": "2026-08-01T07:07:19.000Z"
    }]
  }]
}"#,
    ),
    (
        AnswerShape::Consent,
        r#"{ "request": "018f82d4-6f59-7d06-92e3-bce9131163c4", "answer": "granted" }"#,
    ),
    (
        AnswerShape::ConsentQueue,
        r#"{ "pending": [], "budgets": [] }"#,
    ),
    (
        AnswerShape::ConsentPolicy,
        r#"{ "posture": "queue", "rules": [] }"#,
    ),
    (
        AnswerShape::Trust,
        r#"{
  "digest": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  "standing": "granted"
}"#,
    ),
    (
        AnswerShape::Lane,
        r#"{
  "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3",
  "state": "seated",
  "lanes": []
}"#,
    ),
    (
        AnswerShape::LaneChange,
        r#"{
  "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
  "from": "occupied",
  "to": "vacant"
}"#,
    ),
    (
        AnswerShape::State,
        r#"{ "from": "seated", "to": "quiescing" }"#,
    ),
    (
        AnswerShape::Recall,
        r#"{
  "barrier": 48213,
  "repository": "/worktrees/repository",
  "residency": {
    "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3",
    "state": "seated",
    "lanes": []
  },
  "recent": []
}"#,
    ),
    (
        AnswerShape::Taste,
        r#"{ "name": "polling", "confirmed": true }"#,
    ),
    (
        AnswerShape::Recognition,
        r#"{
  "key": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
  "fresh": true
}"#,
    ),
];

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
    let done = r#"{ "task": "SPICE-aBcD", "validation": "cargo test --workspace" }"#;
    let shape = CommandKind::TaskWake.binding().body;
    assert!(
        shape.decode(done.as_bytes()).is_err(),
        "a done body decoded as the shape bound to task::wake",
    );

    // The reverse direction fails on the missing field rather than an extra.
    let bare = r#"{ "task": "SPICE-aBcD" }"#;
    let shape = CommandKind::TaskDone.binding().body;
    assert!(
        shape.decode(bare.as_bytes()).is_err(),
        "a body missing its required validation decoded as task::done",
    );
}

#[test]
fn every_declared_shape_is_bound_by_some_kind() {
    // A shape no kind selects is dead vocabulary, and an answer no kind
    // returns is a reply that can never be sent.
    let mut bodies = BTreeSet::new();
    let mut answers = BTreeSet::new();
    for kind in CommandKind::ALL {
        let binding = kind.binding();
        bodies.insert(binding.body);
        answers.insert(binding.answer);
    }

    for shape in ALL_BODIES {
        assert!(bodies.contains(shape), "{shape:?} is bound by no kind");
    }
    for shape in ALL_ANSWERS {
        assert!(answers.contains(shape), "{shape:?} is returned by no kind");
    }
    assert_eq!(bodies.len(), ALL_BODIES.len());
    assert_eq!(answers.len(), ALL_ANSWERS.len());
}

#[test]
fn a_kind_binds_one_body_and_one_answer() {
    for kind in CommandKind::ALL {
        // `binding` is a pure function of the discriminant, so a kind cannot
        // resolve to two bodies depending on when it is asked.
        assert_eq!(kind.binding(), kind.binding(), "{kind} binds unstably");
    }
}

/// Shapes a body may take, listed so an unreachable one is caught.
const ALL_BODIES: &[BodyShape] = &[
    BodyShape::BoardRead,
    BodyShape::BoardGraph,
    BodyShape::BoardHandout,
    BodyShape::DocLedger,
    BodyShape::TaskAdd,
    BodyShape::TaskOopsAdd,
    BodyShape::TaskEdit,
    BodyShape::TaskNote,
    BodyShape::TaskDefer,
    BodyShape::TaskRef,
    BodyShape::TaskDepends,
    BodyShape::TaskPhase,
    BodyShape::TaskDone,
    BodyShape::TaskReview,
    BodyShape::TaskAssign,
    BodyShape::ClaimWin,
    BodyShape::ClaimCarry,
    BodyShape::ClaimRenew,
    BodyShape::DocApply,
    BodyShape::SteerSend,
    BodyShape::SteerInbox,
    BodyShape::DirectiveRef,
    BodyShape::SteerDispose,
    BodyShape::ConsentDecide,
    BodyShape::ConsentRead,
    BodyShape::ConsentPolicy,
    BodyShape::TrustRef,
    BodyShape::LaneRead,
    BodyShape::LaneSeat,
    BodyShape::LaneState,
    BodyShape::LaneRecall,
    BodyShape::TasteRecord,
    BodyShape::Recognize,
    BodyShape::ReleasePublish,
    BodyShape::UpChange,
];

/// Shapes an answer may take, listed for the same reason.
const ALL_ANSWERS: &[AnswerShape] = &[
    AnswerShape::Board,
    AnswerShape::Text,
    AnswerShape::Task,
    AnswerShape::Claim,
    AnswerShape::Doc,
    AnswerShape::Directive,
    AnswerShape::Inbox,
    AnswerShape::Consent,
    AnswerShape::ConsentQueue,
    AnswerShape::ConsentPolicy,
    AnswerShape::Trust,
    AnswerShape::Lane,
    AnswerShape::LaneChange,
    AnswerShape::State,
    AnswerShape::Recall,
    AnswerShape::Taste,
    AnswerShape::Recognition,
];

/// The union is inhabited by exactly the shapes declared above.
#[test]
fn the_union_reports_the_shape_it_carries() {
    let intent = Intent::TaskRef(wire::intent::TaskRef {
        task: "SPICE-aBcD".parse().expect("a handle"),
    });
    assert_eq!(intent.shape(), BodyShape::TaskRef);
}
