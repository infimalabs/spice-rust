use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use listen::{
    Dialect, EventKind, Observation, SurfaceEvent, TailBudget, TranscriptContinuation,
    TranscriptFilter, TranscriptPage, TranscriptPageRequest, transcript_page,
};
use tape::Tape;
use wire::output::{Limits, Request, Surface};

const PAGE_SURFACE: Surface = Surface::new(
    Limits {
        rows: 2,
        bytes: 16 * 1024,
        text: 4,
    },
    Limits {
        rows: 8,
        bytes: 64 * 1024,
        text: 1024,
    },
);

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-listen-page-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("scratch root is exclusively created");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn scan_budget(window: u64) -> TailBudget {
    TailBudget::new(window, window + 64 * 1024, 128, 128, Duration::from_secs(1))
        .expect("fixture scan budget is valid")
}

fn request(before: Option<listen::TranscriptCursor>) -> TranscriptPageRequest {
    TranscriptPageRequest {
        scan: scan_budget(64 * 1024),
        output: PAGE_SURFACE
            .budget(Request::default())
            .expect("fixture output budget is valid"),
        before,
        filter: TranscriptFilter::default(),
    }
}

fn prose(text: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "stopReason": "stop",
        },
    }))
    .expect("fixture record encodes")
}

fn append(tape: &Tape, bytes: &[u8]) {
    let mut record = bytes.to_vec();
    record.push(b'\n');
    tape.tee(&record).expect("fixture record is durable");
}

fn prose_rows(page: &TranscriptPage) -> Vec<(&str, usize)> {
    page.records
        .rows
        .iter()
        .flat_map(|record| &record.observations)
        .filter_map(|visible| match &visible.observation {
            Observation::Event {
                event: SurfaceEvent::Prose { text, .. },
                ..
            } => Some((text.as_str(), visible.withheld_text_bytes)),
            _ => None,
        })
        .collect()
}

#[test]
fn pages_are_newest_first_clip_text_and_resume_without_duplicates() {
    let scratch = Scratch::new("resume");
    let tape_path = scratch.path().join("lane-a/omp");
    let tape = Tape::open(&tape_path).expect("fixture tape opens");
    for text in ["first", "second", "third", "fourth"] {
        append(&tape, &prose(text));
    }

    let first = transcript_page(&tape_path, "lane-a", Dialect::Omp, request(None))
        .expect("first page reads");
    assert_eq!(prose_rows(&first), vec![("four", 2), ("thir", 1)]);
    let continuation = match &first
        .records
        .omission
        .as_ref()
        .expect("older decoded rows are accounted")
        .continuation
    {
        TranscriptContinuation::Cursor { cursor } => cursor.clone(),
        TranscriptContinuation::Export { .. } => panic!("ordinary row must be resumable"),
    };
    assert_eq!(continuation.to_string().parse(), Ok(continuation.clone()));

    let second = transcript_page(
        &tape_path,
        "lane-a",
        Dialect::Omp,
        request(Some(continuation.clone())),
    )
    .expect("older page resumes");
    assert_eq!(prose_rows(&second), vec![("seco", 2), ("firs", 1)]);
    assert!(second.records.omission.is_none());
    assert!(second.history.is_none());
    let other_path = scratch.path().join("lane-b/omp");
    let other = Tape::open(&other_path).expect("second generation opens");
    append(&other, &prose("replacement"));
    let stale = transcript_page(
        &other_path,
        "lane-b",
        Dialect::Omp,
        request(Some(continuation)),
    )
    .expect_err("a cursor from another generation is refused");
    assert!(stale.to_string().contains("source changed"));
}

#[test]
fn filters_run_before_presentation_admission() {
    let scratch = Scratch::new("filter-export");
    let tape_path = scratch.path().join("lane-a/omp");
    let tape = Tape::open(&tape_path).expect("fixture tape opens");
    append(&tape, &prose("ordinary prose"));
    let failure = serde_json::to_vec(&serde_json::json!({
        "type": "rpc_frame_error",
        "error": "failed loudly",
    }))
    .expect("failure fixture encodes");
    append(&tape, &failure);

    let filtered = transcript_page(
        &tape_path,
        "lane-a",
        Dialect::Omp,
        TranscriptPageRequest {
            filter: TranscriptFilter::only([EventKind::Failure]),
            ..request(None)
        },
    )
    .expect("filtered page reads");
    assert_eq!(filtered.records.rows.len(), 1);
    assert!(filtered.records.rows.iter().all(|record| {
        record.observations.iter().all(|visible| {
            matches!(
                visible.observation,
                Observation::Event {
                    event: SurfaceEvent::Failure { .. },
                    ..
                }
            )
        })
    }));
}

