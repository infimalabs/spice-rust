//! oh-my-pi's RPC frames and session records under the current decoder.
//!
//! The lane drives omp over `--mode rpc`, so its content frames are
//! `message_end` and the `get_state` reply rather than a per-turn envelope,
//! and the session file persists the same message shape under a `message`
//! envelope beside its own `session` record.

use listen::{
    Decoded, Dialect, Dialog, Event, EventKind, Locus, PromptStatus, ProseRole, RecordDecodeError,
    ToolMoment, ToolResult, TurnMoment, decode, decode_record, restated,
};

const SESSION: &[u8] = br#"{"id":"spice-state","type":"response","command":"get_state","success":true,"data":{"model":{"id":"claude-opus-5","contextWindow":1000000},"sessionId":"01a0a807-666f-7524-be0d-2ee476b6c15c","sessionFile":"/tmp/sessions/x.jsonl","isStreaming":false}}"#;
const PROSE: &[u8] = br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"api":"anthropic-messages","provider":"anthropic","model":"claude-opus-5","usage":{"input":7,"output":2,"cacheRead":5,"cacheWrite":3,"totalTokens":17},"stopReason":"stop","timestamp":1789525452596}}"#;
const REASONING: &[u8] = br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"thinking","thinking":"inspect"}],"usage":{"input":7,"output":2,"cacheRead":5,"cacheWrite":3},"stopReason":"stop"}}"#;
const TOOL: &[u8] = br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"toolCall","id":"call_1","name":"bash","arguments":{"command":"cargo test"},"partialArgs":"{}"}],"usage":{"input":120,"output":14,"cacheRead":0,"cacheWrite":0},"stopReason":"toolUse"}}"#;
const TOOL_RESULT: &[u8] = br#"{"type":"message_end","message":{"role":"toolResult","toolCallId":"call_1","toolName":"bash","content":[{"type":"text","text":"ok"}],"details":{"exitCode":0},"isError":false,"timestamp":1789525455120}}"#;
const OPERATOR: &[u8] = br#"{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"please run it"}],"attribution":"user","timestamp":1789525452570}}"#;
const PARTIAL: &[u8] = br#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"do","partial":{"role":"assistant","content":[{"type":"text","text":"do"}]}},"message":{"role":"assistant","content":[{"type":"text","text":"do"}]}}"#;
const TOOL_PARTIAL: &[u8] = br#"{"type":"tool_execution_update","toolCallId":"call_1","toolName":"bash","partialResult":{"content":[{"type":"text","text":"o"}]}}"#;
const TOOL_STREAM: &[u8] = br#"{"type":"tool_stream_update","toolCallId":"call_1","toolName":"edit","update":{"path":"src/lib.rs","diff":"+done"}}"#;
const TOOL_END: &[u8] = br#"{"type":"tool_execution_end","toolCallId":"call_1","toolName":"bash","result":{"content":[{"type":"text","text":"ok"}]},"isError":false}"#;
const COMPACTION: &[u8] = br#"{"type":"auto_compaction_end","action":"context-full","result":{"summary":"so far","firstKeptEntryId":"e1","tokensBefore":900000},"aborted":false,"willRetry":false}"#;
const FAILURE: &[u8] = br#"{"id":"spice-prompt","type":"response","command":"prompt","success":false,"error":"Agent is already processing"}"#;
const TRANSCRIPT_SESSION: &[u8] = br#"{"type":"session","version":3,"id":"01a0a807-666f-7524-be0d-2ee476b6c15c","timestamp":"2026-09-16T02:24:12.399Z","cwd":"/work/tree"}"#;
const TRANSCRIPT_MESSAGE: &[u8] = br#"{"type":"message","id":"93bc66b5","parentId":"1ded9227","timestamp":"2026-09-16T02:24:25.341Z","message":{"role":"assistant","content":[{"type":"text","text":"echo: second thing"}],"usage":{"input":120,"output":9,"cacheRead":0,"cacheWrite":0},"stopReason":"stop"}}"#;
const PROMPT_COMPLETED: &[u8] = br#"{"type":"prompt_result","id":"prompt-7","agentInvoked":true,"status":"completed","sessionSettled":false}"#;
const PROMPT_FAILED: &[u8] = br#"{"type":"prompt_result","id":"prompt-8","agentInvoked":true,"status":"error","sessionSettled":true,"error":{"message":"capacity exhausted","provider":"anthropic","model":"claude-opus-5-5","retryable":true,"httpStatus":529}}"#;

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
    // Current run-state and retry telemetry is either represented by the
    // settled message/prompt result or carries no authored transcript moment.
    br#"{"type":"session_settled"}"#,
    br#"{"type":"auto_retry_start","attempt":1,"maxAttempts":3,"delayMs":200,"errorMessage":"busy"}"#,
    br#"{"type":"auto_retry_end","success":true,"attempt":1}"#,
    br#"{"type":"retry_fallback_applied","from":"opus","to":"sonnet","role":"default"}"#,
    br#"{"type":"retry_fallback_succeeded","model":"sonnet","role":"default"}"#,
    br#"{"type":"model_changed"}"#,
    br#"{"type":"thinking_level_changed","thinkingLevel":"high"}"#,
    br#"{"type":"config_warnings_changed"}"#,
    br#"{"type":"advisor_yielded"}"#,
    br#"{"type":"ttsr_triggered","rules":[]}"#,
    br#"{"type":"todo_reminder","todos":[],"attempt":1,"maxAttempts":3}"#,
    br#"{"type":"todo_auto_clear"}"#,
    br#"{"type":"goal_updated","goal":null}"#,
    br#"{"type":"irc_message","message":{"role":"custom","content":"status"}}"#,
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
                    intent: None,
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
    let unsupported = br#"{"id":"spice-protocol","type":"response","command":"negotiate_protocol","success":false,"error":"Unsupported RPC protocol version: 2"}"#;
    let refused = decoded(unsupported);
    assert!(
        matches!(
            refused.events(),
            [Event::Failure(failure)]
                if failure.message
                    == format!(
                        "negotiate_protocol refused: Unsupported RPC protocol version: 2; \
                         this lane requires RPC protocol version {}",
                        listen::OMP_PROTOCOL_VERSION
                    )
        ),
        "a refused wire must name the version the lane requires: {:?}",
        refused.events()
    );
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
fn prompt_results_preserve_identity_lifecycle_and_structured_provider_failure() {
    let completed = decoded(PROMPT_COMPLETED);
    assert!(completed.events().is_empty());
    let context = completed.context().expect("prompt result has context");
    assert_eq!(context.turn_id.as_deref(), Some("prompt-7"));
    assert!(matches!(
        context.prompt_outcome,
        Some(ref outcome)
            if outcome.agent_invoked
                && outcome.status == PromptStatus::Completed
                && !outcome.session_settled
    ));

    let failed = decoded(PROMPT_FAILED);
    assert!(matches!(
        failed.events(),
        [Event::Failure(failure)]
            if failure.message == "capacity exhausted"
                && matches!(
                    failure.provider,
                    Some(ref provider)
                        if provider.provider.as_deref() == Some("anthropic")
                            && provider.model.as_deref() == Some("claude-opus-5-5")
                            && provider.http_status == Some(529)
                            && provider.retryable
                )
    ));
    let context = failed.context().expect("failed prompt result has context");
    assert_eq!(context.turn_id.as_deref(), Some("prompt-8"));
    assert!(matches!(
        context.prompt_outcome,
        Some(ref outcome)
            if outcome.agent_invoked
                && outcome.status == PromptStatus::Error
                && outcome.session_settled
    ));

    let aborted = decoded(
        br#"{"type":"prompt_result","id":"prompt-9","agentInvoked":false,"status":"aborted","sessionSettled":true}"#,
    );
    assert!(aborted.events().is_empty());
    assert!(matches!(
        aborted
            .context()
            .and_then(|context| context.prompt_outcome.as_ref()),
        Some(outcome)
            if !outcome.agent_invoked
                && outcome.status == PromptStatus::Aborted
                && outcome.session_settled
    ));

    let settled = decoded(br#"{"type":"session_settled"}"#);
    assert!(settled.events().is_empty());
    assert!(
        settled
            .context()
            .is_some_and(|context| context.session_settled)
    );
}

#[test]
fn dialogs_that_wait_on_an_answer_are_named_while_notices_are_not() {
    let dialog = |bytes: &[u8]| {
        let decoded = decoded(bytes);
        assert!(decoded.events().is_empty(), "a dialog frame is silent");
        decoded.context().and_then(|context| context.dialog.clone())
    };
    assert_eq!(
        dialog(br#"{"type":"extension_ui_request","id":"8","method":"select","title":"Which?","options":["a"]}"#),
        Some(Dialog::Prompt { id: "8".to_owned() })
    );
    assert_eq!(
        dialog(br#"{"type":"extension_ui_request","id":"9","method":"confirm","title":"Deploy","message":"Deploy to prod?"}"#),
        Some(Dialog::Confirm {
            id: "9".to_owned(),
            title: "Deploy".to_owned(),
            message: "Deploy to prod?".to_owned(),
        })
    );
    assert_eq!(
        dialog(br#"{"type":"extension_ui_request","id":"10","method":"notify","message":"hi"}"#),
        None
    );
    assert_eq!(
        dialog(
            br#"{"type":"extension_ui_request","id":"11","method":"open_url","url":"https://x"}"#
        ),
        None
    );
    assert_eq!(dialog(br#"{"type":"agent_start"}"#), None);
}

#[test]
fn a_host_tool_call_is_a_dialog_and_its_cancellation_withdraws_it() {
    let call = decoded(
        br#"{"type":"host_tool_call","id":"h1","toolCallId":"call_9","toolName":"shell","arguments":{"command":"cargo test","timeout":30}}"#,
    );
    assert!(call.events().is_empty(), "a host tool call is silent");
    let context = call.context().expect("a frame has context");
    assert_eq!(
        context.dialog,
        Some(Dialog::HostTool {
            id: "h1".to_owned(),
            tool: "shell".to_owned(),
            arguments: serde_json::json!({"command": "cargo test", "timeout": 30})
                .as_object()
                .expect("object")
                .clone(),
        })
    );
    assert_eq!(context.withdrawn, None);

    let cancel = decoded(br#"{"type":"host_tool_cancel","id":"h2","targetId":"h1"}"#);
    assert!(cancel.events().is_empty(), "a withdrawal is silent");
    let context = cancel.context().expect("a frame has context");
    assert_eq!(context.dialog, None);
    assert_eq!(context.withdrawn, Some("h1".to_owned()));
    assert_eq!(
        decoded(br#"{"type":"agent_start"}"#)
            .context()
            .and_then(|context| context.withdrawn.clone()),
        None
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

/// A session-file message marks the turn its lifecycle frames would have.
#[test]
fn session_file_messages_mark_the_turns_their_frames_are_absent_for() {
    let turn = |bytes: &[u8]| decoded(bytes).context().and_then(|context| context.turn);
    assert_eq!(
        turn(br#"{"type":"message","id":"u1","parentId":null,"timestamp":"2026-09-16T02:24:20.000Z","message":{"role":"user","content":[{"type":"text","text":"second thing"}],"timestamp":1789546460000}}"#),
        Some(TurnMoment::Started)
    );
    assert_eq!(
        turn(br#"{"type":"message","id":"s1","parentId":"u1","timestamp":"2026-09-16T02:24:21.000Z","message":{"role":"user","content":[{"type":"text","text":"stop"}],"steering":true,"attribution":"user","timestamp":1789546461000}}"#),
        None,
        "a steer lands inside the turn it addresses"
    );
    assert_eq!(
        turn(br#"{"type":"message","id":"a1","parentId":"s1","timestamp":"2026-09-16T02:24:22.000Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"c1","name":"bash","arguments":{"command":"true"}}],"usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0},"stopReason":"toolUse"}}"#),
        None,
        "stopping for a tool call is mid-turn"
    );
    assert_eq!(
        turn(br#"{"type":"message","id":"r1","parentId":"a1","timestamp":"2026-09-16T02:24:23.000Z","message":{"role":"toolResult","toolCallId":"c1","toolName":"bash","content":[{"type":"text","text":""}],"isError":false,"timestamp":1789546463000}}"#),
        None
    );
    assert_eq!(turn(TRANSCRIPT_MESSAGE), Some(TurnMoment::Completed));
    assert_eq!(
        turn(br#"{"type":"message","id":"x1","parentId":"r1","timestamp":"2026-09-16T02:24:24.000Z","message":{"role":"assistant","content":[],"stopReason":"aborted"}}"#),
        Some(TurnMoment::Completed)
    );
    // On the wire the lifecycle frames speak and the restated message stays silent.
    assert_eq!(
        turn(br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"stopReason":"stop"}}"#),
        None
    );
}

/// Session-file entries omp writes beside its messages: the fixed-width title
/// slot at the top of every file and the bookkeeping it appends as it runs.
const SESSION_FILE_BOOKKEEPING: &[&[u8]] = &[
    br#"{"type":"title","v":1,"title":"","updatedAt":"2026-09-16T06:43:21.805Z","pad":"   "}"#,
    br#"{"type":"title_change","id":"n1","parentId":null,"timestamp":"2026-09-16T06:43:21.900Z","title":"Current title","source":"auto"}"#,
    br#"{"type":"credential_pin","id":"c1","parentId":null,"timestamp":"2026-09-16T06:43:22.000Z","provider":"anthropic","hash":"deadbeef"}"#,
    br#"{"type":"model_change","id":"m1","parentId":"c1","timestamp":"2026-09-16T06:43:22.100Z","provider":"anthropic","modelId":"claude-opus-5"}"#,
    br#"{"type":"thinking_level_change","id":"t1","parentId":"m1","timestamp":"2026-09-16T06:43:22.200Z","thinkingLevel":"high"}"#,
    br#"{"type":"custom","id":"x1","parentId":"t1","timestamp":"2026-09-16T06:43:22.300Z","customType":"spice","data":{}}"#,
    br#"{"type":"label","id":"l1","parentId":"x1","timestamp":"2026-09-16T06:43:22.400Z","targetId":"x1","label":"here"}"#,
    br#"{"type":"session_init","id":"i1","parentId":null,"timestamp":"2026-09-16T06:43:22.500Z","systemPrompt":"","tools":[]}"#,
    br#"{"type":"mode_change","id":"d1","parentId":"i1","timestamp":"2026-09-16T06:43:22.600Z","mode":"build"}"#,
];

#[test]
fn session_file_bookkeeping_is_owned_ambiently_and_says_nothing() {
    for bytes in SESSION_FILE_BOOKKEEPING {
        let detected = decode_record(None, bytes, locus(bytes)).expect("omp owns its own file");
        assert_eq!(detected.dialect, Dialect::Omp, "{bytes:?}");
        assert!(
            !matches!(detected.decoded, Decoded::Residue(_))
                && detected.decoded.events().is_empty(),
            "bookkeeping is neither residue nor typed: {:?}",
            detected.decoded
        );
    }
    // The session's own compaction record types the same event the RPC frame does.
    let compaction = decoded(
        br#"{"type":"compaction","id":"k1","parentId":"d1","timestamp":"2026-09-16T06:43:23.000Z","summary":"so far","firstKeptEntryId":"e1","tokensBefore":900000}"#,
    );
    assert!(
        matches!(compaction.events(), [Event::Compaction(record)] if record.summary.as_deref() == Some("so far"))
    );
}

/// Both records of one compaction name the same boundary.
///
/// omp announces a compaction on the stream and writes it again into its own
/// session file. The announcement keeps the compaction's fields under
/// `result` and states no identity or time of its own; the session entry
/// repeats those fields beside its own record id and timestamp. A remote
/// compaction additionally names the provider entry that replaced the
/// compacted history, and that entry is the strongest identity either record
/// states.
#[test]
fn both_records_of_one_compaction_name_the_same_boundary() {
    const PRESERVED: &[u8] = br#"{"type":"compaction","id":"k1","parentId":"d1","timestamp":"2026-09-16T06:43:23.000Z","summary":"so far","firstKeptEntryId":"e1","tokensBefore":900000}"#;
    const LATER: &[u8] = br#"{"type":"auto_compaction_end","action":"context-full","result":{"summary":"so far","firstKeptEntryId":"e9","tokensBefore":910000},"aborted":false,"willRetry":false}"#;
    const REMOTE: &[u8] = br#"{"type":"auto_compaction_end","action":"remote","result":{"summary":"remote","firstKeptEntryId":"e2","tokensBefore":260848,"preserveData":{"openaiRemoteCompaction":{"version":"v2","replacementHistory":[{"id":"cmp_03f3","type":"compaction"}]}}},"aborted":false,"willRetry":false}"#;
    const REMOTE_PRESERVED: &[u8] = br#"{"type":"compaction","id":"f6188ab9","parentId":"947360c4","timestamp":"2026-09-19T21:00:53.430Z","summary":"remote","firstKeptEntryId":"e2","tokensBefore":260848,"preserveData":{"openaiRemoteCompaction":{"version":"v2","replacementHistory":[{"id":"cmp_03f3","type":"compaction"}]}}}"#;

    let announced = boundary(COMPACTION);
    let preserved = boundary(PRESERVED);
    assert_eq!(announced.id, "kept:e1:900000");
    assert_eq!(preserved.id, announced.id, "one compaction, one identity");
    assert!(!announced.preserved);
    assert!(preserved.preserved, "the session entry is the vendor's own");

    assert_eq!(boundary(REMOTE).id, "cmp_03f3");
    assert_eq!(boundary(REMOTE_PRESERVED).id, "cmp_03f3");
    assert!(boundary(REMOTE_PRESERVED).preserved);

    assert_ne!(
        boundary(LATER).id,
        announced.id,
        "a later compaction repeating the summary is its own boundary",
    );
}

/// The boundary identity one compaction record states.
fn boundary(bytes: &[u8]) -> wire::MessageBoundary {
    let record = decoded(bytes);
    let [Event::Compaction(compaction)] = record.events() else {
        panic!("a compaction record decodes to one compaction");
    };
    compaction
        .boundary
        .clone()
        .expect("the record names its compaction")
}

#[test]
fn partials_are_restated_by_the_frames_that_end_them_and_decode_to_nothing() {
    for partial in [PARTIAL, TOOL_PARTIAL, TOOL_STREAM] {
        assert!(restated(Dialect::Omp, partial));
        let decoded = decode(Dialect::Omp, partial, locus(partial)).expect("locus matches");
        assert!(decoded.events().is_empty(), "{decoded:?}");
        assert!(!matches!(decoded, Decoded::Residue(_)), "{decoded:?}");
    }
    for whole in [PROSE, TOOL_END, SESSION, COMPACTION] {
        assert!(!restated(Dialect::Omp, whole));
    }
    assert!(
        !restated(Dialect::Omp, b"{\"type\":\"message_update\""),
        "a torn record is kept"
    );
}

#[test]
fn an_unreassembled_transport_envelope_is_residue_not_silent_content() {
    let record = br#"{"type":"rpc_chunk","chunkId":"one","index":0,"count":5,"byteLength":1048577,"data":"eA=="}"#;
    assert!(matches!(decoded(record), Decoded::Residue(_)));
    assert!(!restated(Dialect::Omp, record));
}

#[test]
fn a_record_omp_does_not_own_is_refused_without_an_explicit_dialect() {
    let future = br#"{"type":"future_record"}"#;
    let bare_message = br#"{"type":"message","role":"assistant","content":"done"}"#;
    for record in [&future[..], &bare_message[..]] {
        assert_eq!(
            decode_record(None, record, locus(record)),
            Err(RecordDecodeError::Unowned)
        );
    }
}
