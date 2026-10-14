//! Gather tasks: ordinary evidence-only tasks that run while the open Design
//! they were created under stands in a working group, with no commit authority
//! and no standing as candidate outputs.

use super::super::preparation::{conclude, done_conclusion};
use super::super::taste_review::{configured_repository, issue_repository, modify_with_repository};
use super::design_standing::{active_holder, born, design, move_out, offered, refused, win_named};
use super::*;
use wire::rooms::{
    CandidateRef, CandidateReference, CandidateRetain, RetainedCandidate, RoomSource,
};

/// The witnesses and request adding a task under `parent` through explicit
/// preparation, or outside every Design without one.
fn add_request(
    bus: &Bus,
    epoch: CanonicalUuid,
    parent: Option<&TaskHandle>,
    name: &str,
    flow: Option<&str>,
    intent: Option<wire::TaskIntent>,
) -> (TaskHandle, Vec<Witness>, TaskAdd) {
    let born = stamp(name);
    let mut expected = vec![Witness::Absence { task: born.clone() }];
    if let Some(parent) = parent {
        let current = board(bus, epoch, Actor::Operator);
        let heads = at(&current, parent);
        expected.extend([statement_witness(heads), workflow_witness(heads)]);
    }
    let add = TaskAdd {
        preparation: parent.cloned(),
        intent,
        title: name.to_owned(),
        project: Some("board.test".to_owned()),
        description: None,
        acceptance: vec!["The gather task records what the idea showed".to_owned()],
        requirements: wire::EnvironmentRequirements::default(),
        origin: Some("fixture:gather".parse().expect("fixture origin")),
        flow: flow.map(str::to_owned),
        depends: Vec::new(),
        claim: false,
        draft: false,
    };
    (TaskHandle::for_project("board.test", &born), expected, add)
}

fn add_under(
    bus: &Bus,
    epoch: CanonicalUuid,
    parent: Option<&TaskHandle>,
    name: &str,
    flow: Option<&str>,
    intent: Option<wire::TaskIntent>,
) -> (TaskHandle, Outcome<CommandAnswer>) {
    let (task, expected, add) = add_request(bus, epoch, parent, name, flow, intent);
    let outcome = issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        expected,
        &add,
    );
    (task, outcome)
}

/// Add a gather task under `design` where `repository`'s taste policy applies.
fn add_gather_with_policy(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    design: &TaskHandle,
    name: &str,
) -> (TaskHandle, Outcome<CommandAnswer>) {
    let (task, expected, add) = add_request(bus, epoch, Some(design), name, Some("gather"), None);
    let outcome = issue_repository(
        bus,
        repository,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        expected,
        &add,
    );
    (task, outcome)
}

fn gather(bus: &Bus, epoch: CanonicalUuid, design: &TaskHandle, name: &str) -> TaskHandle {
    let (task, outcome) = add_under(bus, epoch, Some(design), name, Some("gather"), None);
    assert_committed(outcome);
    task
}

fn runnable(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle) -> bool {
    !open_blocked(bus, epoch).contains(&born(task))
}

#[test]
fn a_gather_task_is_a_bare_gather_route_under_an_open_active_design() {
    let (root, bus, epoch, _seats) = fixture("gather-creation", 1);
    let refusal =
        |parent: Option<&TaskHandle>, name: &str, flow: &str, intent: Option<wire::TaskIntent>| {
            let (_, outcome) = add_under(&bus, epoch, parent, name, Some(flow), intent);
            refused(&outcome).to_owned()
        };
    assert_eq!(
        refusal(None, "Unprepared gather", "gather", None),
        "task::gather-design",
        "a gather task outside every Design refuses"
    );
    let task = design(&bus, epoch, "Gathering design");
    assert_eq!(
        refusal(Some(&task), "Passive gather", "gather", None),
        "task::gather-design",
        "a passive Design admits no gather task"
    );
    let plan = add_controller_plan(&bus, epoch);
    assert_eq!(
        refusal(Some(&plan), "Plan gather", "gather", None),
        "task::gather-design",
        "a Plan is no Design to gather for"
    );

    let outputs = design(&bus, epoch, "Output design");
    group_design(&bus, epoch, &outputs);
    for (name, flow, intent) in [
        ("Reviewed gather", "gather,review", None),
        ("Planning gather", "gather", Some(wire::TaskIntent::Plan)),
    ] {
        let (output, added) = add_under(&bus, epoch, Some(&outputs), name, Some(flow), intent);
        assert_committed(added);
        assert!(
            !runnable(&bus, epoch, &output),
            "any other route is an ordinary output gated on authorization"
        );
    }

    group_design(&bus, epoch, &task);
    let admitted = gather(&bus, epoch, &task, "Admitted gather");
    assert!(
        runnable(&bus, epoch, &admitted),
        "an active Design's gather task needs no execution authorization"
    );

    assert_committed(conclude(
        &bus,
        epoch,
        Actor::Operator,
        &task,
        done_conclusion(),
    ));
    assert_eq!(
        refusal(Some(&task), "Concluded gather", "gather", None),
        "task::preparation-refused",
        "a concluded Design is no longer a context to gather for"
    );
    clean(&root, bus);
}

