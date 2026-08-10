use listen::{
    Canary, Decoded, Dialect, Event, EventKind, Locus, ProseRole, ReasoningContent,
    RecordDecodeError, ResidueKind, SurfaceEvent, SurfaceToolMoment, SurfaceToolResult, ToolMoment,
    ToolResult, TurnMoment, decode, decode_record,
};

const CLAUDE_INPUT_TOKENS: u64 = 13;
const CLAUDE_TOOL_OUTPUT_TOKENS: u64 = 191;
const CODEX_INPUT_TOKENS: u64 = 11;

const CLAUDE_SESSION: &[u8] =
    br#"{"type":"system","subtype":"init","session_id":"018f7777-7777-7444-8888-999999999999"}"#;
const CLAUDE_PROSE: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":7,"cache_read_input_tokens":5,"cache_creation_input_tokens":3,"output_tokens":2}}}"#;
const CLAUDE_REASONING: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"thinking","thinking":"inspect"}],"usage":{"input_tokens":7,"cache_read_input_tokens":5,"cache_creation_input_tokens":3,"output_tokens":2}}}"#;
const CLAUDE_OPAQUE_REASONING: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"redacted_thinking","data":"sealed"}],"usage":{"input_tokens":7,"cache_read_input_tokens":5,"cache_creation_input_tokens":3,"output_tokens":2}}}"#;
const CLAUDE_TOOL: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"tool_use","id":"tool-1","name":"shell"}],"usage":{"input_tokens":7,"cache_read_input_tokens":5,"cache_creation_input_tokens":3,"output_tokens":2}}}"#;
const CLAUDE_TOOL_USAGE: &[u8] = concat!(
    "{\"type\":\"stream_event\",\"event\":{\"type\":\"message_delta\",",
    "\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{",
    "\"input_tokens\":2,\"cache_read_input_tokens\":22005,",
    "\"cache_creation_input_tokens\":4213,\"output_tokens\":191,",
    "\"output_tokens_details\":{\"thinking_tokens\":32}}}}",
)
.as_bytes();
const CLAUDE_USAGE: &[u8] = concat!(
    "{\"type\":\"stream_event\",\"event\":{\"type\":\"message_delta\",",
    "\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{",
    "\"input_tokens\":13,\"cache_read_input_tokens\":8,",
    "\"cache_creation_input_tokens\":5,\"output_tokens\":3,",
    "\"output_tokens_details\":{\"thinking_tokens\":2}}}}",
)
.as_bytes();
const CLAUDE_COMPACTION: &[u8] = br#"{"type":"system","subtype":"compact_boundary"}"#;
const CLAUDE_FAILURE: &[u8] =
    br#"{"type":"result","subtype":"error_during_execution","is_error":true,"errors":["unavailable","retry exhausted"]}"#;

const CODEX_PROSE: &[u8] =
    br#"{"type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"working"}}"#;
const CODEX_REASONING: &[u8] =
    br#"{"type":"item.completed","item":{"id":"item-2","type":"reasoning","text":"inspect"}}"#;
const CODEX_TOOL: &[u8] = br#"{"type":"item.started","item":{"id":"item-3","type":"command_execution","command":"cargo test","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#;
const CODEX_WEB_SEARCH: &[u8] = br#"{"type":"item.started","item":{"id":"item-4","type":"web_search","query":"","action":{"type":"other"}}}"#;
const CODEX_USAGE: &[u8] = br#"{"type":"turn.completed","usage":{"input_tokens":11,"cached_input_tokens":7,"cache_write_input_tokens":3,"output_tokens":5,"reasoning_output_tokens":2}}"#;
const CODEX_USAGE_NO_REASONING: &[u8] = concat!(
    "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":11,",
    "\"cached_input_tokens\":7,\"output_tokens\":5}}",
)
.as_bytes();
const CODEX_FAILURE: &[u8] = br#"{"type":"turn.failed","error":{"message":"interrupted"}}"#;
const CODEX_SESSION: &[u8] =
    br#"{"type":"thread.started","thread_id":"019fbc32-beb1-7ee0-b814-a7513f8f8ea5"}"#;

struct CurrentCase {
    dialect: Dialect,
    bytes: &'static [u8],
    expected: EventKind,
}

