use super::*;

// Hidden-visibility board tests, split from the sibling module by the law
// they share: authoritative hidden evidence never crosses into any public
// presentation, while the actor who owns it still reads it in full.

fn add_oops(bus: &Bus, epoch: CanonicalUuid, actor: Actor, task: &str, title: &str) -> TaskHandle {
    let value = task.bytes().fold(0_i64, |value, byte| {
        (value * 131 + i64::from(byte)) % 10_000_000_000_000
    });
    let born = wire::TaskStamp::for_millis(value).expect("oops fixture stamp");
    let task = TaskHandle::for_project(".oops", &born);
    assert_committed(issue(
        bus,
        epoch,
        actor,
        CommandKind::TaskOopsAdd,
        vec![Witness::Absence { task: born }],
        &TaskOopsAdd {
            title: title.to_owned(),
            description: title.to_owned(),
            origin: Some("fixture:oops".parse().expect("fixture origin")),
        },
    ));
    task
}

fn assert_hidden_actor_document(
    answer: &BoardAnswer,
    actor: &Actor,
    hidden: &TaskHandle,
    public: &TaskHandle,
    secret: &str,
) {
    assert_eq!(&answer.principal, actor);
    let hidden_heads = at(answer, hidden);
    assert_eq!(hidden_heads.visibility.hidden_stem(), Some("oops"));
    assert_eq!(hidden_heads.statement.body.handle, *hidden);
    assert_eq!(hidden_heads.statement.body.project, ".oops");
    assert_eq!(hidden_heads.presentation_title(), "Hidden task");
    assert_eq!(hidden_heads.workflow.body.status, "pending");
    let public_heads = at(answer, public);
    assert_eq!(public_heads.presentation_title(), "Visible allocator work");

    let json = serde_json::to_value(answer).expect("board answer encodes");
    let hidden_json = json["tasks"]
        .as_array()
        .expect("task rows")
        .iter()
        .find(|task| task["statement"]["body"]["handle"] == hidden.to_string())
        .expect("hidden JSON row");
    assert_eq!(
        hidden_json["visibility"],
        serde_json::json!({"kind": "hidden", "stem": "oops"})
    );
    assert_eq!(hidden_json["presentation_title"], "Hidden task");
    assert!(hidden_json["statement"]["body"].get("title").is_none());
    assert_eq!(hidden_json["workflow"]["body"]["status"], "pending");
    let public_json = json["tasks"]
        .as_array()
        .expect("task rows")
        .iter()
        .find(|task| task["statement"]["body"]["handle"] == public.to_string())
        .expect("public JSON row");
    assert_eq!(public_json["presentation_title"], "Visible allocator work");
    assert!(public_json["statement"]["body"].get("title").is_none());

    let rendered = Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(answer.cursor),
        body: CommandAnswer::Board(Box::new(answer.clone())),
    });
    for form in [crate::render::Form::Text, crate::render::Form::Json] {
        let document = crate::render::render(&rendered, form)
            .expect("board presentation renders")
            .document()
            .to_vec();
        let document = String::from_utf8(document).expect("presentation is UTF-8");
        assert!(!document.contains(secret), "{form:?} leaked hidden prose");
        assert!(document.contains("Hidden task"), "{form:?} presentation");
        assert!(
            document.contains("Visible allocator work"),
            "{form:?} public title"
        );
    }
}

