use super::*;

struct AppliedDocument {
    preview: board::Preview,
    document: String,
    answer: wire::intent::DocAnswer,
    board: BoardAnswer,
}

#[test]
fn doc_leaf_owns_classification_and_normal_form_reasons() {
    let number = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "spice-board-command-doc-client-{}-{number}",
        std::process::id()
    ));
    fs::create_dir_all(root.join(".git")).expect("doc client fixture exists");
    let path = root.join("tasks.md");
    let repository = Repository::discover(&root).expect("doc client repository resolves");

    fs::write(&path, " \n\t\n").expect("refused document is written");
    let matches = crate::cli::command()
        .try_get_matches_from(["spice", "doc", &path.display().to_string()])
        .expect("doc leaf parses");
    let contract = crate::cli::selected_contract(&matches).expect("doc contract is selected");
    let leaf = crate::cli::selected_matches(&matches);
    let classification = run_doc(&repository, contract, leaf, dialect::emit);
    assert!(
        matches!(
            classification,
            Outcome::Refusal(ref refusal)
                if refusal.reason() == "doc::classification-refused"
        ),
        "doc leaf lost its classification reason: {classification:?}",
    );

    fs::write(&path, "# Goal\n\n- First task\n").expect("valid document is written");
    let matches = crate::cli::command()
        .try_get_matches_from(["spice", "doc", &path.display().to_string()])
        .expect("doc leaf parses");
    let contract = crate::cli::selected_contract(&matches).expect("doc contract is selected");
    let leaf = crate::cli::selected_matches(&matches);
    let normal_form = run_doc(&repository, contract, leaf, |_| " \n\t\n".to_owned());
    assert!(
        matches!(
            normal_form,
            Outcome::Fault(Fault {
                class: FaultClass::Protocol,
                ref reason,
                ..
            }) if reason == "doc::normal-form-invalid"
        ),
        "doc leaf lost its normal-form reason: {normal_form:?}",
    );

    fs::remove_dir_all(&root).expect("doc client fixture is removed");
}

#[test]
fn authored_document_apply_is_atomic_idempotent_and_barrier_bound() {
    let (root, bus, epoch, _) = fixture("document", 0);
    let initial_board = board(&bus, epoch, Actor::Operator);
    let initial = apply_document(
        &bus,
        epoch,
        "# Goal\n\n- First task\n  Acceptance: original acceptance\n  Flow: todo, review\n- Second task\n  After: first-task\n",
        &initial_board,
    );
    assert_initial_document(&bus, &initial);
    let edited = apply_document(
        &bus,
        epoch,
        "# Goal\n\n- First task\n  Flow: verify\n- Second task\n",
        &initial.board,
    );
    assert_edited_document(&bus, &initial, &edited);
    assert_idempotent_document(&bus, epoch, &edited);
    assert_stale_document_refused(&bus, epoch, &initial_board);
    assert_ledger_contains(&bus, epoch, &initial.answer.touched[1], "# First task");
    clean(&root, bus);
}

