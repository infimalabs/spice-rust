use super::*;

/// The refusal reason and the current digest it reports.
fn refused(
    outcome: Result<(CommandKind, Vec<Witness>, serde_json::Value), Outcome<CommandAnswer>>,
) -> (String, Option<String>) {
    match outcome {
        Err(Outcome::Refusal(refusal)) => (
            refusal.reason().to_owned(),
            refusal
                .diagnostic()
                .and_then(|diagnostic| diagnostic.observed())
                .map(str::to_owned),
        ),
        other => panic!("expected a refusal, found {other:?}"),
    }
}

fn submit(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    (kind, witnesses, body): (CommandKind, Vec<Witness>, serde_json::Value),
) -> Outcome<CommandAnswer> {
    issue(bus, epoch, actor, kind, witnesses, &body)
}

#[test]
fn a_replacement_lands_only_over_the_text_its_digest_names() {
    let (_root, bus, epoch, seats) = fixture("task-text-digest", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add_project_with_acceptance(
        &bus,
        epoch,
        "TEXT-1kKfYW01",
        "Replace by digest",
        "board.test",
        Some("todo"),
        &["the clause as first filed"],
    );
    let read = board(&bus, epoch, lane.clone());
    let heads = at(&read, &task).clone();
    let read_digest = TaskText::Description.digest(&heads.statement.body);

    let (reason, observed) = refused(text_edit(&heads, TaskText::Description, None, "new"));
    assert_eq!(reason, "task::text-digest-missing");
    assert_eq!(observed, Some(read_digest.to_string()));
    let elsewhere = Digest::compute(Domain::TaskText, b"text nobody read");
    let (reason, observed) = refused(text_edit(
        &heads,
        TaskText::Description,
        Some(&elsewhere),
        "new",
    ));
    assert_eq!(reason, "task::text-digest-moved");
    assert_eq!(observed, Some(read_digest.to_string()));

    let replacement = text_edit(
        &heads,
        TaskText::Description,
        Some(&read_digest),
        "what the task means\n",
    )
    .expect("the read's digest admits the replacement");
    assert_committed(submit(&bus, epoch, lane.clone(), replacement.clone()));
    let replaced = board(&bus, epoch, Actor::Operator);
    let statement = &at(&replaced, &task).statement.body;
    assert_eq!(
        statement.description.as_deref(),
        Some("what the task means"),
        "the written text replaces the description, less its final line break"
    );
    assert_eq!(statement.acceptance, ["the clause as first filed"]);

    let (reason, _) = refused(text_edit(
        at(&replaced, &task),
        TaskText::Description,
        Some(&read_digest),
        "a second writer's text",
    ));
    assert_eq!(
        reason, "task::text-digest-moved",
        "a digest read before a replacement never admits another"
    );
    assert!(
        matches!(
            &submit(&bus, epoch, lane, replacement),
            Outcome::Refusal(refusal) if refusal.reason() == "board::witness-moved"
        ),
        "a replacement mapped before the statement moved must not land"
    );
    let after = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&after, &task).statement.position,
        at(&replaced, &task).statement.position
    );
}

#[test]
fn a_settled_task_refuses_a_replacement_and_keeps_its_text() {
    let (_root, bus, epoch, seats) = fixture("task-text-settled", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "TEXT-1kKfYW02",
        "Settled work",
        Some("todo"),
    );
    claim_and_complete_task(&bus, epoch, &seats[0], &lane, &task);
    let settled = board(&bus, epoch, lane.clone());
    let heads = at(&settled, &task).clone();
    let replacement = text_edit(
        &heads,
        TaskText::Acceptance,
        Some(&TaskText::Acceptance.digest(&heads.statement.body)),
        "- rewrite what already shipped",
    )
    .expect("a current digest maps onto the modify intent");

    let outcome = submit(&bus, epoch, lane, replacement);
    assert!(
        matches!(&outcome, Outcome::Refusal(refusal) if refusal.reason() == "task::settled"),
        "{outcome:?}"
    );
    let after = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&after, &task).statement, heads.statement);
}

#[test]
fn acceptance_text_reads_back_as_the_items_it_names() {
    let (_root, bus, epoch, _seats) = fixture("task-text-acceptance", 0);
    let items = [
        "a clause a reviewer can check",
        "a clause spanning lines\n- whose second line looks like an item\n\nand survives a blank",
    ];
    let task = add_project_with_acceptance(
        &bus,
        epoch,
        "TEXT-1kKfYW03",
        "Acceptance as text",
        "board.test",
        Some("todo"),
        &items,
    );
    let read = board(&bus, epoch, Actor::Operator);
    let heads = at(&read, &task);
    let text = TaskText::Acceptance.current(&heads.statement.body);
    assert_eq!(
        acceptance_items(&text).expect("a read's text parses"),
        items
    );
    assert_eq!(
        acceptance_items("- one\n\n- two\n  continued\n\n").expect("items parse"),
        ["one", "two\ncontinued"]
    );
    assert!(acceptance_items("no bullet\n- item").is_err());

    let digest = TaskText::Acceptance.digest(&heads.statement.body);
    for empty in ["", "\n\n", "-\n- "] {
        let (reason, _) = refused(text_edit(heads, TaskText::Acceptance, Some(&digest), empty));
        assert_eq!(reason, "task::acceptance-empty", "{empty:?}");
    }
}
