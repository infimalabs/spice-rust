use listen::{
    Canary, Decoded, Dialect, Event, EventKind, Locus, ReasoningContent, ResidueKind, ToolMoment,
    decode,
};

const CLAUDE_SESSION: &[u8] =
    br#"{"type":"system","subtype":"init","session_id":"018f7777-7777-7444-8888-999999999999"}"#;
const CLAUDE_PROSE: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":7,"cache_read_input_tokens":5,"cache_creation_input_tokens":3,"output_tokens":2}}}"#;
const CLAUDE_REASONING: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"thinking","thinking":"inspect"}],"usage":{"input_tokens":7,"cache_read_input_tokens":5,"cache_creation_input_tokens":3,"output_tokens":2}}}"#;
const CLAUDE_OPAQUE_REASONING: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"redacted_thinking","data":"sealed"}],"usage":{"input_tokens":7,"cache_read_input_tokens":5,"cache_creation_input_tokens":3,"output_tokens":2}}}"#;
const CLAUDE_TOOL: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"tool_use","id":"tool-1","name":"shell"}],"usage":{"input_tokens":7,"cache_read_input_tokens":5,"cache_creation_input_tokens":3,"output_tokens":2}}}"#;
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
        bytes: CLAUDE_PROSE,
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
                    name: "web_search".to_owned()
                }
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
        bytes: br#"{"type":"mode"}"#,
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
        bytes: br#"{"type":"world_state"}"#,
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
