use super::*;

fn draft_add(name: &str, intent: Option<wire::TaskIntent>, acceptance: &[&str]) -> TaskAdd {
    TaskAdd {
        preparation: None,
        title: format!("Prospective {name}"),
        pitch: "This work keeps the fixture honest.".to_owned(),
        project: Some("board.test".to_owned()),
        description: Some(format!("Context for {name}")),
        acceptance: acceptance.iter().map(|item| (*item).to_owned()).collect(),
        requirements: wire::EnvironmentRequirements::default(),
        origin: Some(fixture_origin()),
        flow: None,
        depends: Vec::new(),
        intent,
        claim: false,
        draft: true,
    }
}

fn add_draft(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    name: &str,
    intent: Option<wire::TaskIntent>,
    acceptance: &[&str],
) -> TaskHandle {
    let born = stamp(name);
    let CommandAnswer::Task(answer) = assert_committed(issue(
        bus,
        epoch,
        actor,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born.clone() }],
        &draft_add(name, intent, acceptance),
    )) else {
        panic!("draft creation returned another answer shape");
    };
    let draft = TaskHandle::for_filing("board.test", &born, wire::TaskFiling::Draft);
    assert_eq!(answer.task, draft);
    draft
}

fn edit(task: &TaskHandle) -> TaskEdit {
    TaskEdit {
        task: task.clone(),
        title: None,
        pitch: None,
        description: None,
        acceptance: Vec::new(),
        requirements: None,
        intent: None,
        priority: None,
        project: None,
        flow: None,
    }
}

fn fact_count(bus: &Bus) -> usize {
    bus.fact_snapshot()
        .expect("fixture facts remain readable")
        .facts()
        .len()
}

fn file_witnesses(heads: &BoardTask) -> Vec<Witness> {
    vec![
        workflow_witness(heads),
        deferral_witness(heads),
        statement_witness(heads),
    ]
}

fn file(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle) -> Outcome<CommandAnswer> {
    let current = board(bus, epoch, Actor::Operator);
    issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskFile,
        file_witnesses(at(&current, task)),
        &TaskFile {
            task: task.clone(),
            into: None,
            preparation: None,
            claim: false,
        },
    )
}

fn refusal_reason(outcome: &Outcome<CommandAnswer>) -> &str {
    match outcome {
        Outcome::Refusal(refusal) => refusal.reason(),
        other => panic!("expected a refusal: {other:?}"),
    }
}

fn listed(answer: &BoardAnswer, all: bool) -> String {
    task_list(
        answer,
        &TaskListOptions {
            all,
            project: None,
            limit: None,
            status: None,
        },
    )
}

#[test]
fn draft_creation_refusals_name_their_repair_and_write_nothing() {
    let (root, bus, epoch, seats) = fixture("draft-create", 1);
    let lane = Actor::Lane(seats[0].lane);
    authorize_lane_creation(&bus, epoch, &seats[0]);
    let seated = board(&bus, epoch, lane.clone());
    let facts = fact_count(&bus);
    for (actor, add, reason, repair) in [
        (
            Actor::Operator,
            TaskAdd {
                project: None,
                ..draft_add("DRAFT-noproj01", None, &[])
            },
            "task::draft-project-required",
            "--project",
        ),
        (
            lane.clone(),
            TaskAdd {
                claim: true,
                ..draft_add("DRAFT-claim001", None, &[])
            },
            "task::draft-claim-refused",
            "spice task file",
        ),
    ] {
        let mut expected = vec![Witness::Absence {
            task: stamp(&add.title),
        }];
        if add.claim {
            expected.push(Witness::Eligibility {
                rule: "capture".parse().expect("fixture capture rule"),
            });
            expected.push(residency(&seated, &seats[0]));
        }
        let outcome = issue(&bus, epoch, actor, CommandKind::TaskAdd, expected, &add);
        assert_eq!(refusal_reason(&outcome), reason);
        let Outcome::Refusal(refusal) = outcome else {
            unreachable!("refusal was matched above");
        };
        assert!(
            refusal.explanation().contains(repair),
            "{reason} names no repair: {}",
            refusal.explanation(),
        );
    }
    assert_eq!(
        fact_count(&bus),
        facts,
        "refused draft creation wrote facts"
    );
    clean(&root, bus);
}

