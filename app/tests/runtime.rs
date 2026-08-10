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

fn message_frame(
    lane: &str,
    author: &str,
    offset: u64,
    event: u64,
    activity: Option<&str>,
    text: &str,
) -> Vec<u8> {
    let mut message = serde_json::json!({
        "lane": lane,
        "author": author,
        "locus": {"member": 1, "offset": offset, "length": 20, "event": event},
        "text": text,
    });
    if let Some(activity) = activity {
        message["activity"] = serde_json::json!(activity);
    }
    serde_json::to_vec(&serde_json::json!({
        "stream": "message",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "message": message,
    }))
    .expect("message frame serializes")
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
        origin_thread: "operator".to_owned(),
        presentation_title: title.to_owned(),
        statement: wire::intent::Positioned {
            position: seq(1),
            body: wire::intent::BoardStatement {
                handle: handle.parse().expect("fixture task handle"),
                uuid: uuid(id),
                queue_origin: seq(1),
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
        allocation_band: 2,
        recent_cell: None,
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
                topology_revision: 0,
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
/// The display session identity crosses; capabilities remain outside this DTO.
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
                model: bound.then(|| "sonnet".to_owned()),
                effort: bound.then(|| "high".to_owned()),
                git: bound.then(|| "current".to_owned()),
                occupant: occupant.map(uuid),
                lineage: Vec::new(),
                session: bound.then(|| "session-fixture".to_owned()),
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
                topology_revision: 9,
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
        "lane": SECOND_LANE,
        "author": SECRET_LANE_CAPABILITY,
        "locus": {"member": 1, "offset": 10, "length": 20, "event": 0},
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
    assert_eq!(
        runtime.activities().len(),
        2,
        "the same tape coordinates in two lanes are two activities"
    );
    runtime.apply_stream(&admitted_stream(&session, &snapshot));
    assert_eq!(
        runtime.activities().len(),
        2,
        "snapshot replay is idempotent"
    );
    assert_eq!(
        runtime.messages().len(),
        2,
        "snapshot replay restores prose cards"
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
fn lineage_activity_orders_tool_and_prose_without_admitting_snapshot_receipts() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let lanes = lane_response(&[
        (LANE, LANE, Some(LANE_CAPABILITY), true),
        (SECOND_LANE, LANE, Some(SECRET_LANE_CAPABILITY), true),
        (THIRD_LANE, THIRD_LANE, Some(THIRD_LANE), true),
    ]);
    let admitted_lanes: app::Admitted<Response<wire::intent::LaneAnswer>> = session
        .admit_frame(&lanes)
        .expect("lane response is admitted");
    runtime.apply_lanes(&admitted_lanes);

    let tool = serde_json::to_vec(&serde_json::json!({
        "stream": "message",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "message": {
            "lane": LANE,
            "author": LANE_CAPABILITY,
            "locus": {"member": 1, "offset": 60, "length": 20, "event": 0},
            "activity": "tool",
            "text": "Bash",
        },
    }))
    .expect("tool activity fixture serializes");
    runtime.apply_stream(&admitted_stream(&session, &tool));
    let lineage = &runtime.lanes().expect("lane snapshot").lanes[0];
    let activity = runtime
        .newest_lineage_activity(lineage)
        .expect("transcript tool becomes activity");
    assert_eq!(activity.lane(), uuid(LANE));
    assert_eq!(activity.class(), "tool");
    assert_eq!(activity.title(), "tool");
    assert_eq!(activity.detail(), "Bash");
    assert!(
        runtime.messages().is_empty(),
        "tool activity does not become a prose card"
    );

    let prose = serde_json::to_vec(&serde_json::json!({
        "stream": "message",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "message": {
            "lane": SECOND_LANE,
            "author": SECRET_LANE_CAPABILITY,
            "locus": {"member": 1, "offset": 70, "length": 20, "event": 0},
            "text": "newer teammate prose",
        },
    }))
    .expect("message fixture serializes");
    runtime.apply_stream(&admitted_stream(&session, &prose));

    let other_tool = serde_json::to_vec(&serde_json::json!({
        "stream": "message",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "message": {
            "lane": THIRD_LANE,
            "author": THIRD_LANE,
            "locus": {"member": 1, "offset": 80, "length": 20, "event": 0},
            "activity": "tool",
            "text": "Read",
        },
    }))
    .expect("other lineage tool fixture serializes");
    runtime.apply_stream(&admitted_stream(&session, &other_tool));

    // Snapshot receipts still belong to the command-outcome chronology, but
    // none can enter the lineage activity chronology or displace its prose.
    runtime.apply_lanes(&admitted_lanes);
    runtime.apply_board(&board_response(&session, "hidden receipt witness"));
    let inbox = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(43)),
            body: wire::intent::InboxAnswer {
                epoch: uuid(EPOCH),
                lanes: Vec::new(),
            },
        }),
    );
    let admitted_inbox: app::Admitted<Response<wire::intent::InboxAnswer>> = session
        .admit_frame(&inbox)
        .expect("inbox response is admitted");
    runtime.apply_inbox(&admitted_inbox);

    let lanes = runtime.lanes().expect("refreshed lane snapshot");
    let team = runtime
        .newest_lineage_activity(&lanes.lanes[0])
        .expect("team activity survives receipts");
    assert_eq!(team.lane(), uuid(SECOND_LANE));
    assert_eq!(team.class(), "prose");
    assert_eq!(team.title(), "assistant");
    assert_eq!(team.detail(), "newer teammate prose");
    let other = runtime
        .newest_lineage_activity(&lanes.lanes[1])
        .expect("other lineage keeps its own activity");
    assert_eq!(other.lane(), uuid(THIRD_LANE));
    assert_eq!(other.class(), "tool");
    assert_eq!(other.detail(), "Read");
    assert_eq!(
        runtime.newest_event().expect("receipt chronology").title(),
        "Steering inbox"
    );
}

