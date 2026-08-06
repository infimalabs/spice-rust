//! The real canvas runtime retains only admitted, capability-scoped state.

use app::{
    CanvasRuntime, ChainEnd, LaneStatus, Session, Stale, StreamDirective, StreamTerminal,
    stale_reads,
};
use wire::{
    CanonicalUuid, CommandKind, Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition,
    Response, Seq, StreamFrame, ViewSubject, WireVersion,
};

mod support;

const OPERATOR: &str = "018f82d4-6f59-7d06-92e3-bce9131163c1";
const LANE_CAPABILITY: &str = "018f82d4-6f59-7d06-92e3-bce9131163c6";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";
const NEXT_EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c9";
const NEXT_CAPABILITY: &str = "018f82d4-6f59-7d06-92e3-bce9131163ca";
const LANE: &str = "018f82d4-6f59-7d06-92e3-bce9131163c2";
const REQUEST: &str = "018f82d4-6f59-7d06-92e3-bce9131163c4";
const SECRET_LANE_CAPABILITY: &str = "018f82d4-6f59-7d06-92e3-bce9131163cc";
const SECOND_LANE: &str = "018f82d4-6f59-7d06-92e3-bce9131163cd";
const THIRD_LANE: &str = "018f82d4-6f59-7d06-92e3-bce9131163ce";

fn uuid(text: &str) -> CanonicalUuid {
    text.parse().expect("fixture uuid is canonical")
}

fn seq(value: u64) -> Seq {
    Seq::new(value).expect("fixture sequence is non-zero")
}

fn session(capability: &str, epoch: &str) -> Session {
    Session::open(uuid(capability), uuid(epoch))
}

fn acceptance(session: &Session, actor: &str, epoch: &str) -> app::Admitted<wire::Accepted> {
    session
        .admit_accepted(format!(r#"{{"wire":1,"actor":"{actor}","epoch":"{epoch}"}}"#).as_bytes())
        .expect("acceptance is admitted")
}

fn task_fact(position: u64, cause: Option<u64>) -> serde_json::Value {
    let mut value = serde_json::json!({
        "v": 1,
        "seq": position,
        "at": "2030-04-12T18:44:09.271Z",
        "actor": "operator",
        "origin": "task:SPICE-aBcD",
        "fact": {
            "kind": "task::phased",
            "task": "SPICE-aBcD",
            "phase": "verify",
        },
    });
    if let Some(cause) = cause {
        value["cause"] = serde_json::json!(cause);
    }
    value
}

fn seated_fact(position: u64, capability: Option<&str>) -> serde_json::Value {
    let mut value = serde_json::json!({
        "v": 1,
        "seq": position,
        "at": "2030-04-12T18:44:09.271Z",
        "actor": "spice",
        "origin": format!("lane:{LANE}"),
        "fact": {
            "kind": "lane::seated",
            "lane": LANE,
            "lineage_id": LANE,
            "worktree": "/tmp/lane",
            "life": "continuous",
            "vendor": "codex",
        },
    });
    if let Some(capability) = capability {
        value["fact"]["capability"] = serde_json::json!(capability);
    }
    value
}

fn packed_frame(
    kind: &str,
    rows: &[serde_json::Value],
    overflow: Option<(u64, u64)>,
    horizon: u64,
) -> Vec<u8> {
    let mut packed = serde_json::json!({
        "rows": rows,
        "horizon": horizon,
    });
    if let Some((omitted, continuation)) = overflow {
        packed["overflow"] = serde_json::json!({
            "omitted": omitted,
            "continuation": continuation,
        });
    }
    serde_json::to_vec(&serde_json::json!({
        "stream": kind,
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "packed": packed,
    }))
    .expect("stream fixture serializes")
}

fn admitted_stream(session: &Session, bytes: &[u8]) -> app::Admitted<StreamFrame> {
    session
        .admit_stream(bytes)
        .expect("stream frame is admitted")
}

fn board_task(handle: &str, id: &str, title: &str, project: &str) -> wire::intent::BoardTask {
    let visibility = if project.starts_with('.') {
        wire::intent::ProjectVisibility::Hidden {
            stem: "oops".to_owned(),
        }
    } else {
        wire::intent::ProjectVisibility::Public
    };
    wire::intent::BoardTask {
        visibility,
        presentation_title: title.to_owned(),
        statement: wire::intent::Positioned {
            position: seq(1),
            body: wire::intent::BoardStatement {
                handle: handle.parse().expect("fixture task handle"),
                uuid: uuid(id),
                description: None,
                acceptance: Vec::new(),
                project: project.to_owned(),
                tags: Vec::new(),
                dependencies: Vec::new(),
                provenance: format!("task:{handle}").parse().expect("fixture origin"),
                revision: seq(1),
            },
        },
        workflow: wire::intent::Positioned {
            position: seq(2),
            body: wire::WorkflowHead {
                flow: vec!["todo".parse().expect("fixture phase")],
                phase: "todo".parse().expect("fixture phase"),
                status: "pending".to_owned(),
                review: "unassigned".to_owned(),
                assignment: None,
                deferral: None,
                completion: None,
            },
        },
        claim: None,
        lease: None,
    }
}

fn board_response(
    session: &Session,
    hidden_title: &str,
) -> app::Admitted<Response<wire::intent::BoardAnswer>> {
    let body = wire::intent::BoardAnswer {
        schema: 1,
        cursor: seq(4),
        principal: wire::Actor::Operator,
        project_policy: wire::intent::BoardProjectPolicy {
            public_stems: vec!["app".to_owned(), "board".to_owned()],
            public_min_depth: 2,
            public_max_depth: 3,
        },
        actor_route: wire::intent::BoardActorRoute {
            actor: "operator".to_owned(),
            project: "agent.operator.task".to_owned(),
            origin_thread: "operator".to_owned(),
        },
        tasks: vec![
            board_task("HIDDEN-oops1", REQUEST, hidden_title, ".oops.correctness"),
            board_task(
                "PUBLIC-work1",
                NEXT_CAPABILITY,
                "Visible allocator work",
                "board.visibility",
            ),
        ],
        residency: wire::intent::Positioned {
            position: seq(3),
            body: wire::ResidencyHead {
                epoch: uuid(EPOCH),
                state: wire::ResidentState::Seated,
                lanes: Vec::new(),
                subscriptions: Vec::new(),
            },
        },
    };
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(4)),
            body,
        }),
    );
    session
        .admit_frame(&bytes)
        .expect("board response is admitted")
}