#[test]
fn authored_task_creation_omits_review_behind_review_dependencies_unless_its_flow_is_named() {
    let (root, bus, epoch, seats) = fixture("document-dependency-flow", 1);
    let initial_board = board(&bus, epoch, Actor::Operator);
    let initial = apply_document(
        &bus,
        epoch,
        "# Goal\n\n- Review prerequisite\n  Flow: todo, review\n",
        &initial_board,
    );
    let prerequisite = titled(&initial.board, "Review prerequisite")
        .statement
        .body
        .handle
        .clone();
    let lane = Actor::Lane(seats[0].lane);
    advance_task_to_review(&bus, epoch, &seats[0], &lane, &prerequisite);

    let review_board = board(&bus, epoch, Actor::Operator);
    let applied = apply_document(
        &bus,
        epoch,
        concat!(
            "# Goal\n\n",
            "- Review prerequisite\n  Flow: todo, review\n",
            "- Dependent correction\n",
            "  After: review-prerequisite\n",
            "- Material correction\n",
            "  After: review-prerequisite\n",
            "  Flow: plan, todo, review\n",
        ),
        &review_board,
    );
    let flow = |title: &str| {
        titled(&applied.board, title)
            .workflow
            .body
            .flow
            .iter()
            .map(|phase| phase.as_str().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(flow("Dependent correction"), ["plan", "todo"]);
    assert_eq!(flow("Material correction"), ["plan", "todo", "review"]);
    clean(&root, bus);
}

#[test]
fn document_rationale_and_later_notes_recover_through_the_task_ledger() {
    let (root, bus, epoch, _) = fixture("document-context-recovery", 0);
    let initial = board(&bus, epoch, Actor::Operator);
    let applied = apply_document(
        &bus,
        epoch,
        "# Goal\n\n- Recover compact context\n\n  Keep this rationale in the authored document instead of the task packet.\n\n  ```text\n  Imported decision with exact punctuation: yes.\n  ```\n",
        &initial,
    );
    let task = applied
        .answer
        .touched
        .last()
        .expect("document creates its child task")
        .clone();
    assert_eq!(
        at(&applied.board, &task)
            .statement
            .body
            .description
            .as_deref(),
        Some("Keep this rationale in the authored document instead of the task packet.")
    );
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskNote,
        vec![Witness::Existence {
            task: at(&applied.board, &task).statement.body.born.clone(),
        }],
        &TaskNote {
            task: task.clone(),
            note: "Later context remains append-only.".to_owned(),
        },
    ));

    let current = board(&bus, epoch, Actor::Operator);
    let shown = task_show(&current, at(&current, &task), &[]);
    assert!(shown.contains(&format!("spice doc ledger {task}")));
    assert!(
        !shown.contains("Imported decision with exact punctuation: yes.")
            && !shown.contains("Later context remains append-only."),
        "task detail points to document context without duplicating annotation or note bodies"
    );
    let ledger = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::DocLedger,
        Vec::new(),
        &DocLedger { task: task.clone() },
    );
    let Outcome::Reply(Reply {
        body: CommandAnswer::Text(TextAnswer { text }),
        ..
    }) = ledger
    else {
        panic!("task ledger returned another outcome: {ledger:?}");
    };
    assert!(text.contains("Keep this rationale in the authored document"));
    assert!(text.contains("Imported decision with exact punctuation: yes."));
    assert!(text.contains("Later context remains append-only."));
    let recovered = board::Preview::classify(&text).expect("recovered ledger remains canonical");
    let node = &recovered.document().nodes[recovered.document().root];
    assert_eq!(node.annotations.len(), 2);
    assert!(node.annotations[0].contains("Imported decision with exact punctuation: yes."));
    assert!(node.annotations[1].contains("Later context remains append-only."));
    clean(&root, bus);
}

#[test]
fn document_ledger_pages_one_snapshot_and_exports_the_exact_document() {
    let (root, bus, epoch, _) = fixture("document-ledger-output", 0);
    let task = add_project(
        &bus,
        epoch,
        Actor::Operator,
        "DOC-ledger01",
        "Export one ledger",
        "task.document",
        Some("todo"),
    );
    let document = format!("# Export one ledger\n\n{}\n", "é".repeat(4_500));
    let budget = wire::output::INTERACTIVE
        .budget(wire::output::Request::default())
        .expect("interactive defaults admit");
    let digest = Digest::compute(Domain::CliExport, document.as_bytes());
    let first = wire::output::clip(&document, budget.limits().bytes);
    let offset = first.text.len();
    let preview = ledger_output(&task, &document, None, budget, 0, None).expect("preview renders");
    assert!(preview.starts_with(&first.text), "{preview}");
    assert!(
        preview.contains(&format!(
            "continue: spice doc ledger {task} --max-bytes {} --offset {offset} --digest {digest}",
            budget.limits().bytes
        )),
        "{preview}"
    );
    assert!(
        preview.contains(&format!(
            "complete: spice doc ledger {task} --output {task}-ledger.md"
        )),
        "{preview}"
    );
    let remainder = ledger_output(&task, &document, None, budget, offset, Some(&digest))
        .expect("continuation renders");
    assert_eq!(format!("{}{remainder}", first.text), document);

    let moved = format!("{document}later");
    let refusal = ledger_output(&task, &moved, None, budget, offset, Some(&digest))
        .expect_err("a continuation never splices another ledger snapshot");
    assert_eq!(refusal.reason(), "doc::ledger-snapshot-moved");

    let destination = root.join("ledger.md");
    ledger_output(
        &task,
        &document,
        Some(destination.to_str().expect("fixture path is UTF-8")),
        budget,
        0,
        None,
    )
    .expect("ledger exports");
    assert_eq!(
        std::fs::read_to_string(destination).expect("exported ledger reads"),
        document
    );
    clean(&root, bus);
}

