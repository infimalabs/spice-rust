//! The real canvas runtime retains only admitted, capability-scoped state.

use app::{
    CanvasRuntime, ChainEnd, FACT_PAGE_LIMIT, LaneStatus, Session, Stale, StreamDirective,
    StreamTerminal, THREAD_PAGE_LIMIT, stale_reads,
};
use wire::{
    CanonicalUuid, CommandKind, Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition,
    Response, Seq, StreamFrame, ViewSubject, WireVersion,
};

#[path = "runtime/effort.rs"]
mod effort;
#[path = "runtime/invalidation_and_chronology.rs"]
mod invalidation_and_chronology;
#[path = "runtime/stream_terminal.rs"]
mod stream_terminal;
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

fn instant_ms(text: &str) -> i64 {
    text.parse::<wire::Instant>()
        .expect("fixture instant")
        .get()
        .as_millisecond()
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
        "origin": "task:1kKRRsDP",
        "fact": {
            "kind": "task::phased",
            "task": "1kKRRsDP",
            "phase": "verify",
        },
    });
    if let Some(cause) = cause {
        value["cause"] = serde_json::json!(cause);
    }
    value
}

fn operator_message_fact(position: u64, kind: &str, text: Option<&str>) -> serde_json::Value {
    let mut fact = serde_json::json!({
        "kind": kind,
        "key": "1kQnrzNZ",
        "lane": LANE,
    });
    if kind == "steer::sent" {
        fact["directive"] = serde_json::json!(REQUEST);
        fact["text"] = serde_json::json!(text.expect("sent fixture carries operator prose"));
        fact["expires"] = serde_json::json!("2030-04-12T19:44:09.271Z");
    }
    serde_json::json!({
        "v": 1,
        "seq": position,
        "at": "2030-04-12T18:44:09.271Z",
        "actor": "operator",
        "origin": "steer:1kQnrzNZ",
        "fact": fact,
    })
}

fn operator_submission_response(
    session: &Session,
) -> app::Admitted<Response<wire::intent::DirectiveAnswer>> {
    let response = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Committed(wire::SeqRange {
                first: seq(41),
                last: seq(42),
            }),
            body: wire::intent::DirectiveAnswer {
                directive: uuid(REQUEST),
                key: "1kQnrzNZ".to_owned(),
                authored: "2030-04-12T18:44:09.271Z".parse().expect("fixture instant"),
                disposition: wire::Disposition::Live,
                delivery: Box::new(wire::intent::DeliveryStanding::Unknown),
            },
        }),
    );
    session
        .admit_frame(&response)
        .expect("operator submission response is admitted")
}