#[test]
fn board_read_retains_hidden_evidence_while_next_and_presentation_stay_public() {
    let (root, bus, epoch, seats) = fixture("hidden-visibility", 1);
    let lane = Actor::Lane(seats[0].lane);
    let hidden = add_oops(
        &bus,
        epoch,
        Actor::Operator,
        "HIDDEN-oops1",
        "secret allocator failure details",
    );
    let public = add_project(
        &bus,
        epoch,
        Actor::Operator,
        "PUBLIC-work1",
        "Visible allocator work",
        "board.visibility",
        Some("todo"),
    );

    let operator = board(&bus, epoch, Actor::Operator);
    assert_hidden_actor_document(
        &operator,
        &Actor::Operator,
        &hidden,
        &public,
        "secret allocator failure details",
    );
    let available = board(&bus, epoch, lane.clone());
    assert_hidden_actor_document(
        &available,
        &lane,
        &hidden,
        &public,
        "secret allocator failure details",
    );
    let observed = bus.task_snapshot().expect("task authority is readable");
    let authoritative = observed
        .tasks()
        .iter()
        .find(|heads| heads.statement().body().born == at(&available, &hidden).statement.body.born)
        .expect("hidden authoritative statement remains present");
    assert_eq!(
        authoritative.statement().body().title,
        "secret allocator failure details"
    );

    let rule: Name = "next".parse().expect("allocator rule");
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        claim_expected(&available, &public, &seats[0]),
        &ClaimWin { rule },
    ));
    let claimed = board(&bus, epoch, lane);
    assert!(at(&claimed, &hidden).claim.is_none());
    assert!(at(&claimed, &public).claim.is_some());
    clean(&root, bus);
}

#[test]
fn document_ledgers_keep_hidden_titles_actor_facing_and_public_bytes_stable() {
    let (root, bus, epoch, seats) = fixture("hidden-ledger", 1);
    let secret = "secret document ledger details";
    let hidden = add_oops(&bus, epoch, Actor::Operator, "HIDDEN-ledger1", secret);
    let public = add_project(
        &bus,
        epoch,
        Actor::Operator,
        "PUBLIC-ledger1",
        "Visible document ledger",
        "board.visibility",
        Some("todo"),
    );

    let mut public_ledgers = Vec::new();
    for actor in [Actor::Operator, Actor::Lane(seats[0].lane)] {
        let actor_board = board(&bus, epoch, actor.clone());
        let retained_hidden = actor_board
            .tasks
            .iter()
            .find(|task| task.statement.body.handle == hidden)
            .expect("actor retains hidden board row")
            .statement
            .body
            .handle
            .clone();
        let hidden_ledger = issue(
            &bus,
            epoch,
            actor.clone(),
            CommandKind::DocLedger,
            Vec::new(),
            &DocLedger {
                task: retained_hidden,
            },
        );
        let Outcome::Reply(Reply {
            body: CommandAnswer::Text(ref answer),
            ..
        }) = hidden_ledger
        else {
            panic!("hidden ledger was not a text snapshot: {hidden_ledger:?}");
        };
        assert_eq!(answer.text, "# Hidden task\nFlow: plan, todo, review\n");
        for form in [crate::render::Form::Text, crate::render::Form::Json] {
            let document = crate::render::render(&hidden_ledger, form)
                .expect("hidden ledger renders")
                .document()
                .to_vec();
            let document = String::from_utf8(document).expect("ledger rendering is UTF-8");
            assert!(!document.contains(secret), "{form:?} leaked hidden title");
            assert!(
                document.contains("Hidden task"),
                "{form:?} omitted actor-facing title"
            );
        }

        let retained_public = actor_board
            .tasks
            .iter()
            .find(|task| task.statement.body.handle == public)
            .expect("actor retains public board row")
            .statement
            .body
            .handle
            .clone();
        let public_ledger = issue(
            &bus,
            epoch,
            actor,
            CommandKind::DocLedger,
            Vec::new(),
            &DocLedger {
                task: retained_public,
            },
        );
        let Outcome::Reply(Reply {
            body: CommandAnswer::Text(answer),
            ..
        }) = public_ledger
        else {
            panic!("public ledger was not a text snapshot: {public_ledger:?}");
        };
        assert_eq!(
            answer.text.as_bytes(),
            b"# Visible document ledger\nFlow: todo\n"
        );
        public_ledgers.push(answer.text);
    }
    assert_eq!(public_ledgers[0], public_ledgers[1]);
    clean(&root, bus);
}

