use super::*;

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
        "more": [LANE],
    }))
    .expect("message snapshot serializes");
    assert!(
        runtime
            .apply_stream_frame(&admitted_stream(&session, &snapshot))
            .changed()
    );
    assert!(runtime.message_history_more(uuid(LANE)));

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
    assert!(!runtime.message_history_more(uuid(LANE)));
}
