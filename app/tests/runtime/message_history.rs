use super::*;

fn agent_message(
    lane: &str,
    offset: u64,
    authored: &str,
    text: &str,
    activity: Option<&str>,
) -> serde_json::Value {
    let mut message = serde_json::json!({
        "lane": lane,
        "author": LANE_CAPABILITY,
        "locus": {"member": 1, "offset": offset, "length": 20, "event": 0},
        "authored": authored,
        "text": text,
    });
    if let Some(activity) = activity {
        message["activity"] = serde_json::json!(activity);
    }
    message
}

fn message_snapshot(messages: Vec<serde_json::Value>, more: &[&str]) -> Vec<u8> {
    let cursors = more
        .iter()
        .map(|lane| {
            let before = messages
                .iter()
                .filter(|message| message["lane"] == **lane)
                .min_by_key(|message| message["locus"]["offset"].as_u64())
                .map(|message| message["locus"].clone());
            serde_json::json!({"lane": lane, "before": before})
        })
        .collect::<Vec<_>>();
    serde_json::to_vec(&serde_json::json!({
        "stream": "messages",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "limit": 256,
        "messages": messages,
        "more": cursors,
    }))
    .expect("message snapshot serializes")
}

fn history_page(lane: &str, before: u64, messages: Vec<serde_json::Value>, more: bool) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "stream": "message-history",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "lane": lane,
        "before": {"member": 1, "offset": before, "length": 20, "event": 0},
        "more": more,
        "messages": messages,
    }))
    .expect("message history serializes")
}

fn apply(runtime: &mut CanvasRuntime, session: &Session, frame: &[u8]) -> bool {
    runtime
        .apply_stream_frame(&admitted_stream(session, frame))
        .changed()
}

fn target_offset(runtime: &CanvasRuntime, lane: &str) -> Option<u64> {
    runtime
        .message_history_target(uuid(lane))
        .and_then(|target| target.before.map(|before| before.offset))
}

#[test]
fn hidden_turn_history_advances_to_older_prose_without_rendering_a_card() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let snapshot = message_snapshot(
        vec![agent_message(LANE, 100, T10, "newest prose", None)],
        &[LANE],
    );
    apply(&mut runtime, &session, &snapshot);

    let turns = history_page(
        LANE,
        100,
        vec![agent_message(LANE, 50, T09, "", Some("turn"))],
        true,
    );
    apply(&mut runtime, &session, &turns);
    assert_eq!(target_offset(&runtime, LANE), Some(50));
    assert_eq!(
        runtime
            .messages()
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>(),
        ["newest prose"],
    );

    let older = history_page(
        LANE,
        50,
        vec![agent_message(LANE, 25, T08, "older prose", None)],
        false,
    );
    apply(&mut runtime, &session, &older);
    assert_eq!(
        runtime
            .messages()
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>(),
        ["older prose", "newest prose"],
    );
    assert_eq!(
        runtime.newest_lane_activity(uuid(LANE)).unwrap().detail(),
        "newest prose"
    );
    assert!(runtime.message_history_target(uuid(LANE)).is_none());
}

#[test]
fn older_member_history_merges_without_replacing_the_live_activity_head() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let newest = serde_json::json!({
        "lane": LANE,
        "author": LANE_CAPABILITY,
        "locus": {"member": 1, "offset": 100, "length": 20, "event": 0},
        "authored": "2030-04-12T18:44:10.271Z",
        "text": "newest visible activity",
    });
    let snapshot = serde_json::to_vec(&serde_json::json!({
        "stream": "messages",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "limit": 256,
        "messages": [newest],
        "more": [{
            "lane": LANE,
            "before": {"member": 1, "offset": 100, "length": 20, "event": 0},
        }],
    }))
    .expect("message snapshot serializes");
    assert!(
        runtime
            .apply_stream_frame(&admitted_stream(&session, &snapshot))
            .changed()
    );
    assert!(runtime.message_history_target(uuid(LANE)).is_some());

    let history = serde_json::to_vec(&serde_json::json!({
        "stream": "message-history",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "lane": LANE,
        "before": {"member": 1, "offset": 100, "length": 20, "event": 0},
        "more": false,
        "messages": [{
            "lane": LANE,
            "author": LANE_CAPABILITY,
            "locus": {"member": 1, "offset": 50, "length": 20, "event": 0},
            "authored": "2030-04-12T18:44:09.271Z",
            "text": "older retained history",
        }],
    }))
    .expect("message history serializes");
    assert!(
        runtime
            .apply_stream_frame(&admitted_stream(&session, &history))
            .changed()
    );
    assert_eq!(
        runtime
            .messages()
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>(),
        ["older retained history", "newest visible activity"],
    );
    assert_eq!(runtime.activities().len(), 1);
    assert_eq!(runtime.activities()[0].detail(), "newest visible activity");
    assert!(runtime.message_history_target(uuid(LANE)).is_none());
}

const T07: &str = "2030-04-12T18:44:07.271Z";
const T08: &str = "2030-04-12T18:44:08.271Z";
const T09: &str = "2030-04-12T18:44:09.271Z";
const T10: &str = "2030-04-12T18:44:10.271Z";
const T11: &str = "2030-04-12T18:44:11.271Z";
const T12: &str = "2030-04-12T18:44:12.271Z";
const T13: &str = "2030-04-12T18:44:13.271Z";
const T14: &str = "2030-04-12T18:44:14.271Z";
const T15: &str = "2030-04-12T18:44:15.271Z";
const T16: &str = "2030-04-12T18:44:16.271Z";

