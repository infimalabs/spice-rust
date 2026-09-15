use std::sync::Mutex;

use super::super::super::*;

const CLAUDE_TURN_START: &str = r#"{"type":"user","promptId":"turn-1","message":{"type":"message","role":"user","content":[{"type":"text","text":"continue"}]}}"#;
const CLAUDE_STRING_TURN_START: &str = r#"{"type":"user","promptId":"turn-1","message":{"type":"message","role":"user","content":"continue"}}"#;
const CLAUDE_IMAGE_TURN_START: &str = r#"{"type":"user","promptId":"turn-1","message":{"type":"message","role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgo="}}]}}"#;
const CLAUDE_TOOL_RESULT: &str = r#"{"type":"user","promptId":"turn-1","message":{"type":"message","role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":"done"}]}}"#;
const CLAUDE_PROSE: &str = r#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"new work"}]}}"#;

#[derive(Default)]
struct Messages(Mutex<Vec<wire::AgentMessage>>);

impl AgentMessagePublisher for Messages {
    fn publish(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        self.0.lock().expect("message lock").push(message);
        Ok(true)
    }

    fn revise_final(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        let mut messages = self.0.lock().expect("message lock");
        let Some(existing) = messages
            .iter_mut()
            .find(|existing| existing.lane == message.lane && existing.locus == message.locus)
        else {
            return Err(canvas::stream::MessagePublishError::IdentityConflict);
        };
        *existing = message;
        Ok(true)
    }
}

fn lane() -> CanonicalUuid {
    "00000000-0000-4000-8000-000000000001"
        .parse()
        .expect("fixture lane")
}

fn event() -> PresentationEvent {
    PresentationEvent {
        lane: lane(),
        source: None,
        locus: listen::Locus::new(1, 0, 1),
        ordinal: 0,
    }
}

fn capture_record(bytes: &[u8], offset: u64) -> lane::capture::CaptureRecord {
    capture_record_for(listen::Dialect::Codex, bytes, offset)
}

fn capture_record_for(
    dialect: listen::Dialect,
    bytes: &[u8],
    offset: u64,
) -> lane::capture::CaptureRecord {
    let locus = listen::Locus::new(1, offset, bytes.len() as u64);
    lane::capture::CaptureRecord {
        lane: lane(),
        occupant: lane(),
        stream: vendor::RecordStream::Stdout,
        boundary: sill::RecordBoundary::Complete,
        locus,
        declaration: None,
        decoded: listen::decode(dialect, bytes, locus).expect("fixture transcript record decodes"),
    }
}

#[test]
fn vendor_turn_starts_publish_a_boundary_before_new_prose() {
    assert_turn_sequence(
        listen::Dialect::Codex,
        &[
            br#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"turn-1"}}"#
                .as_slice(),
            br#"{"type":"item.completed","item":{"id":"commentary","type":"agent_message","text":"new work"}}"#
                .as_slice(),
        ],
    );
    for start in [
        CLAUDE_STRING_TURN_START,
        CLAUDE_TURN_START,
        CLAUDE_IMAGE_TURN_START,
    ] {
        assert_turn_sequence(
            listen::Dialect::Claude,
            &[
                start.as_bytes(),
                CLAUDE_TOOL_RESULT.as_bytes(),
                CLAUDE_PROSE.as_bytes(),
            ],
        );
    }
}

fn assert_turn_sequence(dialect: listen::Dialect, records: &[&[u8]]) {
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let mut offset = 0;
    for &bytes in records {
        let record = capture_record_for(dialect, bytes, offset);
        offset += bytes.len() as u64;
        publish_agent_messages(&publisher, &record, &mut state, None)
            .expect("turn sequence publishes");
    }

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages[0].activity, wire::AgentActivityKind::Turn);
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.activity == wire::AgentActivityKind::Turn)
            .count(),
        1,
    );
    let prose = messages.last().expect("assistant prose follows the turn");
    assert_eq!(prose.activity, wire::AgentActivityKind::Prose);
    assert_eq!(prose.text, "new work");
    assert!(messages[0].chronology_key() < prose.chronology_key());
}

#[test]
fn completed_codex_tool_publishes_its_operation_before_the_outcome() {
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let bytes = br#"{"type":"item.completed","item":{"id":"item-command","type":"command_execution","command":"cargo test -p spice","aggregated_output":"ok","exit_code":0,"status":"completed"}}"#;
    let record = capture_record(bytes, 0);

    publish_agent_messages(&publisher, &record, &mut state, None)
        .expect("completed tool record publishes");

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(
        messages.len(),
        2,
        "one record carries its operation and outcome"
    );
    assert_eq!(messages[0].text, "cargo test -p spice");
    assert_eq!(messages[1].text, "cargo test -p spice succeeded");
    assert!(
        messages
            .iter()
            .all(|message| message.activity == wire::AgentActivityKind::Tool),
    );
}

