use super::*;

const CLAUDE_NARRATION: &str = r#"{"timestamp":"2030-04-12T18:44:09.271Z","type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"thinking","thinking":"The tape lives under the shared\n  root, not the worktree.","signature":"bmFycmF0aW9u"}],"usage":{"input_tokens":1,"output_tokens":1}}}"#;
const CLAUDE_EMPTY_THINKING: &str = r#"{"timestamp":"2030-04-12T18:44:09.272Z","type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"thinking","thinking":"","signature":"dGhpbmtpbmc="}],"usage":{"input_tokens":1,"output_tokens":1}}}"#;
const CLAUDE_REDACTED_THINKING: &str = r#"{"timestamp":"2030-04-12T18:44:09.273Z","type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"redacted_thinking","data":"opaque"}],"usage":{"input_tokens":1,"output_tokens":1}}}"#;
const CODEX_REASONING: &str = r#"{"type":"item.completed","item":{"id":"reasoning-1","type":"reasoning","text":"Checking the gate selection first."}}"#;

fn publish_all(dialect: listen::Dialect, records: &[&str]) -> Vec<wire::AgentMessage> {
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let mut offset = 0;
    for record in records {
        let bytes = record.as_bytes();
        let record = capture_record_for(dialect, bytes, offset);
        offset += bytes.len() as u64;
        publish_agent_messages(&publisher, &record, &mut state, None)
            .expect("reasoning record publishes");
    }
    let messages = publisher.0.lock().expect("message lock");
    messages.clone()
}

#[test]
fn claude_narration_publishes_one_reasoning_presence_flattened_to_one_line() {
    let messages = publish_all(listen::Dialect::Claude, &[CLAUDE_NARRATION]);
    let [presence] = messages.as_slice() else {
        panic!("one thinking block is one presence, got {messages:?}");
    };
    assert_eq!(presence.activity, wire::AgentActivityKind::Reasoning);
    assert_eq!(
        presence.text,
        "The tape lives under the shared root, not the worktree."
    );
    assert!(!presence.final_answer);
    assert_eq!(presence.badges, wire::AgentMessageBadges::default());
    assert_eq!(presence.automatic_speech, None);
    assert!(presence.media.is_empty());
    assert_eq!(
        presence.authored,
        "2030-04-12T18:44:09.271Z"
            .parse::<wire::Instant>()
            .expect("fixture instant")
    );
    assert!(!presence.authored_synthetic);
}

#[test]
fn empty_claude_thinking_is_presence_without_words_and_redacted_thinking_is_nothing() {
    let messages = publish_all(
        listen::Dialect::Claude,
        &[CLAUDE_EMPTY_THINKING, CLAUDE_REDACTED_THINKING],
    );
    let [presence] = messages.as_slice() else {
        panic!("only the signed-empty block is presence, got {messages:?}");
    };
    assert_eq!(presence.activity, wire::AgentActivityKind::Reasoning);
    assert_eq!(presence.text, "");
    assert_eq!(
        presence.locus.offset, 0,
        "the redacted block published nothing"
    );
}

#[test]
fn reasoning_presence_is_elided_like_a_tool_preview() {
    let narration = "word ".repeat(60);
    let record = format!(
        r#"{{"type":"assistant","message":{{"type":"message","role":"assistant","content":[{{"type":"thinking","thinking":"{narration}","signature":"bmFycmF0aW9u"}}]}}}}"#
    );
    let messages = publish_all(listen::Dialect::Claude, &[&record]);
    let [presence] = messages.as_slice() else {
        panic!("one thinking block is one presence, got {messages:?}");
    };
    assert_eq!(presence.text.chars().count(), tool_preview::MAX_CHARS);
    assert!(presence.text.ends_with('…'), "{}", presence.text);
    assert!(presence.text.starts_with("word word "), "{}", presence.text);
}

#[test]
fn codex_reasoning_summary_is_reasoning_presence_not_prose() {
    let messages = publish_all(listen::Dialect::Codex, &[CODEX_REASONING]);
    let [presence] = messages.as_slice() else {
        panic!("one reasoning item is one presence, got {messages:?}");
    };
    assert_eq!(presence.activity, wire::AgentActivityKind::Reasoning);
    assert_eq!(presence.text, "Checking the gate selection first.");
    assert!(!presence.final_answer);
    assert_eq!(presence.automatic_speech, None);
}