const CURRENT: &[CurrentCase] = &[
    CurrentCase {
        dialect: Dialect::Claude,
        bytes: CLAUDE_SESSION,
        expected: EventKind::Session,
    },
    CurrentCase {
        dialect: Dialect::Claude,
        bytes: CLAUDE_PROSE,
        expected: EventKind::Prose,
    },
    CurrentCase {
        dialect: Dialect::Claude,
        bytes: CLAUDE_REASONING,
        expected: EventKind::Reasoning,
    },
    CurrentCase {
        dialect: Dialect::Claude,
        bytes: CLAUDE_OPAQUE_REASONING,
        expected: EventKind::Reasoning,
    },
    CurrentCase {
        dialect: Dialect::Claude,
        bytes: CLAUDE_TOOL,
        expected: EventKind::Tool,
    },
    CurrentCase {
        dialect: Dialect::Claude,
        bytes: CLAUDE_USAGE,
        expected: EventKind::Usage,
    },
    CurrentCase {
        dialect: Dialect::Claude,
        bytes: CLAUDE_COMPACTION,
        expected: EventKind::Compaction,
    },
    CurrentCase {
        dialect: Dialect::Claude,
        bytes: CLAUDE_FAILURE,
        expected: EventKind::Failure,
    },
    CurrentCase {
        dialect: Dialect::Codex,
        bytes: CODEX_SESSION,
        expected: EventKind::Session,
    },
    CurrentCase {
        dialect: Dialect::Codex,
        bytes: CODEX_PROSE,
        expected: EventKind::Prose,
    },
    CurrentCase {
        dialect: Dialect::Codex,
        bytes: CODEX_REASONING,
        expected: EventKind::Reasoning,
    },
    CurrentCase {
        dialect: Dialect::Codex,
        bytes: CODEX_TOOL,
        expected: EventKind::Tool,
    },
    CurrentCase {
        dialect: Dialect::Codex,
        bytes: CODEX_WEB_SEARCH,
        expected: EventKind::Tool,
    },
    CurrentCase {
        dialect: Dialect::Codex,
        bytes: CODEX_USAGE,
        expected: EventKind::Usage,
    },
    CurrentCase {
        dialect: Dialect::Codex,
        bytes: CODEX_FAILURE,
        expected: EventKind::Failure,
    },
];

fn locus(bytes: &[u8]) -> Locus {
    Locus::new(4, 19, bytes.len() as u64)
}

fn decoded(case: &CurrentCase) -> Decoded {
    decode(case.dialect, case.bytes, locus(case.bytes)).expect("matching locus")
}

#[test]
fn current_corpus_maps_every_kind_for_both_dialects() {
    for case in CURRENT {
        let decoded = decoded(case);
        assert!(
            decoded
                .events()
                .iter()
                .any(|event| event.kind() == case.expected),
            "{:?} did not produce {:?}: {decoded:?}",
            case.dialect,
            case.expected
        );
        assert!(
            decoded
                .events()
                .iter()
                .all(|event| event.locus() == locus(case.bytes))
        );
    }
}

#[test]
fn current_opaque_failure_and_statusless_tool_payloads_are_preserved() {
    let opaque = decode(
        Dialect::Claude,
        CLAUDE_OPAQUE_REASONING,
        locus(CLAUDE_OPAQUE_REASONING),
    )
    .expect("matching locus");
    assert!(matches!(
        opaque.events().first(),
        Some(Event::Reasoning(reasoning)) if reasoning.content == ReasoningContent::Opaque
    ));

    let failure =
        decode(Dialect::Claude, CLAUDE_FAILURE, locus(CLAUDE_FAILURE)).expect("matching locus");
    assert!(matches!(
        failure.events(),
        [Event::Failure(failure)] if failure.message == "unavailable\nretry exhausted"
    ));

    let search =
        decode(Dialect::Codex, CODEX_WEB_SEARCH, locus(CODEX_WEB_SEARCH)).expect("matching locus");
    assert!(matches!(
        search.events(),
        [Event::Tool(tool)]
            if tool.call_id == "item-4"
                && tool.moment == ToolMoment::Call {
                    name: "web_search".to_owned(),
                    arguments: r#"{"query":""}"#.to_owned(),
                }
    ));
    assert!(matches!(
        SurfaceEvent::from(&search.events()[0]),
        SurfaceEvent::Tool {
            call_id,
            moment: SurfaceToolMoment::Call { name, arguments },
        } if call_id == "item-4"
            && name == "web_search"
            && arguments == r#"{"query":""}"#
    ));
}