/// One admitted safe lane roster, one row per `(lane, lineage, occupant, bound)` tuple.
///
/// Session authority and capabilities are intentionally not part of this DTO;
/// only the explicit `bound` projection crosses into the Canvas.
fn lane_response(rows: &[(&str, &str, Option<&str>, bool)]) -> Vec<u8> {
    let subscriptions = rows
        .iter()
        .map(|(_, lineage_id, _, _)| {
            let lineage_id = uuid(lineage_id);
            (
                lineage_id,
                wire::LineageSubscription {
                    lineage_id,
                    policy: wire::intent::LineageLife::Steer,
                    manual_projects: Vec::new(),
                    auto_projects: Vec::new(),
                },
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>()
        .into_values()
        .collect();
    let lanes = rows
        .iter()
        .map(
            |(lane, lineage_id, occupant, bound)| wire::intent::LaneAnswerRow {
                lane: uuid(lane),
                lineage_id: uuid(lineage_id),
                name: None,
                worktree: format!("/tmp/worktree-{}", &lane[..8]),
                life: "steer".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
                occupant: occupant.map(uuid),
                lineage: Vec::new(),
                bound: *bound,
                disposition: if occupant.is_some() {
                    "occupied".to_owned()
                } else {
                    "vacant".to_owned()
                },
                liveness: if occupant.is_some() {
                    wire::intent::LaneLiveness::Running
                } else {
                    wire::intent::LaneLiveness::Vacant
                },
            },
        )
        .collect::<Vec<_>>();
    support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(41)),
            body: wire::intent::LaneAnswer {
                epoch: uuid(EPOCH),
                state: wire::ResidentState::Seated,
                lanes,
                subscriptions,
            },
        }),
    )
}

fn fact_frame(fact: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "stream": "fact",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "fact": fact,
    }))
    .expect("fact fixture serializes")
}

#[test]
fn stream_door_checks_nested_wire_and_terminal_cursor_epoch() {
    let session = session(OPERATOR, EPOCH);
    let mut foreign_envelope = task_fact(41, None);
    foreign_envelope["v"] = serde_json::json!(99);
    let fault = session
        .admit_stream(&packed_frame("facts", &[foreign_envelope], None, 41))
        .expect_err("nested foreign wire faults");
    assert_eq!(fault.reason, "frame::wire-mismatch");

    let stale = serde_json::to_vec(&serde_json::json!({
        "stream": "closing",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "after": {
            "epoch": NEXT_EPOCH,
            "seq": 41,
        },
    }))
    .expect("closing fixture serializes");
    let fault = session
        .admit_stream(&stale)
        .expect_err("foreign terminal cursor faults");
    assert_eq!(fault.reason, "frame::epoch-mismatch");
}

