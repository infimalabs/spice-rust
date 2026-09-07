//! Authentication, generation, law, and body checks at the command door.

use wire::intent::{BodyShape, CommandAnswer, Intent, StateAnswer};
use wire::{
    Accepted, Actor, CanonicalUuid, Command, CommandKind, CommandRejection, Outcome, Reply,
    ReplyPosition, ResidentState, Seq, WireVersion, Witness,
};

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
        .parse()
        .expect("canonical fixture UUID")
}

fn accepted(actor: Actor) -> Accepted {
    Accepted {
        wire: WireVersion::CURRENT,
        actor,
        epoch: uuid(1),
    }
}

fn dark(epoch: CanonicalUuid) -> Command<serde_json::Value> {
    Command {
        v: WireVersion::CURRENT,
        id: uuid(2),
        epoch,
        kind: CommandKind::UpChange,
        expected: vec![Witness::Transition {
            from: ResidentState::Seated,
            to: ResidentState::Dark,
        }],
        body: serde_json::json!({ "state": "dark" }),
    }
}

#[test]
fn current_operator_command_admits_one_kind_selected_body() {
    let command = dark(uuid(1))
        .admit(&accepted(Actor::Operator))
        .expect("current operator command admits");

    assert_eq!(command.id(), uuid(2));
    assert_eq!(command.epoch(), uuid(1));
    assert_eq!(command.actor(), &Actor::Operator);
    assert_eq!(command.kind(), CommandKind::UpChange);
    assert!(matches!(
        command.body(),
        Intent::UpChange(wire::intent::UpChange {
            state: ResidentState::Dark
        })
    ));

    let response = command.response(Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body: CommandAnswer::State(StateAnswer {
            from: ResidentState::Seated,
            to: ResidentState::Dark,
        }),
    }));
    assert_eq!(response.id, uuid(2));
    assert_eq!(response.epoch, uuid(1));
}

#[test]
fn stale_epoch_faults_before_body_admission() {
    let rejection = dark(uuid(3))
        .admit(&accepted(Actor::Operator))
        .expect_err("stale generation faults");

    assert!(matches!(
        rejection,
        CommandRejection::Fault(wire::Fault { ref reason, .. })
            if reason == "command::epoch-mismatch"
    ));
}

#[test]
fn lane_cannot_submit_an_operator_command() {
    let rejection = dark(uuid(1))
        .admit(&accepted(Actor::Lane(uuid(4))))
        .expect_err("lane authority refuses");

    let CommandRejection::Refusal(refusal) = rejection else {
        panic!("lane authority must refuse");
    };
    assert_eq!(refusal.reason(), "command::authority");
}

#[test]
fn task_creation_routes_are_closed_over_distinct_body_contracts() {
    assert_eq!(CommandKind::TaskAdd.binding().body, BodyShape::TaskAdd,);
    assert_eq!(
        CommandKind::TaskOopsAdd.binding().body,
        BodyShape::TaskOopsAdd,
    );
}

#[test]
fn missing_witness_and_wrong_body_stop_before_business_logic() {
    let mut missing = dark(uuid(1));
    missing.expected.clear();
    let Err(CommandRejection::Refusal(refusal)) = missing.admit(&accepted(Actor::Operator)) else {
        panic!("a missing witness must refuse");
    };
    assert_eq!(refusal.reason(), "command::witness-law");

    let mut malformed = dark(uuid(1));
    malformed.body = serde_json::json!({ "task": "SPICE-1kKRRsDP" });
    assert!(matches!(
        malformed.admit(&accepted(Actor::Operator)),
        Err(CommandRejection::Fault(wire::Fault { ref reason, .. }))
            if reason == "command::body-undecodable"
    ));
}

fn wake(into: Option<&str>, statement_tasks: &[wire::TaskStamp]) -> Command<serde_json::Value> {
    let task: wire::TaskStamp = "0000000B".parse().expect("fixture task stamp");
    let mut expected = vec![
        Witness::Workflow {
            task: task.clone(),
            head: Seq::new(10).expect("fixture workflow head"),
            phase: "todo".parse().expect("fixture phase"),
        },
        Witness::Deferral {
            task: task.clone(),
            head: Seq::new(10).expect("fixture deferral head"),
            deferred: true,
        },
    ];
    for statement_task in statement_tasks {
        expected.push(Witness::Task {
            task: statement_task.clone(),
            head: Seq::new(9).expect("fixture statement head"),
        });
    }
    let handle = wire::TaskHandle::for_project("spice", &task);
    Command {
        v: WireVersion::CURRENT,
        id: uuid(5),
        epoch: uuid(1),
        kind: CommandKind::TaskWake,
        expected,
        body: into.map_or_else(
            || serde_json::json!({ "task": handle }),
            |project| serde_json::json!({ "task": handle, "into": project }),
        ),
    }
}

#[test]
fn wake_promotion_requires_statement_authority_but_plain_wake_does_not() {
    let target: wire::TaskStamp = "0000000B".parse().expect("target task stamp");
    let unrelated: wire::TaskStamp = "0000000C".parse().expect("unrelated task stamp");
    let Err(CommandRejection::Refusal(refusal)) =
        wake(Some("board.test"), &[]).admit(&accepted(Actor::Operator))
    else {
        panic!("promotion without a task witness must refuse");
    };
    assert_eq!(refusal.reason(), "command::witness-law");

    let plain = wake(None, &[])
        .admit(&accepted(Actor::Operator))
        .expect("plain wake retains workflow and deferral authority");
    assert!(matches!(
        plain.body(),
        Intent::TaskWake(wire::intent::TaskWake { into: None, .. })
    ));

    let Err(CommandRejection::Refusal(refusal)) =
        wake(Some("board.test"), &[unrelated.clone(), unrelated.clone()])
            .admit(&accepted(Actor::Operator))
    else {
        panic!("duplicate unrelated task witnesses must refuse");
    };
    assert_eq!(refusal.reason(), "command::witness-law");
    assert!(
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::witness)
            .is_some_and(|witness| witness.contains("bound to SPICE-0000000B"))
    );

    wake(Some("board.test"), &[target.clone(), target, unrelated])
        .admit(&accepted(Actor::Operator))
        .expect("exact target authority admits despite duplicate and extra predicates");
}