fn seed_three_member_frontiers(runtime: &mut CanvasRuntime, session: &Session) {
    let snapshot = message_snapshot(
        vec![
            agent_message(LANE, 100, T12, "first head", None),
            agent_message(
                SECOND_LANE,
                200,
                T11,
                "second compaction",
                Some("compaction"),
            ),
            agent_message(THIRD_LANE, 300, T13, "third head", None),
        ],
        &[LANE, THIRD_LANE],
    );
    assert!(apply(runtime, session, &snapshot));
    assert_eq!(target_offset(runtime, LANE), Some(100));
    assert_eq!(target_offset(runtime, SECOND_LANE), None);
    assert_eq!(target_offset(runtime, THIRD_LANE), Some(300));
}

#[test]
fn empty_open_pages_rearm_only_the_completed_lane_at_the_same_cursor() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    seed_three_member_frontiers(&mut runtime, &session);

    let empty = history_page(LANE, 100, Vec::new(), true);
    assert!(
        apply(&mut runtime, &session, &empty),
        "an admitted empty completion republishes the paging surface",
    );
    assert_eq!(target_offset(&runtime, LANE), Some(100));
    assert_eq!(target_offset(&runtime, THIRD_LANE), Some(300));

    assert!(apply(&mut runtime, &session, &empty));
    assert_eq!(target_offset(&runtime, LANE), Some(100));
    assert_eq!(target_offset(&runtime, THIRD_LANE), Some(300));

    let exhausted = history_page(LANE, 100, Vec::new(), false);
    assert!(apply(&mut runtime, &session, &exhausted));
    assert_eq!(target_offset(&runtime, LANE), None);
    assert_eq!(target_offset(&runtime, THIRD_LANE), Some(300));
}

fn interleave_live_work_and_member_pages(runtime: &mut CanvasRuntime, session: &Session) {
    let third_page = history_page(
        THIRD_LANE,
        300,
        vec![
            agent_message(THIRD_LANE, 250, T09, "third oldest", None),
            agent_message(THIRD_LANE, 275, T10, "third older", None),
        ],
        true,
    );
    assert!(apply(runtime, session, &third_page));
    assert_eq!(target_offset(runtime, THIRD_LANE), Some(250));
    assert_eq!(target_offset(runtime, LANE), Some(100));

    let live = message_frame_at(LANE, LANE_CAPABILITY, 150, 0, T14, None, "first live");
    assert!(apply(runtime, session, &live));
    assert_eq!(
        runtime.newest_lane_activity(uuid(LANE)).unwrap().detail(),
        "first live"
    );
    assert_eq!(target_offset(runtime, THIRD_LANE), Some(250));

    let stale = history_page(
        THIRD_LANE,
        300,
        vec![agent_message(THIRD_LANE, 260, T08, "stale response", None)],
        false,
    );
    assert!(!apply(runtime, session, &stale));
    assert_eq!(target_offset(runtime, THIRD_LANE), Some(250));
    assert!(
        runtime
            .messages()
            .iter()
            .all(|message| message.text != "stale response")
    );

    let first_page = history_page(
        LANE,
        100,
        vec![agent_message(LANE, 50, T08, "first exhausted", None)],
        false,
    );
    assert!(apply(runtime, session, &first_page));
    assert_eq!(target_offset(runtime, LANE), None);
    assert_eq!(target_offset(runtime, THIRD_LANE), Some(250));

    let final_third_page = history_page(
        THIRD_LANE,
        250,
        vec![agent_message(THIRD_LANE, 225, T07, "third frontier", None)],
        true,
    );
    assert!(apply(runtime, session, &final_third_page));
    assert_eq!(target_offset(runtime, THIRD_LANE), Some(225));
}

fn reconnect_and_replay_overlap(
    runtime: &mut CanvasRuntime,
    session: &Session,
    accepted: &app::Admitted<wire::Accepted>,
) {
    assert!(!runtime.reconnect(accepted));
    let reconnect = message_snapshot(
        vec![
            agent_message(LANE, 175, T15, "first reconnected head", None),
            agent_message(
                SECOND_LANE,
                200,
                T11,
                "second compaction",
                Some("compaction"),
            ),
            agent_message(THIRD_LANE, 350, T16, "third reconnected head", None),
        ],
        &[LANE, THIRD_LANE],
    );
    assert!(apply(runtime, session, &reconnect));
    assert_eq!(runtime.message_history_revision(), 1);
    assert_eq!(target_offset(runtime, LANE), Some(175));
    assert_eq!(target_offset(runtime, THIRD_LANE), Some(350));
    for retained in ["first exhausted", "third frontier", "second compaction"] {
        assert!(
            runtime
                .messages()
                .iter()
                .any(|message| message.text == retained),
            "same-scope reconnect retains hydrated card {retained}",
        );
    }

    let overlap = history_page(
        THIRD_LANE,
        350,
        vec![agent_message(THIRD_LANE, 300, T13, "third head", None)],
        true,
    );
    assert!(
        apply(runtime, session, &overlap),
        "a duplicate overlap still advances its member-local frontier",
    );
    assert_eq!(target_offset(runtime, THIRD_LANE), Some(300));
    let identities = runtime
        .messages()
        .iter()
        .map(|message| (message.lane, message.locus))
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(identities.len(), runtime.messages().len());
    assert_eq!(
        runtime
            .messages()
            .iter()
            .filter(|message| message.activity == wire::AgentActivityKind::Compaction)
            .count(),
        1,
    );
    assert!(
        runtime
            .messages()
            .windows(2)
            .all(|pair| { pair[0].chronology_key() < pair[1].chronology_key() })
    );
}

#[test]
fn three_member_frontiers_progress_independently_across_live_work_and_reconnect() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);

    seed_three_member_frontiers(&mut runtime, &session);
    interleave_live_work_and_member_pages(&mut runtime, &session);
    reconnect_and_replay_overlap(&mut runtime, &session, &accepted);
}