#[test]
fn operator_and_lane_stream_authority_come_from_the_admitted_principal() {
    let operator_session = session(OPERATOR, EPOCH);
    let operator = acceptance(&operator_session, "operator", EPOCH);
    let operator = CanvasRuntime::new(&operator);
    assert_eq!(
        operator.subjects(),
        vec![
            ViewSubject::Facts,
            ViewSubject::Trust,
            ViewSubject::Messages,
        ]
    );
    assert!(operator.grants());
    assert!(operator.offers(CommandKind::ConsentDecide));

    let lane_session = session(LANE_CAPABILITY, EPOCH);
    let lane = acceptance(&lane_session, &format!("lane:{LANE}"), EPOCH);
    let lane = CanvasRuntime::new(&lane);
    assert_eq!(lane.subjects(), vec![ViewSubject::Facts]);
    assert!(!lane.grants());
    assert!(lane.offers(CommandKind::TaskNote));
    assert!(!lane.offers(CommandKind::ConsentDecide));
}

#[test]
fn message_snapshot_and_live_push_retain_distinct_authors_by_tape_identity() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let first = serde_json::json!({
        "lane": LANE,
        "author": LANE_CAPABILITY,
        "locus": {"member": 1, "offset": 10, "length": 20, "event": 0},
        "text": "first author",
    });
    let second = serde_json::json!({
        "lane": LANE,
        "author": SECRET_LANE_CAPABILITY,
        "locus": {"member": 1, "offset": 30, "length": 20, "event": 0},
        "text": "second author",
    });
    let snapshot = serde_json::to_vec(&serde_json::json!({
        "stream": "messages",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "limit": 2,
        "messages": [first.clone(), second.clone()],
    }))
    .expect("message snapshot serializes");
    assert!(
        runtime
            .apply_stream(&admitted_stream(&session, &snapshot))
            .is_empty()
    );
    assert_eq!(
        runtime
            .messages()
            .iter()
            .map(|message| message.author)
            .collect::<Vec<_>>(),
        [uuid(LANE_CAPABILITY), uuid(SECRET_LANE_CAPABILITY)]
    );

    let duplicate = serde_json::to_vec(&serde_json::json!({
        "stream": "message",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "message": second,
    }))
    .expect("duplicate message serializes");
    runtime.apply_stream(&admitted_stream(&session, &duplicate));
    assert_eq!(runtime.messages().len(), 2);

    let newest = serde_json::to_vec(&serde_json::json!({
        "stream": "message",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "message": {
            "lane": LANE,
            "author": THIRD_LANE,
            "locus": {"member": 1, "offset": 50, "length": 20, "event": 0},
            "text": "third author",
        },
    }))
    .expect("live message serializes");
    runtime.apply_stream(&admitted_stream(&session, &newest));
    assert_eq!(
        runtime
            .messages()
            .iter()
            .map(|message| message.author)
            .collect::<Vec<_>>(),
        [uuid(SECRET_LANE_CAPABILITY), uuid(THIRD_LANE)]
    );
}

#[test]
fn browser_state_retains_hidden_evidence_without_hidden_title_prose() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let secret = "secret allocator failure details";
    let response = board_response(&session, secret);
    let wire::Outcome::Reply(reply) = &response.get().outcome else {
        panic!("board fixture is a reply");
    };
    assert_eq!(reply.body.tasks[0].presentation_title(), secret);

    runtime.apply_board(&response);

    let board = runtime.board().expect("board response is retained");
    let hidden = &board.tasks[0];
    assert_eq!(hidden.presentation_title(), "Hidden task");
    assert_eq!(hidden.statement.body.handle.to_string(), "HIDDEN-oops1");
    assert_eq!(hidden.statement.body.project, ".oops.correctness");
    assert_eq!(hidden.visibility.hidden_stem(), Some("oops"));
    assert_eq!(hidden.workflow.body.status, "pending");
    assert!(!format!("{hidden:?}").contains(secret));
    let hidden_json = serde_json::to_value(hidden).expect("browser row serializes");
    assert!(hidden_json["statement"]["body"].get("title").is_none());
    assert_eq!(
        board.tasks[1].presentation_title(),
        "Visible allocator work"
    );
    assert!(!board.tasks[1].visibility.is_hidden());
    let public_json = serde_json::to_value(&board.tasks[1]).expect("public browser row serializes");
    assert!(public_json["statement"]["body"].get("title").is_none());
}

