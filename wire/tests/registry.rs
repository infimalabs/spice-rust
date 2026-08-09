//! The command registry and its witness law.
//!
//! The registry carries the witness law with the command, so a new command
//! without an explicit row does not compile. That half is enforced by an
//! exhaustive match and cannot be asserted from a test. These fixtures prove
//! the half that can be: every row names the witnesses its command requires, a
//! command missing one is refused, a global journal position is not
//! expressible, and witness order does not move the canonical bytes an identity
//! binds.

use std::collections::BTreeSet;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use wire::{
    Answer, Authority, Command, CommandKind, Digest, Disposition, LawError, Name, ResidentState,
    TaskHandle, TrustState, Witness, WitnessKind,
};

/// Body of the `task::done` command this fixture pins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DoneBody {
    task: TaskHandle,
    validation: String,
}

/// The `task::done` command exactly as it crosses the wire.
const COMMAND: &str = r#"{
  "v": 1,
  "id": "018f82d4-6f59-7d06-92e3-bce9131163c4",
  "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3",
  "kind": "task::done",
  "expected": [
    { "kind": "task", "task": "SPICE-aBcD", "head": 48168 },
    { "kind": "workflow", "task": "SPICE-aBcD", "head": 48169, "phase": "verify" },
    { "kind": "claim", "task": "SPICE-aBcD", "head": 48172 },
    { "kind": "lease", "task": "SPICE-aBcD", "head": 48173 },
    {
      "kind": "residency",
      "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
      "occupant": "018f82d4-6f59-7d06-92e3-bce9131163c5",
      "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3"
    }
  ],
  "body": {
    "task": "SPICE-aBcD",
    "validation": "cargo test --workspace"
  }
}"#;

fn done() -> Command<DoneBody> {
    wire::decode(COMMAND.as_bytes()).expect("the specified command decodes")
}

#[test]
fn the_specified_command_satisfies_its_own_registry_row() {
    let command = done();
    assert_eq!(command.kind, CommandKind::TaskDone);
    command
        .witnessed()
        .expect("this fixture satisfies the row it documents");
}

#[test]
fn every_command_names_the_witnesses_it_requires() {
    for kind in CommandKind::ALL {
        let law = kind.law();
        let scanner_disposition = matches!(kind, CommandKind::SteerDispose);
        let server_projected_gate_work = matches!(kind, CommandKind::GateFindings);
        let lane_lifecycle = matches!(
            kind,
            CommandKind::LaneSeat
                | CommandKind::LaneState
                | CommandKind::LaneArrange
                | CommandKind::LineageConfigure
                | CommandKind::LineageArrange
        );
        assert_eq!(
            law.required.is_empty(),
            kind.is_read() || scanner_disposition || lane_lifecycle || server_projected_gate_work,
            concat!(
                "only reads, lane lifecycle, scanner dispositions, and server-projected gate ",
                "work may omit required witnesses"
            ),
        );
        if scanner_disposition {
            assert_eq!(law.optional, &[WitnessKind::Directive]);
        } else if law.required.is_empty() {
            assert!(law.optional.is_empty());
        }
        for optional in law.optional {
            assert!(
                !law.required.contains(optional),
                "{kind} names {optional:?} as both required and optional",
            );
        }
    }
}

/// Lane lifecycle is the operator's move alone, and this row is the only place
/// that is enforced: `Command::admit` refuses every non-operator actor before a
/// handler runs, so the resident's own import path deliberately carries no
/// second authority check it could never observe violated. Weakening this row
/// would silently let a seated lane import another lane's worktree.
#[test]
fn lane_lifecycle_is_admitted_for_the_operator_alone() {
    for kind in [
        CommandKind::LaneSeat,
        CommandKind::LaneState,
        CommandKind::LaneArrange,
        CommandKind::LineageConfigure,
    ] {
        assert_eq!(
            kind.law().authority,
            Authority::Operator,
            "{kind} is the resident's single enforcement point for lane lifecycle authority",
        );
    }
}

/// The rest of the operator-only moves, held to the same standard for the same
/// reason: each handler reads an `AdmittedCommand`, which only `Command::admit`
/// can construct, and that refuses every non-operator actor first. The handlers
/// therefore carry no authority check of their own, and these rows are what
/// stands between a lane capability and authoring direction, ruling on consent,
/// moving trust, or reading the lane table. `UpChange` belongs to the same set
/// and is deliberately absent here: `lane_cannot_submit_an_operator_command`
/// already refuses a lane that command through `admit` itself, which pins the
/// row more strongly than reading it.
///
/// Recall and resend are the contrasts that explain the shape: their laws admit
/// any connected actor, so their handlers are entitled to discriminate between
/// them and keep the matches this test would otherwise make redundant.
#[test]
fn operator_only_moves_are_admitted_for_the_operator_alone() {
    for kind in [
        CommandKind::SteerSend,
        CommandKind::ConsentDecide,
        CommandKind::TrustGrant,
        CommandKind::TrustRevoke,
        CommandKind::LaneRead,
    ] {
        assert_eq!(
            kind.law().authority,
            Authority::Operator,
            "{kind} is the resident's single enforcement point for its authority",
        );
    }
    assert_eq!(
        CommandKind::LaneRecall.law().authority,
        Authority::Connected,
        "lane recall admits a lane, which is why its handler still tells actors apart",
    );
    assert_eq!(
        CommandKind::SteerResend.law().authority,
        Authority::Connected,
        "steering resend admits a lane, which is why its handler still tells actors apart",
    );
}