#[test]
fn authored_document_apply_refuses_to_edit_a_withdrawn_task() {
    let (root, bus, epoch, _) = fixture("document-withdrawn", 0);
    let initial_board = board(&bus, epoch, Actor::Operator);
    let initial = apply_document(
        &bus,
        epoch,
        "# Goal\n\n- First task\n  Priority: low\n",
        &initial_board,
    );
    let withdrawn = initial
        .answer
        .touched
        .last()
        .expect("document creates one authored task")
        .clone();
    let visible = board(&bus, epoch, Actor::Operator);
    let heads = at(&visible, &withdrawn);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDelete,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ],
        &TaskDelete {
            task: withdrawn,
            reason: "the document task was withdrawn".to_owned(),
            force_claimed: false,
        },
    ));

    let barrier = board(&bus, epoch, Actor::Operator);
    let changed = board::Preview::classify("# Goal\n\n- First task\n  Priority: high\n")
        .expect("changed document previews");
    let document = dialect::emit(changed.document());
    let changed = board::Preview::classify(&document).expect("normal document previews");
    let before = bus
        .fact_snapshot()
        .expect("facts before settled document refusal")
        .facts()
        .len();
    let outcome = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::DocApply,
        vec![
            Witness::Preview {
                digest: changed.digest().clone(),
            },
            Witness::Barrier {
                name: "board".parse().expect("board barrier name"),
                digest: board_barrier(&barrier),
            },
        ],
        &DocApply {
            preview: changed.digest().clone(),
            document,
        },
    );
    assert!(matches!(
        outcome,
        Outcome::Refusal(ref refusal) if refusal.reason() == "doc::settled-task"
    ));
    assert_eq!(
        bus.fact_snapshot()
            .expect("facts after settled document refusal")
            .facts()
            .len(),
        before,
        "a withdrawn document task refuses the whole authored batch",
    );
    clean(&root, bus);
}

#[test]
fn document_ledger_reports_a_board_flow_outside_its_closed_dialect() {
    let (root, bus, epoch, _) = fixture("document-ledger-vocabulary", 0);
    let task = add_raw_task(
        &bus,
        epoch,
        "DOCLEDGER-1kLGp001",
        "Deploy through an ordinary board phase",
        &["deployment is complete"],
        Some("deploy"),
    );

    let outcome = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::DocLedger,
        Vec::new(),
        &DocLedger { task },
    );
    assert!(matches!(
        outcome,
        Outcome::Fault(Fault {
            class: FaultClass::Store,
            ref reason,
            ..
        }) if reason == "doc::ledger-unrepresentable"
    ));
    clean(&root, bus);
}