#[test]
fn packed_pages_keep_explicit_overflow_and_continue_without_merging() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let first = admitted_stream(
        &session,
        &packed_frame("facts", &[task_fact(40, None)], Some((2, 41)), 43),
    );

    assert_eq!(
        runtime.apply_stream(&first),
        vec![StreamDirective::Continue {
            subject: ViewSubject::Facts,
            cursor: wire::StreamCursor {
                epoch: uuid(EPOCH),
                seq: seq(41),
            },
        }]
    );
    let first_page = runtime.fact_pages().remove(0);
    assert_eq!(first_page.rows()[0].seq(), seq(40));
    assert_eq!(first_page.overflow().expect("overflow").omitted, 2);
    assert_eq!(first_page.horizon(), Some(seq(43)));
    assert_eq!(runtime.resume().expect("resume cursor").seq, seq(40));

    let second = admitted_stream(
        &session,
        &packed_frame("facts", &[task_fact(41, Some(40))], None, 43),
    );
    assert_eq!(
        runtime.apply_stream(&second),
        vec![StreamDirective::Thread {
            cursor: wire::StreamCursor {
                epoch: uuid(EPOCH),
                seq: seq(41),
            },
        }]
    );
    let pages = runtime.fact_pages();
    assert_eq!(pages.len(), 2, "producer pages remain separate");
    assert_eq!(pages[0].rows()[0].seq(), seq(40));
    assert_eq!(pages[1].rows()[0].seq(), seq(41));
    assert_eq!(runtime.resume().expect("settled horizon").seq, seq(43));
}

#[test]
fn raw_fact_capability_is_dropped_before_any_renderable_row_is_retained() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let leaked = packed_frame(
        "facts",
        &[seated_fact(51, Some(SECRET_LANE_CAPABILITY))],
        None,
        51,
    );
    let fault = session
        .admit_stream(&leaked)
        .expect_err("credential-bearing stream fact is refused");
    assert_eq!(fault.reason, "frame::undecodable");

    let frame = admitted_stream(
        &session,
        &packed_frame("facts", &[seated_fact(51, None)], None, 51),
    );

    runtime.apply_stream(&frame);

    let rendered = format!("{:?}", runtime.fact_pages()[0].rows()[0]);
    assert!(rendered.contains("LaneSeated"), "{rendered}");
    assert!(!rendered.contains(SECRET_LANE_CAPABILITY), "{rendered}");
}

#[test]
fn admitted_thread_pages_render_received_links_and_their_real_terminal_state() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let facts = admitted_stream(
        &session,
        &packed_frame("facts", &[task_fact(43, Some(42))], None, 43),
    );
    runtime.apply_stream(&facts);
    let thread = admitted_stream(
        &session,
        &packed_frame(
            "thread",
            &[task_fact(43, Some(42)), task_fact(42, None)],
            None,
            43,
        ),
    );

    assert!(runtime.apply_stream(&thread).is_empty());
    let walk = runtime.thread_walk().expect("thread start was admitted");
    assert_eq!(
        walk.links()
            .iter()
            .map(app::CausalLink::seq)
            .collect::<Vec<_>>(),
        vec![seq(43), seq(42)]
    );
    assert_eq!(walk.end(), ChainEnd::Root);
}

#[test]
fn a_stale_thread_reply_cannot_merge_into_the_latest_requested_walk() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let first = admitted_stream(&session, &fact_frame(task_fact(41, None)));
    let latest = admitted_stream(&session, &fact_frame(task_fact(42, Some(41))));

    runtime.apply_stream(&first);
    runtime.apply_stream(&latest);
    assert_eq!(
        runtime.pending_thread().expect("pending thread").seq,
        seq(42)
    );

    let stale = admitted_stream(
        &session,
        &packed_frame("thread", &[task_fact(41, None)], None, 42),
    );
    assert!(runtime.apply_stream(&stale).is_empty());
    assert!(runtime.thread_pages().is_empty());

    let current = admitted_stream(
        &session,
        &packed_frame(
            "thread",
            &[task_fact(42, Some(41)), task_fact(41, None)],
            None,
            42,
        ),
    );
    assert!(runtime.apply_stream(&current).is_empty());
    assert_eq!(
        runtime.thread_walk().expect("current walk").end(),
        ChainEnd::Root
    );
    assert_eq!(runtime.pending_thread(), None);
}

#[test]
fn same_scope_reconnect_preserves_pages_and_changed_scope_clears_everything() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let frame = admitted_stream(
        &session,
        &packed_frame("facts", &[task_fact(41, None)], None, 41),
    );
    runtime.apply_stream(&frame);
    assert!(!runtime.reconnect(&acceptance(&session, "operator", EPOCH)));
    assert_eq!(runtime.fact_pages().len(), 1);
    assert_eq!(runtime.resume().expect("resume retained").seq, seq(41));

    let next = Session::open(uuid(NEXT_CAPABILITY), uuid(NEXT_EPOCH));
    let next_accepted = acceptance(&next, "operator", NEXT_EPOCH);
    assert!(runtime.reconnect(&next_accepted));
    assert!(runtime.fact_pages().is_empty());
    assert!(runtime.trust_pages().is_empty());
    assert!(runtime.thread_pages().is_empty());
    assert!(runtime.events().is_empty());
    assert_eq!(runtime.resume(), None);
    assert_eq!(runtime.terminal(), None);
}

