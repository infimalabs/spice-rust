//! Packets a provider leaves only in the first tool call's intent.

use super::*;

impl Lanes {
    /// Land one settled assistant message whose only verbatim words are the
    /// packet explicitly repeated in its tool-call intent.
    fn author_intent(&self, lane: u8, text: &str) -> listen::Locus {
        let bytes = format!(
            "{}\n",
            serde_json::json!({
                "type": "message_end",
                "message": {
                    "role": "assistant",
                    "content": [{
                        "type": "toolCall",
                        "id": "call-1",
                        "name": "read",
                        "arguments": {"path": "status.txt"},
                        "intent": text,
                    }],
                    "stopReason": "toolUse",
                },
            })
        );
        self.tape(lane)
            .tee(bytes.as_bytes())
            .expect("the intent packet is durable")
    }
}

/// When no visible text survives settlement, the packet deliberately repeated
/// in the first tool intent is the complete authored source. Its event locus
/// makes live and recovery rereads idempotent.
#[test]
fn a_no_prose_tool_intent_publishes_its_packet_exactly_once() {
    let lanes = Lanes::new("msg-intent");
    let first = lanes.address(FIRST);
    let packet = format!("MSG {first}: inspect the retained packet");
    let authored = lanes.author_intent(SENDER, &packet);

    let (outcomes, diagnostics) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    assert_eq!(outcomes, [Ok(sent(&[FIRST], &[]))], "{diagnostics}");
    let messages = lanes.messages();
    let [(_, message)] = messages.as_slice() else {
        panic!("the intent packet publishes exactly one message: {messages:?}")
    };
    assert_eq!(message.text, "inspect the retained packet");
    assert_eq!(
        (
            message.locus.member,
            message.locus.offset,
            message.locus.event,
            message.locus.line,
        ),
        (authored.member, authored.offset, 0, 1),
    );

    let (reread, diagnostics) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    assert_eq!(reread, [Ok(steering::Published::Already)], "{diagnostics}");
    assert_eq!(lanes.messages().len(), 1, "recovery sends nothing twice");
    lanes.finish();
}

/// The skill repeats a packet in the first tool intent beside its visible
/// prose. When the visible block survives settlement, the intent is only a
/// caption, so the packet publishes once from its streamed text.
#[test]
fn a_packet_in_surviving_prose_and_its_tool_intent_publishes_once() {
    let lanes = Lanes::new("msg-intent-twin");
    let first = lanes.address(FIRST);
    let packet = format!("MSG {first}: inspect the retained packet");
    let tape = lanes.tape(SENDER);
    let streamed = serde_json::json!({
        "type": "message_update",
        "message": {"role": "assistant"},
        "assistantMessageEvent": {"type": "text_end", "contentIndex": 1, "content": packet},
    });
    let authored = tape
        .tee(format!("{streamed}\n").as_bytes())
        .expect("the streamed block is durable");
    let settled = serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [
                {"type": "text", "text": packet},
                {
                    "type": "toolCall",
                    "id": "call-1",
                    "name": "read",
                    "arguments": {"path": "status.txt"},
                    "intent": packet,
                },
            ],
            "stopReason": "toolUse",
        },
    });
    tape.tee(format!("{settled}\n").as_bytes())
        .expect("the settled message is durable");

    let (outcomes, diagnostics) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    assert_eq!(outcomes, [Ok(sent(&[FIRST], &[]))], "{diagnostics}");
    let messages = lanes.messages();
    let [(_, message)] = messages.as_slice() else {
        panic!("the twin packet publishes exactly one message: {messages:?}")
    };
    assert_eq!(
        (message.locus.member, message.locus.offset),
        (authored.member, authored.offset),
        "the streamed block is the source"
    );
    lanes.finish();
}
