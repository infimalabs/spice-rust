//! Authentication, generation, law, and body checks at the command door.

use wire::intent::{BodyShape, CommandAnswer, Intent, StateAnswer};
use wire::{
    Accepted, Actor, CanonicalUuid, Command, CommandKind, CommandRejection, Outcome, Reply,
    ReplyPosition, ResidentState, WireVersion, Witness,
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

    assert!(matches!(
        rejection,
        CommandRejection::Refusal(wire::Refusal { ref reason, .. })
            if reason == "command::authority"
    ));
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
    assert!(matches!(
        missing.admit(&accepted(Actor::Operator)),
        Err(CommandRejection::Refusal(wire::Refusal { ref reason, .. }))
            if reason == "command::witness-law"
    ));

    let mut malformed = dark(uuid(1));
    malformed.body = serde_json::json!({ "task": "SPICE-aBcD" });
    assert!(matches!(
        malformed.admit(&accepted(Actor::Operator)),
        Err(CommandRejection::Fault(wire::Fault { ref reason, .. }))
            if reason == "command::body-undecodable"
    ));
}