#[test]
fn shape_cases_415_417_insert_plan_for_bare_capture_only() {
    let (root, bus, epoch, _) = fixture("plan-shape-cases", 0);
    let bare = add_raw_task(
        &bus,
        epoch,
        "PLAN-1kLGp001",
        "Shape 415 bare capture",
        &[],
        None,
    );
    let accepted = add_raw_task(
        &bus,
        epoch,
        "PLAN-1kLGp002",
        "Shape 416 accepted capture",
        &["accepted work can begin"],
        None,
    );
    let explicit = add_raw_task(
        &bus,
        epoch,
        "PLAN-1kLGp003",
        "Shape 417 explicit flow capture",
        &[],
        Some("todo"),
    );
    let shaped = board(&bus, epoch, Actor::Operator);
    assert_workflow(&shaped, &bare, "plan", &["plan", "todo", "review"]);
    assert_workflow(&shaped, &accepted, "todo", &["todo", "review"]);
    assert_workflow(&shaped, &explicit, "todo", &["todo"]);
    clean(&root, bus);
}

#[test]
fn shape_case_418_inserts_plan_without_review_for_bare_review_followup() {
    let (root, bus, epoch, seats) = fixture("plan-shape-followup", 2);
    authorize_lane_creation(&bus, epoch, &seats[1]);
    let lane_one = Actor::Lane(seats[0].lane);
    let lane_two = Actor::Lane(seats[1].lane);
    let reviewed = add_raw_task(
        &bus,
        epoch,
        "PLAN-1kLGp004",
        "Shape 418 reviewed task",
        &["review reaches follow-up creation"],
        Some("todo,review"),
    );
    advance_task_to_review(&bus, epoch, &seats[0], &lane_one, &reviewed);
    let assigned = board(&bus, epoch, lane_two.clone());
    assert_committed(issue(
        &bus,
        epoch,
        lane_two.clone(),
        CommandKind::ClaimWin,
        claim_expected(&assigned, &reviewed, &seats[1]),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    ));
    let assigned = board(&bus, epoch, lane_two.clone());
    let proposed: TaskHandle = "PLAN-1kLGp005".parse().expect("follow-up handle");
    let followup_stamp = proposed
        .born()
        .expect("follow-up handle carries a birth stamp");
    let followup = TaskHandle::for_project("board.test", &followup_stamp);
    let mut expected = progress_witnesses(
        &assigned,
        at(&assigned, &reviewed),
        seats[1].lane,
        seats[1].occupant,
        true,
    )
    .expect("assigned review has progress witnesses");
    expected.push(Witness::Absence {
        task: followup_stamp.clone(),
    });
    assert_committed(issue(
        &bus,
        epoch,
        lane_two,
        CommandKind::TaskReview,
        expected,
        &TaskReview {
            task: reviewed,
            finding: "review needs follow-up".to_owned(),
            note: None,
            then: vec![ReviewFollowup {
                task: followup_stamp,
                add: TaskAdd {
                    preparation: None,
                    intent: None,
                    title: "Shape 418 bare follow-up".to_owned(),
                    pitch: "This work keeps the fixture honest.".to_owned(),
                    project: Some("board.test".to_owned()),
                    description: None,
                    acceptance: Vec::new(),
                    requirements: wire::EnvironmentRequirements::default(),
                    origin: None,
                    flow: None,
                    depends: Vec::new(),
                    claim: false,
                    draft: false,
                },
            }],
            followup: Vec::new(),
        },
    ));
    let followed = board(&bus, epoch, Actor::Operator);
    assert_workflow(&followed, &followup, "plan", &["plan", "todo"]);
    clean(&root, bus);
}

#[test]
fn shape_case_419_inserts_plan_and_retains_document_acceptance() {
    let (root, bus, epoch, _) = fixture("plan-shape-document", 0);
    let barrier = board(&bus, epoch, Actor::Operator);
    let applied = apply_document(
        &bus,
        epoch,
        "# Goal\n\n- Shape 419 bare document task\n- Shape 419 accepted document task\n  Acceptance: document acceptance is retained\n",
        &barrier,
    );
    let bare_doc = titled(&applied.board, "Shape 419 bare document task");
    let accepted_doc = titled(&applied.board, "Shape 419 accepted document task");
    assert_workflow(
        &applied.board,
        &bare_doc.statement.body.handle,
        "plan",
        &["plan", "todo", "review"],
    );
    assert_workflow(
        &applied.board,
        &accepted_doc.statement.body.handle,
        "todo",
        &["todo", "review"],
    );
    assert_eq!(
        accepted_doc.statement.body.acceptance,
        ["document acceptance is retained"]
    );
    clean(&root, bus);
}

