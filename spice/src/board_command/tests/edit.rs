    /// Drive one correction through the door the CLI leaf reaches.
    ///
    /// The witnesses are read from the board immediately before the command, so
    /// a scenario that edits twice in a row observes the head the first edit
    /// published rather than a stale one.
    fn correct(
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
        let handle = task.to_string();
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
                        == Some(handle.as_str())
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
            after.body.uuid, before.body.uuid,
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
            },
        );
        assert!(
            matches!(
                outcome,
                Outcome::Refusal(Refusal { ref reason, .. }) if reason == "task::settled"
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
            },
        );
        assert!(
            matches!(
                outcome,
                Outcome::Refusal(Refusal { ref reason, .. }) if reason == "task::edit-empty"
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
            },
        );
        assert!(
            matches!(
                outcome,
                Outcome::Refusal(Refusal { ref reason, .. }) if reason == "task::unknown"
            ),
            "{outcome:?}"
        );
        assert!(edit_facts(&bus, &missing).is_empty());
    }