fn journal_ingested_fact(position: u64, cause: Option<u64>) -> serde_json::Value {
    let mut value = serde_json::json!({
        "v": 1,
        "seq": position,
        "at": "2030-04-12T18:44:09.271Z",
        "actor": "spice",
        "origin": format!("lane:{LANE}"),
        "fact": {
            "kind": "journal::ingested",
            "lane": LANE,
            "vendor": "codex",
            "stream": "transcript",
            "source": REQUEST,
            "from": {"member": 0, "offset": 0},
            "to": {"member": 0, "offset": 4096},
            "usage": [],
            "non_usage_records": 3,
            "residue_records": 0,
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

fn directions_frame(text: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "stream": "directions",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "limit": 256,
        "directions": [{
            "authored": "2030-04-12T18:44:09.271Z",
            "directive": REQUEST,
            "key": "1kQnrzNZ",
            "lane": LANE,
            "text": text,
        }],
    }))
    .expect("direction snapshot serializes")
}

fn message_frame(
    lane: &str,
    author: &str,
    offset: u64,
    event: u64,
    activity: Option<&str>,
    text: &str,
) -> Vec<u8> {
    message_frame_at(
        lane,
        author,
        offset,
        event,
        "2030-04-12T18:44:09.271Z",
        activity,
        text,
    )
}

fn message_frame_at(
    lane: &str,
    author: &str,
    offset: u64,
    event: u64,
    authored: &str,
    activity: Option<&str>,
    text: &str,
) -> Vec<u8> {
    let mut message = serde_json::json!({
        "lane": lane,
        "author": author,
        "locus": {"member": 1, "offset": offset, "length": 20, "event": event},
        "authored": authored,
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

fn fixture_stamp(label: &str) -> wire::TaskStamp {
    let value = label.bytes().fold(0_i64, |value, byte| {
        (value * 131 + i64::from(byte)) % 10_000_000_000_000
    });
    wire::TaskStamp::for_millis(value).expect("fixture task stamp")
}

fn board_task(label: &str, _id: &str, title: &str, project: &str) -> wire::intent::BoardTask {
    let visibility = if project.starts_with('.') {
        wire::intent::ProjectVisibility::Hidden {
            stem: "oops".to_owned(),
        }
    } else {
        wire::intent::ProjectVisibility::Public
    };
    let born = fixture_stamp(label);
    wire::intent::BoardTask {
        visibility,
        origin_thread: "operator".to_owned(),
        presentation_title: title.to_owned(),
        statement: wire::intent::Positioned {
            position: seq(1),
            body: wire::intent::BoardStatement {
                handle: wire::TaskHandle::for_project(project, &born),
                born: born.clone(),
                queue_origin: seq(1),
                description: None,
                acceptance: Vec::new(),
                project: project.to_owned(),
                tags: Vec::new(),
                dependencies: Vec::new(),
                provenance: format!("task:{born}").parse().expect("fixture origin"),
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

fn trust_frame(position: u64, horizon: u64) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "stream": "trust",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "packed": {
            "rows": [{
                "position": position,
                "head": {
                    "digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "state": "granted",
                },
            }],
            "horizon": horizon,
        },
    }))
    .expect("trust frame serializes")
}

#[test]
fn stream_application_invalidates_only_observable_runtime_changes() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let live = admitted_stream(&session, &fact_frame(task_fact(41, None)));

    assert!(runtime.apply_stream_frame(&live).changed());
    assert!(
        !runtime.apply_stream_frame(&live).changed(),
        "a duplicate live fact does not invalidate the rendered surface"
    );

    let stale = admitted_stream(&session, &fact_frame(task_fact(40, None)));
    assert!(
        !runtime.apply_stream_frame(&stale).changed(),
        "a stale live fact does not invalidate the rendered surface"
    );

    let pong = wire::encode(&StreamFrame::Pong {
        v: WireVersion::CURRENT,
        id: uuid(REQUEST),
        epoch: uuid(EPOCH),
    })
    .expect("liveness answer encodes");
    assert!(
        !runtime
            .apply_stream_frame(&admitted_stream(&session, &pong))
            .changed(),
        "a Pong does not invalidate the rendered surface"
    );

    let next = Session::open(uuid(NEXT_CAPABILITY), uuid(NEXT_EPOCH));
    let next_accepted = acceptance(&next, "operator", NEXT_EPOCH);
    let mut foreign_runtime = CanvasRuntime::new(&next_accepted);
    assert!(
        !foreign_runtime.apply_stream_frame(&live).changed(),
        "an admitted frame from another scope does not invalidate the rendered surface"
    );

    let mut trust_runtime = CanvasRuntime::new(&accepted);
    let trust = admitted_stream(&session, &trust_frame(41, 41));
    assert!(trust_runtime.apply_stream_frame(&trust).changed());
    assert!(
        !trust_runtime.apply_stream_frame(&trust).changed(),
        "a duplicate trust page does not invalidate the rendered surface"
    );
    let stale_trust = admitted_stream(&session, &trust_frame(40, 40));
    assert!(
        !trust_runtime.apply_stream_frame(&stale_trust).changed(),
        "a stale trust page does not invalidate the rendered surface"
    );
}

#[test]
fn operator_direction_and_agent_response_retain_separate_source_identity() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let operator_text = "Keep the operator role from first render.";
    let submission = operator_submission_response(&session);

    runtime.apply_steer_submission(
        &submission,
        uuid(LANE),
        operator_text.to_owned(),
        Vec::new(),
    );
    assert_eq!(runtime.operator_messages().len(), 1);

    runtime.apply_steer_submission(
        &submission,
        uuid(LANE),
        operator_text.to_owned(),
        Vec::new(),
    );
    let operator = runtime.operator_messages();
    assert_eq!(
        operator.len(),
        1,
        "a repeated command response cannot duplicate its operator record"
    );
    assert_eq!(operator[0].directive(), uuid(REQUEST));
    assert_eq!(operator[0].text(), operator_text);

    runtime.apply_stream(&admitted_stream(
        &session,
        &fact_frame(operator_message_fact(
            42,
            "steer::sent",
            Some(operator_text),
        )),
    ));
    assert_eq!(
        runtime.operator_messages().len(),
        1,
        "the durable fact for a local submission reuses its directive identity"
    );

    runtime.apply_stream(&admitted_stream(
        &session,
        &message_frame_at(
            LANE,
            LANE_CAPABILITY,
            80,
            0,
            "2030-04-12T18:44:10.271Z",
            None,
            "Agent acknowledgement remains its own record.",
        ),
    ));
    assert_eq!(runtime.operator_messages()[0].text(), operator_text);
    assert_eq!(runtime.messages().len(), 1);
    assert_eq!(
        runtime.messages()[0].text,
        "Agent acknowledgement remains its own record."
    );

    runtime.apply_stream(&admitted_stream(
        &session,
        &fact_frame(operator_message_fact(43, "steer::canceled", None)),
    ));
    assert_eq!(
        runtime.operator_messages()[0].directive(),
        uuid(REQUEST),
        "later steering reconciliation does not replace the operator record"
    );
    assert_eq!(runtime.messages().len(), 1);

    for position in 44..=(44 + app::FACT_PAGE_LIMIT as u64) {
        runtime.apply_stream(&admitted_stream(
            &session,
            &fact_frame(task_fact(position, None)),
        ));
    }
    assert_eq!(runtime.fact_pages().len(), app::FACT_PAGE_LIMIT);
    assert_eq!(
        runtime.operator_messages()[0].text(),
        operator_text,
        "unrelated chronology retention never turns operator history temporary"
    );
}

#[test]
fn external_operator_direction_replays_once_before_and_after_disposition() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let operator_text = "External direction keeps its durable identity.";
    let sent = operator_message_fact(41, "steer::sent", Some(operator_text));
    let canceled = operator_message_fact(42, "steer::canceled", None);
    let mut live = CanvasRuntime::new(&accepted);

    live.apply_stream(&admitted_stream(&session, &fact_frame(sent.clone())));
    live.apply_stream(&admitted_stream(&session, &fact_frame(sent.clone())));
    let messages = live.operator_messages().to_vec();
    assert_eq!(
        messages.len(),
        1,
        "a repeated live fact cannot duplicate prose"
    );
    assert_eq!(messages[0].directive(), uuid(REQUEST));
    assert_eq!(messages[0].lane(), uuid(LANE));
    assert_eq!(messages[0].key(), "1kQnrzNZ");
    assert_eq!(messages[0].text(), operator_text);

    live.apply_stream(&admitted_stream(&session, &fact_frame(canceled.clone())));
    assert_eq!(
        live.operator_messages(),
        messages,
        "a later disposition does not relabel or retire operator history"
    );

    let mut reloaded = CanvasRuntime::new(&accepted);
    let snapshot = admitted_stream(&session, &directions_frame(operator_text));
    assert!(reloaded.apply_stream_frame(&snapshot).changed());
    assert_eq!(
        reloaded.operator_messages(),
        messages,
        "bootstrap replay restores the same directive-keyed operator record"
    );
    assert!(
        !reloaded.apply_stream_frame(&snapshot).changed(),
        "an identical durable snapshot is a render no-op"
    );

    let mut out_of_order = CanvasRuntime::new(&accepted);
    out_of_order.apply_stream(&admitted_stream(&session, &fact_frame(task_fact(43, None))));
    let delayed = admitted_stream(
        &session,
        &fact_frame(operator_message_fact(
            41,
            "steer::sent",
            Some(operator_text),
        )),
    );
    assert!(
        out_of_order.apply_stream_frame(&delayed).changed(),
        "an unseen directive remains visible below an advanced chronology cursor"
    );
    assert_eq!(out_of_order.operator_messages(), messages);
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
            ViewSubject::Directions,
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
fn message_snapshot_and_live_push_bound_each_lane_without_evicting_its_siblings() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let first = serde_json::json!({
        "lane": LANE,
        "author": LANE_CAPABILITY,
        "locus": {"member": 1, "offset": 10, "length": 20, "event": 0},
        "authored": "2030-04-12T18:44:09.271Z",
        "text": "first author",
    });
    let second = serde_json::json!({
        "lane": SECOND_LANE,
        "author": SECRET_LANE_CAPABILITY,
        "locus": {"member": 1, "offset": 10, "length": 20, "event": 0},
        "authored": "2030-04-12T18:44:10.271Z",
        "text": "second author",
    });
    let snapshot = serde_json::to_vec(&serde_json::json!({
        "stream": "messages",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "limit": 1,
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
    assert!(
        !runtime
            .apply_stream_frame(&admitted_stream(&session, &snapshot))
            .changed(),
        "an identical retained-message snapshot does not invalidate the surface"
    );
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
    assert!(
        !runtime
            .apply_stream_frame(&admitted_stream(&session, &duplicate))
            .changed(),
        "a duplicate live message does not invalidate the surface"
    );
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
            "authored": "2030-04-12T18:44:11.271Z",
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
    assert_eq!(
        runtime.activities().len(),
        2,
        "one lane's new activity replaces only that lane's prior status"
    );
}

#[test]
fn paired_tool_capture_becomes_one_media_card_while_distinct_tape_events_stay_distinct() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let snapshot = serde_json::to_vec(&serde_json::json!({
        "stream": "messages",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "limit": 8,
        "messages": [],
    }))
    .expect("message snapshot serializes");
    runtime.apply_stream(&admitted_stream(&session, &snapshot));

    let call = message_frame(
        LANE,
        LANE_CAPABILITY,
        60,
        0,
        Some("tool"),
        "view_image: capture.png",
    );
    runtime.apply_stream(&admitted_stream(&session, &call));
    assert!(
        runtime.messages().is_empty(),
        "the call remains activity, so its answering image is rendered only once"
    );

    let media = serde_json::json!({
        "name": "capture.png",
        "content_type": "image/png",
        "data_url": "data:image/png;base64,iVBORw0KGgo=",
    });
    let result = serde_json::to_vec(&serde_json::json!({
        "stream": "message",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "message": {
            "lane": LANE,
            "author": LANE_CAPABILITY,
            "locus": {"member": 1, "offset": 80, "length": 20, "event": 1},
            "authored": "2030-04-12T18:44:09.271Z",
            "activity": "tool",
            "text": "view_image: capture.png succeeded",
            "media": [media.clone()],
        },
    }))
    .expect("tool result frame serializes");
    runtime.apply_stream(&admitted_stream(&session, &result));
    runtime.apply_stream(&admitted_stream(&session, &result));
    assert_eq!(runtime.messages().len(), 1, "one tape event is idempotent");
    assert_eq!(runtime.messages()[0].media.len(), 1);

    let distinct = serde_json::to_vec(&serde_json::json!({
        "stream": "message",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "message": {
            "lane": LANE,
            "author": LANE_CAPABILITY,
            "locus": {"member": 1, "offset": 120, "length": 20, "event": 1},
            "authored": "2030-04-12T18:44:10.271Z",
            "activity": "tool",
            "text": "view_image: capture.png succeeded",
            "media": [media],
        },
    }))
    .expect("distinct tool result frame serializes");
    runtime.apply_stream(&admitted_stream(&session, &distinct));
    assert_eq!(
        runtime.messages().len(),
        2,
        "distinct causal tape events remain separately browsable even for identical bytes"
    );
}

#[test]
fn lineage_activity_orders_tool_and_prose_without_admitting_snapshot_receipts() {
    let runtime = lineage_runtime_with_activity_and_receipts();
    let lanes = runtime.lanes().expect("refreshed lane snapshot");
    let team = runtime
        .newest_lineage_activity(&lanes.lanes[0])
        .expect("team activity survives receipts");
    assert_eq!(team.lane(), uuid(SECOND_LANE));
    assert_eq!(
        runtime
            .newest_lane_activity(uuid(SECOND_LANE))
            .expect("the member lane retains its own newest activity")
            .display_millis(),
        instant_ms("2030-04-12T18:44:10.271Z")
    );
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

fn lineage_runtime_with_activity_and_receipts() -> CanvasRuntime {
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

    let tool = message_frame_at(
        LANE,
        LANE_CAPABILITY,
        60,
        0,
        "2030-04-12T18:44:09.271Z",
        Some("tool"),
        "Bash",
    );
    runtime.apply_stream(&admitted_stream(&session, &tool));
    let lineage = &runtime.lanes().expect("lane snapshot").lanes[0];
    let activity = runtime
        .newest_lineage_activity(lineage)
        .expect("transcript tool becomes activity");
    assert_eq!(activity.lane(), uuid(LANE));
    assert_eq!(
        activity.display_millis(),
        instant_ms("2030-04-12T18:44:09.271Z")
    );
    assert_eq!(activity.class(), "tool");
    assert_eq!(activity.title(), "tool");
    assert_eq!(activity.detail(), "Bash");
    assert!(
        runtime.messages().is_empty(),
        "tool activity does not become a prose card"
    );

    let prose = message_frame_at(
        SECOND_LANE,
        SECRET_LANE_CAPABILITY,
        70,
        0,
        "2030-04-12T18:44:10.271Z",
        None,
        "newer teammate prose",
    );
    runtime.apply_stream(&admitted_stream(&session, &prose));

    let other_tool = message_frame_at(
        THIRD_LANE,
        THIRD_LANE,
        80,
        0,
        "2030-04-12T18:44:11.271Z",
        Some("tool"),
        "Read",
    );
    runtime.apply_stream(&admitted_stream(&session, &other_tool));

    apply_receipt_noise(&session, &mut runtime, &admitted_lanes);
    runtime
}

fn apply_receipt_noise(
    session: &Session,
    runtime: &mut CanvasRuntime,
    admitted_lanes: &app::Admitted<Response<wire::intent::LaneAnswer>>,
) {
    // Snapshot receipts still belong to the command-outcome chronology, but
    // none can enter the lineage activity chronology or displace its prose.
    runtime.apply_lanes(admitted_lanes);
    runtime.apply_board(&board_response(session, "hidden receipt witness"));
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
}

#[test]
fn synthetic_agent_activity_retains_provenance_and_gains_a_display_time() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let lanes = lane_response(&[(LANE, LANE, Some(LANE_CAPABILITY), true)]);
    let admitted_lanes: app::Admitted<Response<wire::intent::LaneAnswer>> = session
        .admit_frame(&lanes)
        .expect("lane response is admitted");
    runtime.apply_lanes(&admitted_lanes);

    let synthetic = serde_json::to_vec(&serde_json::json!({
        "stream": "message",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "message": {
            "lane": LANE,
            "author": LANE_CAPABILITY,
            "locus": {"member": 1, "offset": 60, "length": 20, "event": 0},
            "authored": "2000-01-01T00:00:00.087Z",
            "authored_synthetic": true,
            "text": "historical prose without a vendor timestamp",
        },
    }))
    .expect("synthetic activity fixture serializes");
    runtime.apply_stream(&admitted_stream(&session, &synthetic));

    let activity = runtime
        .newest_lineage_activity(&runtime.lanes().expect("lane snapshot").lanes[0])
        .expect("synthetic prose remains visible as activity");
    assert_eq!(
        activity.detail(),
        "historical prose without a vendor timestamp"
    );
    assert_eq!(
        activity.display_millis(),
        instant_ms("2000-01-01T00:00:00.087Z"),
        "a synthetic ordering instant remains available to the display chronology"
    );
    assert_eq!(
        runtime.messages().len(),
        1,
        "synthetic prose still becomes a browsable message card"
    );
    assert!(runtime.messages()[0].authored_synthetic);
    assert_eq!(
        runtime.messages()[0].observed,
        None,
        "display projection does not relabel ordering identity as Spice observation time"
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
    assert_eq!(
        hidden.statement.body.handle,
        wire::TaskHandle::for_project(".oops.correctness", &fixture_stamp("HIDDEN-oops1"))
    );
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

    assert!(
        runtime.apply_stream(&first).is_empty(),
        "a live fact retains one pending walk without requesting it immediately"
    );
    assert!(
        runtime.apply_stream(&latest).is_empty(),
        "a later live fact replaces the pending walk without issuing another request"
    );
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
fn live_fact_pages_retain_only_the_declared_browser_window() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let delivered = FACT_PAGE_LIMIT + 3;

    for position in 1..=delivered {
        let frame = admitted_stream(&session, &fact_frame(task_fact(position as u64, None)));
        assert!(runtime.apply_stream(&frame).is_empty());
    }

    let pages = runtime.fact_pages();
    assert_eq!(pages.len(), FACT_PAGE_LIMIT);
    assert_eq!(pages[0].rows()[0].seq(), seq(4));
    assert_eq!(
        pages.last().expect("newest page remains").rows()[0].seq(),
        seq(delivered as u64)
    );
}

#[test]
fn causal_thread_fetch_stops_at_the_declared_page_window() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let start = (THREAD_PAGE_LIMIT + 2) as u64;
    let live = admitted_stream(&session, &fact_frame(task_fact(start, Some(start - 1))));
    assert!(runtime.apply_stream(&live).is_empty());

    for page in 0..THREAD_PAGE_LIMIT {
        let position = start - page as u64;
        let continuation = position - 1;
        let thread = admitted_stream(
            &session,
            &packed_frame(
                "thread",
                &[task_fact(position, Some(continuation))],
                Some((1, continuation)),
                start,
            ),
        );
        let directives = runtime.apply_stream(&thread);
        if page + 1 == THREAD_PAGE_LIMIT {
            assert!(directives.is_empty());
        } else {
            assert_eq!(
                directives,
                vec![StreamDirective::Thread {
                    cursor: wire::StreamCursor {
                        epoch: uuid(EPOCH),
                        seq: seq(continuation),
                    },
                }]
            );
        }
    }

    assert_eq!(runtime.thread_pages().len(), THREAD_PAGE_LIMIT);
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