#[test]
fn wk_plan_phase_requires_acceptance_before_it_advances() {
    let (root, bus, epoch, seats) = fixture("plan-acceptance", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add_raw_task(
        &bus,
        epoch,
        "PLAN-1kLGp006",
        "WK-16 plan needs acceptance",
        &[],
        None,
    );
    claim_task(&bus, epoch, &seats[0], &lane, &task);
    let held = board(&bus, epoch, lane.clone());
    let refused = issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskDone,
        progress_witnesses(
            &held,
            at(&held, &task),
            seats[0].lane,
            seats[0].occupant,
            false,
        )
        .expect("plan task has progress witnesses"),
        &TaskDone {
            task: task.clone(),
            validation: "plan is not accepted".to_owned(),
            disposition: None,
            operation: Some(completion_operation()),
        },
    );
    assert_refusal(refused, "task::plan-incomplete");

    let visible = board(&bus, epoch, Actor::Operator);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskEdit,
        vec![statement_witness(at(&visible, &task))],
        &TaskEdit {
            intent: None,
            task: task.clone(),
            title: None,
            pitch: None,
            description: None,
            acceptance: vec!["implementation has a checkable contract".to_owned()],
            requirements: None,
            priority: None,
            project: None,
            flow: None,
        },
    ));
    // The operator changed the held task, so the holder acknowledges it first.
    reclaim_held(&bus, epoch, &lane, &seats[0], &task);
    let repaired = board(&bus, epoch, lane.clone());
    assert_committed(issue(
        &bus,
        epoch,
        lane,
        CommandKind::TaskDone,
        progress_witnesses(
            &repaired,
            at(&repaired, &task),
            seats[0].lane,
            seats[0].occupant,
            false,
        )
        .expect("accepted plan has progress witnesses"),
        &TaskDone {
            task: task.clone(),
            validation: "acceptance exists on the task".to_owned(),
            disposition: None,
            operation: Some(completion_operation()),
        },
    ));
    let advanced = board(&bus, epoch, Actor::Operator);
    assert_workflow(&advanced, &task, "todo", &["plan", "todo", "review"]);
    clean(&root, bus);
}

#[test]
fn wk_plan_phase_uses_connected_child_acceptance_with_atomic_witnesses() {
    let (root, bus, epoch, seats) = fixture("plan-child-proof", 1);
    let lane = Actor::Lane(seats[0].lane);
    let parent = add_raw_task(&bus, epoch, "PLAN-1kLGp007", "WK-27 parent plan", &[], None);
    let child = add_raw_task(
        &bus,
        epoch,
        "PLAN-1kLGp008",
        "WK-28 accepted child",
        &["child carries acceptance"],
        None,
    );
    claim_task(&bus, epoch, &seats[0], &lane, &parent);
    connect_child(&bus, epoch, &parent, &child);

    let held = board(&bus, epoch, lane.clone());
    let mut stale_proof = progress_witnesses(
        &held,
        at(&held, &parent),
        seats[0].lane,
        seats[0].occupant,
        false,
    )
    .expect("connected child is witnessed");
    stale_proof.retain(|witness| match witness {
        Witness::Task { task, .. } | Witness::Workflow { task, .. } => {
            child.born().is_ok_and(|born| task != &born)
        }
        _ => true,
    });
    let refused = issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskDone,
        stale_proof,
        &TaskDone {
            task: parent.clone(),
            validation: "child proof was not guarded".to_owned(),
            disposition: None,
            operation: Some(completion_operation()),
        },
    );
    assert_refusal(refused, "task::plan-witness-missing");

    let held = board(&bus, epoch, lane.clone());
    assert_committed(issue(
        &bus,
        epoch,
        lane,
        CommandKind::TaskDone,
        progress_witnesses(
            &held,
            at(&held, &parent),
            seats[0].lane,
            seats[0].occupant,
            false,
        )
        .expect("connected child proof is guarded"),
        &TaskDone {
            task: parent.clone(),
            validation: "accepted child is connected".to_owned(),
            disposition: None,
            operation: Some(completion_operation()),
        },
    ));
    let advanced = board(&bus, epoch, Actor::Operator);
    assert_workflow(&advanced, &parent, "todo", &["plan", "todo", "review"]);
    clean(&root, bus);
}