#[test]
fn every_spelling_is_distinct_and_round_trips() {
    let mut seen = BTreeSet::new();
    for kind in CommandKind::ALL {
        assert!(seen.insert(kind.as_str()), "{kind} is spelled twice");
        assert_eq!(
            CommandKind::from_str(kind.as_str()).expect("a registry spelling parses"),
            *kind,
        );
        let encoded = serde_json::to_value(kind).expect("a kind encodes");
        assert_eq!(encoded, serde_json::Value::String(kind.as_str().to_owned()));
    }
    assert_eq!(seen.len(), CommandKind::ALL.len());
}

#[test]
fn a_spelling_outside_the_registry_is_refused() {
    for absent in ["task::invented", "task", "TASK::DONE", "", "done"] {
        assert!(
            CommandKind::from_str(absent).is_err(),
            "{absent:?} is not in the closed registry",
        );
        let decoded: Result<CommandKind, _> = wire::decode(
            serde_json::to_vec(&absent)
                .expect("text encodes")
                .as_slice(),
        );
        assert!(decoded.is_err(), "{absent:?} decoded as a command");
    }
}

#[test]
fn a_command_missing_a_required_witness_is_refused() {
    let law = CommandKind::TaskDone.law();
    assert!(!law.required.is_empty());

    // Dropping each required witness in turn must name that predicate.
    for required in law.required {
        let mut command = done();
        command
            .expected
            .retain(|witness| witness.kind() != *required);
        match command.witnessed() {
            Err(LawError::Missing {
                command: at,
                missing,
            }) => {
                assert_eq!(at, CommandKind::TaskDone);
                assert_eq!(missing, *required);
            }
            other => panic!("dropping {required:?} was not refused: {other:?}"),
        }
    }
}

#[test]
fn a_command_carrying_an_unwitnessed_subject_is_refused() {
    let mut command = done();
    command.expected.push(Witness::Eligibility {
        rule: "next".parse().expect("a name"),
    });
    match command.witnessed() {
        Err(LawError::Unadmitted { command: at, extra }) => {
            assert_eq!(at, CommandKind::TaskDone);
            assert_eq!(extra, WitnessKind::Eligibility);
        }
        other => panic!("an unwitnessed subject was admitted: {other:?}"),
    }
}

#[test]
fn an_optional_witness_is_admitted_without_being_required() {
    let law = CommandKind::TaskDone.law();
    assert!(law.optional.contains(&WitnessKind::Assignment));

    // The row admits it, so the bare fixture still passes without it.
    done().witnessed().expect("optional means not required");

    let mut command = done();
    command.expected.push(Witness::Assignment {
        task: "SPICE-aBcD".parse().expect("a handle"),
        head: wire::Seq(48174.try_into().expect("a position")),
        reviewer: "lane:018f82d4-6f59-7d06-92e3-bce9131163c2"
            .parse()
            .expect("an actor"),
    });
    command.witnessed().expect("optional means admitted");
}

#[test]
fn a_claim_witness_distinguishes_no_head_from_a_null_or_present_head() {
    let absent = r#"{"kind":"claim","task":"SPICE-aBcD"}"#;
    let witness = wire::decode::<Witness>(absent.as_bytes()).expect("absent claim head decodes");
    assert!(matches!(witness, Witness::Claim { head: None, .. }));
    assert_eq!(
        wire::encode(&witness).expect("absent claim witness encodes"),
        absent.as_bytes()
    );

    let null = r#"{"kind":"claim","task":"SPICE-aBcD","head":null}"#;
    assert!(
        wire::decode::<Witness>(null.as_bytes()).is_err(),
        "null must not alias an absent claim head"
    );

    let present = done()
        .expected
        .into_iter()
        .find(|witness| matches!(witness, Witness::Claim { .. }))
        .expect("specified command carries a claim witness");
    assert!(matches!(present, Witness::Claim { head: Some(_), .. }));
}