#[test]
fn graph_and_handout_reads_never_cross_the_hidden_title_boundary() {
    let (root, bus, epoch, seats) = fixture("hidden-board-documents", 1);
    let secret = "secret graph and handout details";
    let hidden = add_oops(&bus, epoch, Actor::Operator, "HIDDEN-graph1", secret);
    let public = add_project(
        &bus,
        epoch,
        Actor::Operator,
        "PUBLIC-graph1",
        "Visible graph work",
        "board.visibility",
        Some("todo"),
    );

    let mut rows = [(hidden.clone(), "plan"), (public.clone(), "todo")];
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    let expected_graph = format!(
        "graph workflow\n{}: {} [pending]\n{}: {} [pending]\n",
        rows[0].0, rows[0].1, rows[1].0, rows[1].1
    );
    let mut actor_graphs = Vec::new();
    for actor in [Actor::Operator, Actor::Lane(seats[0].lane)] {
        let graph = issue(
            &bus,
            epoch,
            actor,
            CommandKind::BoardGraph,
            Vec::new(),
            &BoardGraph {
                view: "workflow".to_owned(),
                at: None,
            },
        );
        let Outcome::Reply(Reply {
            body: CommandAnswer::Text(ref answer),
            ..
        }) = graph
        else {
            panic!("graph was not a text snapshot: {graph:?}");
        };
        assert_eq!(answer.text.as_bytes(), expected_graph.as_bytes());
        actor_graphs.push(answer.text.clone());
        for form in [crate::render::Form::Text, crate::render::Form::Json] {
            let document = crate::render::render(&graph, form)
                .expect("graph renders")
                .document()
                .to_vec();
            let document = String::from_utf8(document).expect("graph rendering is UTF-8");
            assert!(!document.contains(secret), "{form:?} leaked hidden title");
            assert!(
                document.contains(&format!("{public}: todo [pending]")),
                "{form:?} changed public graph row"
            );
        }
    }
    assert_eq!(actor_graphs[0], actor_graphs[1]);

    let handout = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::BoardHandout,
        Vec::new(),
        &BoardHandout {
            output: "-".to_owned(),
            at: None,
        },
    );
    let Outcome::Reply(Reply {
        body: CommandAnswer::Text(ref answer),
        ..
    }) = handout
    else {
        panic!("handout was not a text snapshot: {handout:?}");
    };
    assert!(answer.text.starts_with("# Board handout\n\n"));
    assert!(answer.text.contains(&expected_graph));
    let sections = [
        "## origin",
        "## dependencies",
        "## review",
        "## workflow",
        "## claims",
    ];
    let positions = sections.map(|section| {
        answer
            .text
            .find(section)
            .unwrap_or_else(|| panic!("missing {section} in {}", answer.text))
    });
    assert!(positions.windows(2).all(|window| window[0] < window[1]));
    assert!(!answer.text.contains(secret), "handout leaked hidden title");
    for form in [crate::render::Form::Text, crate::render::Form::Json] {
        let document = crate::render::render(&handout, form)
            .expect("handout renders")
            .document()
            .to_vec();
        let document = String::from_utf8(document).expect("handout rendering is UTF-8");
        assert!(!document.contains(secret), "{form:?} leaked hidden title");
        assert!(
            document.contains(&format!("{public}: todo [pending]")),
            "{form:?} changed public handout row"
        );
    }
    clean(&root, bus);
}