#[test]
fn exact_current_initialization_records_declare_only_their_session_identity() {
    let claude = decode(Dialect::Claude, CLAUDE_SESSION, locus(CLAUDE_SESSION))
        .expect("matching Claude declaration locus");
    assert!(matches!(
        claude.events(),
        [Event::Session(declaration)]
            if declaration.identity == "018f7777-7777-7444-8888-999999999999"
                && declaration.locus == locus(CLAUDE_SESSION)
    ));

    let codex = decode(Dialect::Codex, CODEX_SESSION, locus(CODEX_SESSION))
        .expect("matching Codex declaration locus");
    assert!(matches!(
        codex.events(),
        [Event::Session(declaration)]
            if declaration.identity == "019fbc32-beb1-7ee0-b814-a7513f8f8ea5"
                && declaration.locus == locus(CODEX_SESSION)
    ));
}

#[test]
fn final_turn_records_are_the_only_usage_events_for_each_dialect() {
    let snapshots = [CLAUDE_PROSE, CLAUDE_PROSE, CLAUDE_REASONING, CLAUDE_TOOL];
    let mut usage_events = 0;
    for snapshot in snapshots {
        let decoded = decode(Dialect::Claude, snapshot, locus(snapshot))
            .expect("matching Claude snapshot locus");
        usage_events += decoded
            .events()
            .iter()
            .filter(|event| matches!(event, Event::Usage(_)))
            .count();
        assert!(
            decoded
                .events()
                .iter()
                .all(|event| !matches!(event, Event::Usage(_))),
            "a repeated assistant snapshot emitted spend: {decoded:?}"
        );
    }
    assert_eq!(usage_events, 0, "repeated snapshots never count spend");

    let claude = decode(Dialect::Claude, CLAUDE_USAGE, locus(CLAUDE_USAGE))
        .expect("matching Claude final usage locus");
    assert!(matches!(
        claude.events(),
        [Event::Usage(usage)]
            if usage.turns == 1
                && usage.input_tokens == CLAUDE_INPUT_TOKENS
                && usage.cached_input_tokens == 8
                && usage.cache_write_input_tokens == 5
                && usage.output_tokens == 3
                && usage.reasoning_output_tokens == Some(2)
                && usage.locus == locus(CLAUDE_USAGE)
    ));

    let claude_tool = decode(Dialect::Claude, CLAUDE_TOOL_USAGE, locus(CLAUDE_TOOL_USAGE))
        .expect("matching Claude tool-turn usage locus");
    assert!(matches!(
        claude_tool.events(),
        [Event::Usage(usage)]
            if usage.turns == 1
                && usage.output_tokens == CLAUDE_TOOL_OUTPUT_TOKENS
                && usage.reasoning_output_tokens == Some(32)
    ));

    let codex = decode(Dialect::Codex, CODEX_USAGE, locus(CODEX_USAGE))
        .expect("matching Codex final usage locus");
    assert!(matches!(
        codex.events(),
        [Event::Usage(usage)]
            if usage.turns == 1
                && usage.input_tokens == CODEX_INPUT_TOKENS
                && usage.cached_input_tokens == 7
                && usage.cache_write_input_tokens == 3
                && usage.output_tokens == 5
                && usage.reasoning_output_tokens == Some(2)
                && usage.locus == locus(CODEX_USAGE)
    ));

    let codex_without_reasoning = decode(
        Dialect::Codex,
        CODEX_USAGE_NO_REASONING,
        locus(CODEX_USAGE_NO_REASONING),
    )
    .expect("optional reasoning usage stays typed");
    assert!(matches!(
        codex_without_reasoning.events(),
        [Event::Usage(usage)]
            if usage.turns == 1
                && usage.cache_write_input_tokens == 0
                && usage.reasoning_output_tokens.is_none()
    ));
}

#[derive(Clone, Copy)]
enum ExpectedResidue {
    Absent,
    Unknown,
    Malformed,
    Truncated,
    VersionDrift,
}

struct ResidueCase {
    dialect: Dialect,
    bytes: &'static [u8],
    expected: ExpectedResidue,
}

