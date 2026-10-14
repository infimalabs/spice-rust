//! Probes: ordinary evidence-only tasks that run while the open Design they
//! were created under stands in a working group, with no commit authority and
//! no standing as candidate outputs.

use super::super::preparation::{conclude, done_conclusion};
use super::design_standing::{active_holder, born, design, move_out, offered, refused, win_named};
use super::*;
use wire::rooms::{
    CandidateRef, CandidateReference, CandidateRetain, RetainedCandidate, RoomSource,
};

/// Add a task under `parent` through explicit preparation, or outside every
/// Design without one.
fn add_under(
    bus: &Bus,
    epoch: CanonicalUuid,
    parent: Option<&TaskHandle>,
    name: &str,
    flow: Option<&str>,
    intent: Option<wire::TaskIntent>,
) -> (TaskHandle, Outcome<CommandAnswer>) {
    let born = stamp(name);
    let mut expected = vec![Witness::Absence { task: born.clone() }];
    if let Some(parent) = parent {
        let current = board(bus, epoch, Actor::Operator);
        let heads = at(&current, parent);
        expected.extend([statement_witness(heads), workflow_witness(heads)]);
    }
    let outcome = issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        expected,
        &TaskAdd {
            preparation: parent.cloned(),
            intent,
            title: name.to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: vec!["The probe records what the idea showed".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some("fixture:probe".parse().expect("fixture origin")),
            flow: flow.map(str::to_owned),
            depends: Vec::new(),
            claim: false,
            draft: false,
        },
    );
    (TaskHandle::for_project("board.test", &born), outcome)
}

fn probe(bus: &Bus, epoch: CanonicalUuid, design: &TaskHandle, name: &str) -> TaskHandle {
    let (task, outcome) = add_under(bus, epoch, Some(design), name, Some("probe"), None);
    assert_committed(outcome);
    task
}

fn runnable(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle) -> bool {
    !open_blocked(bus, epoch).contains(&born(task))
}

#[test]
fn a_probe_is_created_only_as_a_bare_probe_under_an_open_active_design() {
    let (root, bus, epoch, _seats) = fixture("probe-creation", 1);
    let refusal =
        |parent: Option<&TaskHandle>, name: &str, flow: &str, intent: Option<wire::TaskIntent>| {
            let (_, outcome) = add_under(&bus, epoch, parent, name, Some(flow), intent);
            refused(&outcome).to_owned()
        };
    assert_eq!(
        refusal(None, "Unprepared probe", "probe", None),
        "task::probe-design",
        "a probe outside every Design refuses"
    );
    let task = design(&bus, epoch, "Probed design");
    assert_eq!(
        refusal(Some(&task), "Passive probe", "probe", None),
        "task::probe-design",
        "a passive Design admits no probe"
    );
    let plan = add_controller_plan(&bus, epoch);
    assert_eq!(
        refusal(Some(&plan), "Plan probe", "probe", None),
        "task::probe-design",
        "a Plan is no Design to probe for"
    );

    group_design(&bus, epoch, &task);
    assert_eq!(
        refusal(Some(&task), "Reviewed probe", "probe,review", None),
        "task::probe-flow"
    );
    assert_eq!(
        refusal(
            Some(&task),
            "Planning probe",
            "probe",
            Some(wire::TaskIntent::Plan)
        ),
        "task::probe-flow"
    );
    let admitted = probe(&bus, epoch, &task, "Admitted probe");
    assert!(
        runnable(&bus, epoch, &admitted),
        "an active Design's probe needs no execution authorization"
    );

    assert_committed(conclude(
        &bus,
        epoch,
        Actor::Operator,
        &task,
        done_conclusion(),
    ));
    assert_eq!(
        refusal(Some(&task), "Concluded probe", "probe", None),
        "task::preparation-refused",
        "a concluded Design is no longer a context to probe for"
    );
    clean(&root, bus);
}

fn add_controller_plan(bus: &Bus, epoch: CanonicalUuid) -> TaskHandle {
    super::super::preparation::add_controller(
        bus,
        epoch,
        "Probe plan",
        wire::TaskIntent::Plan,
        None,
    )
}

