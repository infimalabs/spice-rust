//! oh-my-pi's RPC frames and session records under the current decoder.
//!
//! The lane drives omp over `--mode rpc`, so its content frames are
//! `message_end` and the `get_state` reply rather than a per-turn envelope,
//! and the session file persists the same message shape under a `message`
//! envelope beside its own `session` record.

use listen::{
    Decoded, Dialect, Event, EventKind, Locus, ProseRole, ToolMoment, ToolResult, TurnMoment,
    decode, decode_record,
};

const SESSION: &[u8] = br#"{"id":"spice-state","type":"response","command":"get_state","success":true,"data":{"model":{"id":"claude-opus-5","contextWindow":1000000},"sessionId":"01a0a807-666f-7524-be0d-2ee476b6c15c","sessionFile":"/tmp/sessions/x.jsonl","isStreaming":false}}"#;
const PROSE: &[u8] = br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"api":"anthropic-messages","provider":"anthropic","model":"claude-opus-5","usage":{"input":7,"output":2,"cacheRead":5,"cacheWrite":3,"totalTokens":17},"stopReason":"stop","timestamp":1789525452596}}"#;
const REASONING: &[u8] = br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"thinking","thinking":"inspect"}],"usage":{"input":7,"output":2,"cacheRead":5,"cacheWrite":3},"stopReason":"stop"}}"#;
const TOOL: &[u8] = br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"toolCall","id":"call_1","name":"bash","arguments":{"command":"cargo test"},"partialArgs":"{}"}],"usage":{"input":120,"output":14,"cacheRead":0,"cacheWrite":0},"stopReason":"toolUse"}}"#;
const TOOL_RESULT: &[u8] = br#"{"type":"message_end","message":{"role":"toolResult","toolCallId":"call_1","toolName":"bash","content":[{"type":"text","text":"ok"}],"details":{"exitCode":0},"isError":false,"timestamp":1789525455120}}"#;
const OPERATOR: &[u8] = br#"{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"please run it"}],"attribution":"user","timestamp":1789525452570}}"#;
const COMPACTION: &[u8] = br#"{"type":"auto_compaction_end","action":"context-full","result":{"summary":"so far","firstKeptEntryId":"e1","tokensBefore":900000},"aborted":false,"willRetry":false}"#;
const FAILURE: &[u8] = br#"{"id":"spice-prompt","type":"response","command":"prompt","success":false,"error":"Agent is already processing"}"#;
const TRANSCRIPT_SESSION: &[u8] = br#"{"type":"session","version":3,"id":"01a0a807-666f-7524-be0d-2ee476b6c15c","timestamp":"2026-09-16T02:24:12.399Z","cwd":"/work/tree"}"#;
const TRANSCRIPT_MESSAGE: &[u8] = br#"{"type":"message","id":"93bc66b5","parentId":"1ded9227","timestamp":"2026-09-16T02:24:25.341Z","message":{"role":"assistant","content":[{"type":"text","text":"echo: second thing"}],"usage":{"input":120,"output":9,"cacheRead":0,"cacheWrite":0},"stopReason":"stop"}}"#;

/// Every event kind the decoder produces, each from the record that carries it.
const KINDS: &[(&[u8], EventKind)] = &[
    (SESSION, EventKind::Session),
    (PROSE, EventKind::Prose),
    (REASONING, EventKind::Reasoning),
    (TOOL, EventKind::Tool),
    (TOOL_RESULT, EventKind::Tool),
    (PROSE, EventKind::Usage),
    (COMPACTION, EventKind::Compaction),
    (FAILURE, EventKind::Failure),
];

/// Frames the lane sees on every turn that carry nothing the tape types.
const SILENT: &[&[u8]] = &[
    br#"{"type":"ready","protocolVersion":1,"supportedProtocolVersions":[1,2],"maxFrameBytes":1048576}"#,
    br#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"do"},"message":{"role":"assistant","content":[{"type":"text","text":"do"}]}}"#,
    br#"{"type":"message_start","message":{"role":"assistant","content":[]}}"#,
    br#"{"type":"tool_execution_start","toolCallId":"call_1","toolName":"bash","args":{"command":"cargo test"}}"#,
    br#"{"type":"tool_execution_end","toolCallId":"call_1","toolName":"bash","result":{"content":[]},"isError":false}"#,
    br#"{"type":"turn_end","message":{"role":"assistant","content":[]},"toolResults":[]}"#,
    br#"{"id":"spice-prompt","type":"response","command":"prompt","success":true}"#,
    br#"{"type":"extension_ui_request","id":"1","method":"setWidget","widgetKey":"x"}"#,
    br#"{"type":"message_end","message":{"role":"custom","customType":"async-result","content":"<system-notice>done</system-notice>"}}"#,
];

