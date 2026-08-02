//! The real canvas runtime retains only admitted, capability-scoped state.

use app::{CanvasRuntime, ChainEnd, Session, StreamDirective, StreamTerminal};
use wire::{CanonicalUuid, CommandKind, Response, Seq, StreamFrame, ViewSubject};

const OPERATOR: &str = "018f82d4-6f59-7d06-92e3-bce9131163c1";
const LANE_CAPABILITY: &str = "018f82d4-6f59-7d06-92e3-bce9131163c6";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";
const NEXT_EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c9";
const NEXT_CAPABILITY: &str = "018f82d4-6f59-7d06-92e3-bce9131163ca";
const LANE: &str = "018f82d4-6f59-7d06-92e3-bce9131163c2";
const REQUEST: &str = "018f82d4-6f59-7d06-92e3-bce9131163c4";
const SECRET_LANE_CAPABILITY: &str = "018f82d4-6f59-7d06-92e3-bce9131163cc";

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

fn board_task(handle: &str, uuid: &str, title: &str, project: &str) -> serde_json::Value {
    let visibility = if project.starts_with('.') {
        serde_json::json!({"kind": "hidden", "stem": "oops"})
    } else {
        serde_json::json!({"kind": "public"})
    };
    serde_json::json!({
        "visibility": visibility,
        "presentation_title": title,
        "statement": {
            "position": 1,
            "body": {
                "handle": handle,
                "uuid": uuid,
                "project": project,
                "tags": [],
                "dependencies": [],
                "provenance": format!("task:{handle}"),
                "revision": 1,
            },
        },
        "workflow": {
            "position": 2,
            "body": {
                "flow": ["todo"],
                "phase": "todo",
                "status": "pending",
                "review": "unassigned",
            },
        },
    })
}

fn board_response(
    session: &Session,
    hidden_title: &str,
) -> app::Admitted<Response<wire::intent::BoardAnswer>> {
    let body = serde_json::json!({
        "schema": 1,
        "cursor": 4,
        "principal": "operator",
        "tasks": [
            board_task(
                "HIDDEN-oops1",
                REQUEST,
                hidden_title,
                ".oops.correctness",
            ),
            board_task(
                "PUBLIC-work1",
                NEXT_CAPABILITY,
                "Visible allocator work",
                "board.visibility",
            ),
        ],
        "residency": {
            "position": 3,
            "body": {
                "epoch": EPOCH,
                "state": "seated",
                "lanes": [],
            },
        },
    });
    let bytes = serde_json::to_vec(&serde_json::json!({
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "outcome": {
            "kind": "reply",
            "cursor": 4,
            "body": body,
        },
    }))
    .expect("board response fixture serializes");
    session
        .admit_frame(&bytes)
        .expect("board response is admitted")
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
        vec![ViewSubject::Facts, ViewSubject::Trust]
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

#[test]
fn admitted_command_reply_refusal_and_fault_remain_three_chronology_rows() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let outcomes = [
        r#"{"kind":"reply","cursor":41,"body":{"task":"SPICE-aBcD","status":"done"}}"#,
        r#"{"kind":"refusal","reason":"task::missing","remedy":"use a current handle"}"#,
        r#"{"kind":"fault","class":"protocol","reason":"command::stale","detail":"epoch moved"}"#,
    ];
    for outcome in outcomes {
        let bytes = format!(r#"{{"v":1,"id":"{REQUEST}","epoch":"{EPOCH}","outcome":{outcome}}}"#);
        let admitted: app::Admitted<Response<wire::intent::TaskAnswer>> = session
            .admit_frame(bytes.as_bytes())
            .expect("response is admitted");
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