#[test]
fn a_probe_runs_only_while_its_design_is_open_and_grouped() {
    let (root, bus, epoch, seats) = fixture("probe-standing", 2);
    prove_seat_addresses(&bus, epoch, &seats);
    let task = design(&bus, epoch, "Standing design");
    group_design(&bus, epoch, &task);
    let designer = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &designer, &task);
    let trial = probe(&bus, epoch, &task, "Standing probe");
    assert_eq!(offered(&bus, epoch, &seats[1]), Some(born(&trial)));
    assert!(
        open_awaiting_selection(&bus, epoch).is_empty(),
        "a probe is no unselected output holding its Design"
    );

    // Out of every group: unoffered and uncapturable, yet a holder keeps it.
    assert_committed(move_out(&bus, epoch, &task).0);
    assert!(!runnable(&bus, epoch, &trial));
    assert_eq!(offered(&bus, epoch, &seats[1]), None);
    assert!(matches!(
        win_named(&bus, epoch, &seats[1], &trial, "capture"),
        Outcome::Refusal(_)
    ));
    group_design(&bus, epoch, &task);
    let prober = Actor::Lane(seats[1].lane);
    claim_task(&bus, epoch, &seats[1], &prober, &trial);
    assert_committed(move_out(&bus, epoch, &task).0);
    assert_eq!(
        active_holder(&bus, epoch, &trial),
        Some(prober.clone()),
        "deactivation leaves a held probe with its holder"
    );

    // Freed while passive it waits; re-entry offers it again.
    free_active(&bus, epoch, &prober, &seats[1], &trial);
    assert_eq!(offered(&bus, epoch, &seats[1]), None);
    group_design(&bus, epoch, &task);
    assert_eq!(offered(&bus, epoch, &seats[1]), Some(born(&trial)));

    // Conclusion ends it for good, even back in a group.
    free_active(&bus, epoch, &designer, &seats[0], &task);
    assert_committed(conclude(
        &bus,
        epoch,
        Actor::Operator,
        &task,
        done_conclusion(),
    ));
    assert!(!runnable(&bus, epoch, &trial));
    assert_eq!(offered(&bus, epoch, &seats[1]), None);
    clean(&root, bus);
}

#[test]
fn a_held_probe_refuses_repository_history() {
    let (root, bus, epoch, seats) = fixture("probe-commit-authority", 1);
    let task = design(&bus, epoch, "Commitless design");
    group_design(&bus, epoch, &task);
    let trial = probe(&bus, epoch, &task, "Commitless probe");
    assert_committed(win_named(&bus, epoch, &seats[0], &trial, "capture"));
    let held = board(&bus, epoch, Actor::Lane(seats[0].lane));
    let Err(outcome) = require_no_planning_implementation(&held, "task capture") else {
        panic!("a probe claim must refuse repository history");
    };
    assert_eq!(refused(&outcome), "task::planning-implementation");
    clean(&root, bus);
}

fn retain(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    controller: &TaskHandle,
    outputs: &[&TaskHandle],
    evidence: RoomSource,
) -> Outcome<CommandAnswer> {
    let current = board(bus, epoch, Actor::Lane(seat.lane));
    let candidate = CandidateRef {
        controller: born(controller),
        candidate: wire::rooms::RoomAuthorityStamp::for_micros(1_760_000_000_000_101)
            .expect("candidate identity"),
    };
    let mut expected = vec![
        Witness::Proposal {
            proposal: wire::rooms::ProposalRef::Candidate {
                candidate: candidate.clone(),
            },
            head: None,
        },
        statement_witness(at(&current, controller)),
    ];
    expected.extend(
        outputs
            .iter()
            .map(|output| statement_witness(at(&current, output))),
    );
    issue(
        bus,
        epoch,
        Actor::Lane(seat.lane),
        CommandKind::CandidateRetain,
        expected,
        &CandidateRetain {
            candidate: RetainedCandidate {
                candidate,
                governed_inputs: Vec::new(),
                outputs: outputs.iter().map(|output| born(output)).collect(),
                evidence: Vec::new(),
                references: vec![CandidateReference {
                    relation: "evidence".parse().expect("relation"),
                    source: evidence,
                }],
            },
        },
    )
}

#[test]
fn a_candidate_cites_probe_evidence_but_never_selects_a_probe() {
    let (root, bus, epoch, seats) = fixture("probe-candidate", 1);
    let task = design(&bus, epoch, "Evidence design");
    group_design(&bus, epoch, &task);
    let trial = probe(&bus, epoch, &task, "Evidence probe");
    let (output, added) = add_under(&bus, epoch, Some(&task), "Real output", None, None);
    assert_committed(added);
    let Outcome::Reply(Reply {
        position: ReplyPosition::Committed(noted),
        ..
    }) = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskNote,
        vec![Witness::Existence { task: born(&trial) }],
        &TaskNote {
            task: trial.clone(),
            note: "the idea held under load".to_owned(),
        },
    )
    else {
        panic!("probe note did not commit");
    };
    let finding = RoomSource::Journal {
        fact: wire::SequenceCode::new(noted.last.get()),
    };

    assert_eq!(
        refused(&retain(
            &bus,
            epoch,
            &seats[0],
            &task,
            &[&trial],
            finding.clone()
        )),
        "preparation::probe-output"
    );
    assert_committed(retain(&bus, epoch, &seats[0], &task, &[&output], finding));
    clean(&root, bus);
}
