//! Authentication, generation, law, and body checks at the command door.

use std::collections::BTreeSet;

use wire::intent::{BodyShape, CommandAnswer, Intent, StateAnswer};
use wire::principal::{PrincipalHead, PrincipalProfile, PrincipalRole, PrincipalRoles};
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
        release: "1.2.3".parse().expect("fixture release"),
    }
}

/// The bootstrap principal holding `roles`, born first, with a later-born
/// operator beside it.
fn bootstrap_holding(roles: &[PrincipalRole]) -> PrincipalRoles {
    let head = |principal, born, roles: &[PrincipalRole]| PrincipalHead {
        principal: uuid(principal),
        born: Seq::new(born).expect("fixture position"),
        profile: PrincipalProfile::default(),
        roles: roles.iter().copied().collect::<BTreeSet<_>>(),
        capability: None,
    };
    PrincipalRoles::new([head(91, 9, &[PrincipalRole::Operator]), head(90, 3, roles)])
}

fn operator() -> PrincipalRoles {
    bootstrap_holding(&[PrincipalRole::Operator])
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
        .admit(&accepted(Actor::Operator), &operator())
        .expect("current operator command admits");

    assert_eq!(command.id(), uuid(2));
    assert_eq!(command.epoch(), uuid(1));
    assert_eq!(command.actor(), &Actor::Operator);
    assert!(command.holds(PrincipalRole::Operator));
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
        .admit(&accepted(Actor::Operator), &operator())
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
        .admit(&accepted(Actor::Lane(uuid(4))), &operator())
        .expect_err("lane authority refuses");

    let CommandRejection::Refusal(refusal) = rejection else {
        panic!("lane authority must refuse");
    };
    assert_eq!(refusal.reason(), "command::authority");
}

#[test]
fn operator_authority_follows_the_bootstrap_principals_current_role() {
    let Err(CommandRejection::Refusal(refusal)) =
        dark(uuid(1)).admit(&accepted(Actor::Operator), &bootstrap_holding(&[]))
    else {
        panic!("an operator actor whose principal lost the role must refuse");
    };
    assert_eq!(refusal.reason(), "command::authority");
    let unborn = dark(uuid(1))
        .admit(&accepted(Actor::Operator), &PrincipalRoles::default())
        .expect("before its recorded birth the bootstrap human is the operator");
    assert!(unborn.holds(PrincipalRole::Operator));

    let regranted = dark(uuid(1))
        .admit(&accepted(Actor::Operator), &operator())
        .expect("the next command after a grant admits");
    assert!(regranted.holds(PrincipalRole::Operator));
}

#[test]
fn connected_commands_admit_a_born_principal_without_the_operator_role() {
    let read = || wake(None, false, &[]);
    let admitted = read()
        .admit(&accepted(Actor::Operator), &bootstrap_holding(&[]))
        .expect("a born principal submits connected commands");
    assert!(!admitted.holds(PrincipalRole::Operator));

    let Err(CommandRejection::Refusal(refusal)) =
        read().admit(&accepted(Actor::Spice), &operator())
    else {
        panic!("an actor acting as no principal is not connected");
    };
    assert_eq!(refusal.reason(), "command::authority");

    let lane = read()
        .admit(&accepted(Actor::Lane(uuid(4))), &PrincipalRoles::default())
        .expect("a lane submits connected commands");
    assert!(!lane.holds(PrincipalRole::Operator));
}

#[test]
fn a_human_actor_holds_exactly_its_own_principals_roles() {
    let roles = bootstrap_holding(&[]);
    let operating = dark(uuid(1))
        .admit(&accepted(Actor::Human(uuid(91))), &roles)
        .expect("a later-born operator-role principal submits operator commands");
    assert!(operating.holds(PrincipalRole::Operator));

    let bootstrap = Actor::Human(uuid(90));
    let Err(CommandRejection::Refusal(refusal)) =
        dark(uuid(1)).admit(&accepted(bootstrap.clone()), &roles)
    else {
        panic!("a principal without the operator role must refuse operator commands");
    };
    assert_eq!(refusal.reason(), "command::authority");
    let connected = wake(None, false, &[])
        .admit(&accepted(bootstrap), &roles)
        .expect("a principal without the operator role still submits connected commands");
    assert!(!connected.holds(PrincipalRole::Operator));

    let unborn = Actor::Human(uuid(77));
    for command in [dark(uuid(1)), wake(None, false, &[])] {
        let Err(CommandRejection::Refusal(refusal)) =
            command.admit(&accepted(unborn.clone()), &roles)
        else {
            panic!("a human actor naming no born principal acts as nobody");
        };
        assert_eq!(refusal.reason(), "command::authority");
    }
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
    let Err(CommandRejection::Refusal(refusal)) =
        missing.admit(&accepted(Actor::Operator), &operator())
    else {
        panic!("a missing witness must refuse");
    };
    assert_eq!(refusal.reason(), "command::witness-law");

    let mut malformed = dark(uuid(1));
    malformed.body = serde_json::json!({ "task": "SPICE-1kKRRsDP" });
    assert!(matches!(
        malformed.admit(&accepted(Actor::Operator), &operator()),
        Err(CommandRejection::Fault(wire::Fault { ref reason, .. }))
            if reason == "command::body-undecodable"
    ));
}