#[test]
fn stream_overflow_is_terminal_and_remains_distinct_in_chronology() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let bytes = serde_json::to_vec(&serde_json::json!({
        "stream": "overflow",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "after": {
            "epoch": EPOCH,
            "seq": 55,
        },
        "dropped": 7,
    }))
    .expect("overflow fixture serializes");
    let frame = admitted_stream(&session, &bytes);

    assert_eq!(
        runtime.apply_stream(&frame),
        vec![StreamDirective::Reconnect]
    );
    assert_eq!(
        runtime.terminal(),
        Some(StreamTerminal::Overflow {
            after: wire::StreamCursor {
                epoch: uuid(EPOCH),
                seq: seq(55),
            },
            dropped: 7,
        })
    );
    assert_eq!(runtime.events()[0].class(), "overflow");
}

/// One durable envelope around a fact body, at a position nothing else uses.
fn envelope(
    position: u64,
    actor: &str,
    origin: &str,
    fact: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "v": 1,
        "seq": position,
        "at": "2030-04-12T18:44:09.271Z",
        "actor": actor,
        "origin": origin,
        "fact": fact,
    })
}

/// Which cached read each fact invalidates, as `[board, lanes, inbox, consent]`.
///
/// This inventory is written out here rather than read back off the mapping it
/// checks, so deleting an arm fails instead of agreeing with itself. Every
/// namespace appears twice under different kinds and different producers: a
/// mapping keyed on one exact kind rather than on the namespace fails on the
/// sibling instead of passing on the twin. Four kinds from namespaces no read
/// is drawn from close the other side — a rule that simply refreshed
/// everything on every fact would pass all the rows above and fail these.
fn staleness_inventory() -> Vec<(serde_json::Value, [bool; 4])> {
    let expires = "2030-04-12T19:44:09.271Z";
    vec![
        (
            serde_json::json!({
                "kind": "steer::sent",
                "directive": REQUEST,
                "key": "QmTKOamJ",
                "lane": LANE,
                "text": "look at the tile",
                "expires": expires,
            }),
            [false, false, true, false],
        ),
        (
            serde_json::json!({"kind": "steer::canceled", "key": "QmTKOamJ", "lane": LANE}),
            [false, false, true, false],
        ),
        (
            serde_json::json!({
                "kind": "lane::seated",
                "lane": LANE,
                "lineage_id": LANE,
                "worktree": "/tmp/lane",
                "life": "continuous",
                "vendor": "codex",
            }),
            [false, true, true, false],
        ),
        (
            serde_json::json!({
                "kind": "lane::exited",
                "lane": LANE,
                "occupant": REQUEST,
                "code": 0,
            }),
            [false, true, true, false],
        ),
        (
            serde_json::json!({"kind": "task::phased", "task": "SPICE-aBcD", "phase": "verify"}),
            [true, false, false, false],
        ),
        (
            serde_json::json!({
                "kind": "task::done",
                "task": "SPICE-aBcD",
                "validation": "the suite is green",
            }),
            [true, false, false, false],
        ),
        (
            serde_json::json!({
                "kind": "claim::won",
                "task": "SPICE-aBcD",
                "actor": "operator",
                "lane": LANE,
                "occupant": REQUEST,
                "epoch": EPOCH,
                "until": expires,
            }),
            [true, true, false, false],
        ),
        (
            serde_json::json!({"kind": "claim::freed", "task": "SPICE-aBcD", "actor": "operator"}),
            [true, true, false, false],
        ),
        (
            serde_json::json!({
                "kind": "consent::asked",
                "request": REQUEST,
                "lane": LANE,
                "operation": "write /etc/hosts",
            }),
            [false, false, false, true],
        ),
        (
            serde_json::json!({
                "kind": "consent::granted",
                "request": REQUEST,
                "lane": LANE,
                "rule": "operator",
            }),
            [false, false, false, true],
        ),
        (
            serde_json::json!({"kind": "up::dark", "epoch": EPOCH}),
            [false, false, false, false],
        ),
        (
            serde_json::json!({"kind": "vendor::armed", "vendor": "codex", "version": "0.1.0"}),
            [false, false, false, false],
        ),
        (
            serde_json::json!({"kind": "taste::cleared", "taste": "loud", "locus": "app/src"}),
            [false, false, false, false],
        ),
        (
            serde_json::json!({"kind": "journal::migrated", "from": 1, "to": 2, "epoch": EPOCH}),
            [false, false, false, false],
        ),
    ]
}