fn add_controller_plan(bus: &Bus, epoch: CanonicalUuid) -> TaskHandle {
    super::super::preparation::add_controller(
        bus,
        epoch,
        "Gather plan",
        wire::TaskIntent::Plan,
        None,
    )
}

#[test]
fn a_gather_task_runs_only_while_its_design_is_open_and_grouped() {
    let (root, bus, epoch, seats) = fixture("gather-standing", 2);
    prove_seat_addresses(&bus, epoch, &seats);
    let task = design(&bus, epoch, "Standing design");
    group_design(&bus, epoch, &task);
    let designer = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &designer, &task);
    let trial = gather(&bus, epoch, &task, "Standing gather");
    assert_eq!(offered(&bus, epoch, &seats[1]), Some(born(&trial)));
    assert!(
        open_awaiting_selection(&bus, epoch).is_empty(),
        "a gather task is no unselected output holding its Design"
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
    let gatherer = Actor::Lane(seats[1].lane);
    claim_task(&bus, epoch, &seats[1], &gatherer, &trial);
    assert_committed(move_out(&bus, epoch, &task).0);
    assert_eq!(
        active_holder(&bus, epoch, &trial),
        Some(gatherer.clone()),
        "deactivation leaves a held gather task with its holder"
    );

    // Freed while passive it waits; re-entry offers it again.
    free_active(&bus, epoch, &gatherer, &seats[1], &trial);
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
fn a_held_gather_task_refuses_repository_history() {
    let (root, bus, epoch, seats) = fixture("gather-commit-authority", 1);
    let task = design(&bus, epoch, "Commitless design");
    group_design(&bus, epoch, &task);
    let trial = gather(&bus, epoch, &task, "Commitless gather");
    assert_committed(win_named(&bus, epoch, &seats[0], &trial, "capture"));
    let held = board(&bus, epoch, Actor::Lane(seats[0].lane));
    let Err(outcome) = require_no_planning_implementation(&held, "task capture") else {
        panic!("a gather claim must refuse repository history");
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

/// Record `text` on `trial` and return the exact source a candidate cites.
fn gathered(bus: &Bus, epoch: CanonicalUuid, trial: &TaskHandle, text: &str) -> RoomSource {
    let Outcome::Reply(Reply {
        position: ReplyPosition::Committed(noted),
        ..
    }) = issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskNote,
        vec![Witness::Existence { task: born(trial) }],
        &TaskNote {
            task: trial.clone(),
            note: text.to_owned(),
        },
    )
    else {
        panic!("gather note did not commit");
    };
    RoomSource::Journal {
        fact: wire::SequenceCode::new(noted.last.get()),
    }
}

#[test]
fn a_candidate_cites_gathered_evidence_but_never_selects_a_gather_task() {
    let (root, bus, epoch, seats) = fixture("gather-candidate", 1);
    let task = design(&bus, epoch, "Evidence design");
    group_design(&bus, epoch, &task);
    let trial = gather(&bus, epoch, &task, "Evidence gather");
    let (output, added) = add_under(&bus, epoch, Some(&task), "Real output", None, None);
    assert_committed(added);
    let finding = gathered(&bus, epoch, &trial, "the idea held under load");

    assert_eq!(
        refused(&retain(
            &bus,
            epoch,
            &seats[0],
            &task,
            &[&trial],
            finding.clone()
        )),
        "preparation::gather-output"
    );
    assert_committed(retain(&bus, epoch, &seats[0], &task, &[&output], finding));
    clean(&root, bus);
}

#[test]
fn a_taste_hold_never_changes_what_a_gather_task_is() {
    let (root, bus, epoch, seats) = fixture("gather-taste-hold", 1);
    let repository = configured_repository(&root, &["sloppy"]);
    let task = design(&bus, epoch, "Held design");
    let (_, passive) = add_gather_with_policy(&bus, &repository, epoch, &task, "Sloppy passive");
    assert_eq!(
        refused(&passive),
        "task::gather-design",
        "a held gather task still needs an active Design"
    );

    group_design(&bus, epoch, &task);
    let (trial, added) = add_gather_with_policy(&bus, &repository, epoch, &task, "Sloppy gather");
    assert_committed(added);
    let route = |bus: &Bus| {
        at(&board(bus, epoch, Actor::Operator), &trial)
            .workflow
            .body
            .flow
            .iter()
            .map(|phase| phase.as_str().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(route(&bus), ["plan", "gather"], "the hold waits it in plan");
    let finding = gathered(&bus, epoch, &trial, "the held idea still showed something");

    for standing in ["held", "cleared"] {
        assert!(
            open_awaiting_selection(&bus, epoch).is_empty(),
            "a {standing} gather task holds no Design awaiting selection"
        );
        assert_eq!(
            refused(&retain(
                &bus,
                epoch,
                &seats[0],
                &task,
                &[&trial],
                finding.clone()
            )),
            "preparation::gather-output",
            "a {standing} gather task is never a candidate output"
        );
        assert!(
            runnable(&bus, epoch, &trial),
            "a {standing} gather task runs"
        );
        if standing == "held" {
            assert_committed(modify_with_repository(
                &bus,
                &repository,
                epoch,
                Actor::Operator,
                &trial,
                "Careful gather",
            ));
            assert_eq!(route(&bus), ["gather"], "clearing returns its gather route");
        }
    }
    clean(&root, bus);
}
