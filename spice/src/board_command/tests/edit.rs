use super::*;

mod environment;

/// Drive one correction through the door the CLI leaf reaches.
///
/// The witnesses are read from the board immediately before the command, so
/// a scenario that edits twice in a row observes the head the first edit
/// published rather than a stale one.
pub(super) fn correct(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    edit: &TaskEdit,
) -> Outcome<CommandAnswer> {
    let visible = board(bus, epoch, actor.clone());
    let heads = at(&visible, &edit.task);
    issue(
        bus,
        epoch,
        actor,
        CommandKind::TaskEdit,
        vec![statement_witness(heads)],
        edit,
    )
}

fn edited_status(outcome: Outcome<CommandAnswer>) -> String {
    let CommandAnswer::Task(answer) = assert_committed(outcome) else {
        panic!("task edit answers with a task status");
    };
    answer.status
}

/// Facts recording a correction of this task, in journal order.
fn edit_facts(bus: &Bus, task: &TaskHandle) -> Vec<(Actor, u64)> {
    let born = task.born().expect("fixture handle carries a birth stamp");
    bus.fact_snapshot()
        .expect("edit facts read")
        .facts()
        .iter()
        .filter(|fact| {
            fact.fact.kind() == wire::FactKind::TaskEdited
                && fact
                    .fact
                    .body()
                    .get("task")
                    .and_then(serde_json::Value::as_str)
                    == Some(born.as_str())
        })
        .map(|fact| {
            (
                fact.actor.clone(),
                fact.fact
                    .body()
                    .get("revision")
                    .and_then(serde_json::Value::as_u64)
                    .expect("an edit fact carries its revision"),
            )
        })
        .collect()
}

pub(super) fn edit_bodies(bus: &Bus, task: &TaskHandle) -> Vec<(Actor, Seq, body::TaskEdited)> {
    let born = task.born().expect("fixture handle carries a birth stamp");
    bus.fact_snapshot()
        .expect("edit facts read")
        .facts()
        .iter()
        .filter_map(|fact| {
            fact.fact
                .read::<body::TaskEdited>()
                .filter(|edited| edited.task == born)
                .map(|edited| (fact.actor.clone(), fact.seq, edited))
        })
        .collect()
}

/// Shape CASE 146 / INVARIANTS ID-01: re-homing changes only the derived name,
/// so the handle a task was filed under keeps naming it by birth stamp.
#[test]
fn rehoming_rerenders_the_handle_without_migrating_identity() {
    let (_root, bus, epoch, _seats) = fixture("task-edit-rehome-identity", 0);
    let original = add_project(
        &bus,
        epoch,
        Actor::Operator,
        "ALPHA-1kKfYW09",
        "Move between projects",
        "board.alpha",
        Some("todo"),
    );
    let born = original
        .born()
        .expect("filed handle carries its sole identity");

    let expected = TaskHandle::for_project("board.beta", &born);
    let CommandAnswer::Task(edited) = assert_committed(correct(
        &bus,
        epoch,
        Actor::Operator,
        &TaskEdit {
            task: original.clone(),
            title: None,
            description: None,
            acceptance: Vec::new(),
            requirements: None,
            priority: None,
            project: Some("board.beta".to_owned()),
        },
    )) else {
        panic!("task edit answers with a task status");
    };
    assert_eq!(edited.status, "edited");
    assert_eq!(edited.task, expected);

    let answer = board(&bus, epoch, Actor::Operator);
    let moved = at(&answer, &expected);
    assert_eq!(moved.statement.body.born, born);
    assert_eq!(moved.statement.body.handle, expected);
    let stale = task_heads(&answer, &original).expect("the pre-move handle resolves by stamp");
    assert_eq!(stale.statement.body.handle, expected);
    let CommandAnswer::Task(noted) = assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskNote,
        vec![Witness::Existence { task: born.clone() }],
        &TaskNote {
            task: original.clone(),
            note: "named through the pre-move handle".to_owned(),
        },
    )) else {
        panic!("task note answers with a task status");
    };
    assert_eq!(noted.task, expected, "answers render the current handle");
    let CommandAnswer::Task(retitled) = assert_committed(correct(
        &bus,
        epoch,
        Actor::Operator,
        &TaskEdit {
            title: Some("Moved and still addressable".to_owned()),
            ..only(&original)
        },
    )) else {
        panic!("task edit answers with a task status");
    };
    assert_eq!(retitled.task, expected);
    assert_eq!(
        at(&board(&bus, epoch, Actor::Operator), &expected).presentation_title(),
        "Moved and still addressable",
    );
    let stored = statement_head(&bus, &expected);
    let stored = serde_json::to_value(stored.body).expect("stored head JSON");
    let stored = stored.as_object().expect("stored head object");
    assert!(!stored.contains_key("handle"));
    assert!(!stored.contains_key("uuid"));
}