#[test]
fn a_live_fact_invalidates_exactly_the_reads_its_namespace_moves() {
    let session = session(OPERATOR, EPOCH);
    for (fact, expected) in staleness_inventory() {
        let kind = fact["kind"]
            .as_str()
            .expect("fixture names its kind")
            .to_owned();
        let pushed = fact_frame(envelope(41, "spice", &format!("lane:{LANE}"), &fact));
        let stale = stale_reads(&admitted_stream(&session, &pushed));
        assert_eq!(
            [stale.board(), stale.lanes(), stale.inbox(), stale.consent()],
            expected,
            "{kind} invalidated the wrong reads"
        );
        assert_eq!(
            stale.any(),
            expected.contains(&true),
            "{kind} disagrees with its own summary"
        );
    }
}

#[test]
fn only_the_tail_of_a_backlog_invalidates_reads_and_a_thread_walk_never_does() {
    let session = session(OPERATOR, EPOCH);
    let sent = serde_json::json!({
        "kind": "steer::sent",
        "directive": REQUEST,
        "key": "QmTKOamJ",
        "lane": LANE,
        "text": "look at the tile",
        "expires": "2030-04-12T19:44:09.271Z",
    });
    let won = serde_json::json!({
        "kind": "claim::won",
        "task": "SPICE-aBcD",
        "actor": "operator",
        "lane": LANE,
        "occupant": REQUEST,
        "epoch": EPOCH,
        "until": "2030-04-12T19:44:09.271Z",
    });
    let lane = format!("lane:{LANE}");
    let ascending = [
        envelope(41, "spice", &lane, &sent),
        envelope(42, "operator", "task:SPICE-aBcD", &won),
    ];

    let tail = admitted_stream(&session, &packed_frame("facts", &ascending, None, 42));
    let stale = stale_reads(&tail);
    assert_eq!(
        [stale.board(), stale.lanes(), stale.inbox(), stale.consent()],
        [true, true, true, false],
        "a page widens to the union of its rows and no further"
    );

    let continued = admitted_stream(
        &session,
        &packed_frame("facts", &ascending, Some((7, 43)), 99),
    );
    assert_eq!(
        stale_reads(&continued),
        Stale::default(),
        "a page that continues defers to the page that ends the backlog"
    );

    let descending = [
        envelope(42, "operator", "task:SPICE-aBcD", &won),
        envelope(41, "spice", &lane, &sent),
    ];
    let thread = admitted_stream(&session, &packed_frame("thread", &descending, None, 42));
    assert_eq!(
        stale_reads(&thread),
        Stale::default(),
        "a causal walk is history rather than an advance"
    );
}

/// The tile ladder is the predecessor's, and every rung has its own cause.
///
/// The five rungs are read from three independent inputs — queued direction,
/// typed occupant liveness, and a declared session — so the fixture varies one
/// at a time rather than presenting lanes that differ in every field. The
/// labels are written out here rather than read back from [`LaneStatus`],
/// because a test that asks the enum what its wording is passes after the
/// wording changes.
///
/// The rank is asserted through a shuffled sort because the strip orders its
/// tiles by `Ord` alone: a ladder that named the five states in the right
/// order but derived the wrong comparison would still put the lane waiting on
/// the operator at the far end of the strip.
#[test]
fn a_lane_tile_ranks_queued_direction_above_liveness_and_liveness_above_a_cold_seat() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let bytes = lane_response(&[
        (SECOND_LANE, SECOND_LANE, None, false),
        (LANE, LANE, Some(OPERATOR), false),
        (THIRD_LANE, THIRD_LANE, None, true),
    ]);
    let admitted: app::Admitted<Response<wire::intent::LaneAnswer>> = session
        .admit_frame(&bytes)
        .expect("lane answer is admitted");
    runtime.apply_lanes(&admitted);

    let lanes = runtime
        .lanes()
        .expect("the roster is retained")
        .lanes
        .clone();
    let [never_seated, occupied, seated_before] = <[_; 3]>::try_from(lanes)
        .unwrap_or_else(|rows| panic!("three rows survive admission, got {}", rows.len()));
    let never_seated = never_seated.host().expect("lineage has one member");
    let occupied = occupied.host().expect("lineage has one member");
    let seated_before = seated_before.host().expect("lineage has one member");
    assert_eq!(never_seated.status(0), LaneStatus::Unbound);
    assert_eq!(
        seated_before.status(0),
        LaneStatus::Bound,
        "a seat with a declared session is bound rather than unbound once its occupant leaves"
    );
    assert_eq!(occupied.status(0), LaneStatus::Running);
    assert_eq!(occupied.status(2), LaneStatus::Queued);
    assert_eq!(
        never_seated.status(1),
        LaneStatus::Queued,
        "queued direction outranks liveness even in a seat nobody has ever occupied"
    );

    assert_eq!(LaneStatus::Queued.label(), "steering queued");
    assert_eq!(LaneStatus::Running.label(), "agent running");
    assert_eq!(LaneStatus::RunningStale.label(), "agent running-stale");
    assert_eq!(LaneStatus::Bound.label(), "agent bound");
    assert_eq!(LaneStatus::Unbound.label(), "agent unbound");
    assert_eq!(LaneStatus::Queued.key(), "queued");
    assert_eq!(LaneStatus::Running.key(), "running");
    assert_eq!(LaneStatus::RunningStale.key(), "running-stale");
    assert_eq!(LaneStatus::Bound.key(), "bound");
    assert_eq!(LaneStatus::Unbound.key(), "unbound");

    let mut strip = vec![
        LaneStatus::Unbound,
        LaneStatus::RunningStale,
        LaneStatus::Running,
        LaneStatus::Bound,
        LaneStatus::Queued,
    ];
    strip.sort_unstable();
    assert_eq!(
        strip,
        vec![
            LaneStatus::Queued,
            LaneStatus::Running,
            LaneStatus::RunningStale,
            LaneStatus::Bound,
            LaneStatus::Unbound,
        ],
        "the strip reads left to right as the predecessor's targetChoiceStatusValues"
    );
}

