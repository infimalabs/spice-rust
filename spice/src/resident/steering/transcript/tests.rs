use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::transcript_recognition;
use crate::resident::RuntimePaths;
use steer::Outcome as ScanOutcome;
use wire::{CanonicalUuid, Disposition, Outcome};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

const KEY: &str = "1kKRRsDS";

/// A thinking block the harness wrote for a header the model narrated beside
/// the ack call: a paraphrase, never the header, and no text sibling.
const NARRATED_HEADER: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"thinking","thinking":"Acknowledging the revision and running the ack command now.","signature":"CAQSAw=="}]}}
"#;
const ACK_CALL: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_fixture_ack","name":"Bash","input":{"command":"spice ack 1kKRRsDS"}}]}}
"#;
const LEADING_HEADER: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"ACK 1kKRRsDS: revision accepted."}]}}
"#;
const SILENT_TEXT: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"Reading the request before answering it.\nA second line the refusal never quotes."}]}}
"#;
const LONG_TEXT: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"This first line runs well past the eighty characters the refusal quotes back to the lane that wrote it."}]}}
"#;
const TOOL_RESULT: &[u8] = br#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_fixture_earlier","content":"ok"}]}}
"#;
const OPERATOR_PROMPT: &[u8] = br#"{"type":"user","message":{"role":"user","content":"ACK 1kKRRsDS: the operator restating the header is not the occupant answering."}}
"#;

struct LiveKey;

impl steer::Standing for LiveKey {
    fn disposition(&self, key: &steer::Key) -> Option<Disposition> {
        (key.as_str() == KEY).then_some(Disposition::Live)
    }
}

#[test]
fn a_header_narrated_beside_the_ack_call_is_refused_as_reasoning_without_a_text_record() {
    let fixture = Fixture::new(&[SILENT_TEXT, TOOL_RESULT, NARRATED_HEADER, ACK_CALL]);

    let refusal = fixture.refusal();

    assert_eq!(refusal.reason(), "steer::transcript-evidence-absent");
    assert_eq!(
        refusal.diagnostic().and_then(|diagnostic| diagnostic.observed()),
        Some(
            format!(
                "the newest assistant message (member 1 offset {}) persisted as reasoning with no text record",
                SILENT_TEXT.len() + TOOL_RESULT.len()
            )
            .as_str()
        )
    );
    assert_eq!(
        refusal
            .diagnostic()
            .and_then(|diagnostic| diagnostic.detail()),
        Some(
            "speak the header as one short line of visible assistant text, \
             `ACK <key>: <one clause>`, immediately before the ack command; prose narrated beside \
             a tool call persists as reasoning and is never evidence"
        )
    );
}

#[test]
fn the_newest_text_without_the_header_is_quoted_back_on_one_line() {
    let fixture = Fixture::new(&[TOOL_RESULT, SILENT_TEXT, ACK_CALL]);

    let refusal = fixture.refusal();

    assert_eq!(
        refusal
            .diagnostic()
            .and_then(|diagnostic| diagnostic.observed()),
        Some(
            format!(
                "the newest assistant text (member 1 offset {}) does not carry the header: \
                 \"Reading the request before answering it.\"",
                TOOL_RESULT.len()
            )
            .as_str()
        )
    );
    assert_eq!(
        refusal
            .diagnostic()
            .and_then(|diagnostic| diagnostic.detail()),
        Some("emit an ACK or NACK header as the first line of assistant text before invoking ack")
    );
}

#[test]
fn a_long_first_line_is_cut_at_eighty_characters() {
    let fixture = Fixture::new(&[LONG_TEXT]);

    let refusal = fixture.refusal();

    assert_eq!(
        refusal
            .diagnostic()
            .and_then(|diagnostic| diagnostic.observed()),
        Some(
            "the newest assistant text (member 1 offset 0) does not carry the header: \
             \"This first line runs well past the eighty characters the refusal quotes back to \
             ...\""
        )
    );
}