#[test]
fn redelivered_older_activity_does_not_displace_newer_lane_activity() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let lanes = lane_response(&[(LANE, LANE, Some(LANE_CAPABILITY), true)]);
    let admitted_lanes: app::Admitted<Response<wire::intent::LaneAnswer>> = session
        .admit_frame(&lanes)
        .expect("lane response is admitted");
    runtime.apply_lanes(&admitted_lanes);

    let older_tool = message_frame(LANE, LANE_CAPABILITY, 60, 0, Some("tool"), "Bash");
    let newer_prose = message_frame(
        LANE,
        LANE_CAPABILITY,
        90,
        1,
        None,
        "the newest thing this agent did",
    );
    for frame in [&older_tool, &newer_prose, &older_tool] {
        runtime.apply_stream(&admitted_stream(&session, frame));
    }

    let activity = runtime
        .newest_lineage_activity(&runtime.lanes().expect("lane snapshot").lanes[0])
        .expect("lane keeps its newest activity");
    assert_eq!(
        activity.detail(),
        "the newest thing this agent did",
        "a redelivered older activity must not displace the newer one"
    );
    assert_eq!(
        runtime.activities().len(),
        2,
        "both unique identities are retained and the replay adds no third row"
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

/// A liveness answer proves the socket carries traffic and nothing else.
///
/// The cursor it must not move is the one a reconnect resumes strictly after,
/// so a probe that advanced it would not show up as a redraw or a stray row --
/// it would show up later, as the frames between the old cursor and the new one
/// never being asked for again.
#[test]
fn a_liveness_answer_moves_neither_the_resume_cursor_nor_any_page() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    runtime.apply_stream(&admitted_stream(
        &session,
        &packed_frame("facts", &[task_fact(41, None)], None, 41),
    ));
    let settled = runtime.resume().expect("resume cursor");
    // Debug covers rows, horizon and overflow together, so this compares more
    // of the page than an assertion naming the fields a probe was expected to
    // leave alone.
    let pages = format!("{:?}", runtime.fact_pages());
    let events = format!("{:?}", runtime.events());
    let pending = runtime.pending_thread();
    let terminal = runtime.terminal();

    let pong = wire::encode(&StreamFrame::Pong {
        v: WireVersion::CURRENT,
        id: uuid(REQUEST),
        epoch: uuid(EPOCH),
    })
    .expect("liveness answer encodes");
    assert_eq!(
        runtime.apply_stream(&admitted_stream(&session, &pong)),
        vec![],
        "a liveness answer asks the surface to do nothing"
    );

    assert_eq!(runtime.resume(), Some(settled));
    assert_eq!(format!("{:?}", runtime.fact_pages()), pages);
    assert_eq!(format!("{:?}", runtime.events()), events);
    // Both are asserted unchanged rather than absent: the fact applied above
    // already opened a thread walk, so requiring absence here would have passed
    // only by describing the fixture wrongly.
    assert_eq!(runtime.pending_thread(), pending);
    assert_eq!(runtime.terminal(), terminal);

    // Reconnecting to the same generation still continues from the cursor the
    // fact settled, which is the property the probe could have quietly spent.
    assert!(!runtime.reconnect(&acceptance(&session, "operator", EPOCH)));
    assert_eq!(runtime.resume(), Some(settled));
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

/// One proven phase whose counters are unlike any other number in this suite.
///
/// The values are distinctive on purpose. A surface that showed a total it
/// derived rather than the one the fold produced would have to reproduce these
/// exact digits by accident to escape notice.
fn proven_usage() -> wire::intent::EffortUsage {
    wire::intent::EffortUsage {
        turns: 12,
        input_tokens: 990_113,
        cached_input_tokens: 990_227,
        cache_write_input_tokens: 990_331,
        output_tokens: 990_449,
        reasoning_output_tokens: Some(990_557),
    }
}

fn launch(model: &str, effort: &str) -> wire::intent::EffortIdentity {
    wire::intent::EffortIdentity {
        vendor: "claude".parse().expect("fixture vendor is a name"),
        model: model.to_owned(),
        effort: effort.to_owned(),
    }
}

fn effort_phase(availability: wire::intent::EffortAvailability) -> wire::intent::EffortPhaseAnswer {
    wire::intent::EffortPhaseAnswer {
        task: "SPICE-aBcD".parse().expect("fixture handle is canonical"),
        phase: "todo".parse().expect("fixture phase is a phase"),
        phase_index: 0,
        opening: seq(41),
        closing: Some(seq(57)),
        renewals: 1,
        opening_boundary: wire::body::SourceBoundary::Proven { heads: Vec::new() },
        closing_boundary: Some(wire::body::SourceBoundary::Proven { heads: Vec::new() }),
        segments: Vec::new(),
        availability,
    }
}

fn effort_answer() -> wire::intent::EffortAnswer {
    wire::intent::EffortAnswer {
        phases: vec![
            effort_phase(wire::intent::EffortAvailability::Available {
                identity: launch("claude-opus-5", "high"),
                usage: proven_usage(),
                wall_ms: 990_661,
            }),
            effort_phase(wire::intent::EffortAvailability::Unavailable {
                reasons: vec![wire::intent::EffortUnavailable {
                    code: wire::intent::EffortUnavailableCode::UntaggedLaunch,
                    detail: "the launch recorded no model".to_owned(),
                }],
            }),
        ],
        aggregates: Vec::new(),
    }
}

fn effort_response(
    session: &Session,
    outcome: Outcome<wire::intent::EffortAnswer>,
) -> app::Admitted<Response<wire::intent::EffortAnswer>> {
    let bytes = support::response(WireVersion::CURRENT, uuid(REQUEST), uuid(EPOCH), outcome);
    session
        .admit_frame(&bytes)
        .expect("effort response is admitted")
}

#[test]
fn an_effort_refusal_replaces_the_held_fold_rather_than_standing_beside_it() {
    let session = session(OPERATOR, EPOCH);
    let mut runtime = CanvasRuntime::new(&acceptance(&session, "operator", EPOCH));
    assert_eq!(runtime.effort(), None, "nothing is held before the read");

    runtime.apply_effort(&effort_response(
        &session,
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(57)),
            body: effort_answer(),
        }),
    ));
    let app::EffortState::Answered(answer) = runtime.effort().expect("the fold is held") else {
        panic!("a reply is held as an answer");
    };
    assert_eq!(answer.phases.len(), 2);

    runtime.apply_effort(&effort_response(
        &session,
        Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new("effort::selection-failed", None, None, None)
                .with_detail("name a task that exists"),
        )),
    ));
    // The counters are gone rather than dimmed: a refusal that left the last
    // good total readable would show a number no read behind it produced.
    let app::EffortState::Withheld(withheld) = runtime.effort().expect("the refusal is held")
    else {
        panic!("a refusal replaces the answer instead of leaving it standing");
    };
    assert_eq!(withheld.class(), "refusal");
    assert_eq!(withheld.reason(), "effort::selection-failed");
    assert_eq!(withheld.detail(), "name a task that exists");

    runtime.apply_effort(&effort_response(
        &session,
        Outcome::Fault(Fault {
            class: FaultClass::Store,
            reason: "effort::fold-failed".to_owned(),
            detail: "the fold store is not readable".to_owned(),
        }),
    ));
    let app::EffortState::Withheld(withheld) = runtime.effort().expect("the fault is held") else {
        panic!("a fault replaces the answer instead of leaving it standing");
    };
    assert_eq!(
        withheld.class(),
        "fault",
        "a fault is a distinct state from a refusal rather than one error word"
    );
    assert_eq!(withheld.detail(), "the fold store is not readable");
}