/// A filed statement is correctable, and the correction is durable and
/// attributable.
///
/// Two different principals correct the same statement in turn, because one
/// editor cannot separate recorded attribution from a constant: a fact that
/// always spelled `Operator` would pass a single-editor test. The second
/// edit is also what makes the first one durable in the interesting sense --
/// a later revision must carry forward text it did not name, not silently
/// drop it.
#[test]
fn a_correction_publishes_a_new_revision_and_names_each_editor() {
    let (_root, bus, epoch, seats) = fixture("task-edit", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "EDITED-1kKfYW01",
        "Original title",
        "board.test",
        Some("todo"),
        &["the clause as first filed"],
    );
    let filed = board(&bus, epoch, Actor::Operator);
    let before = at(&filed, &task).statement.clone();
    assert_eq!(before.body.description, None);

    assert_eq!(
        edited_status(correct(
            &bus,
            epoch,
            Actor::Operator,
            &TaskEdit {
                task: task.clone(),
                title: None,
                description: Some("what the task actually means".to_owned()),
                acceptance: Vec::new(),
                requirements: None,
                priority: None,
                project: None,
            },
        )),
        "edited"
    );
    assert_eq!(
        edited_status(correct(
            &bus,
            epoch,
            lane.clone(),
            &TaskEdit {
                task: task.clone(),
                title: None,
                description: None,
                acceptance: vec![
                    "a clause a reviewer can check".to_owned(),
                    "and a second the first one missed".to_owned(),
                ],
                requirements: None,
                priority: None,
                project: None,
            },
        )),
        "edited"
    );

    let corrected = board(&bus, epoch, Actor::Operator);
    let row = at(&corrected, &task).clone();
    let after = row.statement.clone();
    assert_eq!(
        after.body.description.as_deref(),
        Some("what the task actually means"),
        "the acceptance edit must carry the earlier description forward",
    );
    assert_eq!(
        after.body.acceptance,
        vec![
            "a clause a reviewer can check".to_owned(),
            "and a second the first one missed".to_owned(),
        ],
        "a named acceptance replaces the filed clauses outright",
    );
    assert_eq!(
        row.presentation_title(),
        "Original title",
        "a field no correction named must not move",
    );
    assert_eq!(after.body.project, before.body.project);
    assert_eq!(
        after.body.born, before.body.born,
        "a correction edits the task rather than replacing it",
    );
    assert!(
        after.position > before.position,
        "a correction publishes a new statement head: {:?} then {:?}",
        before.position,
        after.position,
    );

    let editors = edit_facts(&bus, &task);
    assert_eq!(
        editors.iter().map(|(actor, _)| actor).collect::<Vec<_>>(),
        vec![&Actor::Operator, &lane],
        "each correction records the principal that made it",
    );
    assert!(
        editors[0].1 < editors[1].1,
        "the recorded revision advances with each correction: {editors:?}",
    );
}