#[test]
fn witness_order_does_not_change_canonical_bytes() {
    let forward = done();
    let mut reversed = forward.clone();
    reversed.expected.reverse();
    assert_ne!(
        forward.expected, reversed.expected,
        "the fixture must carry enough witnesses to reorder",
    );
    assert_eq!(
        forward.canonical_json().expect("canonical bytes"),
        reversed.canonical_json().expect("canonical bytes"),
        "witness order moved the bytes an identity binds",
    );
}

#[test]
fn witness_order_does_not_change_the_law() {
    let mut reversed = done();
    reversed.expected.reverse();
    reversed
        .witnessed()
        .expect("the law is over predicate classes, not their order");
}

#[test]
fn a_global_journal_position_is_not_expressible() {
    // A bare position with no subject is not a witness spelling.
    for bare in [
        r#"{ "kind": "seq", "head": 48168 }"#,
        r#"{ "kind": "journal", "seq": 48168 }"#,
        r#"{ "kind": "snapshot", "head": 48168 }"#,
    ] {
        let decoded: Result<Witness, _> = wire::decode(bare.as_bytes());
        assert!(decoded.is_err(), "{bare} decoded as a witness");
    }

    // Every witness that exists binds a subject beside any position it names.
    for witness in sample_witnesses() {
        let encoded = serde_json::to_value(&witness).expect("a witness encodes");
        let object = encoded.as_object().expect("a witness is an object");
        let subjects = object
            .keys()
            .filter(|key| key.as_str() != "kind" && key.as_str() != "head")
            .count();
        assert!(
            subjects > 0,
            "{:?} names a position with no subject",
            witness.kind(),
        );
    }
}

#[test]
fn every_witness_class_has_a_sample_and_reports_its_own_kind() {
    let mut seen = BTreeSet::new();
    for witness in sample_witnesses() {
        assert!(
            seen.insert(witness.kind()),
            "{:?} is sampled twice",
            witness.kind(),
        );
    }

    // Every class the registry actually cites must be sampled above, so the
    // subject check cannot silently skip a class.
    for kind in CommandKind::ALL {
        let law = kind.law();
        for cited in law.required.iter().chain(law.optional) {
            assert!(
                seen.contains(cited),
                "{cited:?} is cited by {kind} but never sampled"
            );
        }
    }
}

/// One value of every witness class, used to prove structural properties that
/// hold for the whole vocabulary rather than for one command.
fn sample_witnesses() -> Vec<Witness> {
    let task: TaskHandle = "SPICE-aBcD".parse().expect("a handle");
    let uuid = |text: &str| text.parse().expect("a canonical uuid");
    let seq = |value: u64| wire::Seq(value.try_into().expect("a position"));
    let name = |text: &str| Name::from_str(text).expect("a name");
    let digest =
        |byte: char| Digest::from_str(&String::from(byte).repeat(64)).expect("a digest spelling");
    let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163c2");

    vec![
        Witness::Task {
            task: task.clone(),
            head: seq(1),
        },
        Witness::Workflow {
            task: task.clone(),
            head: seq(2),
            phase: "verify".parse().expect("a phase"),
        },
        Witness::Claim {
            task: task.clone(),
            head: Some(seq(3)),
        },
        Witness::Lease {
            task: task.clone(),
            head: seq(4),
        },
        Witness::Residency {
            lane,
            occupant: uuid("018f82d4-6f59-7d06-92e3-bce9131163c5"),
            epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
        },
        Witness::Absence { task: task.clone() },
        Witness::Existence { task: task.clone() },
        Witness::Edge {
            task: task.clone(),
            after: "SPICE-eFgH".parse().expect("a handle"),
        },
        Witness::Deferral {
            task: task.clone(),
            head: seq(5),
            deferred: true,
        },
        Witness::Eligibility { rule: name("next") },
        Witness::Assignment {
            task,
            head: seq(6),
            reviewer: "lane:018f82d4-6f59-7d06-92e3-bce9131163c2"
                .parse()
                .expect("an actor"),
        },
        Witness::Preview {
            digest: digest('a'),
        },
        Witness::Barrier {
            name: name("board"),
            digest: digest('b'),
        },
        Witness::Directive {
            steer: lane,
            disposition: Disposition::Live,
        },
        Witness::Lineage { steer: lane },
        Witness::Consent {
            request: lane,
            answer: Answer::Asked,
        },
        Witness::Rule {
            name: name("reads"),
        },
        Witness::Budget {
            rule: name("reads"),
            remaining: 25,
        },
        Witness::Trust {
            digest: digest('c'),
            standing: TrustState::Pended,
        },
        Witness::Taste {
            name: name("polling"),
            digest: digest('d'),
        },
        Witness::Observation {
            key: digest('e'),
            head: seq(7),
        },
        Witness::Sealed {
            proof: digest('f'),
            manifest: digest('0'),
        },
        Witness::Transition {
            from: ResidentState::Seated,
            to: ResidentState::Quiescing,
        },
    ]
}
