//! The real canvas runtime retains only admitted, capability-scoped state.

use app::watchband::{ticker_accessible_label, ticker_chunks};
use app::{
    CanvasRuntime, LaneStatus, Session, Stale, StreamDirective, StreamTerminal, stale_reads,
};
use wire::{
    CanonicalUuid, CommandKind, Outcome, Reply, ReplyPosition, Response, Seq, StreamFrame,
    ViewSubject, WireVersion,
};

#[path = "runtime/detail.rs"]
mod detail;
#[path = "runtime/invalidation_and_chronology.rs"]
mod invalidation_and_chronology;
#[path = "runtime/lane_authority.rs"]
mod lane_authority;
#[path = "runtime/message_history.rs"]
mod message_history;
#[path = "runtime/renewal.rs"]
mod renewal;
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

fn empty_directions_frame() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "stream": "directions",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "limit": 256,
        "directions": [],
    }))
    .expect("empty direction snapshot serializes")
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
    wire::TaskStamp::for_micros(value).expect("fixture task stamp")
}

fn board_task(
    label: &str,
    _id: &str,
    title: &str,
    project: &str,
    requirements: wire::EnvironmentRequirements,
) -> wire::intent::BoardTask {
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
        creator_team: None,
        presentation_title: title.to_owned(),
        statement: wire::intent::Positioned {
            position: seq(1),
            body: wire::intent::BoardStatement {
                handle: wire::TaskHandle::for_project(project, &born),
                born: born.clone(),
                queue_origin: seq(1),
                description: None,
                acceptance: Vec::new(),
                requirements,
                project: project.to_owned(),
                tags: Vec::new(),
                dependencies: Vec::new(),
                provenance: format!("task:{born}").parse().expect("fixture origin"),
                creation: wire::body::TaskCreationContext::unavailable(
                    wire::body::TaskCreationUnavailable::OperatorActor,
                ),
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
        claim_context: None,
        git_context: None,
    }
}

fn board_response(
    session: &Session,
    hidden_title: &str,
) -> app::Admitted<Response<wire::intent::BoardAnswer>> {
    let body = wire::intent::BoardAnswer {
        allocation_band: 2,
        recent_cell: None,
        offer: Vec::new(),
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
            board_task(
                "HIDDEN-oops1",
                REQUEST,
                hidden_title,
                ".oops.correctness",
                wire::EnvironmentRequirements::default(),
            ),
            board_task(
                "PUBLIC-work1",
                NEXT_CAPABILITY,
                "Visible allocator work",
                "board.visibility",
                wire::EnvironmentRequirements::one(
                    wire::EnvironmentRequirement::journal_schema(2)
                        .expect("future fixture schema is positive"),
                ),
            ),
        ],
        history: None,
        trust: Vec::new(),
        residency: wire::intent::Positioned {
            position: seq(3),
            body: wire::ResidencyHead {
                epoch: uuid(EPOCH),
                state: wire::ResidentState::Seated,
                topology_revision: 0,
                lanes: Vec::new(),
                subscriptions: Vec::new(),
                renewals: Vec::new(),
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

fn admitted_board_answer(
    session: &Session,
    body: wire::intent::BoardAnswer,
) -> app::Admitted<Response<wire::intent::BoardAnswer>> {
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(body.cursor),
            body,
        }),
    );
    session
        .admit_frame(&bytes)
        .expect("board answer is admitted")
}

/// One admitted safe lane roster, one row per `(lane, team, occupant, bound)` tuple.
///
/// The display session identity crosses; capabilities remain outside this DTO.
fn lane_response(rows: &[(&str, &str, Option<&str>, bool)]) -> Vec<u8> {
    lane_response_with_renewals(rows, Vec::new())
}

fn lane_response_with_renewals(
    rows: &[(&str, &str, Option<&str>, bool)],
    renewals: Vec<wire::LaneRenewalRecord>,
) -> Vec<u8> {
    let subscriptions = rows
        .iter()
        .map(|(_, team, _, _)| {
            let team = uuid(team);
            (
                team,
                wire::TeamSubscription {
                    team,
                    policy: wire::intent::TeamLife::Steer,
                },
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>()
        .into_values()
        .collect();
    let lanes = rows
        .iter()
        .map(
            |(lane, team, occupant, bound)| wire::intent::LaneAnswerRow {
                lane: uuid(lane),
                team: uuid(team),
                name: None,
                worktree: format!("/tmp/worktree-{}", &lane[..8]),
                life: "steer".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
                model: bound.then(|| "sonnet".to_owned()),
                seat_model: None,
                effort: bound.then(|| "high".to_owned()),
                git: bound.then(|| "current".to_owned()),
                rtk: None,
                occupant: occupant.map(uuid),
                lineage: Vec::new(),
                jobs: Vec::new(),
                session: bound.then(|| "session-fixture".to_owned()),
                bound: *bound,
                disposition: if occupant.is_some() {
                    "occupied".to_owned()
                } else {
                    "vacant".to_owned()
                },
                liveness: if occupant.is_some() {
                    wire::intent::LaneLiveness::Working
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
                renewals,
                refresh_after_millis: None,
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
    let operator = runtime.operator_messages().to_vec();
    assert_eq!(
        operator.len(),
        1,
        "a repeated command response cannot duplicate its operator record"
    );
    assert_eq!(operator[0].directive(), uuid(REQUEST));
    assert_eq!(operator[0].text(), operator_text);

    assert!(
        !runtime
            .apply_stream_frame(&admitted_stream(&session, &empty_directions_frame()))
            .changed(),
        "a stale refresh cannot erase a locally accepted direction"
    );
    assert_eq!(
        runtime.operator_messages(),
        operator.as_slice(),
        "the accepted directive keeps one stable identity across refresh"
    );

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

    assert_eq!(
        runtime.operator_messages()[0].text(),
        operator_text,
        "unrelated facts never turn operator history temporary"
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
            ViewSubject::Messages,
        ]
    );
    assert!(operator.offers(CommandKind::ConsentDecide));

    let lane_session = session(LANE_CAPABILITY, EPOCH);
    let lane = acceptance(&lane_session, &format!("lane:{LANE}"), EPOCH);
    let lane = CanvasRuntime::new(&lane);
    assert_eq!(lane.subjects(), vec![ViewSubject::Facts]);
    assert!(lane.offers(CommandKind::TaskNote));
    assert!(!lane.offers(CommandKind::ConsentDecide));
}

/// One lane filled to its message floor beside a lane that said one thing.
fn lane_floor_snapshot() -> (Vec<serde_json::Value>, serde_json::Value, Vec<u8>) {
    let floor = u64::try_from(wire::MESSAGE_FLOOR).expect("the floor fits an offset");
    let busy = (0..floor)
        .map(|offset| {
            serde_json::json!({
                "lane": LANE,
                "author": LANE_CAPABILITY,
                "locus": {"member": 1, "offset": 10 + offset, "length": 20, "event": 0},
                "authored": "2030-04-12T18:44:09.271Z",
                "text": format!("first author {offset}"),
            })
        })
        .collect::<Vec<_>>();
    let second = serde_json::json!({
        "lane": SECOND_LANE,
        "author": SECRET_LANE_CAPABILITY,
        "locus": {"member": 1, "offset": 10, "length": 20, "event": 0},
        "authored": "2030-04-12T18:44:10.271Z",
        "text": "second author",
    });
    let mut incoming = busy.clone();
    incoming.push(second.clone());
    let snapshot = serde_json::to_vec(&serde_json::json!({
        "stream": "messages",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "limit": wire::MESSAGE_FLOOR,
        "messages": incoming,
    }))
    .expect("message snapshot serializes");
    (busy, second, snapshot)
}

#[test]
fn message_snapshot_and_live_push_bound_each_lane_without_evicting_its_siblings() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let (busy, second, snapshot) = lane_floor_snapshot();
    assert!(
        runtime
            .apply_stream(&admitted_stream(&session, &snapshot))
            .is_empty()
    );
    assert_eq!(runtime.messages().len(), wire::MESSAGE_FLOOR + 1);
    assert_eq!(
        runtime
            .activities()
            .iter()
            .filter(|activity| activity.lane() == uuid(SECOND_LANE))
            .count(),
        1,
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
        wire::MESSAGE_FLOOR + 1,
        "snapshot replay is idempotent"
    );
    assert_eq!(
        runtime.messages().len(),
        wire::MESSAGE_FLOOR + 1,
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
    assert_eq!(runtime.messages().len(), wire::MESSAGE_FLOOR + 1);

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
    let retained = runtime.messages();
    assert_eq!(
        retained
            .iter()
            .filter(|message| message.lane == uuid(LANE))
            .map(|message| message.locus.offset)
            .collect::<Vec<_>>(),
        busy.iter()
            .skip(1)
            .map(|card| card["locus"]["offset"].as_u64().expect("fixture offset"))
            .chain(std::iter::once(50))
            .collect::<Vec<_>>(),
        "the busy lane slides its own floor forward",
    );
    assert_eq!(
        retained
            .iter()
            .filter(|message| message.lane == uuid(SECOND_LANE))
            .map(|message| message.author)
            .collect::<Vec<_>>(),
        [uuid(SECRET_LANE_CAPABILITY)],
        "the quiet sibling keeps everything it said",
    );
    assert_eq!(
        runtime
            .activities()
            .iter()
            .filter(|activity| activity.lane() == uuid(SECOND_LANE))
            .count(),
        1,
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
fn team_activity_orders_tool_and_prose_without_admitting_snapshot_receipts() {
    let runtime = team_runtime_with_activity_and_receipts();
    let lanes = runtime.lanes().expect("refreshed lane snapshot");
    let team = runtime
        .newest_team_activity(&lanes.lanes[0])
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
    assert_eq!(team.detail(), "newer team member prose");
    let other = runtime
        .newest_team_activity(&lanes.lanes[1])
        .expect("other team keeps its own activity");
    assert_eq!(other.lane(), uuid(THIRD_LANE));
    assert_eq!(other.class(), "tool");
    assert_eq!(other.detail(), "Read");
}

fn team_runtime_with_activity_and_receipts() -> CanvasRuntime {
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
    let team = &runtime.lanes().expect("lane snapshot").lanes[0];
    let activity = runtime
        .newest_team_activity(team)
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
        "newer team member prose",
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
    // none can enter the team activity chronology or displace its prose.
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
        .newest_team_activity(&runtime.lanes().expect("lane snapshot").lanes[0])
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
        .newest_team_activity(&runtime.lanes().expect("lane snapshot").lanes[0])
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
fn team_ticker_prose_leads_with_the_first_authored_line_after_borrowed_quotes() {
    // The chrome ticker's line budget; the composition below is the one
    // `render_team_ticker` performs over `detail()` for a prose activity.
    const TICKER_BUDGET: usize = 160;
    const STANDING_LABEL: &str = "Jeff";

    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let lanes = lane_response(&[(LANE, LANE, Some(LANE_CAPABILITY), true)]);
    let admitted_lanes: app::Admitted<Response<wire::intent::LaneAnswer>> = session
        .admit_frame(&lanes)
        .expect("lane response is admitted");
    runtime.apply_lanes(&admitted_lanes);

    let quoted_reply = message_frame(
        LANE,
        LANE_CAPABILITY,
        60,
        0,
        None,
        "> ship the ticker fix today\n>\n  > and keep the composer rule\n\n\
         On it: the ticker shares the composer header rule.\n\
         A second authored line stays on the card.",
    );
    runtime.apply_stream(&admitted_stream(&session, &quoted_reply));
    let activity = runtime
        .newest_team_activity(&runtime.lanes().expect("lane snapshot").lanes[0])
        .expect("quoted prose still becomes activity");
    assert_eq!(activity.class(), "prose");
    assert_eq!(
        activity.detail(),
        "On it: the ticker shares the composer header rule.",
        "the ticker leads with the first authored line, never the borrowed quote"
    );
    let (prefix, detail) = ticker_chunks(STANDING_LABEL, activity.detail(), TICKER_BUDGET);
    let line = format!("{prefix}: {detail}");
    let accessible =
        ticker_accessible_label(STANDING_LABEL, activity.class(), &line, TICKER_BUDGET);
    assert_eq!(
        line,
        "Jeff: On it: the ticker shares the composer header rule."
    );
    assert_eq!(
        accessible,
        "Jeff: Jeff: On it: the ticker shares the composer header rule."
    );
    for quoted in ["ship the ticker fix", "keep the composer rule", ">"] {
        assert!(
            !line.contains(quoted) && !accessible.contains(quoted),
            "{quoted:?} is the operator's quoted text, not the agent's words"
        );
    }
    assert!(
        runtime
            .messages()
            .iter()
            .any(|message| message.text.starts_with("> ship the ticker fix today\n")),
        "the conversation card still carries the message verbatim, quotes included"
    );

    let entirely_quoted = message_frame(
        LANE,
        LANE_CAPABILITY,
        90,
        1,
        None,
        "> only the operator's words\n>\n> and nothing authored underneath",
    );
    runtime.apply_stream(&admitted_stream(&session, &entirely_quoted));
    let activity = runtime
        .newest_team_activity(&runtime.lanes().expect("lane snapshot").lanes[0])
        .expect("an entirely quoted reply is still the newest activity");
    assert_eq!(
        activity.detail(),
        "quoted message",
        "with no authored line the ticker wears the composer header's absent-preview label"
    );
    let (prefix, detail) = ticker_chunks(STANDING_LABEL, activity.detail(), TICKER_BUDGET);
    assert_eq!(format!("{prefix}: {detail}"), "Jeff: quoted message");

    let tool = message_frame(
        LANE,
        LANE_CAPABILITY,
        120,
        2,
        Some("tool"),
        "Bash: grep -n '> quoted' app/src/watchband.rs",
    );
    runtime.apply_stream(&admitted_stream(&session, &tool));
    let activity = runtime
        .newest_team_activity(&runtime.lanes().expect("lane snapshot").lanes[0])
        .expect("tool activity follows");
    assert_eq!(
        activity.detail(),
        "Bash: grep -n '> quoted' app/src/watchband.rs",
        "only prose adopts the quote rule; a tool detail keeps its text verbatim"
    );
}

#[test]
fn browser_state_retains_hidden_rows_with_their_authored_prose() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let title = "Allocator failure needing triage";
    let response = board_response(&session, title);
    let wire::Outcome::Reply(reply) = &response.get().outcome else {
        panic!("board fixture is a reply");
    };
    assert_eq!(reply.body.tasks[0].presentation_title(), title);

    runtime.apply_board(&response);

    let board = runtime.board().expect("board response is retained");
    assert_eq!(board.schema, 1);
    let hidden = &board.tasks[0];
    assert_eq!(hidden.presentation_title(), title);
    assert_eq!(
        hidden.statement.body.handle,
        wire::TaskHandle::for_project(".oops.correctness", &fixture_stamp("HIDDEN-oops1"))
    );
    assert_eq!(hidden.statement.body.project, ".oops.correctness");
    assert_eq!(hidden.visibility.hidden_stem(), Some("oops"));
    assert_eq!(hidden.workflow.body.status, "pending");
    let hidden_json = serde_json::to_value(hidden).expect("browser row serializes");
    assert_eq!(hidden_json["presentation_title"], title);
    assert_eq!(
        board.tasks[1].presentation_title(),
        "Visible allocator work"
    );
    assert!(!board.tasks[1].visibility.is_hidden());
    assert!(
        !board.environment_satisfies(&board.tasks[1]),
        "the Canvas snapshot evaluates requirements against its observed schema",
    );
}

#[test]
fn work_history_merges_once_and_old_pages_cannot_cross_a_board_refresh() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let open = board_response(&session, "Hidden open work");
    runtime.apply_board(&open);

    let first = runtime
        .begin_work_history()
        .expect("the current open board admits its first history page");
    assert_eq!(first.before, None);
    assert!(
        runtime.begin_work_history().is_none(),
        "duplicate activation is coalesced while the page is loading",
    );

    let mut history_task = board_task(
        "HISTORY-one",
        THIRD_LANE,
        "Retained terminal work",
        "app.history",
        wire::EnvironmentRequirements::default(),
    );
    history_task.workflow.position = seq(10);
    history_task.workflow.body.status = "done".to_owned();
    let next = wire::intent::TaskHistoryCursor {
        workflow: history_task.workflow.position,
        task: history_task.statement.body.born.clone(),
    };
    let mut body = match &open.get().outcome {
        Outcome::Reply(reply) => reply.body.clone(),
        Outcome::Refusal(_) | Outcome::Fault(_) => panic!("open fixture is a reply"),
    };
    body.tasks = vec![history_task];
    body.history = Some(wire::intent::TaskHistoryPage {
        before: None,
        limit: 50,
        total: 2,
        next: Some(next.clone()),
    });
    runtime.apply_work_history(&first, &admitted_board_answer(&session, body.clone()));

    assert_eq!(runtime.work_history().loaded(), 1);
    assert_eq!(runtime.work_history().total(), Some(2));
    assert_eq!(
        runtime.board().expect("board remains present").tasks.len(),
        3
    );
    let second = runtime
        .begin_work_history()
        .expect("the advertised next cursor is requested");
    assert_eq!(second.before.as_ref(), Some(&next));

    runtime.apply_board(&board_response(&session, "Refreshed hidden work"));
    body.history = Some(wire::intent::TaskHistoryPage {
        before: Some(next),
        limit: 50,
        total: 2,
        next: None,
    });
    runtime.apply_work_history(&second, &admitted_board_answer(&session, body));

    assert_eq!(runtime.work_history().loaded(), 0);
    assert_eq!(runtime.work_history().total(), None);
    assert_eq!(
        runtime
            .board()
            .expect("refreshed board remains present")
            .tasks
            .len(),
        2,
        "a page from the previous revision cannot mix into the new snapshot",
    );
}

/// A liveness answer proves the socket carries traffic and nothing else.
///
/// The cursor it must not move is the one a reconnect resumes strictly after,
/// so a probe that advanced it would show up later as skipped frames.
#[test]
fn a_liveness_answer_moves_neither_the_resume_cursor_nor_terminal() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    runtime.apply_stream(&admitted_stream(
        &session,
        &packed_frame("facts", &[task_fact(41, None)], None, 41),
    ));
    let settled = runtime.resume().expect("resume cursor");
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
    assert_eq!(runtime.terminal(), terminal);

    // Reconnecting to the same generation still continues from the cursor the
    // fact settled, which is the property the probe could have quietly spent.
    assert!(!runtime.reconnect(&acceptance(&session, "operator", EPOCH)));
    assert_eq!(runtime.resume(), Some(settled));
}

#[test]
fn same_scope_reconnect_preserves_cursor_and_changed_scope_clears_it() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let frame = admitted_stream(
        &session,
        &packed_frame("facts", &[task_fact(41, None)], None, 41),
    );
    runtime.apply_stream(&frame);
    assert!(!runtime.reconnect(&acceptance(&session, "operator", EPOCH)));
    assert_eq!(runtime.resume().expect("resume retained").seq, seq(41));

    let next = Session::open(uuid(NEXT_CAPABILITY), uuid(NEXT_EPOCH));
    let next_accepted = acceptance(&next, "operator", NEXT_EPOCH);
    assert!(runtime.reconnect(&next_accepted));
    assert_eq!(runtime.resume(), None);
    assert_eq!(runtime.terminal(), None);
}