#[test]
fn origin_refusal_offers_the_draft_route_only_to_an_operator_add() {
    let (root, bus, epoch, seats) = fixture("draft-origin-route", 1);
    let lane = Actor::Lane(seats[0].lane);
    authorize_lane_creation(&bus, epoch, &seats[0]);
    let facts = fact_count(&bus);
    for (actor, draft, offered) in [
        (Actor::Operator, false, true),
        (lane.clone(), false, false),
        (lane, true, false),
    ] {
        let add = TaskAdd {
            origin: None,
            draft,
            ..draft_add("DRAFT-route001", None, &[])
        };
        let outcome = issue(
            &bus,
            epoch,
            actor.clone(),
            CommandKind::TaskAdd,
            vec![Witness::Absence {
                task: stamp(&add.title),
            }],
            &add,
        );
        assert_eq!(refusal_reason(&outcome), "task::origin-required");
        let Outcome::Refusal(refusal) = outcome else {
            unreachable!("refusal was matched above");
        };
        let explanation = refusal.explanation();
        assert!(explanation.contains("--origin ack:"), "{explanation}");
        assert_eq!(
            explanation.contains("--draft --project PROJECT"),
            offered,
            "{actor:?} draft={draft}: {explanation}",
        );
    }
    assert_eq!(fact_count(&bus), facts, "refused creation wrote facts");
    clean(&root, bus);
}

#[test]
fn an_operator_draft_keeps_editable_content_off_the_allocator() {
    let (root, bus, epoch, seats) = fixture("draft-edit", 1);
    let lane = Actor::Lane(seats[0].lane);
    let born = stamp("DRAFT-edit0001");
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born.clone() }],
        &TaskAdd {
            origin: None,
            ..draft_add("DRAFT-edit0001", None, &[])
        },
    ));
    let draft = TaskHandle::for_filing("board.test", &born, wire::TaskFiling::Draft);
    let created = board(&bus, epoch, Actor::Operator);
    let heads = at(&created, &draft);
    assert!(heads.statement.body.filing.is_draft());
    assert_eq!(
        heads.statement.body.provenance.to_string(),
        "operator:draft"
    );
    assert_eq!(allocated(&bus, epoch, &lane), None, "a Draft was offered");

    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskEdit,
        vec![statement_witness(heads)],
        &TaskEdit {
            title: Some("Revised prospective work".to_owned()),
            acceptance: vec!["The revised criterion holds".to_owned()],
            intent: Some(wire::TaskIntentEdit::Set {
                intent: wire::TaskIntent::Plan,
            }),
            flow: Some("plan,review".to_owned()),
            project: Some("board.identity".to_owned()),
            ..edit(&draft)
        },
    ));
    let revised_handle = TaskHandle::for_filing(
        "board.identity",
        &draft.born().expect("draft handle carries a birth stamp"),
        wire::TaskFiling::Draft,
    );
    let revised = board(&bus, epoch, Actor::Operator);
    let heads = at(&revised, &revised_handle);
    assert_eq!(heads.presentation_title, "Revised prospective work");
    assert_eq!(
        heads.statement.body.acceptance,
        ["The revised criterion holds"]
    );
    assert_eq!(heads.statement.body.intent, Some(wire::TaskIntent::Plan));
    assert_eq!(heads.statement.body.project, "board.identity");
    assert_eq!(
        heads
            .workflow
            .body
            .flow
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["plan", "review"],
    );
    assert!(heads.statement.body.filing.is_draft());
    assert_eq!(
        allocated(&bus, epoch, &lane),
        None,
        "an edited Draft was offered"
    );
    clean(&root, bus);
}

#[test]
fn a_filed_task_keeps_its_flow() {
    let (root, bus, epoch, _seats) = fixture("draft-flow-fixed", 1);
    let filed = add(
        &bus,
        epoch,
        Actor::Operator,
        "DRAFT-fixed001",
        "Filed",
        Some("todo"),
    );
    let filed_heads = at(&board(&bus, epoch, Actor::Operator), &filed).clone();
    let fixed = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskEdit,
        vec![statement_witness(&filed_heads)],
        &TaskEdit {
            flow: Some("todo,review".to_owned()),
            ..edit(&filed)
        },
    );
    assert_eq!(refusal_reason(&fixed), "task::flow-fixed");
    clean(&root, bus);
}

