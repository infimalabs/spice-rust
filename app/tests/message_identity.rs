use std::rc::Rc;

use app::{CanvasRuntime, Session};
use wire::{CanonicalUuid, StreamFrame};

const OPERATOR: &str = "018f82d4-6f59-7d06-92e3-bce9131163c1";
const LANE_CAPABILITY: &str = "018f82d4-6f59-7d06-92e3-bce9131163c6";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";
const REQUEST: &str = "018f82d4-6f59-7d06-92e3-bce9131163c4";
const SECRET_LANE_CAPABILITY: &str = "018f82d4-6f59-7d06-92e3-bce9131163cc";
const LANE: &str = "018f82d4-6f59-7d06-92e3-bce9131163c2";
const SECOND_LANE: &str = "018f82d4-6f59-7d06-92e3-bce9131163cd";

fn uuid(text: &str) -> CanonicalUuid {
    text.parse().expect("fixture uuid is canonical")
}

fn admitted(session: &Session, bytes: &[u8]) -> app::Admitted<StreamFrame> {
    session
        .admit_stream(bytes)
        .expect("stream frame is admitted")
}

fn snapshot(messages: Vec<serde_json::Value>) -> Vec<u8> {
    snapshot_with_more(messages, &[])
}

fn snapshot_with_more(messages: Vec<serde_json::Value>, more: &[&str]) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "stream": "messages",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "limit": 8,
        "messages": messages,
        "more": more,
    }))
    .expect("message snapshot serializes")
}

fn live(message: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "stream": "message",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "message": message,
    }))
    .expect("live message serializes")
}

fn card_bytes(runtime: &CanvasRuntime) -> Vec<u8> {
    let messages = runtime
        .messages()
        .iter()
        .map(|message| message.as_ref())
        .collect::<Vec<_>>();
    serde_json::to_vec(&messages).expect("cards serialize")
}