#[test]
fn one_lineage_groups_multiple_steerable_worktrees_without_losing_member_order() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let bytes = lane_response(&[
        (LANE, EPOCH, Some(OPERATOR), false),
        (SECOND_LANE, EPOCH, None, true),
        (THIRD_LANE, THIRD_LANE, None, false),
    ]);
    let admitted: app::Admitted<Response<wire::intent::LaneAnswer>> = session
        .admit_frame(&bytes)
        .expect("plural lineage answer is admitted");

    runtime.apply_lanes(&admitted);

    let lanes = &runtime.lanes().expect("the roster is retained").lanes;
    assert_eq!(lanes.len(), 2);
    assert_eq!(lanes[0].lane, uuid(EPOCH));
    assert_eq!(lanes[0].members.len(), 2);
    assert_eq!(lanes[0].members[0].lane, uuid(LANE));
    assert_eq!(lanes[0].members[1].lane, uuid(SECOND_LANE));
    assert_eq!(lanes[1].members[0].lane, uuid(THIRD_LANE));
    assert!(
        lanes
            .iter()
            .all(|lineage| lineage.subscription == wire::intent::LineageLife::Steer)
    );
}

#[test]
fn every_distinct_lineage_forms_one_console_even_when_members_have_no_name() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let bytes = lane_response(&[
        (LANE, LANE, None, false),
        (SECOND_LANE, SECOND_LANE, None, false),
        (THIRD_LANE, THIRD_LANE, None, false),
    ]);
    let admitted: app::Admitted<Response<wire::intent::LaneAnswer>> = session
        .admit_frame(&bytes)
        .expect("distinct lineage answer is admitted");

    runtime.apply_lanes(&admitted);

    let lineages = &runtime.lanes().expect("the roster is retained").lanes;
    assert_eq!(
        lineages.len(),
        3,
        "distinct lineage keys never share a console"
    );
    assert_eq!(
        lineages
            .iter()
            .map(|lineage| lineage.lane)
            .collect::<Vec<_>>(),
        vec![uuid(LANE), uuid(SECOND_LANE), uuid(THIRD_LANE)]
    );
    assert!(lineages.iter().all(|lineage| {
        lineage.members.len() == 1 && lineage.host().is_some_and(|member| member.name.is_none())
    }));
}

/// The one steering receipt shape quotes the key exactly as the resident set it.
///
/// The key is a case-sensitive stamp drawn from a vowel-free mixed-case
/// alphabet, and it is the handle an operator types back at the CLI to cancel
/// or recover the directive they just sent. Folding its case would print a key
/// that names no directive. Only the disposition is a word, and it is spelled
/// by an exhaustive match rather than by rendering the variant, so a new
/// retirement state must state its spelling instead of reaching the operator
/// as a debug rendering.
#[test]
fn a_steering_receipt_preserves_the_directive_key_and_lowercases_only_its_disposition() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(41)),
            body: wire::intent::DirectiveAnswer {
                directive: uuid(LANE),
                key: "1kKQtLsP".to_owned(),
                disposition: wire::Disposition::Acked,
            },
        }),
    );
    let admitted: app::Admitted<Response<wire::intent::DirectiveAnswer>> =
        session.admit_frame(&bytes).expect("response is admitted");
    runtime.apply_steer(&admitted, uuid(LANE));

    let event = runtime
        .events()
        .first()
        .expect("steering records one event");
    assert_eq!(event.title(), format!("Steered lane {LANE}"));
    assert_eq!(event.detail(), "key 1kKQtLsP acked");
}

