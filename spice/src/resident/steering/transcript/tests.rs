use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{SEARCH_SOURCE_BYTES, SEARCH_WINDOW_BYTES, TranscriptEvidence, transcript_recognition};
use crate::resident::RuntimePaths;
use steer::Outcome as ScanOutcome;
use wire::{CanonicalUuid, Disposition};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

const KEY: &str = "1kKRRsDS";

const ACK_CALL: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_fixture_ack","name":"Bash","input":{"command":"spice ack 1kKRRsDS"}}]}}
"#;
const LEADING_HEADER: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"ACK 1kKRRsDS: revision accepted."}]}}
"#;
const TOOL_RESULT: &[u8] = br#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_fixture_earlier","content":"ok"}]}}
"#;

struct LiveKey;

impl steer::Standing for LiveKey {
    fn disposition(&self, key: &steer::Key) -> Option<Disposition> {
        (key.as_str() == KEY).then_some(Disposition::Live)
    }
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

#[test]
fn the_newest_placed_answer_wins_within_the_recent_window() {
    let declined = String::from_utf8(LEADING_HEADER.to_vec())
        .unwrap()
        .replace("ACK ", "NACK ");
    let fixture = Fixture::new(&[LEADING_HEADER, declined.as_bytes(), ACK_CALL]);
    let evidence = TranscriptEvidence::read(&fixture.paths, lane(), &vendor(), KEY)
        .unwrap_or_else(|error| panic!("{error:?}"));
    let recognition = evidence
        .recognize(KEY, None, &LiveKey)
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert!(matches!(
        recognition.outcome,
        ScanOutcome::Retired {
            disposition: Disposition::Declined,
            ..
        }
    ));
    assert_eq!(recognition.locus.offset, LEADING_HEADER.len() as u64);
}

#[test]
fn bounded_evidence_does_not_replay_older_tape_history() {
    let fixture = Fixture::new(&[]);
    let tape =
        tape::Tape::open_existing(fixture.paths.tape.join(lane().to_string()).join("claude"))
            .unwrap();
    // Enough complete large records to put the older answer outside the window.
    tape.tee(LEADING_HEADER).unwrap();
    let mut padding = vec![b' '; 16 * 1024 - 3];
    padding.extend_from_slice(b"{}\n");
    for _ in 0..(SEARCH_WINDOW_BYTES as usize / padding.len() + 2) {
        tape.tee(&padding).unwrap();
    }
    let evidence = TranscriptEvidence::read(&fixture.paths, lane(), &vendor(), KEY)
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert!(evidence.tail.start.next > listen::Position::start());
    assert!(evidence.tail.work.source_bytes <= SEARCH_SOURCE_BYTES);
    assert!(
        matches!(evidence.recognize(KEY, None, &LiveKey), Err(wire::Outcome::Refusal(refusal)) if refusal.reason() == "steer::transcript-search-limited")
    );
    let newest = tape.tee(LEADING_HEADER).unwrap();
    let recognition =
        transcript_recognition(&fixture.paths, lane(), &vendor(), KEY, None, &LiveKey)
            .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(recognition.locus.offset, newest.offset);
}

#[test]
fn oversized_or_unfinished_records_cannot_supply_unbounded_evidence() {
    let oversized = vec![b'x'; SEARCH_SOURCE_BYTES as usize + 1];
    let fixture = Fixture::new(&[LEADING_HEADER, &oversized]);
    assert!(
        matches!(transcript_recognition(&fixture.paths, lane(), &vendor(), KEY, None, &LiveKey), Err(wire::Outcome::Refusal(refusal)) if refusal.reason() == "steer::transcript-search-limited")
    );
    let unfinished = Fixture::new(&[&LEADING_HEADER[..LEADING_HEADER.len() - 1]]);
    assert!(
        matches!(transcript_recognition(&unfinished.paths, lane(), &vendor(), KEY, None, &LiveKey), Err(wire::Outcome::Refusal(refusal)) if refusal.reason() == "steer::transcript-evidence-absent")
    );
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
        degraded: root.join("degraded"),
        tape: root.join("tape"),
        supervisors: root.join("supervisors"),
        stage: root.join("stage"),
    }
}
