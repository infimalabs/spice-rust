use wire::intent::AnswerShape;

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
pub(super) const ANSWERS: &[(AnswerShape, &str)] = &[
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
          "born": "1kKRRsDP",
          "queue_origin": 2,
          "description": "One task spelled through every field the shape carries.",
          "acceptance": ["the row decodes", "the row re-encodes unchanged"],
          "project": "wire.binding",
          "tags": ["binding"],
          "dependencies": ["1kKRRsDQ"],
          "provenance": "task:1kKRRsDP",
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
          "claimed": 4,
          "cell": {
            "project": "wire.binding",
            "phase": "todo"
          },
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
      "topology_revision": 0,
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
  "completion": {
    "operation": "018f82d4-6f59-7d06-92e3-bce9131163c4",
    "state": "running"
  },
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
  "authored": "2026-08-01T06:07:19.000Z",
  "disposition": "acked"
}"#,
    ),
    (
        AnswerShape::Delivery,
        r#"{
  "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
  "delivery": {
    "standing": "parked",
    "source": 41,
    "keys": ["1kJJP87B"],
    "failure": "steer::inbox-unwritable: inbox directory blocked",
    "repair": "spice steer repair 018f82d4-6f59-7d06-92e3-bce9131163c2"
  }
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
      "stage": "published",
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
    "task": "1kKRRsDP",
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
    "task": "1kKRRsDP",
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
  "topology_revision": 9,
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
  "topology_revision": 10,
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
    "topology_revision": 9,
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
      "disposition": "acked",
      "stage": "overdue",
      "escalations": [{
        "stage": "reminded",
        "at": "2026-08-06T03:00:02.000Z"
      }, {
        "stage": "escalated",
        "at": "2026-08-06T03:01:00.000Z"
      }, {
        "stage": "overdue",
        "at": "2026-08-06T03:05:00.000Z"
      }]
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
