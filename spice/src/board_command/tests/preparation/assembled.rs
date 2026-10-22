//! The Design pass proposal `preparation propose` assembles from what the
//! pass built under the Design and what the Design's room holds.

use super::contribution::{contribute, contributed, contribution_body};
use super::*;

use crate::board_command::client::{PassRecord, assemble, held_design, pass_record};
use wire::rooms::{DesignPassBody, DesignPassRecommendation, RoomContributionKind, RoomPost};

/// Add `name` under `design` through explicit preparation, after `depends`.
fn add_output_after(
    bus: &Bus,
    epoch: CanonicalUuid,
    design: &TaskHandle,
    name: &str,
    depends: Vec<TaskHandle>,
) -> TaskHandle {
    let born = stamp(name);
    let current = board(bus, epoch, Actor::Operator);
    let parent = at(&current, design);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![
            Witness::Absence { task: born.clone() },
            statement_witness(parent),
            workflow_witness(parent),
        ],
        &TaskAdd {
            preparation: Some(design.clone()),
            intent: None,
            title: name.to_owned(),
            pitch: "This work keeps the fixture honest.".to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: vec!["The output is delivered".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some(fixture_origin()),
            flow: Some("todo".to_owned()),
            depends,
            claim: false,
            draft: false,
        },
    ));
    TaskHandle::for_project("board.test", &born)
}

/// The refusal reason and the subject `assembled` named.
fn refused(assembled: Result<DesignPassBody, Outcome<CommandAnswer>>) -> (String, String) {
    let Err(Outcome::Refusal(refusal)) = assembled else {
        panic!("assembly must refuse: {assembled:?}");
    };
    let subject = refusal
        .diagnostic()
        .and_then(|diagnostic| diagnostic.observed())
        .unwrap_or_default()
        .to_owned();
    (refusal.reason().to_owned(), subject)
}

/// A held Design whose pass built two outputs, the first after a pending
/// input, beside a pending task that came from elsewhere.
struct Built {
    design: TaskHandle,
    input: TaskHandle,
    stray: TaskHandle,
    first: TaskHandle,
    second: TaskHandle,
}

fn built_pass(bus: &Bus, epoch: CanonicalUuid, seat: &Seat) -> Built {
    let design = add_controller(
        bus,
        epoch,
        "Assembled Design",
        wire::TaskIntent::Design,
        None,
    );
    group_design(bus, epoch, &design);
    let input = add(
        bus,
        epoch,
        Actor::Operator,
        "ASM-input",
        "Pending input",
        Some("todo"),
    );
    let stray = add(
        bus,
        epoch,
        Actor::Operator,
        "ASM-stray",
        "Work with its own origin",
        Some("todo"),
    );
    let first = add_output_after(bus, epoch, &design, "Assembled first", vec![input.clone()]);
    let second = add_output_after(bus, epoch, &design, "Assembled second", Vec::new());
    claim_task(bus, epoch, seat, &Actor::Lane(seat.lane), &design);
    Built {
        design,
        input,
        stray,
        first,
        second,
    }
}

/// The operator's post in `design`'s room, by its publication stamp.
fn operator_post(bus: &Bus, epoch: CanonicalUuid, design: &TaskHandle) -> RoomAuthorityStamp {
    let heads = at(&board(bus, epoch, Actor::Operator), design).clone();
    let command = fixture_command(
        epoch,
        Actor::Operator,
        CommandKind::RoomPost,
        vec![statement_witness(&heads)],
        &RoomPost {
            room: design.clone(),
            text: "The operator's framing.".to_owned(),
        },
    );
    match assert_committed(dispatch(bus, &command).expect("room post dispatches")) {
        CommandAnswer::RoomPost(post) => post.publication,
        other => panic!("room post answered {other:?}"),
    }
}

fn stamp_of(task: &TaskHandle) -> wire::TaskStamp {
    task.born().expect("task stamp")
}

fn relation(word: &str) -> wire::Name {
    word.parse().expect("relation")
}

fn relation_of(body: &DesignPassBody, source: &RoomSource) -> Option<String> {
    body.references
        .iter()
        .find(|reference| reference.source == *source)
        .map(|reference| reference.relation.to_string())
}