const RESIDUE: &[ResidueCase] = &[
    ResidueCase {
        dialect: Dialect::Claude,
        bytes: br#"{}"#,
        expected: ExpectedResidue::Absent,
    },
    ResidueCase {
        dialect: Dialect::Claude,
        bytes: br#"{"type":"mystery"}"#,
        expected: ExpectedResidue::Unknown,
    },
    ResidueCase {
        dialect: Dialect::Claude,
        bytes: br#"{"type":7}"#,
        expected: ExpectedResidue::Malformed,
    },
    ResidueCase {
        dialect: Dialect::Claude,
        bytes: br#"{"type":"assistant""#,
        expected: ExpectedResidue::Truncated,
    },
    ResidueCase {
        dialect: Dialect::Claude,
        bytes: br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"citation"}],"usage":{"input_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":1}}}"#,
        expected: ExpectedResidue::VersionDrift,
    },
    ResidueCase {
        dialect: Dialect::Codex,
        bytes: br#"{"type":"item.completed"}"#,
        expected: ExpectedResidue::Absent,
    },
    ResidueCase {
        dialect: Dialect::Codex,
        bytes: br#"{"type":"mode"}"#,
        expected: ExpectedResidue::Unknown,
    },
    ResidueCase {
        dialect: Dialect::Codex,
        bytes: br#"[]"#,
        expected: ExpectedResidue::Malformed,
    },
    ResidueCase {
        dialect: Dialect::Codex,
        bytes: br#"{"type":"item.completed","item":"#,
        expected: ExpectedResidue::Truncated,
    },
    ResidueCase {
        dialect: Dialect::Codex,
        bytes: br#"{"type":"item.completed","item":{"id":"item-9","type":"computer_call"}}"#,
        expected: ExpectedResidue::VersionDrift,
    },
];

#[test]
fn non_events_remain_exactly_accounted_residue() {
    for case in RESIDUE {
        let decoded = decode(case.dialect, case.bytes, locus(case.bytes)).expect("matching locus");
        let residue = decoded.residue().expect("corpus case must be residue");
        assert_eq!(residue.accounted_bytes(), case.bytes.len() as u64);
        assert!(matches!(
            (&residue.kind, case.expected),
            (ResidueKind::Absent { .. }, ExpectedResidue::Absent)
                | (ResidueKind::Unknown { .. }, ExpectedResidue::Unknown)
                | (ResidueKind::Malformed { .. }, ExpectedResidue::Malformed)
                | (ResidueKind::Truncated, ExpectedResidue::Truncated)
                | (
                    ResidueKind::VersionDrift { .. },
                    ExpectedResidue::VersionDrift
                )
        ));
    }
}

#[test]
fn locus_length_mismatch_fails_before_classification() {
    let wrong = Locus::new(4, 19, CLAUDE_PROSE.len() as u64 + 1);
    let error = decode(Dialect::Claude, CLAUDE_PROSE, wrong).expect_err("length mismatch");
    assert_eq!(error.locus, wrong);
    assert_eq!(error.actual_length, CLAUDE_PROSE.len() as u64);
}

#[test]
fn canary_names_each_disappeared_kind_while_residue_survives() {
    let observations: Vec<_> = CURRENT
        .iter()
        .map(decoded)
        .chain(std::iter::once(
            decode(Dialect::Codex, b"{}", locus(b"{}")).expect("matching locus"),
        ))
        .collect();

    assert!(Canary::inspect(&EventKind::ALL, &observations).is_empty());
    for missing in EventKind::ALL {
        let without_kind: Vec<_> = observations
            .iter()
            .filter(|decoded| decoded.events().iter().all(|event| event.kind() != missing))
            .cloned()
            .collect();
        assert_eq!(
            Canary::inspect(&EventKind::ALL, &without_kind),
            vec![listen::Alarm { kind: missing }]
        );
        assert!(
            observations
                .iter()
                .any(|decoded| decoded.residue().is_some())
        );
    }
}

#[test]
fn codex_canary_exposes_absent_compaction_signal() {
    let observations: Vec<_> = CURRENT
        .iter()
        .filter(|case| case.dialect == Dialect::Codex)
        .map(decoded)
        .collect();

    assert_eq!(
        Canary::inspect(&EventKind::ALL, &observations),
        vec![listen::Alarm {
            kind: EventKind::Compaction
        }]
    );
}

