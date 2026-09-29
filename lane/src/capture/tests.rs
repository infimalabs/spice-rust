use super::*;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use vendor::{Attribution, Effort, Launch, Resume, SteeringHook};

#[derive(Default)]
struct Reports(Mutex<Vec<CaptureRecord>>);

impl CaptureReporter for Reports {
    fn record(&self, record: CaptureRecord) -> Result<(), CaptureReportError> {
        self.0.lock().unwrap().push(record);
        Ok(())
    }
    fn failed(&self, failure: CaptureFailure) {
        panic!("unexpected failure: {failure:?}");
    }
}

struct Fixture {
    root: PathBuf,
    observer: LaneObserver,
    reports: Arc<Reports>,
}

impl Fixture {
    fn new(dialect: listen::Dialect) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-lane-capture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let launch = Launch {
            cwd: root.clone(),
            prompt: "start".to_owned(),
            skill: "skill".to_owned(),
            model: "fixture".to_owned(),
            effort: Effort::XHigh,
            personality: None,
            fast_mode: Some(false),
            model_context_window: (dialect == listen::Dialect::Codex).then_some(1_000_000),
            auto_compact_window_tokens: Some(220_000),
            config_overlay: (dialect == listen::Dialect::Omp).then(|| root.join("omp.yml")),
            lsp: (dialect == listen::Dialect::Omp).then_some(false),
            attribution: Attribution::Harness,
            hook: SteeringHook {
                command: "spice hook omp".to_owned(),
            },
            steering_key: None,
            mcp: None,
            resume: Resume::Fresh,
        };
        let record = match dialect {
            listen::Dialect::Omp => vendor::omp::record("omp", &launch),
            listen::Dialect::Codex => vendor::codex::record("codex", &launch),
            listen::Dialect::Claude => unreachable!(),
        }
        .unwrap();
        let reports = Arc::new(Reports::default());
        let observer = LaneObserver {
            lane: "575c6f37-462a-423f-824a-3ae83ba2d448".parse().unwrap(),
            occupant: "171596b1-121a-409c-aaca-4f638c3bc0c9".parse().unwrap(),
            tape: Tape::open(&root).unwrap(),
            record,
            reporter: reports.clone(),
            stage: AtomicU8::new(DeliveryStage::Decode as u8),
            transport: (dialect == listen::Dialect::Omp)
                .then(|| Mutex::new(vendor::omp::frame::FrameDecoder::default())),
        };
        Self {
            root,
            observer,
            reports,
        }
    }

    fn chunks(&self, bytes: &[u8], incomplete: bool) {
        let count = bytes.len().div_ceil(256 * 1024);
        for (index, chunk) in bytes.chunks(256 * 1024).enumerate() {
            if incomplete && index + 1 == count {
                break;
            }
            let mut frame = serde_json::to_vec(&serde_json::json!({
                "type":"rpc_chunk", "chunkId":"capture", "index":index,
                "count":count, "byteLength":bytes.len(), "data":STANDARD.encode(chunk),
            }))
            .unwrap();
            frame.push(b'\n');
            self.observer
                .receive(&frame, RecordBoundary::Complete)
                .unwrap();
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn large(value: serde_json::Value) -> Vec<u8> {
    let mut value = value;
    value["padding"] = serde_json::json!("x".repeat(1024 * 1024));
    serde_json::to_vec(&value).unwrap()
}

#[test]
fn reassembled_session_and_content_survive_byte_exact_tape_replay() {
    let fixture = Fixture::new(listen::Dialect::Omp);
    let session = large(serde_json::json!({"type":"response", "command":"get_state",
        "success":true, "data":{"sessionId":"exact-session"}}));
    let prose = large(serde_json::json!({"type":"message_end", "message":{
        "role":"assistant", "content":[{"type":"text", "text":"retained content"}]}}));
    fixture.chunks(&session, false);
    fixture.chunks(&prose, false);
    fixture.observer.finish().unwrap();
    let reports = fixture.reports.0.lock().unwrap();
    assert_eq!(reports.len(), 2);
    assert_eq!(
        reports[0].declaration.as_ref().unwrap().session.as_str(),
        "exact-session"
    );
    assert!(
        reports[1]
            .decoded
            .events()
            .iter()
            .any(|event| matches!(event,
        listen::Event::Prose(prose) if prose.text == "retained content"))
    );
    let replay = listen::scan(&fixture.root, listen::Dialect::Omp, None).unwrap();
    assert_eq!(replay.records.len(), 2);
    for ((report, replayed), mut expected) in
        reports.iter().zip(&replay.records).zip([session, prose])
    {
        expected.push(b'\n');
        assert_eq!(fixture.observer.tape.read(report.locus).unwrap(), expected);
        assert_eq!(replayed.locus, report.locus);
        let listen::RecordPayload::Events(events) = &replayed.payload else {
            panic!("reassembled content became residue");
        };
        assert_eq!(
            events.iter().map(|event| &event.event).collect::<Vec<_>>(),
            report.decoded.events().iter().collect::<Vec<_>>()
        );
    }
}

#[test]
fn eof_with_a_missing_chunk_has_neither_tape_record_nor_session_declaration() {
    let fixture = Fixture::new(listen::Dialect::Omp);
    let session = large(serde_json::json!({"type":"response", "command":"get_state",
        "success":true, "data":{"sessionId":"never-declared"}}));
    fixture.chunks(&session, true);
    assert!(fixture.observer.finish().is_err());
    assert!(fixture.reports.0.lock().unwrap().is_empty());
    assert!(
        listen::scan(&fixture.root, listen::Dialect::Omp, None)
            .unwrap()
            .records
            .is_empty()
    );
}

#[test]
fn chunked_partials_are_still_restated_and_never_replayed_twice() {
    let fixture = Fixture::new(listen::Dialect::Omp);
    fixture.chunks(&large(serde_json::json!({"type":"message_update"})), false);
    let complete = b"{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"whole\"}]}}\n";
    fixture
        .observer
        .receive(complete, RecordBoundary::Complete)
        .unwrap();
    fixture.observer.finish().unwrap();
    let replay = listen::scan(&fixture.root, listen::Dialect::Omp, None).unwrap();
    assert_eq!(replay.records.len(), 1);
    assert_eq!(
        fixture.observer.tape.read(replay.records[0].locus).unwrap(),
        complete
    );
}

#[test]
fn terminal_omp_text_update_is_retained_as_non_presented_prose_evidence() {
    let fixture = Fixture::new(listen::Dialect::Omp);
    let evidence = br#"{"type":"message_update","assistantMessageEvent":{"type":"text_end","content":"ACK 1kQnrzNZ: Applied."},"message":{"role":"assistant","content":[{"type":"text","text":"ACK 1kQnrzNZ: Applied."}]}}
"#;
    let settled = br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Acknowledged the direction."},{"type":"toolCall","id":"call-1","name":"read","arguments":{"path":"Cargo.toml"}}],"stopReason":"toolUse"}}
"#;

    fixture
        .observer
        .receive(evidence, RecordBoundary::Complete)
        .expect("terminal text update is captured");
    fixture
        .observer
        .receive(settled, RecordBoundary::Complete)
        .expect("settled message is captured");

    let replay =
        listen::scan(&fixture.root, listen::Dialect::Omp, None).expect("retained update replays");
    assert_eq!(replay.records.len(), 2);
    let listen::RecordPayload::Events(events) = &replay.records[0].payload else {
        panic!("terminal text update became residue");
    };
    assert!(matches!(
        events.as_slice(),
        [listen::SourcedEvent {
            event: listen::Event::ProseEvidence(prose),
            ..
        }] if prose.text == "ACK 1kQnrzNZ: Applied."
    ));
}

#[test]
fn non_omp_capture_preserves_terminal_and_truncated_boundaries() {
    let fixture = Fixture::new(listen::Dialect::Codex);
    fixture
        .observer
        .receive(b"{\"type\":", RecordBoundary::Truncated)
        .unwrap();
    let terminal = b"{\"type\":\"thread.started\",\"thread_id\":\"codex-session\"}";
    fixture
        .observer
        .receive(terminal, RecordBoundary::Terminal)
        .unwrap();
    fixture.observer.finish().unwrap();
    let reports = fixture.reports.0.lock().unwrap();
    assert_eq!(reports[0].boundary, RecordBoundary::Truncated);
    assert_eq!(reports[1].boundary, RecordBoundary::Terminal);
    assert_eq!(
        reports[1].declaration.as_ref().unwrap().session.as_str(),
        "codex-session"
    );
    assert_eq!(
        fixture.observer.tape.read(reports[1].locus).unwrap(),
        terminal
    );
}