#[test]
fn a_live_claimed_correction_carries_bounded_contract_changes_without_moving_ownership() {
    let (_root, bus, epoch, seats) = fixture("task-edit-claim-notice", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "EDITED-1kKfYW11",
        "Original title",
        "board.test",
        Some("todo"),
        &["original acceptance"],
    );
    claim_task(&bus, epoch, &seats[0], &lane, &task);
    let held = board(&bus, epoch, Actor::Operator);
    let before = at(&held, &task).clone();

    assert_eq!(
        edited_status(correct(
            &bus,
            epoch,
            lane.clone(),
            &TaskEdit {
                task: task.clone(),
                title: Some("Corrected title".to_owned()),
                description: Some("Corrected description".to_owned()),
                acceptance: vec!["corrected acceptance".to_owned()],
                requirements: None,
                priority: Some("high".to_owned()),
                project: Some("board.identity".to_owned()),
            },
        )),
        "edited",
    );

    let edits = edit_bodies(&bus, &task);
    let [(editor, edit, edited)] = edits.as_slice() else {
        panic!("one typed edit fact is retained");
    };
    assert_eq!(
        editor, &lane,
        "a claimant can receive its own contract edit"
    );
    let notice = edited
        .notice
        .as_ref()
        .expect("a live claim receives notice authority");
    assert_eq!(notice.lane, seats[0].lane);
    assert_eq!(notice.previous, before.statement.position);
    assert_eq!(
        notice.claim,
        before.claim.as_ref().expect("claim head").body.claimed
    );
    assert_eq!(
        notice.handle,
        TaskHandle::for_project("board.identity", &task.born().expect("task stamp")),
    );
    assert_eq!(
        notice
            .changes
            .iter()
            .map(|change| change.field.as_str())
            .collect::<Vec<_>>(),
        ["title", "description", "acceptance", "priority", "project"],
    );
    assert_eq!(notice.changes[0].before, "Original title");
    assert_eq!(notice.changes[0].after, "Corrected title");
    assert_eq!(notice.changes[3].before, "none");
    assert_eq!(notice.changes[3].after, "high");
    assert!(
        notice.previous < *edit,
        "the exact statement revision advances"
    );

    let corrected = board(&bus, epoch, Actor::Operator);
    let moved = at(&corrected, &notice.handle);
    assert_eq!(
        moved.claim, before.claim,
        "the edit preserves the claim head"
    );
    assert_eq!(
        moved.lease, before.lease,
        "the edit preserves the lease head"
    );
}

#[test]
fn a_preclaim_correction_establishes_the_future_claimants_baseline() {
    let (_root, bus, epoch, _seats) = fixture("task-edit-before-claim", 1);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "EDITED-1kKfYW12",
        "Original title",
        Some("todo"),
    );
    assert_eq!(
        edited_status(correct(
            &bus,
            epoch,
            Actor::Operator,
            &TaskEdit {
                title: Some("Claimant baseline".to_owned()),
                ..only(&task)
            },
        )),
        "edited",
    );
    let edits = edit_bodies(&bus, &task);
    let [(_, _, edited)] = edits.as_slice() else {
        panic!("one baseline edit fact is retained");
    };
    assert_eq!(edited.notice, None);
}

/// A settled statement is history, so correcting it is refused.
#[test]
fn a_settled_task_refuses_a_correction() {
    let (_root, bus, epoch, seats) = fixture("task-edit-settled", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "EDITED-1kKfYW02",
        "Settled work",
        Some("todo"),
    );
    claim_and_complete_task(&bus, epoch, &seats[0], &lane, &task);
    let settled = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&settled, &task).workflow.body.status, "completed");
    let before = at(&settled, &task).statement.clone();

    let outcome = correct(
        &bus,
        epoch,
        Actor::Operator,
        &TaskEdit {
            task: task.clone(),
            title: Some("Rewrite what already shipped".to_owned()),
            description: None,
            acceptance: Vec::new(),
            requirements: None,
            priority: None,
            project: None,
        },
    );
    assert!(
        matches!(
            outcome,
            Outcome::Refusal(ref refusal) if refusal.reason() == "task::settled"
        ),
        "{outcome:?}"
    );
    let after = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&after, &task).presentation_title(), "Settled work");
    assert_eq!(at(&after, &task).statement.position, before.position);
    assert!(edit_facts(&bus, &task).is_empty());
}