#[test]
fn manual_hidden_creation_is_zero_write_and_system_routes_are_explicit() {
    let (root, bus, epoch, seats) = fixture("hidden-creation", 1);
    let lane = Actor::Lane(seats[0].lane);

    assert_manual_system_project_refuses(&bus, epoch, &lane);
    assert_forged_hidden_route_refuses(&bus, epoch, &lane);

    let ungrounded = stamp("SYSTEM-oops0");
    let before = facts_len(&bus, "facts before ungrounded oops");
    let refused = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskOopsAdd,
        vec![Witness::Absence { task: ungrounded }],
        &TaskOopsAdd {
            title: "Ungrounded system filing".to_owned(),
            description: "The hidden route still needs causal authority".to_owned(),
            origin: None,
        },
    );
    assert!(matches!(
        refused,
        Outcome::Refusal(ref refusal) if refusal.reason() == "task::origin-required"
    ));
    assert_eq!(facts_len(&bus, "facts after ungrounded oops"), before);

    let operator_oops = add_oops(
        &bus,
        epoch,
        Actor::Operator,
        "SYSTEM-oops1",
        "Operator oops route",
    );
    let lane_oops = add_oops(&bus, epoch, lane, "SYSTEM-oops2", "Lane oops route");

    let answer = board(&bus, epoch, Actor::Operator);
    assert_eq!(answer.tasks.len(), 2);
    assert!(answer.tasks.iter().all(|task| task.visibility.is_hidden()));
    assert!(
        answer
            .tasks
            .iter()
            .all(|task| task.statement.body.project == ".oops")
    );
    let expected: wire::Origin = "fixture:oops".parse().expect("fixture origin");
    for task in [operator_oops, lane_oops] {
        assert_eq!(at(&answer, &task).statement.body.provenance, expected);
        assert_eq!(creation_fact_origin(&bus, &task), expected);
    }
    clean(&root, bus);
}

/// Journal fact count, which is the evidence a refusal wrote nothing.
fn facts_len(bus: &Bus, when: &str) -> usize {
    bus.fact_snapshot().expect(when).facts().len()
}

/// Naming a system project on the ordinary add route refuses, for either
/// principal, and leaves the journal exactly as it found it.
fn assert_manual_system_project_refuses(bus: &Bus, epoch: CanonicalUuid, lane: &Actor) {
    let manual = [
        (Actor::Operator, "MANUAL-oops1", ".oops"),
        (lane.clone(), "MANUAL-maxim1", ".maxim_proposal.theme_2"),
        (Actor::Operator, "MANUAL-sand1", ".sandbox.audit"),
    ];
    for (actor, _handle, project) in manual {
        let before = facts_len(bus, "facts read before manual refusal");
        let outcome = issue(
            bus,
            epoch,
            actor,
            CommandKind::TaskAdd,
            vec![Witness::Absence {
                task: wire::TaskStamp::for_millis(42).expect("fixture task stamp"),
            }],
            &TaskAdd {
                title: "Manual hidden creation must refuse".to_owned(),
                project: Some(project.to_owned()),
                description: None,
                acceptance: Vec::new(),
                origin: None,
                flow: None,
                depends: Vec::new(),
            },
        );
        assert!(matches!(
            outcome,
            Outcome::Refusal(ref refusal)
                if refusal.reason() == "task::project-system-reserved"
                    && refusal.diagnostic().and_then(wire::RefusalDiagnostic::observed).is_some_and(|detail| {
                        detail.contains("reserved for system task creation")
                    })
        ));
        assert_eq!(
            facts_len(bus, "facts read after manual refusal"),
            before,
            "manual hidden creation mutated the journal for {project}",
        );
    }
}

