//! Visible assistant text is a packet's only source; tool-call intent is call
//! metadata that never publishes, even when it repeats a packet header.

use steering::{Published, Unpublished};

use super::conversation::asked;
use super::*;

/// Where the visible block sits in its assistant message.
#[derive(Clone, Copy, Debug)]
enum Placement {
    /// A tool call follows, whose intent repeats the packet header while the
    /// provider settles the visible block as paraphrased reasoning.
    BeforeCall,
    /// The turn's final message, settled with its text and no tool call.
    Final,
}

impl Lanes {
    /// Land one visible text block and its settled message on `lane`, account
    /// both, and hear each record as the lane's seated occupant wrote it.
    fn speak(
        &self,
        lane: u8,
        text: &str,
        placement: Placement,
    ) -> Vec<Result<Published, Unpublished>> {
        let tape = self.tape(lane);
        let streamed = serde_json::json!({
            "type": "message_update",
            "message": {"role": "assistant"},
            "assistantMessageEvent": {"type": "text_end", "contentIndex": 1, "content": text},
        });
        tape.tee(format!("{streamed}\n").as_bytes())
            .expect("the streamed block is durable");
        let (content, stop) = match placement {
            Placement::BeforeCall => {
                let header = text
                    .lines()
                    .find_map(|line| {
                        ["MSG ", "ASK ", "ANS "].iter().find_map(|marker| {
                            line.starts_with(marker)
                                .then(|| &line[..=line.find(':').expect("header colon")])
                        })
                    })
                    .expect("the block holds a packet header");
                (
                    serde_json::json!([
                        {"type": "thinking", "thinking": "paraphrased commentary"},
                        {
                            "type": "toolCall",
                            "id": "call-1",
                            "name": "read",
                            "arguments": {"path": "status.txt"},
                            "intent": format!("{header} reading the parser"),
                        },
                    ]),
                    "toolUse",
                )
            }
            Placement::Final => (serde_json::json!([{"type": "text", "text": text}]), "stop"),
        };
        let settled = serde_json::json!({
            "type": "message_end",
            "message": {"role": "assistant", "content": content, "stopReason": stop},
        });
        tape.tee(format!("{settled}\n").as_bytes())
            .expect("the settled message is durable");
        self.account(lane);
        let (source, records) = self.records(lane);
        let mut outcomes = Vec::new();
        for record in &records[records.len() - 2..] {
            let listen::RecordPayload::Events(events) = &record.payload else {
                panic!("the record decodes");
            };
            let events = events
                .iter()
                .map(|sourced| sourced.event.clone())
                .collect::<Vec<_>>();
            outcomes.extend(steering::publish_authored(
                &self.bus,
                &self.paths,
                self.epoch,
                &steer::Config::default(),
                &steering::AuthoredRecord {
                    lane: test_uuid(lane),
                    author: test_uuid(lane + 1),
                    source,
                    locus: record.locus,
                    events: &events,
                },
                steering::Refusals::Record,
                &mut Vec::new(),
            ));
        }
        outcomes
    }

    /// The exact body a conversation record's publication reads back.
    fn conversation_body(&self, publication: wire::rooms::RoomAuthorityStamp) -> String {
        let outcome = crate::room_command::conversation_text(
            &self.bus,
            &self.paths.tape,
            &wire::intent::RoomConversationText { publication },
        );
        let wire::Outcome::Reply(reply) = outcome else {
            panic!("the conversation text read answers: {outcome:?}");
        };
        let wire::intent::CommandAnswer::RoomConversationText(answer) = reply.body else {
            panic!("another answer shape: {:?}", reply.body);
        };
        match answer.content {
            wire::intent::RoomConversationContent::Text { text } => text,
            other => panic!("the body is unavailable: {other:?}"),
        }
    }
}