/// A correction naming no field is refused rather than published.
///
/// Publishing it would cost a revision and an entry in a record other lanes
/// read, for a statement nobody changed, so the position must not move.
#[test]
fn a_correction_naming_no_field_is_refused() {
    let (_root, bus, epoch, _seats) = fixture("task-edit-empty", 1);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "EDITED-1kKfYW03",
        "Filed correctly the first time",
        Some("todo"),
    );
    let filed = board(&bus, epoch, Actor::Operator);
    let before = at(&filed, &task).statement.clone();

    let outcome = correct(
        &bus,
        epoch,
        Actor::Operator,
        &TaskEdit {
            task: task.clone(),
            title: None,
            description: None,
            acceptance: Vec::new(),
            requirements: None,
            priority: None,
            project: None,
        },
    );
    assert!(
        matches!(
            outcome,
            Outcome::Refusal(ref refusal) if refusal.reason() == "task::edit-empty"
        ),
        "{outcome:?}"
    );
    let after = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&after, &task).statement.position,
        before.position,
        "a refused correction must not cost a revision",
    );
    assert!(edit_facts(&bus, &task).is_empty());
}

/// A handle no board carries is refused before any head is read.
#[test]
fn a_correction_to_an_unknown_task_is_refused() {
    let (_root, bus, epoch, _seats) = fixture("task-edit-unknown", 1);
    let known = add(
        &bus,
        epoch,
        Actor::Operator,
        "EDITED-1kKfYW04",
        "The only filed task",
        Some("todo"),
    );
    let visible = board(&bus, epoch, Actor::Operator);
    let heads = at(&visible, &known);
    let missing: TaskHandle = "EDITED-1kKfYW05".parse().expect("absent fixture handle");
    let outcome = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskEdit,
        vec![statement_witness(heads)],
        &TaskEdit {
            task: missing.clone(),
            title: Some("Correct a task that was never filed".to_owned()),
            description: None,
            acceptance: Vec::new(),
            requirements: None,
            priority: None,
            project: None,
        },
    );
    assert!(
        matches!(
            outcome,
            Outcome::Refusal(ref refusal) if refusal.reason() == "task::unknown"
        ),
        "{outcome:?}"
    );
    assert!(edit_facts(&bus, &missing).is_empty());
}

/// A correction naming one field, with every other field left alone.
fn only(task: &TaskHandle) -> TaskEdit {
    TaskEdit {
        task: task.clone(),
        title: None,
        description: None,
        acceptance: Vec::new(),
        requirements: None,
        priority: None,
        project: None,
    }
}

/// The task this actor's allocator would hand out now, and what it scored.
///
/// This is the ranking itself rather than a field read back off the row, so
/// a correction that stored a priority the allocator never scores would
/// leave the answer unmoved.
pub(super) fn allocated(bus: &Bus, epoch: CanonicalUuid, actor: &Actor) -> Option<(String, u64)> {
    let answer = board(bus, epoch, actor.clone());
    let entries = actor_entries(&answer).expect("allocation board entries validate");
    let subscription = allocation_subscription(&answer.residency.body, actor)
        .expect("allocation board has one team subscription");
    board::select(
        &entries,
        actor,
        &subscription,
        answer.allocation_band,
        answer.recent_cell.as_ref(),
        &answer.environment(),
    )
    .map(|selection| {
        (
            selection.entry().handle().to_string(),
            selection.urgency().value(),
        )
    })
}

/// The statement head the journal carries, read without a board answer.
///
/// A recorded revision points at a durable head, so a test proving the
/// record covers a field has to read the record rather than the rendered
/// row that a board answer would hand back.
fn statement_head(bus: &Bus, task: &TaskHandle) -> Positioned<TaskHead> {
    let snapshot = bus.task_snapshot().expect("statement head reads");
    let heads = snapshot
        .tasks()
        .iter()
        .find(|heads| {
            task.born()
                .is_ok_and(|stamp| heads.statement().body().born == stamp)
        })
        .expect("the journal carries this task");
    positioned(heads.statement())
}