#[test]
fn a_draft_edit_that_leaves_a_design_any_other_route_refuses() {
    let (root, bus, epoch, _seats) = fixture("draft-design-route", 1);
    let design = add_draft(
        &bus,
        epoch,
        Actor::Operator,
        "DRAFT-desnflow",
        Some(wire::TaskIntent::Design),
        &["The design is supported"],
    );
    let ordinary = add_draft(
        &bus,
        epoch,
        Actor::Operator,
        "DRAFT-ordflow1",
        None,
        &["The work is verified"],
    );
    let set_design = Some(wire::TaskIntentEdit::Set {
        intent: wire::TaskIntent::Design,
    });
    let facts = fact_count(&bus);
    for (task, refused) in [
        (
            &design,
            TaskEdit {
                flow: Some("design,review".to_owned()),
                ..edit(&design)
            },
        ),
        (
            &ordinary,
            TaskEdit {
                intent: set_design,
                ..edit(&ordinary)
            },
        ),
        (
            &ordinary,
            TaskEdit {
                intent: set_design,
                flow: Some("todo".to_owned()),
                ..edit(&ordinary)
            },
        ),
    ] {
        let current = board(&bus, epoch, Actor::Operator);
        let outcome = issue(
            &bus,
            epoch,
            Actor::Operator,
            CommandKind::TaskEdit,
            vec![statement_witness(at(&current, task))],
            &refused,
        );
        assert_eq!(refusal_reason(&outcome), "task::design-flow");
    }
    assert_eq!(fact_count(&bus), facts, "a refused route edit wrote facts");

    let current = board(&bus, epoch, Actor::Operator);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskEdit,
        vec![statement_witness(at(&current, &ordinary))],
        &TaskEdit {
            intent: set_design,
            flow: Some("design".to_owned()),
            ..edit(&ordinary)
        },
    ));
    let heads = at(&board(&bus, epoch, Actor::Operator), &ordinary).clone();
    assert_eq!(heads.statement.body.intent, Some(wire::TaskIntent::Design));
    assert_eq!(
        heads
            .workflow
            .body
            .flow
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["design"],
    );
    clean(&root, bus);
}

#[test]
fn default_reads_show_a_draft_only_to_the_operator_and_its_origin_thread() {
    let (root, bus, epoch, seats) = fixture("draft-visibility", 2);
    authorize_lane_creation(&bus, epoch, &seats[0]);
    let operator_draft = add_draft(&bus, epoch, Actor::Operator, "DRAFT-oper0001", None, &[]);
    let lane_draft = add_draft(
        &bus,
        epoch,
        Actor::Lane(seats[0].lane),
        "DRAFT-lane0001",
        None,
        &[],
    );

    let operator = board(&bus, epoch, Actor::Operator);
    let author = board(&bus, epoch, Actor::Lane(seats[0].lane));
    let peer = board(&bus, epoch, Actor::Lane(seats[1].lane));
    let operator_list = listed(&operator, false);
    assert!(
        operator_list.contains(operator_draft.as_str()),
        "{operator_list}"
    );
    assert!(
        operator_list.contains(lane_draft.as_str()),
        "{operator_list}"
    );
    let author_list = listed(&author, false);
    assert!(author_list.contains(lane_draft.as_str()), "{author_list}");
    assert!(
        !author_list.contains(operator_draft.as_str()),
        "{author_list}"
    );
    let peer_list = listed(&peer, false);
    assert!(!peer_list.contains(lane_draft.as_str()), "{peer_list}");
    assert!(!peer_list.contains(operator_draft.as_str()), "{peer_list}");
    let complete = listed(&operator, true);
    assert!(complete.contains(operator_draft.as_str()), "{complete}");
    assert!(complete.contains(lane_draft.as_str()), "{complete}");
    clean(&root, bus);
}