#[test]
fn oversized_record_export_continues_with_strictly_older_rows() {
    let scratch = Scratch::new("export-resume");
    let tape_path = scratch.path().join("lane-a/omp");
    let tape = Tape::open(&tape_path).expect("fixture tape opens");
    append(&tape, &prose("older first"));
    append(&tape, &prose("older second"));

    let media = (0..8)
        .map(|_| {
            serde_json::json!({
                "type": "image",
                "data": "iVBORw0KGgo=",
                "mimeType": "image/png",
            })
        })
        .collect::<Vec<_>>();
    let image = serde_json::to_vec(&serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "toolResult",
            "toolCallId": "call-image",
            "toolName": "read",
            "content": media,
            "isError": false,
        },
    }))
    .expect("image fixture encodes");
    append(&tape, &image);
    let tiny = Surface::new(
        Limits {
            rows: 2,
            bytes: 256,
            text: 32,
        },
        Limits {
            rows: 8,
            bytes: 512,
            text: 64,
        },
    );
    let oversized = transcript_page(
        &tape_path,
        "lane-a",
        Dialect::Omp,
        TranscriptPageRequest {
            scan: scan_budget(64 * 1024),
            output: tiny
                .budget(Request::default())
                .expect("tiny output budget is valid"),
            before: None,
            filter: TranscriptFilter::only([EventKind::Tool]),
        },
    )
    .expect("oversized presentation is accounted");
    assert!(oversized.records.rows.is_empty());
    let omission = oversized
        .records
        .omission
        .as_ref()
        .expect("oversized record has omission metadata");
    let (spans, continuation) = match &omission.continuation {
        TranscriptContinuation::Export {
            spans,
            continuation,
        } => (spans, continuation.clone()),
        TranscriptContinuation::Cursor { .. } => {
            panic!("record above the surface maximum requires exact export")
        }
    };
    assert_eq!(spans.len(), 1, "one source record is exported once");
    assert!(oversized.canaries.iter().any(|observation| matches!(
        observation,
        Observation::Canary {
            expected: EventKind::Reasoning,
            ..
        }
    )));
    assert!(oversized.records.omission.as_ref().is_some_and(|omission| {
        omission.population == 1 && omission.shown == 0 && omission.withheld == 1
    }));
    let older = transcript_page(
        &tape_path,
        "lane-a",
        Dialect::Omp,
        TranscriptPageRequest {
            before: Some(continuation),
            ..request(None)
        },
    )
    .expect("exact export continuation reads strictly older rows");
    assert_eq!(older.records.rows.len(), 2);
    assert!(older.records.rows.iter().all(|record| {
        record.observations.iter().all(|visible| {
            !matches!(
                visible.observation,
                Observation::Event {
                    event: SurfaceEvent::Tool { .. },
                    ..
                }
            )
        })
    }));

    let tool = oversized
        .records
        .omission
        .as_ref()
        .expect("oversized tool is omitted");
    assert!(tool.next_bytes > tiny.default_limits().bytes);
}

#[test]
fn source_window_reports_exact_older_bytes_and_continues_before_it() {
    let scratch = Scratch::new("history-window");
    let tape_path = scratch.path().join("lane-a/omp");
    let tape = Tape::open(&tape_path).expect("fixture tape opens");
    for index in 0..12 {
        append(&tape, &prose(&format!("record-{index:02}")));
    }
    let output = PAGE_SURFACE
        .budget(Request {
            rows: Some(8),
            bytes: None,
            text: None,
        })
        .expect("wide row budget is valid");
    let first = transcript_page(
        &tape_path,
        "lane-a",
        Dialect::Omp,
        TranscriptPageRequest {
            scan: scan_budget(256),
            output,
            before: None,
            filter: TranscriptFilter::default(),
        },
    )
    .expect("bounded tail page reads");
    let history = first.history.expect("older source bytes are explicit");
    assert!(history.withheld_source_bytes > 0);
    let newest = first
        .records
        .rows
        .iter()
        .map(|record| record.end.clone())
        .collect::<Vec<_>>();

    let older = transcript_page(
        &tape_path,
        "lane-a",
        Dialect::Omp,
        TranscriptPageRequest {
            scan: scan_budget(256),
            output,
            before: Some(history.continuation),
            filter: TranscriptFilter::default(),
        },
    )
    .expect("older source window reads");
    assert!(
        older
            .records
            .rows
            .iter()
            .all(|record| !newest.contains(&record.end))
    );
}
#[test]
fn hard_scan_allowance_refuses_a_single_noisy_record() {
    let scratch = Scratch::new("source-limit");
    let tape_path = scratch.path().join("lane-a/omp");
    let tape = Tape::open(&tape_path).expect("fixture tape opens");
    append(&tape, &prose(&"noise".repeat(4096)));

    let error = transcript_page(
        &tape_path,
        "lane-a",
        Dialect::Omp,
        TranscriptPageRequest {
            scan: TailBudget::new(128, 256, 8, 8, Duration::from_secs(1))
                .expect("small hard scan budget is valid"),
            ..request(None)
        },
    )
    .expect_err("a record beyond the hard source allowance is refused");
    assert!(error.to_string().contains("source bytes"));
}