#[test]
fn persisted_records_publish_vendor_neutral_context_and_facts() {
    let codex_start = br#"{"timestamp":"2026-08-09T01:00:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-1"}}"#;
    let detected = decode_record(None, codex_start, locus(codex_start))
        .expect("the Codex envelope owns its one decoder");
    assert_eq!(detected.dialect, Dialect::Codex);
    assert!(matches!(
        detected.decoded,
        Decoded::Events { context, events }
            if context.timestamp.as_deref() == Some("2026-08-09T01:00:00Z")
                && context.turn_id.as_deref() == Some("turn-1")
                && context.turn == Some(TurnMoment::Started)
                && events.is_empty()
    ));

    let codex_result = br#"{"timestamp":"2026-08-09T01:00:01Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call-1","output":"Process exited with code 7"}}"#;
    let detected = decode_record(None, codex_result, locus(codex_result))
        .expect("the Codex tool outcome stays typed");
    assert!(matches!(
        detected.decoded.events(),
        [Event::Tool(tool)]
            if matches!(
                &tool.moment,
                ToolMoment::Result(outcome)
                    if outcome.result == ToolResult::Failed
                        && outcome.exit_code == Some(7)
                        && outcome.output.as_deref() == Some("Process exited with code 7")
            )
    ));
    assert!(matches!(
        SurfaceEvent::from(&detected.decoded.events()[0]),
        SurfaceEvent::Tool {
            call_id,
            moment: SurfaceToolMoment::Result {
                standing: SurfaceToolResult::Failed,
                output: Some(output),
                exit_code: Some(7),
                media,
            },
        } if call_id == "call-1"
            && output == "Process exited with code 7"
            && media.is_empty()
    ));

    let claude_user = br#"{"timestamp":"2026-08-09T02:00:00Z","type":"user","promptId":"prompt-1","message":{"type":"message","role":"user","content":[{"type":"text","text":"inspect"}]}}"#;
    let detected = decode_record(None, claude_user, locus(claude_user))
        .expect("the Claude envelope owns its one decoder");
    assert_eq!(detected.dialect, Dialect::Claude);
    assert!(matches!(
        &detected.decoded,
        Decoded::Events { context, events }
            if context.turn_id.as_deref() == Some("prompt-1")
                && context.turn == Some(TurnMoment::Observed)
                && matches!(events.as_slice(), [Event::Prose(prose)]
                    if prose.role == ProseRole::Operator && prose.text == "inspect")
    ));

    let claude_final = br#"{"timestamp":"2026-08-09T02:00:01Z","type":"assistant","message":{"type":"message","role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":5,"cache_read_input_tokens":2,"output_tokens":3}}}"#;
    let detected = decode_record(None, claude_final, locus(claude_final))
        .expect("the Claude final carries message and usage facts");
    assert!(matches!(
        &detected.decoded,
        Decoded::Events { context, events }
            if context.turn == Some(TurnMoment::Completed)
                && matches!(events.as_slice(), [Event::Prose(prose), Event::Usage(usage)]
                    if prose.role == ProseRole::Assistant
                        && prose.phase.as_deref() == Some("final_answer")
                        && usage.input_tokens == 5
                        && usage.cached_input_tokens == 2
                        && usage.output_tokens == 3)
    ));
}

#[test]
fn codex_and_claude_tool_results_preserve_one_admitted_image_per_call() {
    const PNG: &str = "iVBORw0KGgo=";
    let codex_call = serde_json::to_vec(&serde_json::json!({
        "type": "response_item",
        "payload": {
            "type": "function_call",
            "name": "mcp__functions__view_image",
            "arguments": r#"{"path":"/tmp/capture.png"}"#,
            "call_id": "call-image",
        },
    }))
    .expect("Codex MCP call fixture encodes");
    let codex_call =
        decode(Dialect::Codex, &codex_call, locus(&codex_call)).expect("Codex MCP call decodes");
    assert!(matches!(
        SurfaceEvent::from(&codex_call.events()[0]),
        SurfaceEvent::Tool {
            call_id,
            moment: SurfaceToolMoment::Call { name, arguments },
        } if call_id == "call-image"
            && name == "mcp__functions__view_image"
            && arguments == r#"{"path":"/tmp/capture.png"}"#
    ));

    let codex = serde_json::to_vec(&serde_json::json!({
        "type": "event_msg",
        "payload": {
            "type": "mcp_tool_call_end",
            "call_id": "call-image",
            "invocation": {
                "server": "functions",
                "tool": "view_image",
                "arguments": {"path": "/tmp/capture.png"},
            },
            "result": {
                "Ok": {
                    "content": [
                        {"type": "image", "data": PNG, "mimeType": "image/png"},
                        {"type": "image", "data": PNG, "mimeType": "image/png"},
                    ],
                    "isError": false,
                },
            },
        },
    }))
    .expect("Codex MCP result fixture encodes");
    let codex = decode(Dialect::Codex, &codex, locus(&codex)).expect("Codex image result decodes");
    assert!(matches!(
        SurfaceEvent::from(&codex.events()[0]),
        SurfaceEvent::Tool {
            call_id,
            moment: SurfaceToolMoment::Result {
                standing: SurfaceToolResult::Succeeded,
                output: Some(_),
                media,
                ..
            },
        } if call_id == "call-image" && media.len() == 1
    ));
    let [Event::Tool(codex)] = codex.events() else {
        panic!("Codex image result stays one tool event");
    };
    let ToolMoment::Result(codex) = &codex.moment else {
        panic!("Codex image record is a result");
    };
    assert_eq!(codex.media.len(), 1, "duplicate display blocks collapse");

    let claude = format!(
        concat!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"call-image","is_error":false,"content":["#,
            r#"{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{0}"}}}}]}}]}}}}"#,
        ),
        PNG,
    );
    let claude = decode(Dialect::Claude, claude.as_bytes(), locus(claude.as_bytes()))
        .expect("Claude image result decodes");
    let [Event::Tool(claude)] = claude.events() else {
        panic!("Claude image result stays one tool event");
    };
    let ToolMoment::Result(claude) = &claude.moment else {
        panic!("Claude image record is a result");
    };
    assert_eq!(claude.media.len(), 1);
    assert_eq!(
        codex.media[0].identity().unwrap(),
        claude.media[0].identity().unwrap(),
        "vendor spellings fold onto one decoded-byte identity",
    );
}