#[test]
fn a_draft_is_never_offered_or_captured() {
    let (root, bus, epoch, seats) = fixture("draft-authority", 1);
    let seat = &seats[0];
    let lane = Actor::Lane(seat.lane);
    let draft = add_draft(
        &bus,
        epoch,
        Actor::Operator,
        "DRAFT-auth0001",
        None,
        &["The work is verified"],
    );
    let facts = fact_count(&bus);
    let offered = allocated(&bus, epoch, &lane).map(|(handle, _)| handle);
    assert_ne!(
        offered.as_deref(),
        Some(draft.as_str()),
        "a Draft was offered"
    );

    let current = board(&bus, epoch, lane.clone());
    let heads = at(&current, &draft);
    let rule: Name = "capture".parse().expect("capture rule");
    let capture = issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility { rule: rule.clone() },
            residency(&current, seat),
        ],
        &ClaimWin {
            rule,
            recovery: None,
        },
    );
    assert_eq!(refusal_reason(&capture), "task::draft-unfiled");
    assert_eq!(fact_count(&bus), facts, "a refused capture wrote facts");
    clean(&root, bus);
}

#[test]
fn a_draft_satisfies_no_dependency_but_keeps_its_own() {
    let (root, bus, epoch, _seats) = fixture("draft-dependencies", 1);
    let draft = add_draft(
        &bus,
        epoch,
        Actor::Operator,
        "DRAFT-auth0002",
        None,
        &["The work is verified"],
    );
    let public = add(
        &bus,
        epoch,
        Actor::Operator,
        "DRAFT-public01",
        "Public",
        Some("todo"),
    );
    let operator = board(&bus, epoch, Actor::Operator);
    let follows_draft = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDepends,
        dependency_witnesses(&operator, &public, &[&draft], None, false),
        &TaskDepends {
            task: public.clone(),
            after: vec![draft.clone()],
            remove: false,
        },
    );
    assert_eq!(refusal_reason(&follows_draft), "task::dependency-unfiled");
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDepends,
        dependency_witnesses(&operator, &draft, &[&public], None, false),
        &TaskDepends {
            task: draft.clone(),
            after: vec![public.clone()],
            remove: false,
        },
    ));
    let revised = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&revised, &draft).statement.body.dependencies,
        [public.born().expect("public handle carries a birth stamp")],
        "a Draft's own prerequisites are editable content",
    );
    clean(&root, bus);
}

#[test]
fn a_draft_refuses_closure_and_deferral() {
    let (root, bus, epoch, _seats) = fixture("draft-authority-rails", 1);
    let draft = add_draft(
        &bus,
        epoch,
        Actor::Operator,
        "DRAFT-auth0003",
        Some(wire::TaskIntent::Design),
        &["The work is verified"],
    );
    let facts = fact_count(&bus);
    let operator = board(&bus, epoch, Actor::Operator);
    let heads = at(&operator, &draft);
    let close = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskClose,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ],
        &TaskClose {
            task: draft.clone(),
            validation: "Closed early".to_owned(),
            force_claimed: false,
        },
    );
    assert_eq!(refusal_reason(&close), "task::draft-unfiled");
    let conclude = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskConclude,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ],
        &TaskConclude {
            task: draft.clone(),
            conclusion: wire::body::DesignConclusion::Done {},
        },
    );
    assert_eq!(refusal_reason(&conclude), "task::draft-unfiled");
    let defer = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDefer,
        vec![workflow_witness(heads), deferral_witness(heads)],
        &TaskDefer {
            task: draft.clone(),
            wait: fixture_instant("draft defer"),
        },
    );
    assert_eq!(refusal_reason(&defer), "task::draft-unfiled");
    assert_eq!(
        fact_count(&bus),
        facts,
        "a refused Draft authority wrote facts"
    );
    clean(&root, bus);
}