#[test]
fn a_changed_capability_clears_the_held_effort_fold() {
    let session = session(OPERATOR, EPOCH);
    let mut runtime = CanvasRuntime::new(&acceptance(&session, "operator", EPOCH));
    runtime.apply_effort(&effort_response(
        &session,
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(57)),
            body: effort_answer(),
        }),
    ));
    assert!(runtime.effort().is_some(), "the fold is held in its scope");

    let next = Session::open(uuid(NEXT_CAPABILITY), uuid(NEXT_EPOCH));
    assert!(runtime.reconnect(&acceptance(&next, "operator", NEXT_EPOCH)));
    assert_eq!(
        runtime.effort(),
        None,
        "effort is scoped exactly like the board, the roster and consent"
    );
}

#[test]
fn an_effort_answer_from_another_scope_moves_nothing() {
    let session = session(OPERATOR, EPOCH);
    let mut runtime = CanvasRuntime::new(&acceptance(&session, "operator", EPOCH));
    // The same epoch under another capability: the frame is decodable here,
    // so what stops it is the scope check rather than an earlier refusal.
    let elsewhere = Session::open(uuid(SECRET_LANE_CAPABILITY), uuid(EPOCH));

    runtime.apply_effort(&effort_response(
        &elsewhere,
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(57)),
            body: effort_answer(),
        }),
    ));

    assert_eq!(runtime.effort(), None, "another scope's fold is not held");
    assert!(
        runtime.events().is_empty(),
        "and it leaves no chronology row either"
    );
}