/// Priority tags carried by the current statement, in stored order.
fn priority_tags(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle) -> Vec<String> {
    at(&board(bus, epoch, Actor::Operator), task)
        .statement
        .body
        .tags
        .iter()
        .filter(|tag| tag.starts_with(board::PRIORITY_TAG_PREFIX))
        .cloned()
        .collect()
}

/// A corrected priority moves the allocator's ranking of the task.
///
/// Both tasks are filed without a priority, so the allocator ranks them by
/// handle alone and hands out the lower one. That tie is what makes the
/// test able to fail: a correction the allocator never reads would leave
/// the same handle selected every time, and the filed order would stand
/// through all three corrections.
#[test]
fn a_corrected_priority_moves_the_allocator_ranking_off_the_filed_order() {
    let (_root, bus, epoch, seats) = fixture("task-edit-priority", 1);
    let lane = Actor::Lane(seats[0].lane);
    // The handles decide the filed order: EDITED-1kKg6RB1 sorts first, so
    // the allocator hands it out until a priority says otherwise.
    let first = add(
        &bus,
        epoch,
        Actor::Operator,
        "EDITED-1kKg6RB1",
        "Filed first and ranked first",
        Some("todo"),
    );
    let second = add(
        &bus,
        epoch,
        Actor::Operator,
        "EDITED-1kKg6RB2",
        "Filed second and ranked second",
        Some("todo"),
    );
    let filed = allocated(&bus, epoch, &lane).expect("a filed task is allocatable");
    assert_eq!(
        filed.0,
        first.to_string(),
        "with no priority declared the allocator ranks by handle",
    );

    assert_eq!(
        edited_status(correct(
            &bus,
            epoch,
            Actor::Operator,
            &TaskEdit {
                priority: Some("high".to_owned()),
                ..only(&second)
            },
        )),
        "edited"
    );
    let raised = allocated(&bus, epoch, &lane).expect("a corrected task is allocatable");
    assert_eq!(
        raised.0,
        second.to_string(),
        "the correction has to outrank the filed order, not sit beside it",
    );
    assert!(
        filed.1 > raised.1,
        "effective priority outranks the newer task's lower queue age without being folded into it",
    );

    // The first task overtakes it, so the ranking is following each
    // correction rather than having latched onto the second task once.
    assert_eq!(
        edited_status(correct(
            &bus,
            epoch,
            Actor::Operator,
            &TaskEdit {
                priority: Some("critical".to_owned()),
                ..only(&first)
            },
        )),
        "edited"
    );
    assert_eq!(
        allocated(&bus, epoch, &lane)
            .expect("a corrected task is allocatable")
            .0,
        first.to_string(),
    );

    // A second correction of the same task replaces its declaration rather
    // than adding one: two priority tags would let the allocator keep
    // scoring the stronger one it already stored.
    assert_eq!(
        edited_status(correct(
            &bus,
            epoch,
            Actor::Operator,
            &TaskEdit {
                priority: Some("none".to_owned()),
                ..only(&first)
            },
        )),
        "edited"
    );
    assert_eq!(
        priority_tags(&bus, epoch, &first),
        vec!["priority:none".to_owned()],
        "a corrected task carries exactly one priority declaration",
    );
    assert_eq!(
        allocated(&bus, epoch, &lane)
            .expect("a corrected task is allocatable")
            .0,
        second.to_string(),
        "lowering a priority has to give the ranking back",
    );
}