#[test]
fn a_pass_assembles_what_it_built_and_what_its_room_holds() {
    let (root, bus, epoch, seats) = fixture("preparation-assembled-pass", 1);
    let lane = Actor::Lane(seats[0].lane);
    let built = built_pass(&bus, epoch, &seats[0]);
    let born = stamp_of(&built.design);
    let charge = |kind, text: &str, sources| {
        contributed(contribute(
            &bus,
            epoch,
            root.as_ref(),
            lane.clone(),
            contribution_body(&built.design, kind, text, sources),
        ))
        .source
    };
    let support = charge(RoomContributionKind::charge(1), "Supports.", Vec::new());
    let oppose = charge(RoomContributionKind::charge(-1), "Opposes.", Vec::new());
    let measured = journal_source(&board(&bus, epoch, Actor::Operator), &built.input);
    let level = charge(RoomContributionKind::charge(0), "Observes.", vec![measured]);
    let asked = charge(RoomContributionKind::Question, "Asks.", Vec::new());
    let post = operator_post(&bus, epoch, &built.design);

    // A cite relabels an assembled contribution, and one naming the
    // operator's post becomes the journal fact it is.
    let cites = vec![
        CandidateReference {
            relation: relation("framing"),
            source: RoomSource::Contribution {
                room: born.clone(),
                publication: post,
            },
        },
        CandidateReference {
            relation: relation("evidence"),
            source: support.clone(),
        },
    ];
    let (record, head, cites) = bus
        .read(move |read| {
            let mut cites = cites;
            pass_record(read, &born, &mut cites).map(|(record, head)| (record, head, cites))
        })
        .expect("the pass record reads");
    assert!(head.is_some(), "the room's newest publication is witnessed");
    let RoomSource::Journal { fact: post_fact } = cites[0].source else {
        panic!("an operator post cites as its journal fact: {:?}", cites[0]);
    };
    let current = board(&bus, epoch, lane);
    let held = held_design(&current, None).expect("the pass holds its Design");
    let body = assemble(
        held,
        &current.tasks,
        &record,
        None,
        cites,
        "The built outputs answer the Design.".to_owned(),
    )
    .expect("the pass assembles");
    assert_eq!(body.recommendation, DesignPassRecommendation::Work);
    assert_eq!(body.outputs.len(), 2, "{:?}", body.outputs);
    assert!(body.outputs.contains(&stamp_of(&built.first)));
    assert!(body.outputs.contains(&stamp_of(&built.second)));
    assert!(!body.outputs.contains(&stamp_of(&built.stray)));
    assert_eq!(body.governed_inputs, vec![stamp_of(&built.input)]);
    for (source, expected) in [
        (&support, "evidence"),
        (&oppose, "against"),
        (&level, "evidence"),
        (&asked, "question"),
        (&RoomSource::Journal { fact: post_fact }, "framing"),
    ] {
        assert_eq!(relation_of(&body, source).as_deref(), Some(expected));
    }
    assert_eq!(body.references.len(), 5);

    // A decision that resolved an output leaves it out of later passes,
    // with the inputs only it governed; the decision is itself a reference.
    let decided = PassRecord {
        contributions: record.contributions,
        decisions: vec![(head.expect("room head"), vec![stamp_of(&built.first)])],
    };
    let later = assemble(
        held,
        &current.tasks,
        &decided,
        None,
        Vec::new(),
        "Only the second output remains.".to_owned(),
    )
    .expect("a later pass assembles");
    assert_eq!(later.outputs, vec![stamp_of(&built.second)]);
    assert!(later.governed_inputs.is_empty());
    assert!(
        later
            .references
            .iter()
            .any(|reference| reference.relation.to_string() == "decision")
    );
    clean(&root, bus);
}

#[test]
fn each_assembly_refusal_names_what_the_pass_must_change() {
    let (root, bus, epoch, seats) = fixture("preparation-assembled-refusals", 1);
    let built = built_pass(&bus, epoch, &seats[0]);
    let current = board(&bus, epoch, Actor::Lane(seats[0].lane));
    let held = held_design(&current, None).expect("the pass holds its Design");
    let decided = PassRecord {
        contributions: Vec::new(),
        decisions: vec![(held.statement.position, Vec::new())],
    };
    let attempt = |tasks: &[BoardTask], record: &PassRecord, recommend| {
        assemble(
            held,
            tasks,
            record,
            recommend,
            Vec::new(),
            "Justified.".to_owned(),
        )
    };
    let (reason, subject) = refused(attempt(
        &current.tasks,
        &decided,
        Some(DesignPassRecommendation::Conclude),
    ));
    assert_eq!(reason, "preparation::recommend-with-outputs");
    assert!(subject.contains(&built.first.to_string()), "{subject}");

    let outputs = [stamp_of(&built.first), stamp_of(&built.second)];
    let unbuilt = current
        .tasks
        .iter()
        .filter(|task| !outputs.contains(&task.statement.body.born))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        refused(attempt(&unbuilt, &decided, None)).0,
        "preparation::proposal-empty"
    );
    let collapse = attempt(&unbuilt, &decided, Some(DesignPassRecommendation::Collapse))
        .expect("a pass that built nothing recommends collapse");
    assert_eq!(collapse.recommendation, DesignPassRecommendation::Collapse);
    assert!(collapse.outputs.is_empty() && collapse.governed_inputs.is_empty());

    let mut ungated = current.tasks.clone();
    let first = ungated
        .iter_mut()
        .find(|task| task.statement.body.born == outputs[0])
        .expect("first output");
    first
        .statement
        .body
        .dependencies
        .retain(|dependency| *dependency != stamp_of(&built.design));
    let (reason, subject) = refused(attempt(&ungated, &decided, None));
    assert_eq!(reason, "preparation::output-ungated");
    assert!(subject.contains(&built.first.to_string()), "{subject}");

    let mut unaccepted = current.tasks.clone();
    let second = unaccepted
        .iter_mut()
        .find(|task| task.statement.body.born == outputs[1])
        .expect("second output");
    second.statement.body.acceptance.clear();
    let (reason, subject) = refused(attempt(&unaccepted, &decided, None));
    assert_eq!(reason, "preparation::output-unaccepted");
    assert!(subject.contains(&built.second.to_string()), "{subject}");

    assert_eq!(
        refused(attempt(&current.tasks, &PassRecord::default(), None)).0,
        "preparation::proposal-unreferenced"
    );
    clean(&root, bus);
}