#[test]
fn grouping_crosses_no_part_of_the_launch_tuple() {
    // Each pair differs in exactly one of the three parts, so a tag that
    // dropped or folded any one of them collides here instead of quietly
    // grouping two launches that the fold kept apart.
    let base = launch("claude-opus-5", "high");
    let model = launch("claude-opus-4-8", "high");
    let effort = launch("claude-opus-5", "low");
    let vendor = wire::intent::EffortIdentity {
        vendor: "codex".parse().expect("fixture vendor is a name"),
        ..base.clone()
    };

    let tag = app::effort_identity_tag;
    assert_eq!(tag(&base), "claude/claude-opus-5/high");
    assert_ne!(tag(&base), tag(&model), "the model is part of the tuple");
    assert_ne!(tag(&base), tag(&effort), "the effort is part of the tuple");
    assert_ne!(tag(&base), tag(&vendor), "the vendor is part of the tuple");
    assert_eq!(tag(&base), tag(&launch("claude-opus-5", "high")));
}

#[test]
fn a_complete_zero_is_distinct_from_every_phase_that_spent_something() {
    let nothing = wire::intent::EffortUsage::default();
    assert!(
        app::effort_is_zero(&nothing, 0),
        "a phase that spent nothing reads as a real zero"
    );
    assert!(
        app::effort_is_zero(
            &wire::intent::EffortUsage {
                reasoning_output_tokens: None,
                ..nothing.clone()
            },
            0,
        ),
        "an unreported reasoning counter reports nothing spent"
    );

    // One counter at a time, so a conjunction missing a term fails on the term
    // it dropped rather than passing on the five it kept.
    let spent = [
        wire::intent::EffortUsage {
            turns: 1,
            ..nothing.clone()
        },
        wire::intent::EffortUsage {
            input_tokens: 1,
            ..nothing.clone()
        },
        wire::intent::EffortUsage {
            cached_input_tokens: 1,
            ..nothing.clone()
        },
        wire::intent::EffortUsage {
            cache_write_input_tokens: 1,
            ..nothing.clone()
        },
        wire::intent::EffortUsage {
            output_tokens: 1,
            ..nothing.clone()
        },
        wire::intent::EffortUsage {
            reasoning_output_tokens: Some(1),
            ..nothing.clone()
        },
    ];
    for usage in &spent {
        assert!(
            !app::effort_is_zero(usage, 0),
            "{usage:?} spent something and is not a complete zero"
        );
    }
    assert!(
        !app::effort_is_zero(&nothing, 1),
        "wall time alone is enough to make a phase non-zero"
    );
}

include!("runtime/invalidation_and_chronology.rs");