#[test]
fn a_failed_filing_writes_nothing_and_keeps_the_editable_draft() {
    let (root, bus, epoch, seats) = fixture("draft-file-refused", 1);
    let lane = Actor::Lane(seats[0].lane);
    let draft = add_draft(
        &bus,
        epoch,
        Actor::Operator,
        "DRAFT-incomp01",
        None,
        &["The filed work is verified"],
    );
    let ordinary = add(
        &bus,
        epoch,
        Actor::Operator,
        "DRAFT-ordin001",
        "Ordinary",
        Some("todo"),
    );
    let before = board(&bus, epoch, Actor::Operator);
    let heads = at(&before, &draft).clone();
    let ordinary_heads = at(&before, &ordinary);
    let facts = fact_count(&bus);
    let mut expected = file_witnesses(&heads);
    expected.extend([
        statement_witness(ordinary_heads),
        workflow_witness(ordinary_heads),
        claim_witness(ordinary_heads),
    ]);
    let refused = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskFile,
        expected,
        &TaskFile {
            task: draft.clone(),
            into: None,
            preparation: Some(ordinary.clone()),
            claim: false,
        },
    );
    assert_eq!(refusal_reason(&refused), "task::preparation-refused");
    assert_eq!(fact_count(&bus), facts, "a refused filing wrote facts");
    let after = board(&bus, epoch, Actor::Operator);
    let kept = at(&after, &draft);
    assert_eq!(kept.statement, heads.statement);
    assert_eq!(kept.workflow, heads.workflow);
    assert_eq!(
        allocated(&bus, epoch, &lane).map(|(handle, _)| handle),
        Some(ordinary.to_string()),
        "a refused Draft displaced filed work",
    );

    assert_committed(file(&bus, epoch, &draft));
    clean(&root, bus);
}

#[test]
fn filing_turns_a_complete_task_plan_or_design_draft_into_offered_work() {
    for (name, intent, flow) in [
        ("DRAFT-task0001", None, &["todo", "review"][..]),
        (
            "DRAFT-plan0001",
            Some(wire::TaskIntent::Plan),
            &["plan", "review"][..],
        ),
        (
            "DRAFT-desn0001",
            Some(wire::TaskIntent::Design),
            &["design"][..],
        ),
    ] {
        let (root, bus, epoch, seats) = fixture(name, 1);
        let lane = Actor::Lane(seats[0].lane);
        let draft = add_draft(
            &bus,
            epoch,
            Actor::Operator,
            name,
            intent,
            &["The filed work is verified"],
        );
        let CommandAnswer::Task(answer) = assert_committed(file(&bus, epoch, &draft)) else {
            panic!("filing returned another answer shape");
        };
        let filed = TaskHandle::for_project(
            "board.test",
            &draft.born().expect("draft handle carries a birth stamp"),
        );
        assert_eq!(answer.task, filed);
        let current = board(&bus, epoch, Actor::Operator);
        let heads = at(&current, &filed);
        assert!(heads.statement.body.filing.is_filed());
        assert_eq!(heads.statement.body.intent, intent);
        assert_eq!(
            heads
                .workflow
                .body
                .flow
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            flow,
        );
        assert!(
            current
                .tasks
                .iter()
                .all(|task| task.statement.body.handle != draft)
        );
        if intent == Some(wire::TaskIntent::Design) {
            assert_eq!(
                allocated(&bus, epoch, &lane).map(|(handle, _)| handle),
                None,
                "a filed Design gathering input is not offered before it is ready",
            );
            group_design(&bus, epoch, &filed);
        }
        assert_eq!(
            allocated(&bus, epoch, &lane).map(|(handle, _)| handle),
            Some(filed.to_string()),
            "filed {name} is not offered once eligible",
        );
        let listed = listed(&current, false);
        assert!(listed.contains(filed.as_str()), "{listed}");
        clean(&root, bus);
    }
}

#[test]
fn filing_a_draft_can_claim_it_in_the_same_transaction() {
    let (root, bus, epoch, seats) = fixture("draft-file-claim", 1);
    let seat = &seats[0];
    let lane = Actor::Lane(seat.lane);

    authorize_lane_creation(&bus, epoch, seat);
    let held = add_draft(
        &bus,
        epoch,
        lane.clone(),
        "DRAFT-claim002",
        None,
        &["Held work is verified"],
    );
    let visible = board(&bus, epoch, lane.clone());
    let heads = at(&visible, &held);
    let mut expected = file_witnesses(heads);
    expected.push(Witness::Eligibility {
        rule: "capture".parse().expect("fixture capture rule"),
    });
    expected.push(residency(&visible, seat));
    let CommandAnswer::Task(answer) = assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskFile,
        expected,
        &TaskFile {
            task: held.clone(),
            into: None,
            preparation: None,
            claim: true,
        },
    )) else {
        panic!("file-and-claim returned another answer shape");
    };
    let claim = answer
        .claim
        .expect("filing with claim returns the held claim");
    assert_eq!(claim.holder, Some(lane));
    clean(&root, bus);
}
