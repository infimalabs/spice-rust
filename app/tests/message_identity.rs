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
    let snapshot = |first, second| {
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
    let admitted = |bytes: &[u8]| -> app::Admitted<StreamFrame> {
        session
            .admit_stream(bytes)
            .expect("stream frame is admitted")
    };
    runtime.apply_stream(&admitted(&snapshot(first.clone(), second.clone())));
    let retained_first = Rc::clone(&runtime.messages()[0]);
    let retained_second = Rc::clone(&runtime.messages()[1]);

    let mut revised_second = second;
    revised_second["text"] = serde_json::json!("second author revised");
    assert!(
        runtime
            .apply_stream_frame(&admitted(&snapshot(first, revised_second)))
            .changed(),
        "a revised retained message invalidates the surface"
    );
    assert!(Rc::ptr_eq(&retained_first, &runtime.messages()[0]));
    assert!(!Rc::ptr_eq(&retained_second, &runtime.messages()[1]));
}