/// A priority the allocator cannot score is refused rather than stored.
///
/// Storing it would rank the task as though it had declared nothing, which
/// is the silent form of the failure the correction exists to repair.
#[test]
fn a_priority_the_allocator_does_not_score_is_refused() {
    let (_root, bus, epoch, _seats) = fixture("task-edit-priority-unknown", 1);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "EDITED-1kKg6RB3",
        "Filed without a priority",
        Some("todo"),
    );
    let filed = board(&bus, epoch, Actor::Operator);
    let before = at(&filed, &task).statement.clone();

    let outcome = correct(
        &bus,
        epoch,
        Actor::Operator,
        &TaskEdit {
            priority: Some("urgent".to_owned()),
            ..only(&task)
        },
    );
    assert!(
        matches!(
            outcome,
            Outcome::Refusal(ref refusal) if refusal.reason() == "task::edit-priority-unknown"
        ),
        "{outcome:?}"
    );
    let after = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&after, &task).statement.position,
        before.position,
        "a refused priority must not cost a revision",
    );
    assert!(priority_tags(&bus, epoch, &task).is_empty());
    assert!(edit_facts(&bus, &task).is_empty());

    // The same word with the case a caller is likely to type is admitted,
    // and reaches the tag in the one spelling the allocator scores.
    assert_eq!(
        edited_status(correct(
            &bus,
            epoch,
            Actor::Operator,
            &TaskEdit {
                priority: Some("  High  ".to_owned()),
                ..only(&task)
            },
        )),
        "edited"
    );
    assert_eq!(
        priority_tags(&bus, epoch, &task),
        vec!["priority:high".to_owned()],
        "an authored priority is normalized before it is stored",
    );

    // Every letter the board renders is a spelling the door takes back, so
    // an agent correcting the row it just read does not have to translate.
    for (letter, stored) in [
        ("C", "priority:critical"),
        ("h", "priority:high"),
        ("M", "priority:medium"),
        ("L", "priority:low"),
        ("N", "priority:none"),
    ] {
        assert_eq!(
            edited_status(correct(
                &bus,
                epoch,
                Actor::Operator,
                &TaskEdit {
                    priority: Some(letter.to_owned()),
                    ..only(&task)
                },
            )),
            "edited",
            "the board prints {letter}, so the door has to take it",
        );
        assert_eq!(priority_tags(&bus, epoch, &task), vec![stored.to_owned()]);
    }
}

/// A correction cannot move a task onto a resident-owned hidden route.
///
/// `task add` refuses to file there, so a correction that reached further
/// than creation would be the way to put a manually filed row on a route
/// only the resident is supposed to write.
#[test]
fn a_correction_cannot_move_a_task_onto_a_hidden_route() {
    let (_root, bus, epoch, _seats) = fixture("task-edit-project-hidden", 1);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "EDITED-1kKg6RB4",
        "Filed on a public project",
        Some("todo"),
    );
    let filed = board(&bus, epoch, Actor::Operator);
    let before = at(&filed, &task).statement.clone();

    for destination in [".oops", ".sandbox.secret"] {
        let outcome = correct(
            &bus,
            epoch,
            Actor::Operator,
            &TaskEdit {
                project: Some(destination.to_owned()),
                ..only(&task)
            },
        );
        assert!(
            matches!(
                outcome,
                Outcome::Refusal(ref refusal)
                    if refusal.reason() == "task::project-system-reserved"
            ),
            "{destination} was not refused by name: {outcome:?}"
        );
    }

    // A public project this board does not know is refused too, under a
    // reason that says the destination rather than the route it protects.
    let outcome = correct(
        &bus,
        epoch,
        Actor::Operator,
        &TaskEdit {
            project: Some("nowhere.at.all".to_owned()),
            ..only(&task)
        },
    );
    assert!(
        matches!(
            outcome,
            Outcome::Refusal(ref refusal)
                if refusal.reason() == "task::edit-project-refused"
        ),
        "{outcome:?}"
    );

    let after = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&after, &task).statement.body.project, "board.test");
    assert_eq!(
        at(&after, &task).statement.position,
        before.position,
        "a refused move must not cost a revision",
    );
    assert!(edit_facts(&bus, &task).is_empty());
}