#[test]
fn codex_and_claude_authored_directives_retain_delivered_media_beside_the_text() {
    let name = r"operator](capture)\雪.png";
    let media = wire::Media::from_base64(name, "image/png", "iVBORw0KGgo=")
        .expect("fixture attachment admits");
    let rendered = format!(
        "<1kInbox>\ndirective=018f82d4-6f59-7d06-92e3-bce9131163af \
         key=1kAsk001 stage=published expires=2026-08-01T06:30:00.000Z\n\
         inspect this capture\n\n{}\n</1kInbox>\n",
        media.markdown().expect("fixture Markdown renders"),
    );
    let codex = serde_json::to_vec(&serde_json::json!({
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": rendered}],
        },
    }))
    .expect("Codex authored directive encodes");
    let codex =
        decode(Dialect::Codex, &codex, locus(&codex)).expect("Codex authored directive decodes");
    let [Event::Prose(codex)] = codex.events() else {
        panic!("Codex directive stays one authored prose event");
    };

    let claude = serde_json::to_vec(&serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": rendered}],
        },
    }))
    .expect("Claude authored directive encodes");
    let claude = decode(Dialect::Claude, &claude, locus(&claude))
        .expect("Claude authored directive decodes");
    let [Event::Prose(claude)] = claude.events() else {
        panic!("Claude directive stays one authored prose event");
    };

    assert_eq!(codex.text, rendered);
    assert_eq!(claude.text, rendered);
    assert_eq!(codex.media.len(), 1);
    assert_eq!(claude.media.len(), 1);
    assert_eq!(codex.media[0].name, name);
    assert_eq!(claude.media[0].name, name);
    assert_eq!(
        codex.media[0].identity().unwrap(),
        claude.media[0].identity().unwrap(),
        "the same delivered directive bytes retain one vendor-neutral identity",
    );
}

#[test]
fn automatic_ownership_refuses_disagreement_without_a_fallback() {
    let codex = br#"{"timestamp":"2026-08-09T01:00:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-1"}}"#;
    assert!(matches!(
        decode_record(Some(Dialect::Claude), codex, locus(codex)),
        Err(RecordDecodeError::Disagreement {
            expected: Dialect::Claude,
            observed: Dialect::Codex,
            ..
        })
    ));
    assert!(matches!(
        decode_record(None, b"{", locus(b"{")),
        Err(RecordDecodeError::Invalid {
            kind: ResidueKind::Truncated,
        })
    ));

    let unknown = b"{\"type\":\"mystery\"}";
    for dialect in [Dialect::Codex, Dialect::Claude] {
        let residue = decode_record(Some(dialect), unknown, locus(unknown))
            .expect("an explicit owner accounts unknown bytes")
            .decoded;
        assert_eq!(
            residue
                .residue()
                .expect("unknown current record remains residue")
                .accounted_bytes(),
            unknown.len() as u64
        );
    }
}
