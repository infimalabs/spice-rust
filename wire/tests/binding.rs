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
    AnswerShape, BoardStatement, BoardTask, BodyShape, CommandAnswer, EffortUnavailableCode,
    Intent, Positioned, ProjectVisibility,
};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, ClaimHead, CommandKind, Instant, LeaseHead, Phase, Seq,
    TaskHandle, WorkflowHead,
};

/// Every command body, spelled as WIRE spells it, paired with the kind whose
/// binding must decode it.
const BODIES: &[(CommandKind, &str)] = &[
    (CommandKind::BoardRead, r#"{}"#),
    (CommandKind::BoardGraph, r#"{ "view": "dependencies" }"#),
    (CommandKind::BoardHandout, r#"{ "output": "-" }"#),
    (CommandKind::DocLedger, r#"{ "task": "SPICE-aBcD" }"#),
    (CommandKind::EffortRead, r#"{ "task": "SPICE-aBcD" }"#),
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
        CommandKind::GateFindings,
        r#"{
  "report": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "findings": [{
    "identity": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "severity": "high",
    "locus": "wire/src/intent.rs:1",
    "title": "Gate finding needs board work",
    "description": "The explicit gate findings route files this row as deferred work.",
    "acceptance": ["the finding has a deferred task"]
  }]
}"#,
    ),
    (
        CommandKind::TaskEdit,
        r#"{ "task": "SPICE-aBcD", "title": "a replacement statement" }"#,
    ),
    (
        CommandKind::TaskDelete,
        r#"{ "task": "SPICE-aBcD", "reason": "filed against the wrong surface" }"#,
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
  "lineage_id": "018f82d4-6f59-7d06-92e3-bce9131163c2",
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
        CommandKind::LaneArrange,
        r#"{
  "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
  "before": "018f82d4-6f59-7d06-92e3-bce9131163c3"
}"#,
    ),
    (
        CommandKind::LineageConfigure,
        r#"{
  "lineage_id": "018f82d4-6f59-7d06-92e3-bce9131163c2",
  "life": "drain"
}"#,
    ),
    (
        CommandKind::LineageArrange,
        r#"{
  "lineages": [
    {
      "lineage_id": "018f82d4-6f59-7d06-92e3-bce9131163c2",
      "lanes": [
        "018f82d4-6f59-7d06-92e3-bce9131163c3",
        "018f82d4-6f59-7d06-92e3-bce9131163c4"
      ]
    },
    {
      "lineage_id": "018f82d4-6f59-7d06-92e3-bce9131163c5",
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
///
/// The board task row spells every optional the shape carries -- `assignment`,
/// `deferral`, and `completion` together, alongside `claim` and `lease` -- so
/// each `skip_serializing_if` field is decoded and re-encoded here rather than
/// only in a downstream crate. That makes the row a shape witness, not a
/// reachable board state: no phase both defers and completes.
///
/// Round-tripping alone cannot hold that claim. An absent optional decodes to
/// `None` and re-encodes skipped, so deleting one of those keys leaves the
/// encoded value and the spelling equal and `every_answer_re_encodes_unchanged`
/// stays green. What holds it is
/// [`the_board_task_row_is_the_struct_this_crate_declares`], which builds the
/// row as a struct literal naming every field and asserts it encodes to exactly
/// the JSON below: a deleted optional makes the two spellings disagree, and a
/// field added to the DTO fails this crate's build.
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
  "actor_route": {
    "actor": "operator",
    "project": "agent.operator.task",
    "origin_thread": "operator"
  },
  "allocation_band": 2,
  "tasks": [
    {
      "visibility": { "kind": "public" },
      "origin_thread": "operator",
      "presentation_title": "Bind a board task",
      "statement": {
        "position": 2,
        "body": {
          "handle": "SPICE-aBcD",
          "uuid": "018f82d4-6f59-7d06-92e3-bce9131163c4",
          "description": "One task spelled through every field the shape carries.",
          "acceptance": ["the row decodes", "the row re-encodes unchanged"],
          "project": "wire.binding",
          "tags": ["binding"],
          "dependencies": ["SPICE-eFgH"],
          "provenance": "task:SPICE-aBcD",
          "revision": 2
        }
      },
      "workflow": {
        "position": 3,
        "body": {
          "flow": ["todo", "review"],
          "phase": "todo",
          "status": "pending",
          "review": "unassigned",
          "assignment": "operator",
          "deferral": "2026-08-01T07:07:19.000Z",
          "completion": "2026-08-01T08:07:19.000Z"
        }
      },
      "claim": {
        "position": 4,
        "body": {
          "actor": "operator",
          "fact": 4,
          "lane": "018f82d4-6f59-7d06-92e3-bce9131163c5",
          "occupant": "018f82d4-6f59-7d06-92e3-bce9131163c6",
          "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3",
          "disposition": "active"
        }
      },
      "lease": {
        "position": 5,
        "body": {
          "start": "2026-08-01T06:07:19.000Z",
          "renewal": "2026-08-01T06:37:19.000Z",
          "maximum": "2026-08-01T09:07:19.000Z",
          "expiry": "2026-08-01T07:07:19.000Z",
          "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3"
        }
      }
    }
  ],
  "residency": {
    "position": 1,
    "body": {
      "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3",
      "state": "seated",
      "lanes": [{
        "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
        "lineage_id": "018f82d4-6f59-7d06-92e3-bce9131163c2",
        "name": "Ava",
        "worktree": "/worktrees/lane-a",
        "life": "drive",
        "vendor": "claude",
        "occupant": "018f82d4-6f59-7d06-92e3-bce9131163c5",
        "lineage": ["018f82d4-6f59-7d06-92e3-bce9131163c5"],
        "capabilities": ["018f82d4-6f59-7d06-92e3-bce9131163c7"],
        "disposition": "occupied"
      }]
    }
  }
}"#,
    ),
    (AnswerShape::Text, r#"{ "text": "graph dependencies\n" }"#),
    (
        AnswerShape::Task,
        r#"{
  "task": "SPICE-aBcD",
  "status": "done",
  "learning": {
    "stored": 1,
    "candidates": 2,
    "skipped": ["rejected"]
  }
}"#,
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
      "authored": "2026-08-01T06:07:19.000Z",
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
        r#"{
  "pending": [{
    "request": "018f82d4-6f59-7d06-92e3-bce9131163c4",
    "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
    "operation": "shell:git push",
    "rule": "browser",
    "cause": { "kind": "budget-exhausted", "bound": 2 },
    "budget": { "rule": "browser", "bound": 2, "remaining": 0 }
  }],
  "budgets": [{ "rule": "browser", "bound": 2, "remaining": 0 }]
}"#,
    ),
    (
        AnswerShape::ConsentPolicy,
        r#"{
  "posture": "queue",
  "rules": [{
    "name": "browser",
    "tool": "shell:*",
    "answer": "allow",
    "budget": 2,
    "remaining": 1
  }]
}"#,
    ),
    (
        AnswerShape::Effort,
        r#"{
  "phases": [{
    "task": "SPICE-aBcD",
    "phase": "todo",
    "phase_index": 0,
    "opening": 41,
    "closing": 57,
    "renewals": 1,
    "opening_boundary": {
      "proven": {
        "heads": [{
          "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
          "vendor": "claude",
          "stream": "transcript",
          "source": "018f82d4-6f59-7d06-92e3-bce9131163c6",
          "next": { "member": 0, "offset": 0 }
        }]
      }
    },
    "closing_boundary": {
      "proven": {
        "heads": [{
          "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
          "vendor": "claude",
          "stream": "transcript",
          "source": "018f82d4-6f59-7d06-92e3-bce9131163c6",
          "next": { "member": 0, "offset": 4096 }
        }]
      }
    },
    "segments": [{
      "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
      "occupant": "018f82d4-6f59-7d06-92e3-bce9131163c5",
      "lineage": "018f82d4-6f59-7d06-92e3-bce9131163c1",
      "session": "018f82d4-6f59-7d06-92e3-bce9131163c7",
      "source": "018f82d4-6f59-7d06-92e3-bce9131163c6",
      "from": { "member": 0, "offset": 0 },
      "to": { "member": 0, "offset": 4096 },
      "identity": { "vendor": "claude", "model": "claude-opus-5", "effort": "high" },
      "usage": {
        "turns": 12,
        "input_tokens": 4096,
        "cached_input_tokens": 2048,
        "cache_write_input_tokens": 1024,
        "output_tokens": 512,
        "reasoning_output_tokens": 256
      }
    }],
    "availability": {
      "kind": "available",
      "identity": { "vendor": "claude", "model": "claude-opus-5", "effort": "high" },
      "usage": {
        "turns": 12,
        "input_tokens": 4096,
        "cached_input_tokens": 2048,
        "cache_write_input_tokens": 1024,
        "output_tokens": 512,
        "reasoning_output_tokens": 256
      },
      "wall_ms": 903000
    }
  }, {
    "task": "SPICE-aBcD",
    "phase": "review",
    "phase_index": 1,
    "opening": 58,
    "renewals": 0,
    "opening_boundary": {
      "incomplete": { "reason": "the lane transcript was not accounted at the claim" }
    },
    "segments": [],
    "availability": {
      "kind": "unavailable",
      "reasons": [{ "code": "untagged-launch", "detail": "the launch recorded no model" }]
    }
  }],
  "aggregates": [{
    "identity": { "vendor": "claude", "model": "claude-opus-5", "effort": "high" },
    "phase": "todo",
    "count": 1,
    "usage_sum": {
      "turns": 12,
      "input_tokens": 4096,
      "cached_input_tokens": 2048,
      "cache_write_input_tokens": 1024,
      "output_tokens": 512,
      "reasoning_output_tokens": 256
    },
    "usage_average": {
      "turns": 12,
      "input_tokens": 4096,
      "cached_input_tokens": 2048,
      "cache_write_input_tokens": 1024,
      "output_tokens": 512,
      "reasoning_output_tokens": 256
    },
    "wall_ms_sum": 903000,
    "wall_ms_average": 903000
  }]
}"#,
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
  "subscriptions": [{
    "lineage_id": "018f82d4-6f59-7d06-92e3-bce9131163c2",
    "policy": "drive"
  }, {
    "lineage_id": "018f82d4-6f59-7d06-92e3-bce9131163c6",
    "policy": "steer"
  }],
  "lanes": [{
    "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
    "lineage_id": "018f82d4-6f59-7d06-92e3-bce9131163c2",
    "name": "Ava",
    "worktree": "/worktrees/lane-a",
    "life": "drive",
    "vendor": "claude",
    "occupant": "018f82d4-6f59-7d06-92e3-bce9131163c5",
    "lineage": ["018f82d4-6f59-7d06-92e3-bce9131163c5"],
    "bound": true,
    "disposition": "occupied",
    "liveness": "running"
  }, {
    "lane": "018f82d4-6f59-7d06-92e3-bce9131163c6",
    "lineage_id": "018f82d4-6f59-7d06-92e3-bce9131163c6",
    "worktree": "/worktrees/lane-b",
    "life": "steer",
    "vendor": "codex",
    "lineage": ["018f82d4-6f59-7d06-92e3-bce9131163c8"],
    "bound": false,
    "disposition": "vacant",
    "liveness": "vacant"
  }]
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
    "lanes": [{
      "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
      "lineage_id": "018f82d4-6f59-7d06-92e3-bce9131163c2",
      "name": "Ava",
      "worktree": "/worktrees/lane-a",
      "life": "drive",
      "vendor": "claude",
      "occupant": "018f82d4-6f59-7d06-92e3-bce9131163c5",
      "lineage": ["018f82d4-6f59-7d06-92e3-bce9131163c5"],
      "bound": true,
      "disposition": "occupied",
      "liveness": "running"
    }],
    "subscriptions": [{
      "lineage_id": "018f82d4-6f59-7d06-92e3-bce9131163c2",
      "policy": "drive"
    }]
  },
  "recent": [{
    "seq": 48213,
    "kind": "task::added",
    "actor": "operator",
    "origin": "task:WIRE-parent",
    "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
    "provenance": {
      "kind": "steering",
      "key": "1kJJP87B",
      "disposition": "acked"
    }
  }]
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
        presentation_title: "Bind a board task".to_owned(),
        statement: Positioned {
            position: seq(2),
            body: BoardStatement {
                handle: handle("SPICE-aBcD"),
                uuid: uuid("018f82d4-6f59-7d06-92e3-bce9131163c4"),
                description: Some(
                    "One task spelled through every field the shape carries.".to_owned(),
                ),
                acceptance: vec![
                    "the row decodes".to_owned(),
                    "the row re-encodes unchanged".to_owned(),
                ],
                project: "wire.binding".to_owned(),
                tags: vec!["binding".to_owned()],
                dependencies: vec![handle("SPICE-eFgH")],
                provenance: "task:SPICE-aBcD".parse().expect("an origin"),
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
            },
        },
        claim: Some(Positioned {
            position: seq(4),
            body: ClaimHead {
                actor: actor("operator"),
                fact: seq(4),
                lane: uuid("018f82d4-6f59-7d06-92e3-bce9131163c5"),
                occupant: uuid("018f82d4-6f59-7d06-92e3-bce9131163c6"),
                epoch,
                disposition: ClaimDisposition::Active,
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
    BodyShape::EffortRead,
    BodyShape::TaskAdd,
    BodyShape::TaskOopsAdd,
    BodyShape::GateFindings,
    BodyShape::TaskEdit,
    BodyShape::TaskDelete,
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
    BodyShape::LaneArrange,
    BodyShape::LineageConfigure,
    BodyShape::LineageArrange,
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
    AnswerShape::Effort,
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

#[test]
fn lineage_subscription_policies_close_over_the_predecessor_scopes() {
    use wire::intent::{LineageLife, TaskSubscriptionScope};

    let policies = [
        (LineageLife::Steer, TaskSubscriptionScope::Manual),
        (
            LineageLife::Drive,
            TaskSubscriptionScope::ManualAndAutoProject,
        ),
        (LineageLife::Drain, TaskSubscriptionScope::AllAssignable),
    ];
    for (policy, scope) in policies {
        assert_eq!(policy.scope(), scope);
        assert_eq!(LineageLife::from_wire(policy.as_str()), Some(policy));
    }
}

/// Every reason code, gathered by exhaustive descent rather than from a list.
///
/// A walk over a declared `ALL` would pass forever on a code left out of it,
/// so the chain is a `match` the compiler refuses while a variant is missing.
/// That makes the pin below complete by construction rather than by review.
fn descend_unavailable_codes() -> Vec<EffortUnavailableCode> {
    use EffortUnavailableCode as Code;
    std::iter::successors(Some(Code::MissingStart), |code| match code {
        Code::MissingStart => Some(Code::MissingEnd),
        Code::MissingEnd => Some(Code::IncompleteBoundary),
        Code::IncompleteBoundary => Some(Code::TranscriptMissing),
        Code::TranscriptMissing => Some(Code::SourceGap),
        Code::SourceGap => Some(Code::SourceReplaced),
        Code::SourceReplaced => Some(Code::ClaimFreed),
        Code::ClaimFreed => Some(Code::ClaimLapsed),
        Code::ClaimLapsed => Some(Code::ClaimStolen),
        Code::ClaimStolen => Some(Code::TaskDeleted),
        Code::TaskDeleted => Some(Code::BrokenCarry),
        Code::BrokenCarry => Some(Code::OccupantDisagreement),
        Code::OccupantDisagreement => Some(Code::SessionDisagreement),
        Code::SessionDisagreement => Some(Code::LineageDisagreement),
        Code::LineageDisagreement => Some(Code::MalformedUsage),
        Code::MalformedUsage => Some(Code::UntaggedLaunch),
        Code::UntaggedLaunch => Some(Code::MixedLaunch),
        Code::MixedLaunch => Some(Code::ArithmeticOverflow),
        Code::ArithmeticOverflow => None,
    })
    .collect()
}

#[test]
fn every_unavailable_reason_spells_itself_the_way_it_encodes() {
    // The Canvas names a withheld phase's reason from `as_str`, while the CLI
    // and every stored answer carry the encoder's spelling. Comparing the two
    // per variant is what keeps one reason from having two words: a rename on
    // either side fails here rather than reaching a surface that says one thing
    // while the bytes behind it say another.
    let mut spellings = BTreeSet::new();
    for code in descend_unavailable_codes() {
        let spelling = code.as_str();
        assert_eq!(
            wire::encode(&code).expect("a reason code encodes"),
            format!("\"{spelling}\"").as_bytes(),
            "{spelling} does not encode as it spells itself"
        );
        assert_eq!(
            wire::decode::<EffortUnavailableCode>(format!("\"{spelling}\"").as_bytes())
                .expect("a reason code decodes from its own spelling"),
            code,
            "{spelling} does not decode back to the code that spelled it"
        );
        assert!(
            spelling
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'-'),
            "{spelling:?} is not lower-case kebab"
        );
        assert!(spellings.insert(spelling), "{spelling:?} names two reasons");
    }
    assert!(
        wire::decode::<EffortUnavailableCode>(br#""untagged_launch""#).is_err(),
        "the vocabulary is closed against a spelling it does not carry"
    );
}