mod acknowledgement;
mod file_change;
mod reasoning;

#[test]
fn successful_codex_cli_turn_promotes_only_its_latest_agent_message() {
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let records = [
        br#"{"type":"item.completed","item":{"id":"commentary","type":"agent_message","text":"working through it"}}"#.as_slice(),
        br#"{"type":"item.completed","item":{"id":"final","type":"agent_message","text":"settled final"}}"#.as_slice(),
        br#"{"type":"turn.completed","usage":{"input_tokens":1,"cached_input_tokens":0,"output_tokens":1}}"#.as_slice(),
    ];
    let mut offset = 0;
    for bytes in records {
        let record = capture_record(bytes, offset);
        offset += bytes.len() as u64;
        publish_agent_messages(&publisher, &record, &mut state, None)
            .expect("CLI transcript record publishes");
    }

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].text, "working through it");
    assert!(!messages[0].final_answer);
    assert_eq!(messages[1].text, "settled final");
    assert!(messages[1].final_answer);
    assert_eq!(
        messages[1]
            .automatic_speech
            .as_ref()
            .map(|speech| speech.kind),
        Some(wire::AutomaticSpeechKind::Final),
    );
}

#[test]
fn failed_codex_cli_turn_leaves_its_agent_message_nonfinal() {
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let records = [
        br#"{"type":"item.completed","item":{"id":"failed","type":"agent_message","text":"not a final answer"}}"#.as_slice(),
        br#"{"type":"turn.failed","error":{"message":"model failed"}}"#.as_slice(),
    ];
    let mut offset = 0;
    for bytes in records {
        let record = capture_record(bytes, offset);
        offset += bytes.len() as u64;
        publish_agent_messages(&publisher, &record, &mut state, None)
            .expect("failed CLI transcript record publishes");
    }

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1);
    assert!(!messages[0].final_answer);
    assert!(messages[0].automatic_speech.is_none());
}

#[test]
fn persisted_codex_final_and_task_complete_companion_share_one_card() {
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let records = [
        br#"{"timestamp":"2030-04-12T18:44:09.270Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","id":"message-1","content":[{"type":"Text","text":"settled final"}],"phase":"final_answer"}}}"#.as_slice(),
        br#"{"timestamp":"2030-04-12T18:44:09.271Z","type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"settled final"}]}}"#.as_slice(),
        br#"{"timestamp":"2030-04-12T18:44:09.272Z","type":"event_msg","payload":{"type":"task_complete","last_agent_message":"settled final"}}"#.as_slice(),
    ];
    let mut offset = 0;
    for bytes in records {
        let record = capture_record(bytes, offset);
        offset += bytes.len() as u64;
        publish_agent_messages(&publisher, &record, &mut state, None)
            .expect("persisted transcript record publishes");
    }

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "settled final");
    assert!(messages[0].final_answer);
}

#[test]
fn claude_promotes_terminal_stop_reasons_but_not_tool_use() {
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let records = [
        br#"{"timestamp":"2030-04-12T18:44:09.271Z","type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"tool-bound prose"}],"usage":{"input_tokens":1,"output_tokens":1}}}"#.as_slice(),
        br#"{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":1,"output_tokens":1}}}"#.as_slice(),
        br#"{"timestamp":"2030-04-12T18:44:09.272Z","type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"terminal prose"}],"usage":{"input_tokens":1,"output_tokens":1}}}"#.as_slice(),
        br#"{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":1,"output_tokens":1}}}"#.as_slice(),
    ];
    let mut offset = 0;
    for bytes in records {
        let record = capture_record_for(listen::Dialect::Claude, bytes, offset);
        offset += bytes.len() as u64;
        publish_agent_messages(&publisher, &record, &mut state, None)
            .expect("Claude transcript record publishes");
    }

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].text, "tool-bound prose");
    assert!(!messages[0].final_answer);
    assert_eq!(messages[1].text, "terminal prose");
    assert!(messages[1].final_answer);
}

#[test]
fn terminal_assistant_prose_preserves_final_answer_state() {
    let publisher = Messages::default();
    let prose = listen::Event::Prose(listen::Prose {
        role: listen::ProseRole::Assistant,
        text: "settled final".to_owned(),
        phase: Some("final_answer".to_owned()),
        media: Vec::new(),
        locus: listen::Locus::new(1, 0, 1),
    });

    publish_agent_event(
        &publisher,
        AgentEvent {
            lane: lane(),
            author: lane(),
            source: None,
            worktree: None,
            ordinal: 0,
            decoded: &prose,
            authored: None,
            observed: None,
            observations: &MessageObservations::default(),
            unavailable_tool_terminals: &[],
        },
        &mut CallPreviews::default(),
        None,
    )
    .expect("final prose publishes");

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "settled final");
    assert!(messages[0].final_answer);
}