#[test]
fn revised_message_snapshot_preserves_unchanged_allocation_identity() {
    let session = Session::open(uuid(OPERATOR), uuid(EPOCH));
    let accepted = session
        .admit_accepted(format!(r#"{{"wire":1,"actor":"operator","epoch":"{EPOCH}"}}"#).as_bytes())
        .expect("acceptance is admitted");
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
    let bounded_snapshot = |first, second| {
        serde_json::to_vec(&serde_json::json!({
            "stream": "messages",
            "v": 1,
            "id": REQUEST,
            "epoch": EPOCH,
            "limit": 1,
            "messages": [first, second],
        }))
        .expect("message snapshot serializes")
    };
    runtime.apply_stream(&admitted(
        &session,
        &bounded_snapshot(first.clone(), second.clone()),
    ));
    let retained_first = Rc::clone(&runtime.messages()[0]);
    let retained_second = Rc::clone(&runtime.messages()[1]);

    let mut revised_second = second.clone();
    revised_second["text"] = serde_json::json!("second author revised");
    assert!(
        runtime
            .apply_stream_frame(&admitted(&session, &live(revised_second.clone())))
            .changed(),
        "a live content revision invalidates the owning card"
    );
    assert!(Rc::ptr_eq(&retained_first, &runtime.messages()[0]));
    assert!(!Rc::ptr_eq(&retained_second, &runtime.messages()[1]));
    let retained_revision = Rc::clone(&runtime.messages()[1]);
    assert!(
        !runtime
            .apply_stream_frame(&admitted(&session, &live(revised_second)))
            .changed(),
        "replaying the revision is idempotent"
    );
    assert!(Rc::ptr_eq(&retained_first, &runtime.messages()[0]));
    assert!(Rc::ptr_eq(&retained_revision, &runtime.messages()[1]));
    assert_eq!(
        runtime
            .newest_lane_activity(uuid(SECOND_LANE))
            .expect("revised activity remains projected")
            .detail(),
        "second author revised"
    );
}

#[test]
fn mixed_cards_reduce_identically_across_snapshot_history_and_replay() {
    let session = Session::open(uuid(OPERATOR), uuid(EPOCH));
    let accepted = session
        .admit_accepted(format!(r#"{{"wire":1,"actor":"operator","epoch":"{EPOCH}"}}"#).as_bytes())
        .expect("acceptance is admitted");
    let older = serde_json::json!({
        "lane": LANE,
        "author": LANE_CAPABILITY,
        "locus": {"member": 1, "offset": 10, "length": 20, "event": 0},
        "authored": "2030-04-12T18:44:09.271Z",
        "text": "older prose",
    });
    let boundary = serde_json::json!({
        "lane": LANE,
        "author": LANE_CAPABILITY,
        "locus": {"member": 1, "offset": 30, "length": 20, "event": 0},
        "authored": "2030-04-12T18:44:10.271Z",
        "activity": "compaction",
        "text": "Compacted",
        "unavailable_tool_terminals": ["Bash"],
    });
    let media = serde_json::json!({
        "lane": LANE,
        "author": LANE_CAPABILITY,
        "locus": {"member": 1, "offset": 50, "length": 20, "event": 1},
        "authored": "2030-04-12T18:44:11.271Z",
        "activity": "tool",
        "text": "view_image: capture.png succeeded",
        "media": [{
            "name": "capture.png",
            "content_type": "image/png",
            "data_url": "data:image/png;base64,iVBORw0KGgo=",
        }],
    });
    let command = serde_json::json!({
        "lane": LANE,
        "author": LANE_CAPABILITY,
        "locus": {"member": 1, "offset": 50, "length": 20, "event": 0},
        "authored": "2030-04-12T18:44:10.771Z",
        "activity": "tool",
        "text": "view_image: capture.png",
    });
    let second_provider = serde_json::json!({
        "lane": SECOND_LANE,
        "author": SECRET_LANE_CAPABILITY,
        "locus": {"member": 1, "offset": 15, "length": 20, "event": 0},
        "authored": "2030-04-12T18:44:12.271Z",
        "text": "second provider prose",
    });

    let mut one_shot = CanvasRuntime::new(&accepted);
    one_shot.apply_stream(&admitted(
        &session,
        &snapshot(vec![
            second_provider.clone(),
            boundary.clone(),
            command.clone(),
            older.clone(),
            media.clone(),
        ]),
    ));

    let mut staged = CanvasRuntime::new(&accepted);
    staged.apply_stream(&admitted(
        &session,
        &snapshot_with_more(
            vec![second_provider.clone(), media.clone(), command.clone()],
            &[LANE],
        ),
    ));
    let retained_media = Rc::clone(&staged.messages()[0]);
    let history = serde_json::to_vec(&serde_json::json!({
        "stream": "message-history",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "lane": LANE,
        "before": {"member": 1, "offset": 50, "length": 20, "event": 1},
        "more": false,
        "messages": [boundary, older],
    }))
    .expect("message history serializes");
    staged.apply_stream(&admitted(&session, &history));
    assert!(
        Rc::ptr_eq(&retained_media, &staged.messages()[2]),
        "history backfill does not replace or reposition the retained media allocation"
    );
    assert!(
        !staged
            .apply_stream_frame(&admitted(&session, &live(command)))
            .changed(),
        "replayed tool commands remain activity rather than merging into media cards"
    );
    assert!(
        !staged
            .apply_stream_frame(&admitted(&session, &live(media)))
            .changed(),
        "replayed media does not duplicate or invalidate its card"
    );
    assert!(
        !staged
            .apply_stream_frame(&admitted(&session, &live(second_provider)))
            .changed(),
        "replayed prose does not duplicate or invalidate its card"
    );

    assert_eq!(
        card_bytes(&one_shot),
        card_bytes(&staged),
        "all delivery paths produce byte-equivalent ordered card projections"
    );
    assert_eq!(staged.messages().len(), 4);
    assert!(staged.messages()[1].media.is_empty());
    assert_eq!(staged.messages()[1].unavailable_tool_terminals, ["Bash"]);
    assert_eq!(staged.messages()[2].media.len(), 1);
}
