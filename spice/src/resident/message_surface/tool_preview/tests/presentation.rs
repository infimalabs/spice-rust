use std::sync::Mutex;

use super::super::super::*;
use crate::resident::message_surface::projection::TEST_TAPE_SOURCE;

const OMP_TURN_START: &str = r#"{"type":"agent_start"}"#;
const OMP_PROSE: &str = r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"new work"}],"stopReason":"toolUse"}}"#;

/// Published cards by identity, and how many final revisions reached them.
#[derive(Default)]
struct Messages(Mutex<Vec<wire::AgentMessage>>, Mutex<usize>);

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
        *self.1.lock().expect("revision lock") += 1;
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
    let locus = listen::Locus::new(1, offset, bytes.len() as u64);
    lane::capture::CaptureRecord {
        lane: lane(),
        occupant: lane(),
        stream: vendor::RecordStream::Stdout,
        boundary: sill::RecordBoundary::Complete,
        locus,
        declaration: None,
        decoded: listen::decode(listen::Dialect::Omp, bytes, locus)
            .expect("fixture transcript record decodes"),
    }
}

#[test]
fn vendor_turn_starts_publish_a_boundary_before_new_prose() {
    assert_turn_sequence(&[OMP_TURN_START.as_bytes(), OMP_PROSE.as_bytes()]);
}

fn assert_turn_sequence(records: &[&[u8]]) {
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let mut offset = 0;
    for &bytes in records {
        let record = capture_record(bytes, offset);
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
    assert_eq!(
        prose.quote_target,
        Some(wire::QuoteTarget::Tape {
            lane: prose.lane,
            source: TEST_TAPE_SOURCE.parse().expect("fixture source"),
            locus: prose.locus,
            digest: wire::QuoteTarget::text_digest("new work"),
        })
    );
    assert!(messages[0].quote_target.is_none());
    assert!(messages[0].chronology_key() < prose.chronology_key());
}

mod acknowledgement;
mod reasoning;

#[test]
fn a_settled_turn_promotes_only_its_latest_assistant_message() {
    let messages = publish_records(&[
        br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"working through it"}],"stopReason":"toolUse"}}"#,
        br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"settled final"}],"stopReason":"stop"}}"#,
        br#"{"type":"prompt_result","id":"prompt-7","agentInvoked":true,"status":"completed","sessionSettled":true}"#,
    ]);
    let messages = messages.0.lock().expect("message lock");
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
fn a_failed_turn_leaves_its_assistant_message_nonfinal() {
    let messages = publish_records(&[
        br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"not a final answer"}],"stopReason":"error"}}"#,
        br#"{"type":"prompt_result","id":"prompt-7","agentInvoked":true,"status":"error","sessionSettled":true,"error":{"message":"model failed","provider":"openai","model":"gpt-5.4","httpStatus":429,"retryable":true}}"#,
    ]);
    let messages = messages.0.lock().expect("message lock");
    let prose = messages
        .iter()
        .find(|message| message.activity == wire::AgentActivityKind::Prose)
        .expect("the failed turn's prose publishes");
    assert!(!prose.final_answer);
    assert!(prose.automatic_speech.is_none());
    let failure = messages
        .iter()
        .find(|message| message.activity == wire::AgentActivityKind::Failure)
        .expect("the structured provider failure publishes");
    assert_eq!(failure.text, "model failed");
    assert_eq!(
        failure.provider_error,
        Some(wire::AgentProviderError {
            provider: Some("openai".to_owned()),
            model: Some("gpt-5.4".to_owned()),
            http_status: Some(429),
            retryable: true,
        })
    );
}

/// Publish `records` in tape order and return every card they produced.
fn publish_records(records: &[&[u8]]) -> Messages {
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let mut offset = 0;
    for &bytes in records {
        let record = capture_record(bytes, offset);
        offset += bytes.len() as u64;
        publish_agent_messages(&publisher, &record, &mut state, None)
            .expect("transcript record publishes");
    }
    publisher
}

#[test]
fn omp_terminal_text_evidence_does_not_publish_a_duplicate_card() {
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let records = [
        br#"{"type":"message_update","assistantMessageEvent":{"type":"text_end","content":"ACK 1kKRRsDS: revision accepted."},"message":{"role":"assistant","content":[{"type":"text","text":"ACK 1kKRRsDS: revision accepted."}]}}"#
            .as_slice(),
        br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"settled answer"}],"stopReason":"toolUse"}}"#
            .as_slice(),
    ];
    let mut offset = 0;
    for bytes in records {
        let record = capture_record(bytes, offset);
        offset += bytes.len() as u64;
        publish_agent_messages(&publisher, &record, &mut state, None)
            .expect("OMP transcript record publishes");
    }

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "settled answer");
}

#[test]
fn prompt_result_raises_the_previous_prose_once() {
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let prose = br#"{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"streamed answer"}],"stopReason":"stop"}}"#;
    let record = capture_record(prose, 0);
    publish_agent_messages(&publisher, &record, &mut state, None).expect("the prose publishes");
    {
        let messages = publisher.0.lock().expect("message lock");
        let prose = messages.last().expect("the prose publishes");
        assert!(!prose.final_answer);
        assert_eq!(*publisher.1.lock().expect("revision lock"), 0);
    }

    let completed = br#"{"type":"prompt_result","id":"prompt-7","agentInvoked":true,"status":"completed","sessionSettled":true}"#;
    let record = capture_record(completed, prose.len() as u64);
    publish_agent_messages(&publisher, &record, &mut state, None)
        .expect("the prompt result publishes");

    let messages = publisher.0.lock().expect("message lock");
    let prose = messages.last().expect("the prose remains");
    assert_eq!(prose.text, "streamed answer");
    assert!(prose.final_answer);
    assert_eq!(*publisher.1.lock().expect("revision lock"), 1);
}