/// MSG, ASK, and ANS publish once from their visible block, whether a tool
/// call follows it or it ends the turn. Each body runs to the next placed
/// header or the block's end across lines, and an intent repeating the header
/// neither publishes again nor replaces the body.
#[test]
fn each_packet_kind_publishes_its_visible_body_once_wherever_its_message_ends() {
    for placement in [Placement::BeforeCall, Placement::Final] {
        let lanes = Lanes::new(match placement {
            Placement::BeforeCall => "source-before-call",
            Placement::Final => "source-final",
        });
        let room = lanes.room("dVrD244Jd");
        let (sender, first) = (lanes.address(SENDER), lanes.address(FIRST));

        let asking = lanes.speak(
            SENDER,
            &format!("Status first.\nASK {first}#{room}: which parser\nowns terminal text?"),
            placement,
        );
        let [asking] = <[_; 1]>::try_from(asking)
            .unwrap_or_else(|outcomes| panic!("{placement:?}: one ASK publishes: {outcomes:?}"));
        let result = asked(&asking);

        let answering = lanes.speak(
            FIRST,
            &format!("ANS {result}#{room}: the visible one,\nalways."),
            placement,
        );
        let [answering] = <[_; 1]>::try_from(answering)
            .unwrap_or_else(|outcomes| panic!("{placement:?}: one ANS publishes: {outcomes:?}"));
        assert!(answering.is_ok(), "{placement:?}: {answering:?}");

        let messaging = lanes.speak(
            FIRST,
            &format!("MSG {sender}: rebase first\nthen rerun.\nMSG {sender}: second packet"),
            placement,
        );
        assert_eq!(
            messaging,
            [Ok(sent(&[SENDER], &[])), Ok(sent(&[SENDER], &[]))],
            "{placement:?}: each packet in the block publishes once"
        );

        let bodies = lanes
            .conversations()
            .into_iter()
            .map(|conversed| lanes.conversation_body(conversed.publication))
            .collect::<Vec<_>>();
        assert_eq!(
            bodies,
            [
                "which parser\nowns terminal text?",
                "the visible one,\nalways."
            ],
            "{placement:?}: the recorded question and answer are their visible bodies"
        );
        let sent_bodies = lanes
            .messages()
            .into_iter()
            .filter(|(_, message)| message.from == test_uuid(FIRST))
            .map(|(_, message)| message.text)
            .collect::<Vec<_>>();
        assert_eq!(
            sent_bodies,
            [
                "the visible one,\nalways.",
                "rebase first\nthen rerun.",
                "second packet"
            ],
            "{placement:?}: the answer returns its visible body, and each MSG body ends at the \
             next header or the block's end"
        );
        lanes.finish();
    }
}

/// A packet header that appears only in tool-call intent publishes nothing.
#[test]
fn a_tool_intent_alone_originates_no_packet() {
    let lanes = Lanes::new("source-intent-only");
    let first = lanes.address(FIRST);
    lanes.intend(
        SENDER,
        &[],
        &[
            &format!("MSG {first}: from call metadata"),
            &format!("ASK {first}: from call metadata?"),
        ],
    );

    let (outcomes, diagnostics) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    assert!(outcomes.is_empty(), "{outcomes:?} {diagnostics}");
    assert!(lanes.messages().is_empty());
    assert!(lanes.conversations().is_empty());
    lanes.finish();
}

/// Tool-call intent stays answer evidence but is never packet authorship. A
/// successor's intent retires the MSG its lane inherited, while the MSG header
/// beside it in another intent publishes nothing.
#[test]
fn a_tool_intent_retires_an_inherited_msg_but_publishes_no_packet() {
    let lanes = Lanes::new("msg-tool-intent");
    let first = lanes.address(FIRST);
    let sender = lanes.address(SENDER);
    lanes.author(SENDER, &format!("MSG {first}: hold the merge"));
    lanes.hear(SENDER, test_uuid(SENDER + 1));
    let message = delivered(&lanes, FIRST);
    lanes.vacate(FIRST);
    let successor = test_uuid(41);
    lanes.renew(FIRST, successor);
    lanes.declare(FIRST);

    lanes.intend(
        FIRST,
        &[&message.key],
        &[
            &format!("MSG {sender}: intent packet"),
            &format!("ACK {}: Holding.", message.key),
        ],
    );
    let (published, diagnostics) = lanes.hear(FIRST, successor);

    assert!(
        published.is_empty(),
        "an intent publishes no packet: {published:?} {diagnostics}"
    );
    assert_eq!(lanes.messages().len(), 1, "only the visible MSG is durable");
    assert!(
        lanes.pending(FIRST).is_empty(),
        "the intent's ACK retires the inherited key"
    );
    let [reply] = lanes
        .pending(SENDER)
        .try_into()
        .unwrap_or_else(|pending| panic!("only the reply reaches the sender lane: {pending:?}"));
    assert!(
        reply.control.is_some(),
        "the inherited MSG receives its reply"
    );
    assert_eq!(reply.sender.map(|sender| sender.occupant), Some(successor));
    lanes.finish();
}