#[test]
fn wk_plan_only_flow_completes_only_with_connected_accepted_child() {
    let (root, bus, epoch, seats) = fixture("plan-only-child", 1);
    let lane = Actor::Lane(seats[0].lane);
    let own_acceptance = add_raw_task(
        &bus,
        epoch,
        "PLAN-1kLGp009",
        "WK-29 own acceptance is not enough",
        &["plan-only task states intent"],
        Some("plan"),
    );
    claim_task(&bus, epoch, &seats[0], &lane, &own_acceptance);
    let held = board(&bus, epoch, lane.clone());
    let refused = issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskDone,
        progress_witnesses(
            &held,
            at(&held, &own_acceptance),
            seats[0].lane,
            seats[0].occupant,
            false,
        )
        .expect("plan-only task has witnesses"),
        &TaskDone {
            task: own_acceptance.clone(),
            validation: "own acceptance does not create implementation work".to_owned(),
            disposition: None,
            operation: Some(completion_operation()),
        },
    );
    assert_refusal(refused, "task::plan-incomplete");
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimFree,
        claim_free_witnesses(&bus, epoch, &lane, &seats[0], &own_acceptance),
        &TaskRef {
            task: own_acceptance,
        },
    ));
    clean(&root, bus);

    let (root, bus, epoch, seats) = fixture("plan-only-child-completes", 1);
    let lane = Actor::Lane(seats[0].lane);
    let parent = add_raw_task(
        &bus,
        epoch,
        "PLAN-1kLGp00B",
        "WK-30 plan-only parent",
        &[],
        Some("plan"),
    );
    let child = add_raw_task(
        &bus,
        epoch,
        "PLAN-1kLGp00C",
        "WK-30 accepted child",
        &["child owns implementation acceptance"],
        None,
    );
    claim_task(&bus, epoch, &seats[0], &lane, &parent);
    connect_child(&bus, epoch, &parent, &child);
    let held = board(&bus, epoch, lane.clone());
    assert_committed(issue(
        &bus,
        epoch,
        lane,
        CommandKind::TaskDone,
        progress_witnesses(
            &held,
            at(&held, &parent),
            seats[0].lane,
            seats[0].occupant,
            false,
        )
        .expect("plan-only child proof is witnessed"),
        &TaskDone {
            task: parent.clone(),
            validation: "accepted child is connected".to_owned(),
            disposition: None,
            operation: Some(completion_operation()),
        },
    ));
    let completed = board(&bus, epoch, Actor::Operator);
    assert!(at(&completed, &parent).workflow.body.status.is_completed());
    clean(&root, bus);
}

fn apply_document(
    bus: &Bus,
    epoch: CanonicalUuid,
    source: &str,
    barrier: &BoardAnswer,
) -> AppliedDocument {
    let preview = board::Preview::classify(source).expect("document previews");
    let document = dialect::emit(preview.document());
    let preview = board::Preview::classify(&document).expect("normal document previews");
    let answer = assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::DocApply,
        vec![
            Witness::Preview {
                digest: preview.digest().clone(),
            },
            Witness::Barrier {
                name: "board".parse().expect("board barrier name"),
                digest: board_barrier(barrier),
            },
        ],
        &DocApply {
            preview: preview.digest().clone(),
            document: document.clone(),
        },
    ));
    let CommandAnswer::Doc(answer) = answer else {
        panic!("document apply returned the wrong answer shape")
    };
    AppliedDocument {
        preview,
        document,
        answer,
        board: board(bus, epoch, Actor::Operator),
    }
}