/// Project correction does not move immutable creator-team allocation scope.
#[test]
fn a_corrected_project_keeps_creator_team_allocation() {
    let (_root, bus, epoch, seats) = fixture("task-edit-project-scope", 1);
    let lane = Actor::Lane(seats[0].lane);
    authorize_lane_creation(&bus, epoch, &seats[0]);
    subscription::configure_subscription(&bus, epoch, &seats[0], wire::intent::TeamLife::Drive);
    let task = add(
        &bus,
        epoch,
        lane.clone(),
        "EDITED-1kKg6RB5",
        "Filed under the wrong project",
        Some("todo"),
    );
    assert_eq!(
        subscription::selectable_handles(&board(&bus, epoch, lane.clone()), &lane),
        [task.to_string()]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        "the creating team owns the row before its project changes",
    );

    assert_eq!(
        edited_status(correct(
            &bus,
            epoch,
            lane.clone(),
            &TaskEdit {
                project: Some("rim.elsewhere".to_owned()),
                ..only(&task)
            },
        )),
        "edited"
    );

    let moved = board(&bus, epoch, lane.clone());
    let moved_handle = TaskHandle::for_project(
        "rim.elsewhere",
        &task.born().expect("fixture handle carries a birth stamp"),
    );
    assert_eq!(
        at(&moved, &moved_handle).statement.body.project,
        "rim.elsewhere",
        "the move is a correction, not a refusal",
    );
    assert_eq!(
        subscription::selectable_handles(&moved, &lane),
        [moved_handle.to_string()]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        "project text does not change creator-team scope",
    );
    assert_eq!(
        allocated(&bus, epoch, &lane).map(|(task, _)| task),
        Some(moved_handle.to_string()),
        "the same Drive team can still receive the corrected task",
    );

    let listed = task_list(
        &moved,
        &TaskListOptions {
            all: false,
            project: None,
            limit: None,
            status: None,
        },
    );
    assert!(
        listed.contains(&format!("{moved_handle} [todo] rim.elsewhere")),
        "project-independent allocation does not narrow ordinary public reading: {listed}",
    );
}

/// The revision and attribution a correction publishes cover the widened
/// fields, read off the durable facts rather than the board.
///
/// Each correction here names one widened field and nothing else, so a
/// revision or an editor recorded for prose alone would leave both facts
/// missing. Two different principals make them, because one editor cannot
/// separate recorded attribution from a constant.
#[test]
fn a_priority_or_project_correction_records_its_revision_and_editor() {
    let (_root, bus, epoch, seats) = fixture("task-edit-widened-facts", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "EDITED-1kKg6RB6",
        "Filed at the wrong priority and under the wrong project",
        Some("todo"),
    );
    let filed = statement_head(&bus, &task);

    assert_eq!(
        edited_status(correct(
            &bus,
            epoch,
            lane.clone(),
            &TaskEdit {
                priority: Some("critical".to_owned()),
                ..only(&task)
            },
        )),
        "edited"
    );
    assert_eq!(
        edited_status(correct(
            &bus,
            epoch,
            Actor::Operator,
            &TaskEdit {
                project: Some("rim.elsewhere".to_owned()),
                ..only(&task)
            },
        )),
        "edited"
    );

    let editors = edit_facts(&bus, &task);
    assert_eq!(
        editors.iter().map(|(actor, _)| actor).collect::<Vec<_>>(),
        vec![&lane, &Actor::Operator],
        "a priority correction and a project correction each record their editor",
    );
    assert!(
        editors[0].1 < editors[1].1,
        "the recorded revision advances with each correction: {editors:?}",
    );

    // The head the journal carries, not the board answer, is what the
    // recorded revisions point at.
    let corrected = statement_head(&bus, &task);
    assert!(corrected.position > filed.position);
    assert_eq!(corrected.body.tags, vec!["priority:critical".to_owned()]);
    assert_eq!(corrected.body.project, "rim.elsewhere");
    assert_eq!(
        corrected.body.title, filed.body.title,
        "a widened correction carries the prose it did not name forward",
    );
    assert_eq!(corrected.body.born, filed.body.born);
}