/// A caller-selected project on the system route never reaches the journal:
/// the route DTO rejects it at decode, before any command is admitted.
fn assert_forged_hidden_route_refuses(bus: &Bus, epoch: CanonicalUuid, lane: &Actor) {
    for actor in [Actor::Operator, lane.clone()] {
        for project in [".oops", ".maxim_proposal.theme_2", ".sandbox.audit"] {
            let _ = NEXT_COMMAND.fetch_add(1, Ordering::Relaxed);
            let before = facts_len(bus, "facts before route decode refusal");
            let rejection = Command {
                v: WireVersion::CURRENT,
                id: uuid(NEXT_COMMAND.fetch_add(1, Ordering::Relaxed)),
                epoch,
                kind: CommandKind::TaskOopsAdd,
                expected: vec![Witness::Absence {
                    task: wire::TaskStamp::for_millis(43).expect("fixture task stamp"),
                }],
                body: serde_json::json!({
                    "title": "forged hidden route",
                    "description": "caller-selected project must not cross",
                    "project": project,
                }),
            }
            .admit(&Accepted {
                wire: WireVersion::CURRENT,
                actor: actor.clone(),
                epoch,
            })
            .expect_err("route DTO rejects a caller-selected project");
            assert!(
                matches!(rejection, wire::CommandRejection::Fault(_)),
                "unexpected rejection for {project}: {rejection:?}",
            );
            assert_eq!(
                facts_len(bus, "facts after route decode refusal"),
                before,
                "forged route mutated the journal for {project}",
            );
        }
    }
}

pub(super) fn seed_edit_tasks(
    bus: &Bus,
    epoch: CanonicalUuid,
) -> (TaskHandle, TaskHandle, TaskHandle) {
    let alpha = add(
        bus,
        epoch,
        Actor::Operator,
        "BOARD-alpha001",
        "Alpha work",
        Some("todo"),
    );
    let beta = add(
        bus,
        epoch,
        Actor::Operator,
        "BOARD-beta0001",
        "Beta prerequisite",
        Some("todo"),
    );
    let oops = add_oops(
        bus,
        epoch,
        Actor::Operator,
        "OOPS-oops0001",
        "A command was unexpectedly awkward",
    );
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskNote,
        vec![Witness::Existence {
            task: at(&board(bus, epoch, Actor::Operator), &alpha)
                .statement
                .body
                .born
                .clone(),
        }],
        &TaskNote {
            task: alpha.clone(),
            note: "one current observation".to_owned(),
        },
    ));
    (alpha, beta, oops)
}

pub(super) fn exercise_deferral_staleness(bus: &Bus, epoch: CanonicalUuid, alpha: &TaskHandle) {
    let before_defer = board(bus, epoch, Actor::Operator);
    let alpha_before = at(&before_defer, alpha).clone();
    let until = instant_after(current_instant().expect("clock"), 300).expect("future instant");
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDefer,
        vec![
            workflow_witness(&alpha_before),
            deferral_witness(&alpha_before),
        ],
        &TaskDefer {
            task: alpha.clone(),
            wait: until,
        },
    ));
    let stale = issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskWake,
        vec![
            workflow_witness(&alpha_before),
            deferral_witness(&alpha_before),
        ],
        &TaskRef {
            task: alpha.clone(),
        },
    );
    assert!(matches!(
        stale,
        Outcome::Refusal(ref refusal) if refusal.reason() == "board::witness-moved"
    ));
    let deferred = board(bus, epoch, Actor::Operator);
    assert_eq!(at(&deferred, alpha).workflow.body.deferral, Some(until));
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskWake,
        vec![
            workflow_witness(at(&deferred, alpha)),
            deferral_witness(at(&deferred, alpha)),
        ],
        &TaskRef {
            task: alpha.clone(),
        },
    ));
}

pub(super) fn add_dependency(
    bus: &Bus,
    epoch: CanonicalUuid,
    alpha: &TaskHandle,
    beta: &TaskHandle,
) {
    let before_edge = board(bus, epoch, Actor::Operator);
    let alpha_heads = at(&before_edge, alpha);
    let beta_heads = at(&before_edge, beta);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDepends,
        vec![
            statement_witness(alpha_heads),
            workflow_witness(alpha_heads),
            statement_witness(beta_heads),
            workflow_witness(beta_heads),
            Witness::Edge {
                task: alpha_heads.statement.body.born.clone(),
                after: beta_heads.statement.body.born.clone(),
            },
        ],
        &TaskDepends {
            task: alpha.clone(),
            after: vec![beta.clone()],
        },
    ));
}