fn assert_initial_document(bus: &Bus, applied: &AppliedDocument) {
    assert_eq!(applied.answer.touched.len(), 3);
    assert_eq!(applied.board.tasks.len(), 3);
    let causal: wire::Origin = format!("doc:{}", applied.preview.digest())
        .parse()
        .expect("document digest forms an origin");
    for task in &applied.answer.touched {
        assert_eq!(
            at(&applied.board, task).statement.body.provenance.origin(),
            Some(causal.clone())
        );
        assert_eq!(creation_fact_origin(bus, task), causal);
    }
    assert!(
        at(&applied.board, &applied.answer.touched[2])
            .statement
            .body
            .dependencies
            .contains(
                &at(&applied.board, &applied.answer.touched[1])
                    .statement
                    .body
                    .born
            )
    );
}

fn assert_edited_document(bus: &Bus, initial: &AppliedDocument, edited: &AppliedDocument) {
    assert_ne!(edited.preview.digest(), initial.preview.digest());
    assert_eq!(edited.answer.touched, initial.answer.touched);
    assert_eq!(edited.board.tasks.len(), 3);
    let first = at(&edited.board, &edited.answer.touched[1]);
    assert!(
        first.statement.body.acceptance.is_empty(),
        "document edits replace absent acceptance instead of treating it as a patch"
    );
    assert_eq!(
        first
            .workflow
            .body
            .flow
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec!["todo".to_owned(), "review".to_owned()],
        "document edits do not replace an existing workflow flow"
    );
    assert_eq!(
        first.statement.body.provenance,
        at(&initial.board, &initial.answer.touched[1])
            .statement
            .body
            .provenance,
        "a changed document digest does not namespace project-plus-slug identity"
    );
    let first_born = first.statement.body.born.clone();
    assert!(
        !at(&edited.board, &edited.answer.touched[2])
            .statement
            .body
            .dependencies
            .contains(&first_born),
        "omitting an explicit document edge removes that dependency"
    );
    let facts = bus.fact_snapshot().expect("document edit facts read");
    assert_eq!(
        facts
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::TaskEdited)
            .count(),
        3,
        "an edited document must update its stable task identities",
    );
}

fn assert_idempotent_document(bus: &Bus, epoch: CanonicalUuid, edited: &AppliedDocument) {
    let before = bus
        .fact_snapshot()
        .expect("facts before idempotent apply")
        .facts()
        .len();
    let repeated = issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::DocApply,
        vec![
            Witness::Preview {
                digest: edited.preview.digest().clone(),
            },
            Witness::Barrier {
                name: "board".parse().expect("board barrier name"),
                digest: board_barrier(&edited.board),
            },
        ],
        &DocApply {
            preview: edited.preview.digest().clone(),
            document: edited.document.clone(),
        },
    );
    assert!(matches!(
        repeated,
        Outcome::Reply(Reply {
            position: ReplyPosition::Committed(_),
            body: CommandAnswer::Doc(_),
        })
    ));
    assert_eq!(
        bus.fact_snapshot()
            .expect("facts after idempotent apply")
            .facts()
            .len(),
        before,
        "the same document digest must not append a second durable path",
    );
}

fn assert_stale_document_refused(bus: &Bus, epoch: CanonicalUuid, initial_board: &BoardAnswer) {
    let changed =
        board::Preview::classify("# Goal\n\n- Third task\n").expect("changed document previews");
    let stale = issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::DocApply,
        vec![
            Witness::Preview {
                digest: changed.digest().clone(),
            },
            Witness::Barrier {
                name: "board".parse().expect("board barrier name"),
                digest: board_barrier(initial_board),
            },
        ],
        &DocApply {
            preview: changed.digest().clone(),
            document: dialect::emit(changed.document()),
        },
    );
    assert!(matches!(
        stale,
        Outcome::Refusal(ref refusal) if refusal.reason() == "doc::barrier-moved"
    ));
}