#[test]
fn an_ack_call_with_no_prose_is_named_as_a_tool_call_without_a_text_record() {
    let fixture = Fixture::new(&[SILENT_TEXT, TOOL_RESULT, ACK_CALL]);

    let refusal = fixture.refusal();

    assert_eq!(
        refusal.diagnostic().and_then(|diagnostic| diagnostic.observed()),
        Some(
            format!(
                "the newest assistant message (member 1 offset {}) is a tool call with no text record",
                SILENT_TEXT.len() + TOOL_RESULT.len()
            )
            .as_str()
        )
    );
}

#[test]
fn an_operator_record_newer_than_every_assistant_record_bounds_the_message() {
    let fixture = Fixture::new(&[SILENT_TEXT, OPERATOR_PROMPT]);

    let refusal = fixture.refusal();

    assert_eq!(
        refusal
            .diagnostic()
            .and_then(|diagnostic| diagnostic.observed()),
        Some(
            format!(
                "no assistant text newer than the operator record (member 1 offset {})",
                SILENT_TEXT.len()
            )
            .as_str()
        )
    );
}

#[test]
fn an_empty_tape_has_no_assistant_text() {
    let fixture = Fixture::new(&[]);

    let refusal = fixture.refusal();

    assert_eq!(
        refusal
            .diagnostic()
            .and_then(|diagnostic| diagnostic.observed()),
        Some("no assistant text on the tape")
    );
}

#[test]
fn a_leading_header_older_than_the_ack_call_is_still_recognized() {
    let fixture = Fixture::new(&[TOOL_RESULT, LEADING_HEADER, ACK_CALL]);

    let recognition = transcript_recognition(
        &fixture.paths,
        lane(),
        &vendor(),
        KEY,
        Some(Disposition::Acked),
        &LiveKey,
    )
    .unwrap_or_else(|outcome| panic!("the leading header is evidence: {outcome:?}"));

    assert!(matches!(
        recognition.outcome,
        ScanOutcome::Retired {
            disposition: Disposition::Acked,
            ..
        }
    ));
    assert_eq!(recognition.locus.member, 1);
    assert_eq!(recognition.locus.offset, TOOL_RESULT.len() as u64);
}

struct Fixture {
    root: PathBuf,
    paths: RuntimePaths,
}

impl Fixture {
    fn new(records: &[&[u8]]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-transcript-recognition-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("fixture root");
        let paths = runtime_paths(&root);
        let tape = tape::Tape::open(paths.tape.join(lane().to_string()).join("claude"))
            .expect("the lane tape opens");
        for record in records {
            tape.tee(record).expect("the transcript record is captured");
        }
        Self { root, paths }
    }

    fn refusal(&self) -> wire::Refusal {
        match transcript_recognition(&self.paths, lane(), &vendor(), KEY, None, &LiveKey) {
            Err(Outcome::Refusal(refusal)) => refusal,
            Err(other) => panic!("the tape carries no evidence: {other:?}"),
            Ok(recognition) => panic!(
                "the tape carries no evidence: recognized {:?} at {:?}",
                recognition.outcome, recognition.locus
            ),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove transcript fixture");
    }
}

fn lane() -> CanonicalUuid {
    "018f82d4-6f59-7d06-92e3-bce913116301"
        .parse()
        .expect("canonical fixture uuid")
}

fn vendor() -> wire::Name {
    "claude".parse().expect("vendor name")
}

fn runtime_paths(root: &Path) -> RuntimePaths {
    RuntimePaths {
        worktree: root.to_path_buf(),
        common: root.join("common"),
        runtime: root.join("runtime"),
        data: root.join("data"),
        journal: root.join("journal"),
        resident: root.join("resident"),
        canvas: root.join("canvas"),
        lane_canvas: root.join("lane-canvas"),
        inbox: root.join("inbox"),
        legacy_inbox: root.join("legacy-inbox"),
        degraded: root.join("degraded"),
        tape: root.join("tape"),
        supervisors: root.join("supervisors"),
        stage: root.join("stage"),
    }
}