pub(super) fn assert_graph_handout_and_ledger(
    bus: &Bus,
    epoch: CanonicalUuid,
    alpha: &TaskHandle,
) -> Outcome<CommandAnswer> {
    let graph_one = graph(bus, epoch);
    let graph_two = graph(bus, epoch);
    assert_eq!(graph_one, graph_two);
    let Outcome::Reply(Reply {
        position: graph_position @ ReplyPosition::Snapshot(_),
        body: CommandAnswer::Text(ref graph),
    }) = graph_one
    else {
        panic!("graph was not a text snapshot: {graph_one:?}");
    };
    let visible = board(bus, epoch, Actor::Operator);
    let alpha_heads = at(&visible, alpha);
    let beta_stamp = alpha_heads
        .statement
        .body
        .dependencies
        .first()
        .expect("alpha depends on beta");
    let beta = visible
        .tasks
        .iter()
        .find(|task| &task.statement.body.born == beta_stamp)
        .expect("dependency task is visible");
    assert!(
        graph
            .text
            .contains(&format!("{} -> {}", alpha, beta.statement.body.handle))
    );
    let handout = issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::BoardHandout,
        Vec::new(),
        &BoardHandout {
            output: "-".to_owned(),
            at: None,
        },
    );
    let Outcome::Reply(Reply {
        position: handout_position,
        body: CommandAnswer::Text(ref document),
    }) = handout
    else {
        panic!("handout was not a text snapshot: {handout:?}");
    };
    assert_eq!(handout_position, graph_position);
    assert!(document.text.contains(&graph.text));
    assert_ledger_contains(bus, epoch, alpha, "# Alpha work");
    graph_one
}

fn graph(bus: &Bus, epoch: CanonicalUuid) -> Outcome<CommandAnswer> {
    issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::BoardGraph,
        Vec::new(),
        &BoardGraph {
            view: "dependencies".to_owned(),
            at: None,
        },
    )
}

pub(super) fn assert_ledger_contains(
    bus: &Bus,
    epoch: CanonicalUuid,
    task: &TaskHandle,
    expected: &str,
) {
    let ledger = issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::DocLedger,
        Vec::new(),
        &DocLedger { task: task.clone() },
    );
    assert!(matches!(
        ledger,
        Outcome::Reply(Reply {
            body: CommandAnswer::Text(TextAnswer { ref text }),
            ..
        }) if text.contains(expected)
    ));
}

pub(super) fn assert_render_form_parity(graph: &Outcome<CommandAnswer>) {
    let text = crate::render::render(graph, crate::render::Form::Text).expect("graph text renders");
    let json = crate::render::render(graph, crate::render::Form::Json).expect("graph JSON renders");
    let text_body = wire::intent::AnswerShape::Text
        .decode(text.document())
        .expect("text body is the selected command answer");
    let json_outcome: Outcome<serde_json::Value> =
        wire::decode(json.document()).expect("JSON is the typed outcome");
    let Outcome::Reply(Reply { body, .. }) = graph else {
        unreachable!("graph reply matched above")
    };
    assert_eq!(&text_body, body);
    assert_eq!(
        serde_json::to_value(json_outcome).expect("decoded JSON outcome re-encodes"),
        serde_json::to_value(graph).expect("source outcome encodes"),
    );
}

pub(super) fn assert_oops_note_exists(bus: &Bus, oops: &TaskHandle) {
    let facts = bus.fact_snapshot().expect("fixture facts read");
    let born = oops
        .born()
        .expect("fixture oops handle carries an inception stamp");
    assert!(facts.facts().iter().any(|fact| {
        fact.fact.kind() == wire::FactKind::TaskNoted
            && fact
                .fact
                .body()
                .get("task")
                .and_then(serde_json::Value::as_str)
                == Some(born.as_str())
    }));
}