/// A seating receipt names the lane and both dispositions it moved between.
///
/// This is the only confirmation the operator gets that the seat form did
/// anything: the lane list refreshes on its own schedule, so between the click
/// and that refresh the receipt is the whole answer. All three fields are
/// resident-authored and each is load-bearing — the lane says which of several
/// rows moved, and `absent` to `occupied` is what distinguishes a fresh seat
/// from a wake of a lane that was merely vacant.
#[test]
fn a_seating_receipt_names_the_lane_and_the_dispositions_it_moved_between() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(17)),
            body: wire::intent::LaneChangeAnswer {
                lane: uuid(LANE),
                from: "absent".to_owned(),
                to: "occupied".to_owned(),
            },
        }),
    );
    let admitted: app::Admitted<Response<wire::intent::LaneChangeAnswer>> =
        session.admit_frame(&bytes).expect("response is admitted");
    runtime.apply_lane_change(&admitted);

    let event = runtime.events().first().expect("seating records one event");
    assert_eq!(event.title(), "Agent seated");
    assert_eq!(
        event.detail(),
        format!("{LANE} moved from absent to occupied")
    );
}

#[test]
fn admitted_command_reply_refusal_and_fault_remain_three_chronology_rows() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let outcomes: [Outcome<wire::intent::TaskAnswer>; 3] = [
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(41)),
            body: wire::intent::TaskAnswer {
                task: "SPICE-aBcD".parse().expect("fixture task handle"),
                status: "done".to_owned(),
            },
        }),
        Outcome::Refusal(Refusal {
            reason: "task::missing".to_owned(),
            witness: None,
            expected: None,
            observed: None,
            remedy: "use a current handle".to_owned(),
        }),
        Outcome::Fault(Fault {
            class: FaultClass::Protocol,
            reason: "command::stale".to_owned(),
            detail: "epoch moved".to_owned(),
        }),
    ];
    for outcome in outcomes {
        let bytes = support::response(WireVersion::CURRENT, uuid(REQUEST), uuid(EPOCH), outcome);
        let admitted: app::Admitted<Response<wire::intent::TaskAnswer>> =
            session.admit_frame(&bytes).expect("response is admitted");
        runtime.apply_note(&admitted, "SPICE-aBcD");
    }

    assert_eq!(
        runtime
            .events()
            .iter()
            .map(app::RuntimeEvent::class)
            .collect::<Vec<_>>(),
        vec!["fault", "refusal", "reply"]
    );
}

#[test]
fn the_second_command_outcome_is_the_ticker_source() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let outcomes: [Outcome<wire::intent::TaskAnswer>; 2] = [
        Outcome::Refusal(Refusal {
            reason: "task::first".to_owned(),
            witness: None,
            expected: None,
            observed: None,
            remedy: "use the first remedy".to_owned(),
        }),
        Outcome::Fault(Fault {
            class: FaultClass::Protocol,
            reason: "command::second".to_owned(),
            detail: "second detail".to_owned(),
        }),
    ];
    for outcome in outcomes {
        let bytes = support::response(WireVersion::CURRENT, uuid(REQUEST), uuid(EPOCH), outcome);
        let admitted: app::Admitted<Response<wire::intent::TaskAnswer>> =
            session.admit_frame(&bytes).expect("response is admitted");
        runtime.apply_note(&admitted, "SPICE-aBcD");
    }

    let ticker = runtime.newest_event().expect("the ticker has one source");
    assert_eq!(ticker.class(), "fault");
    assert_eq!(ticker.title(), "command::second");
    assert_eq!(ticker.detail(), "second detail");
}

#[test]
fn admitted_steer_reply_names_the_durable_directive_in_chronology() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(41)),
            body: wire::intent::DirectiveAnswer {
                directive: uuid(LANE),
                key: "1kQTest".to_owned(),
                disposition: wire::Disposition::Live,
            },
        }),
    );
    let admitted: app::Admitted<Response<wire::intent::DirectiveAnswer>> = session
        .admit_frame(&bytes)
        .expect("steer response is admitted");

    runtime.apply_steer(&admitted, uuid(LANE));

    assert_eq!(runtime.events()[0].class(), "reply");
    assert_eq!(runtime.events()[0].title(), format!("Steered lane {LANE}"));
    assert_eq!(runtime.events()[0].detail(), "key 1kQTest live");
}
