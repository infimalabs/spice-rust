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
    message_snapshot_at(1, messages, more)
}

fn message_snapshot_at(
    generation: u64,
    messages: Vec<serde_json::Value>,
    more: &[&str],
) -> Vec<u8> {
    let lanes = messages
        .iter()
        .filter_map(|message| message["lane"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let frontiers = lanes
        .into_iter()
        .map(|lane| {
            let truncated = more.contains(&lane);
            let mut frontier = serde_json::json!({
                "lane": lane,
                "state": if truncated { "truncated" } else { "complete" },
            });
            if truncated
                && let Some(before) = messages
                    .iter()
                    .filter(|message| message["lane"] == lane)
                    .min_by_key(|message| message["locus"]["offset"].as_u64())
                    .map(|message| message["locus"].clone())
            {
                frontier["before"] = before;
            }
            frontier
        })
        .collect();
    message_snapshot_frame(generation, 256, messages, frontiers)
}

fn history_page(lane: &str, before: u64, messages: Vec<serde_json::Value>, more: bool) -> Vec<u8> {
    history_page_at(1, lane, before, messages, more)
}

fn history_page_at(
    generation: u64,
    lane: &str,
    before: u64,
    messages: Vec<serde_json::Value>,
    more: bool,
) -> Vec<u8> {
    let requested = serde_json::json!({
        "generation": generation,
        "lane": lane,
        "before": {"member": 1, "offset": before, "length": 20, "event": 0},
    });
    let next = messages
        .iter()
        .min_by_key(|message| message["locus"]["offset"].as_u64())
        .map(|message| message["locus"].clone())
        .or_else(|| more.then(|| requested["before"].clone()));
    let mut frontier = serde_json::json!({
        "lane": lane,
        "state": if more { "truncated" } else { "complete" },
    });
    if let Some(next) = next {
        frontier["before"] = next;
    }
    serde_json::to_vec(&serde_json::json!({
        "stream": "message-history",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "requested": [requested],
        "generation": transcript_generation(generation, messages, vec![frontier]),
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
    let snapshot = message_snapshot(vec![newest], &[LANE]);
    assert!(
        runtime
            .apply_stream_frame(&admitted_stream(&session, &snapshot))
            .changed()
    );
    assert!(runtime.message_history_target(uuid(LANE)).is_some());

    let history = history_page(
        LANE,
        100,
        vec![serde_json::json!({
            "lane": LANE,
            "author": LANE_CAPABILITY,
            "locus": {"member": 1, "offset": 50, "length": 20, "event": 0},
            "authored": "2030-04-12T18:44:09.271Z",
            "text": "older retained history",
        })],
        false,
    );
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

#[test]
fn history_rejects_a_new_card_inside_the_retained_interval() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let snapshot = message_snapshot(
        vec![
            agent_message(LANE, 100, T10, "retained edge", None),
            agent_message(LANE, 200, T12, "retained head", None),
        ],
        &[LANE],
    );
    assert!(apply(&mut runtime, &session, &snapshot));
    assert_eq!(target_offset(&runtime, LANE), Some(100));

    let incompatible = history_page(
        LANE,
        100,
        vec![agent_message(LANE, 150, T11, "not an edge backfill", None)],
        false,
    );
    assert!(!apply(&mut runtime, &session, &incompatible));
    assert_eq!(target_offset(&runtime, LANE), Some(100));
    assert_eq!(
        runtime
            .messages()
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>(),
        ["retained edge", "retained head"],
    );
}

#[test]
fn an_older_reconnect_snapshot_cannot_roll_back_the_transcript_generation() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let current = message_snapshot_at(
        2,
        vec![agent_message(LANE, 200, T12, "current generation", None)],
        &[],
    );
    assert!(apply(&mut runtime, &session, &current));

    assert!(!runtime.reconnect(&accepted));
    let stale = message_snapshot_at(
        1,
        vec![agent_message(LANE, 100, T10, "stale generation", None)],
        &[],
    );
    let application = runtime.apply_stream_frame(&admitted_stream(&session, &stale));
    assert!(!application.changed());
    assert_eq!(application.directives(), [StreamDirective::Reconnect]);
    assert_eq!(
        runtime
            .messages()
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>(),
        ["current generation"],
    );
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
fn coordinated_history_rejects_duplicate_response_frontiers_before_mutation() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    seed_three_member_frontiers(&mut runtime, &session);
    let requested = vec![
        serde_json::json!({
            "generation": 1,
            "lane": LANE,
            "before": {"member": 1, "offset": 100, "length": 20, "event": 0},
        }),
        serde_json::json!({
            "generation": 1,
            "lane": THIRD_LANE,
            "before": {"member": 1, "offset": 300, "length": 20, "event": 0},
        }),
    ];
    let duplicate = serde_json::json!({"lane": LANE, "state": "complete"});
    let response = serde_json::to_vec(&serde_json::json!({
        "stream": "message-history",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "requested": requested,
        "generation": transcript_generation(
            1,
            vec![agent_message(LANE, 50, T09, "must not be merged", None)],
            vec![duplicate.clone(), duplicate],
        ),
    }))
    .expect("duplicate-frontier history serializes");

    assert!(!apply(&mut runtime, &session, &response));
    assert_eq!(target_offset(&runtime, LANE), Some(100));
    assert_eq!(target_offset(&runtime, THIRD_LANE), Some(300));
    assert!(
        runtime
            .messages()
            .iter()
            .all(|message| message.text != "must not be merged")
    );
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

    let live = message_generation_frame(
        2,
        vec![agent_message(LANE, 150, T14, "first live", None)],
        vec![
            serde_json::json!({
                "lane": LANE,
                "state": "truncated",
                "before": {"member": 1, "offset": 100, "length": 20, "event": 0},
            }),
            serde_json::json!({"lane": SECOND_LANE, "state": "complete"}),
            serde_json::json!({
                "lane": THIRD_LANE,
                "state": "truncated",
                "before": {"member": 1, "offset": 250, "length": 20, "event": 0},
            }),
        ],
    );
    assert!(apply(runtime, session, &live));
    assert_eq!(
        runtime.newest_lane_activity(uuid(LANE)).unwrap().detail(),
        "first live"
    );
    assert_eq!(target_offset(runtime, THIRD_LANE), Some(250));

    let stale = history_page_at(
        2,
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

    let first_page = history_page_at(
        2,
        LANE,
        100,
        vec![agent_message(LANE, 50, T08, "first exhausted", None)],
        false,
    );
    assert!(apply(runtime, session, &first_page));
    assert_eq!(target_offset(runtime, LANE), None);
    assert_eq!(target_offset(runtime, THIRD_LANE), Some(250));

    let final_third_page = history_page_at(
        2,
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
    let reconnect = message_snapshot_at(
        2,
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

    let overlap = history_page_at(
        2,
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
