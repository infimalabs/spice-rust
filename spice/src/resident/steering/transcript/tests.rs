use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{TranscriptEvidence, transcript_recognition};
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
const OMP_ACK_UPDATE: &[u8] = br#"{"type":"message_update","assistantMessageEvent":{"type":"text_end","content":"ACK 1kKRRsDS: revision accepted."},"message":{"role":"assistant","content":[{"type":"text","text":"ACK 1kKRRsDS: revision accepted."}]}}
"#;
const OMP_PARAPHRASED_END: &[u8] = br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Acknowledging the operator direction."},{"type":"toolCall","id":"call-1","name":"shell","arguments":{"command":"echo ACK 1kKRRsDS"}}],"stopReason":"toolUse"}}
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
fn omp_terminal_text_update_survives_a_header_stripped_settled_message() {
    let fixture = Fixture::omp(&[OMP_ACK_UPDATE, OMP_PARAPHRASED_END]);

    let recognition =
        transcript_recognition(&fixture.paths, lane(), &omp_vendor(), KEY, None, &LiveKey)
            .unwrap_or_else(|outcome| {
                panic!("the streamed visible header is evidence: {outcome:?}")
            });

    assert!(matches!(
        recognition.outcome,
        ScanOutcome::Retired {
            disposition: Disposition::Acked,
            ..
        }
    ));
    assert_eq!(recognition.locus.offset, 0);
}

#[test]
fn omp_paraphrase_and_tool_payload_do_not_invent_acknowledgement_evidence() {
    let fixture = Fixture::omp(&[OMP_PARAPHRASED_END]);

    assert!(matches!(
        transcript_recognition(&fixture.paths, lane(), &omp_vendor(), KEY, None, &LiveKey),
        Err(wire::Outcome::Refusal(refusal))
            if refusal.reason() == "steer::transcript-evidence-absent"
    ));
}

#[test]
fn the_newest_placed_answer_wins_within_the_recent_window() {
    let declined = String::from_utf8(LEADING_HEADER.to_vec())
        .unwrap()
        .replace("ACK ", "NAK ");
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
fn an_answer_behind_multi_megabyte_records_is_still_recognized() {
    let mut oversized = vec![b' '; 3 * 1024 * 1024];
    oversized.extend_from_slice(b"{}\n");
    let fixture = Fixture::new(&[
        TOOL_RESULT,
        LEADING_HEADER,
        &oversized,
        &oversized,
        ACK_CALL,
    ]);
    let recognition =
        transcript_recognition(&fixture.paths, lane(), &vendor(), KEY, None, &LiveKey)
            .unwrap_or_else(|error| panic!("{error:?}"));
    assert!(matches!(
        recognition.outcome,
        ScanOutcome::Retired {
            disposition: Disposition::Acked,
            ..
        }
    ));
    assert_eq!(recognition.locus.offset, TOOL_RESULT.len() as u64);
}

#[test]
fn an_unfinished_record_cannot_supply_evidence() {
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
        Self::for_vendor("claude", records)
    }

    fn omp(records: &[&[u8]]) -> Self {
        Self::for_vendor("omp", records)
    }

    fn for_vendor(vendor: &str, records: &[&[u8]]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-transcript-recognition-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("fixture root");
        let paths = runtime_paths(&root);
        let tape = tape::Tape::open(paths.tape.join(lane().to_string()).join(vendor))
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

fn omp_vendor() -> wire::Name {
    "omp".parse().expect("vendor name")
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