fn locus(bytes: &[u8]) -> Locus {
    Locus::new(4, 19, bytes.len() as u64)
}

fn decoded(bytes: &[u8]) -> Decoded {
    decode(Dialect::Omp, bytes, locus(bytes)).expect("matching locus")
}

#[test]
fn every_kind_is_produced_once_under_the_record_locus() {
    for (bytes, expected) in KINDS {
        let decoded = decoded(bytes);
        assert!(
            decoded
                .events()
                .iter()
                .any(|event| event.kind() == *expected),
            "omp did not produce {expected:?}: {decoded:?}"
        );
        assert!(
            decoded
                .events()
                .iter()
                .all(|event| event.locus() == locus(bytes))
        );
    }
}

#[test]
fn the_get_state_reply_declares_the_session_and_its_context_window() {
    let session = decoded(SESSION);
    assert!(matches!(
        session.events(),
        [Event::Session(declaration)]
            if declaration.identity == "01a0a807-666f-7524-be0d-2ee476b6c15c"
    ));
    assert_eq!(
        session
            .context()
            .and_then(|context| context.model_context_window),
        Some(1_000_000)
    );
    let refused =
        decode_record(None, PROSE, locus(PROSE)).expect("an omp envelope is uniquely owned");
    assert_eq!(refused.dialect, Dialect::Omp);
}

#[test]
fn assistant_messages_type_prose_reasoning_tool_calls_and_usage() {
    let prose = decoded(PROSE);
    assert!(matches!(
        prose.events(),
        [Event::Prose(prose), Event::Usage(usage)]
            if prose.role == ProseRole::Assistant
                && prose.text == "done"
                && prose.phase.as_deref() == Some("final_answer")
                && usage.input_tokens == 7
                && usage.cached_input_tokens == 5
                && usage.cache_write_input_tokens == 3
                && usage.output_tokens == 2
                && usage.reasoning_output_tokens.is_none()
    ));
    let tool = decoded(TOOL);
    assert!(matches!(
        tool.events(),
        [Event::Tool(call), Event::Usage(_)]
            if call.call_id == "call_1"
                && call.moment == ToolMoment::Call {
                    name: "bash".to_owned(),
                    arguments: r#"{"command":"cargo test"}"#.to_owned(),
                }
    ));
    let result = decoded(TOOL_RESULT);
    assert!(matches!(
        result.events(),
        [Event::Tool(tool)]
            if tool.call_id == "call_1"
                && matches!(
                    &tool.moment,
                    ToolMoment::Result(outcome)
                        if outcome.result == ToolResult::Succeeded
                            && outcome.output.as_deref() == Some("ok")
                            && outcome.exit_code == Some(0)
                )
    ));
    let operator = decoded(OPERATOR);
    assert!(matches!(
        operator.events(),
        [Event::Prose(prose)]
            if prose.role == ProseRole::Operator
                && prose.text == "please run it"
                && prose.phase.as_deref() == Some("prompt")
    ));
}

#[test]
fn compaction_and_refusals_are_typed_while_lifecycle_frames_stay_silent() {
    let compaction = decoded(COMPACTION);
    assert!(matches!(
        compaction.events(),
        [Event::Compaction(compaction)] if compaction.summary.as_deref() == Some("so far")
    ));
    let failure = decoded(FAILURE);
    assert!(matches!(
        failure.events(),
        [Event::Failure(failure)]
            if failure.message == "prompt refused: Agent is already processing"
    ));
    for silent in SILENT {
        let decoded = decoded(silent);
        assert!(
            matches!(decoded, Decoded::Events { ref events, .. } if events.is_empty()),
            "a silent omp frame produced something: {decoded:?}"
        );
    }
    let started = br#"{"type":"agent_start"}"#;
    let ended = br#"{"type":"agent_end","messages":[]}"#;
    assert_eq!(
        decoded(started).context().and_then(|context| context.turn),
        Some(TurnMoment::Started)
    );
    assert_eq!(
        decoded(ended).context().and_then(|context| context.turn),
        Some(TurnMoment::Completed)
    );
}

#[test]
fn the_session_file_reads_under_the_same_dialect() {
    let session = decoded(TRANSCRIPT_SESSION);
    assert!(matches!(
        session.events(),
        [Event::Session(declaration)]
            if declaration.identity == "01a0a807-666f-7524-be0d-2ee476b6c15c"
    ));
    let message = decoded(TRANSCRIPT_MESSAGE);
    assert!(matches!(
        message.events(),
        [Event::Prose(prose), Event::Usage(_)] if prose.text == "echo: second thing"
    ));
    assert_eq!(
        message
            .context()
            .and_then(|context| context.timestamp.clone()),
        Some("2026-09-16T02:24:25.341Z".to_owned())
    );
}