fn wake(
    into: Option<&str>,
    claim: bool,
    statement_tasks: &[wire::TaskStamp],
) -> Command<serde_json::Value> {
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
    if claim {
        expected.push(Witness::Eligibility {
            rule: "capture".parse().expect("fixture capture rule"),
        });
        expected.push(Witness::Residency {
            lane: uuid(2),
            occupant: Some(uuid(3)),
            epoch: uuid(1),
        });
    }
    let handle = wire::TaskHandle::for_project("spice", &task);
    let body = match (into, claim) {
        (Some(project), true) => {
            serde_json::json!({ "task": handle, "into": project, "claim": true })
        }
        (Some(project), false) => serde_json::json!({ "task": handle, "into": project }),
        (None, true) => serde_json::json!({ "task": handle, "claim": true }),
        (None, false) => serde_json::json!({ "task": handle }),
    };
    Command {
        v: WireVersion::CURRENT,
        id: uuid(5),
        epoch: uuid(1),
        kind: if into.is_some() {
            CommandKind::TaskFile
        } else {
            CommandKind::TaskWake
        },
        expected,
        body,
    }
}

#[test]
fn filing_requires_statement_authority_but_plain_wake_does_not() {
    let target: wire::TaskStamp = "0000000B".parse().expect("target task stamp");
    let unrelated: wire::TaskStamp = "0000000C".parse().expect("unrelated task stamp");
    let Err(CommandRejection::Refusal(refusal)) =
        wake(Some("board.test"), false, &[]).admit(&accepted(Actor::Operator), &operator())
    else {
        panic!("filing without a task witness must refuse");
    };
    assert_eq!(refusal.reason(), "command::witness-law");

    let plain = wake(None, false, &[])
        .admit(&accepted(Actor::Operator), &operator())
        .expect("plain wake retains workflow and deferral authority");
    assert!(matches!(plain.body(), Intent::TaskWake(_)));

    let Err(CommandRejection::Refusal(refusal)) = wake(
        Some("board.test"),
        false,
        &[unrelated.clone(), unrelated.clone()],
    )
    .admit(&accepted(Actor::Operator), &operator()) else {
        panic!("duplicate unrelated task witnesses must refuse");
    };
    assert_eq!(refusal.reason(), "command::witness-law");
    assert!(
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::witness)
            .is_some_and(|witness| witness.contains("bound to SPICE-0000000B"))
    );

    wake(
        Some("board.test"),
        false,
        &[target.clone(), target, unrelated],
    )
    .admit(&accepted(Actor::Operator), &operator())
    .expect("exact target authority admits despite duplicate and extra predicates");
}

#[test]
fn wake_claim_requires_capture_authority_and_decodes_the_atomic_request() {
    let mut missing = wake(None, true, &[]);
    missing
        .expected
        .retain(|witness| !matches!(witness, Witness::Eligibility { .. }));
    let Err(CommandRejection::Refusal(refusal)) =
        missing.admit(&accepted(Actor::Operator), &operator())
    else {
        panic!("wake claim without eligibility must refuse");
    };
    assert_eq!(refusal.reason(), "command::witness-law");

    let admitted = wake(None, true, &[])
        .admit(&accepted(Actor::Operator), &operator())
        .expect("wake claim with capture authority admits");
    assert!(matches!(
        admitted.body(),
        Intent::TaskWake(wire::intent::TaskWake { claim: true, .. })
    ));
}

#[test]
fn a_lane_reads_working_groups_but_cannot_mutate_them() {
    let lane = accepted(Actor::Lane(uuid(4)));
    let read = Command {
        v: WireVersion::CURRENT,
        id: uuid(2),
        epoch: uuid(1),
        kind: CommandKind::WorkingGroupRead,
        expected: vec![],
        body: serde_json::json!({}),
    };
    read.admit(&lane, &PrincipalRoles::default())
        .expect("a lane reads the working-group partition");

    for kind in [
        CommandKind::WorkingGroupMove,
        CommandKind::WorkingGroupArrange,
        CommandKind::WorkingGroupDisband,
        CommandKind::WorkingGroupFast,
        CommandKind::WorkingGroupStance,
    ] {
        let command = Command {
            v: WireVersion::CURRENT,
            id: uuid(2),
            epoch: uuid(1),
            kind,
            expected: vec![],
            body: serde_json::json!({}),
        };
        let Err(CommandRejection::Refusal(refusal)) =
            command.admit(&lane, &PrincipalRoles::default())
        else {
            panic!("a lane must not mutate working groups");
        };
        assert_eq!(refusal.reason(), "command::authority");
    }
}

#[test]
fn a_lane_cannot_inspect_or_terminate_keeper_jobs() {
    for kind in [
        CommandKind::LaneJobsRead,
        CommandKind::LaneJobRead,
        CommandKind::LaneJobStop,
    ] {
        let command = Command {
            v: WireVersion::CURRENT,
            id: uuid(2),
            epoch: uuid(1),
            kind,
            expected: vec![],
            body: if kind == CommandKind::LaneJobsRead {
                serde_json::json!({})
            } else {
                serde_json::json!({"scope": {"topology": "0", "lane": "0"}, "id": "1kKRRsDP"})
            },
        };
        let Err(CommandRejection::Refusal(refusal)) =
            command.admit(&accepted(Actor::Lane(uuid(4))), &operator())
        else {
            panic!("lane job control requires operator authority");
        };
        assert_eq!(refusal.reason(), "command::authority");
    }
}
