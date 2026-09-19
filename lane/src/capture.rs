//! Durable selected-record capture beneath one supervised lane occupant.

use std::error::Error;
use std::fmt;
use std::sync::Arc;

use journal::crossing::CanonicalUuid;
use listen::{Decoded, Locus};
use sill::{
    Observation, ObservationFailure, ObservationSpecError, ObserveError, ObservedRecord,
    OutputStream, RecordBoundary, RecordObserver,
};
use tape::Tape;
use vendor::{Record, RecordStream};

/// One selected record committed and decoded under its exact tape locus.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureRecord {
    /// Lane owning the transcript.
    pub lane: CanonicalUuid,
    /// Occupant that emitted the bytes.
    pub occupant: CanonicalUuid,
    /// Stream selected by the vendor contract.
    pub stream: RecordStream,
    /// Newline or terminal boundary closing the record.
    pub boundary: RecordBoundary,
    /// Durable location covering exactly the emitted record bytes.
    pub locus: Locus,
    /// Exact admitted vendor session declaration, when this is the one current
    /// initialization record.
    pub declaration: Option<vendor::SessionDeclaration>,
    /// Typed events or accounted residue decoded after the append.
    pub decoded: Decoded,
}

/// Terminal loss of capture for one supervised occupant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureFailure {
    /// Lane whose transcript path failed.
    pub lane: CanonicalUuid,
    /// Occupant terminated because capture failed.
    pub occupant: CanonicalUuid,
    /// Exact SILL capture failure.
    pub failure: ObservationFailure,
}

/// Kernel-backed child exit notification for one supervised occupant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureExit {
    /// Lane whose child exited.
    pub lane: CanonicalUuid,
    /// Exact occupant observed by the process watcher.
    pub occupant: CanonicalUuid,
}

/// Resident-facing bounded report sink.
pub trait CaptureReporter: Send + Sync + 'static {
    /// Deliver one durable decoded record without blocking the capture worker.
    ///
    /// # Errors
    ///
    /// A refusal terminates the occupant because supervision can no longer
    /// account for the selected record stream.
    fn record(&self, record: CaptureRecord) -> Result<(), CaptureReportError>;

    /// Deliver the single terminal failure after SILL cleanup completes.
    fn failed(&self, failure: CaptureFailure);

    /// Deliver the child process exit without polling its process group.
    fn exited(&self, _exit: CaptureExit) {}
}

/// A resident-facing report channel refused or lost a record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureReportError(String);

impl CaptureReportError {
    /// Name a report refusal.
    #[must_use]
    pub fn new(detail: impl Into<String>) -> Self {
        Self(detail.into())
    }
}

impl fmt::Display for CaptureReportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CaptureReportError {}

/// Build the sole SILL observation contract for one vendor record stream.
///
/// # Errors
///
/// Returns an observation-spec error when either bound is zero.
pub fn observation(
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    record: Record,
    tape: Tape,
    frame_limit: usize,
    queue_capacity: usize,
    reporter: Arc<dyn CaptureReporter>,
) -> Result<Observation, ObservationSpecError> {
    let stream = match record.stream() {
        RecordStream::Stdout => OutputStream::Stdout,
        RecordStream::Stderr => OutputStream::Stderr,
    };
    Observation::new(
        stream,
        frame_limit,
        queue_capacity,
        Arc::new(LaneObserver {
            lane,
            occupant,
            record,
            tape,
            reporter,
        }),
    )
}

struct LaneObserver {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    record: Record,
    tape: Tape,
    reporter: Arc<dyn CaptureReporter>,
}

impl RecordObserver for LaneObserver {
    fn observe(&self, observed: ObservedRecord<'_>) -> Result<(), ObserveError> {
        let bytes = observed.bytes();
        if self.record.restated(self.record.stream(), bytes) {
            // What this partial carries arrives again, whole, in the frame
            // that ends it; keeping every partial grows the tape by the
            // square of what the occupant said.
            return Ok(());
        }
        let locus = self
            .tape
            .tee(bytes)
            .map_err(|error| ObserveError::new(format!("tape refused record: {error}")))?;
        if observed.boundary() == RecordBoundary::Truncated {
            // A truncated head carries no newline of its own; delimiting it on
            // the tape keeps every later record newline-addressable.
            self.tape
                .tee(b"\n")
                .map_err(|error| ObserveError::new(format!("tape refused delimiter: {error}")))?;
        }
        let decoded = self
            .record
            .decode(self.record.stream(), bytes, locus)
            .map_err(|error| ObserveError::new(format!("decode contract failed: {error}")))?
            .expect("the selected record stream always decodes");
        let declaration = self
            .record
            .session_declaration(&decoded)
            .map_err(|error| ObserveError::new(format!("session declaration refused: {error}")))?;
        self.reporter
            .record(CaptureRecord {
                lane: self.lane,
                occupant: self.occupant,
                stream: self.record.stream(),
                boundary: observed.boundary(),
                locus,
                declaration,
                decoded,
            })
            .map_err(|error| ObserveError::new(format!("record reporter refused: {error}")))
    }

    fn failed(&self, failure: &ObservationFailure) {
        self.reporter.failed(CaptureFailure {
            lane: self.lane,
            occupant: self.occupant,
            failure: failure.clone(),
        });
    }

    fn exited(&self) {
        self.reporter.exited(CaptureExit {
            lane: self.lane,
            occupant: self.occupant,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{
        CaptureExit, CaptureFailure, CaptureRecord, CaptureReportError, CaptureReporter,
        observation,
    };
    use journal::crossing::CanonicalUuid;
    use listen::{Decoded, ResidueKind};
    use sill::{
        Argv, Cancellation, Deadlines, Environment, ObservationFailure, Output, ProcessGroup,
        RecordBoundary, Redaction, RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy,
        spawn_observed,
    };
    use tape::Tape;
    use vendor::{Attribution, Effort, Launch, McpServer, Resume, SteeringHook};

    #[derive(Default)]
    struct Reports {
        records: Mutex<Vec<CaptureRecord>>,
        failures: Mutex<Vec<CaptureFailure>>,
        exits: Mutex<Vec<CaptureExit>>,
    }

    impl CaptureReporter for Reports {
        fn record(&self, record: CaptureRecord) -> Result<(), CaptureReportError> {
            self.records
                .lock()
                .expect("record reports lock")
                .push(record);
            Ok(())
        }

        fn failed(&self, failure: CaptureFailure) {
            self.failures
                .lock()
                .expect("failure reports lock")
                .push(failure);
        }

        fn exited(&self, exit: CaptureExit) {
            self.exits.lock().expect("exit reports lock").push(exit);
        }
    }

    #[test]
    fn exact_complete_and_terminal_records_are_taped_before_decode() {
        let lane = uuid(1);
        let occupant = uuid(2);
        let root =
            std::env::temp_dir().join(format!("lane-capture-{lane}-{}-exact", std::process::id()));
        fs::create_dir_all(&root).expect("capture fixture root");
        let tape = Tape::open(root.join("tape")).expect("capture tape opens");
        let record =
            vendor::codex::record("/bin/sh", &launch()).expect("fixture vendor record contract");
        let reports = Arc::new(Reports::default());
        let reporter: Arc<dyn CaptureReporter> = reports.clone();
        let observation = observation(lane, occupant, record, tape.clone(), 1024, 4, reporter)
            .expect("capture observation");
        let complete = concat!(
            r#"{"type":"item.completed","item":{"id":"item-1","#,
            r#""type":"agent_message","text":"working"}}"#,
            "\n",
        )
        .as_bytes();
        let terminal = br#"{"type":"turn.failed""#;
        let mut command = Vec::new();
        command.extend_from_slice(b"printf '");
        command.extend_from_slice(complete);
        command.extend_from_slice(terminal);
        command.extend_from_slice(b"'");
        let spec = fixture_spec(String::from_utf8(command).expect("ASCII fixture"));
        let outcome = spawn_observed(spec, observation)
            .expect("capture child launches")
            .wait()
            .expect("capture child reaps");
        assert!(outcome.completion().status().success());
        // Every observer callback has already run once the drain completed, so
        // the reports below are read rather than waited for. A drain abandoned
        // at the spec deadline would leave them partial, and says so here
        // instead of surfacing as a short record count with no explanation.
        assert!(
            outcome.outputs_drained(),
            "the drain was abandoned with records still outstanding"
        );

        let records = reports.records.lock().expect("record reports lock");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].boundary, RecordBoundary::Complete);
        assert_eq!(records[1].boundary, RecordBoundary::Terminal);
        assert_eq!(
            tape.read(records[0].locus).expect("complete locus"),
            complete
        );
        assert_eq!(
            tape.read(records[1].locus).expect("terminal locus"),
            terminal
        );
        assert!(matches!(records[0].decoded, Decoded::Events { .. }));
        assert!(matches!(
            &records[1].decoded,
            Decoded::Residue(residue) if residue.kind == ResidueKind::Truncated
        ));
        assert!(reports.failures.lock().expect("failures lock").is_empty());
        assert_eq!(
            *reports.exits.lock().expect("exits lock"),
            vec![CaptureExit { lane, occupant }],
        );
        drop(records);
        fs::remove_dir_all(root).expect("capture fixture cleanup");
    }

    #[test]
    fn a_partial_the_stream_restates_whole_is_left_off_the_tape() {
        let lane = uuid(5);
        let occupant = uuid(6);
        let root = std::env::temp_dir().join(format!(
            "lane-capture-{lane}-{}-restated",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("capture fixture root");
        let tape = Tape::open(root.join("tape")).expect("capture tape opens");
        let mut launch = launch();
        launch.model_context_window = None;
        launch.config_overlay = Some(root.join("omp.yml"));
        launch.lsp = Some(false);
        let record =
            vendor::omp::record("/bin/sh", &launch).expect("fixture vendor record contract");
        let reports = Arc::new(Reports::default());
        let reporter: Arc<dyn CaptureReporter> = reports.clone();
        let observation = observation(lane, occupant, record, tape.clone(), 1024, 4, reporter)
            .expect("capture observation");
        let partial = concat!(
            r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","#,
            r#""contentIndex":0,"delta":"wor","partial":{"role":"assistant","content":"#,
            r#"[{"type":"text","text":"wor"}]}},"message":{"role":"assistant","content":"#,
            r#"[{"type":"text","text":"wor"}]}}"#,
            "
",
        )
        .as_bytes();
        let whole = concat!(
            r#"{"type":"message_end","message":{"role":"assistant","content":"#,
            r#"[{"type":"text","text":"working"}],"usage":{"input":1,"output":1,"#,
            r#""cacheRead":0,"cacheWrite":0},"stopReason":"stop"}}"#,
            "
",
        )
        .as_bytes();
        let mut command = Vec::new();
        command.extend_from_slice(b"printf '");
        command.extend_from_slice(partial);
        command.extend_from_slice(whole);
        command.extend_from_slice(b"'");
        let spec = fixture_spec(String::from_utf8(command).expect("ASCII fixture"));
        let outcome = spawn_observed(spec, observation)
            .expect("capture child launches")
            .wait()
            .expect("capture child reaps");
        assert!(outcome.completion().status().success());
        assert!(outcome.outputs_drained());

        let records = reports.records.lock().expect("record reports lock");
        assert_eq!(
            records.len(),
            1,
            "the partial is neither taped nor reported"
        );
        assert_eq!(records[0].boundary, RecordBoundary::Complete);
        assert_eq!(
            records[0].locus.offset, 0,
            "the whole record opens the tape"
        );
        assert_eq!(tape.read(records[0].locus).expect("whole locus"), whole);
        assert!(matches!(records[0].decoded, Decoded::Events { .. }));
        drop(records);
        fs::remove_dir_all(root).expect("capture fixture cleanup");
    }

    /// A killed occupant cannot silently shorten the transcript it emitted.
    ///
    /// This is the durability half of the resident's termination grace. An
    /// occupant that outlives the grace is killed, so what decides whether that
    /// is cheap is not the size of the grace but what the tape holds afterwards.
    /// The fixture emits one newline-closed record and one unterminated record
    /// and then refuses to end, so the kill lands with a record still open. Both
    /// survive it, and the open one is closed as `Terminal` rather than dropped,
    /// which is what keeps a kill-truncated capture a named boundary instead of
    /// a short tape indistinguishable from a quiet occupant.
    #[test]
    fn a_killed_occupant_keeps_every_record_it_emitted() {
        let lane = uuid(5);
        let occupant = uuid(6);
        let root =
            std::env::temp_dir().join(format!("lane-capture-{lane}-{}-killed", std::process::id()));
        fs::create_dir_all(&root).expect("capture kill root");
        let tape = Tape::open(root.join("tape")).expect("capture kill tape opens");
        let record =
            vendor::codex::record("/bin/sh", &launch()).expect("fixture vendor record contract");
        let reports = Arc::new(Reports::default());
        let reporter: Arc<dyn CaptureReporter> = reports.clone();
        let observation = observation(lane, occupant, record, tape.clone(), 1024, 4, reporter)
            .expect("capture observation");
        let complete = concat!(
            r#"{"type":"item.completed","item":{"id":"item-1","#,
            r#""type":"agent_message","text":"emitted"}}"#,
            "\n",
        )
        .as_bytes();
        let terminal = br#"{"type":"turn.failed""#;
        let mut command = Vec::new();
        command.extend_from_slice(b"printf '");
        command.extend_from_slice(complete);
        command.extend_from_slice(terminal);
        command.extend_from_slice(b"'; while :; do sleep 1; done");
        let spec = fixture_spec(String::from_utf8(command).expect("ASCII fixture"));
        let running = spawn_observed(spec, observation).expect("capture child launches");

        // The complete record closes on its own newline while the child is
        // still alive, so waiting for it proves the bytes reached the observer
        // before the kill rather than because of it. The open record cannot
        // appear until EOF, and only the kill produces that.
        wait_for_reported_records(&reports, 1);
        running.signal_kill().expect("capture child is killed");
        let outcome = running.wait().expect("capture child reaps");
        assert_eq!(
            outcome.completion().status().code(),
            None,
            "the child ended on a signal rather than an exit of its own"
        );
        assert!(
            outcome.outputs_drained(),
            "the drain was abandoned with records still outstanding"
        );

        let records = reports.records.lock().expect("record reports lock");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].boundary, RecordBoundary::Complete);
        assert_eq!(records[1].boundary, RecordBoundary::Terminal);
        assert_eq!(
            tape.read(records[0].locus).expect("complete locus"),
            complete
        );
        assert_eq!(
            tape.read(records[1].locus).expect("terminal locus"),
            terminal
        );
        drop(records);
        fs::remove_dir_all(root).expect("capture kill cleanup");
    }

    /// An oversized record must not end capture; it survives as bounded residue.
    ///
    /// This is the outage class that motivated `RecordBoundary::Truncated`: one
    /// giant emitted line used to raise a framing failure that ended capture
    /// for the rest of the occupancy. The fixture emits a record past the
    /// frame limit between two ordinary records and proves the bounded head is
    /// taped with its own delimiter, decodes as truncated residue, and the
    /// record after it still decodes as events.
    #[test]
    fn an_oversized_record_survives_as_truncated_residue_and_capture_continues() {
        let lane = uuid(7);
        let occupant = uuid(8);
        let root = std::env::temp_dir().join(format!(
            "lane-capture-{lane}-{}-truncated",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("capture truncation root");
        let tape = Tape::open(root.join("tape")).expect("capture truncation tape opens");
        let record =
            vendor::codex::record("/bin/sh", &launch()).expect("fixture vendor record contract");
        let reports = Arc::new(Reports::default());
        let reporter: Arc<dyn CaptureReporter> = reports.clone();
        let observation = observation(lane, occupant, record, tape.clone(), 1024, 4, reporter)
            .expect("capture observation");
        let complete = concat!(
            r#"{"type":"item.completed","item":{"id":"item-1","#,
            r#""type":"agent_message","text":"before"}}"#,
            "\n",
        )
        .as_bytes();
        let mut oversized = concat!(
            r#"{"type":"item.completed","item":{"id":"item-2","#,
            r#""type":"agent_message","text":""#,
        )
        .as_bytes()
        .to_vec();
        oversized.resize(oversized.len() + 1200, b'x');
        oversized.extend_from_slice(b"\"}}\n");
        let after = concat!(
            r#"{"type":"item.completed","item":{"id":"item-3","#,
            r#""type":"agent_message","text":"after"}}"#,
            "\n",
        )
        .as_bytes();
        let mut command = Vec::new();
        command.extend_from_slice(b"printf '");
        command.extend_from_slice(complete);
        command.extend_from_slice(&oversized);
        command.extend_from_slice(after);
        command.extend_from_slice(b"'");
        let spec = fixture_spec(String::from_utf8(command).expect("ASCII fixture"));
        let outcome = spawn_observed(spec, observation)
            .expect("capture child launches")
            .wait()
            .expect("capture child reaps");
        assert!(outcome.completion().status().success());
        assert!(
            outcome.outputs_drained(),
            "the drain was abandoned with records still outstanding"
        );
        assert!(outcome.observation_failure().is_none());
        let records = reports.records.lock().expect("record reports lock");
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].boundary, RecordBoundary::Complete);
        assert_eq!(records[1].boundary, RecordBoundary::Truncated);
        assert_eq!(records[2].boundary, RecordBoundary::Complete);
        assert_eq!(
            tape.read(records[1].locus).expect("head locus"),
            &oversized[..1024]
        );
        assert!(matches!(
            &records[1].decoded,
            Decoded::Residue(residue) if residue.kind == ResidueKind::Truncated
        ));
        assert!(matches!(records[2].decoded, Decoded::Events { .. }));
        let extents = tape.extents().expect("capture truncation extents");
        assert_eq!(extents.len(), 1);
        let whole = tape
            .read(tape::Locus::new(extents[0].member, 0, extents[0].length))
            .expect("whole extent");
        let expected = [complete, &oversized[..1024], b"\n", after].concat();
        assert_eq!(whole, expected);
        assert!(reports.failures.lock().expect("failures lock").is_empty());
        drop(records);
        fs::remove_dir_all(root).expect("capture truncation cleanup");
    }

    /// Block until the observer has reported `count` records.
    ///
    /// The bound is a failure threshold rather than a wait: a healthy observer
    /// has already reported by the first look, and only one that has genuinely
    /// stopped reaches the end, where it says so instead of letting the kill
    /// race the bytes it is meant to follow.
    fn wait_for_reported_records(reports: &Arc<Reports>, count: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if reports.records.lock().expect("record reports lock").len() >= count {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the observer reported fewer than {count} records before the fixture bound");
    }

    #[test]
    fn tape_refusal_terminates_and_reports_the_occupant() {
        let lane = uuid(3);
        let occupant = uuid(4);
        let root = std::env::temp_dir().join(format!(
            "lane-capture-{lane}-{}-refusal",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("capture refusal root");
        let tape = Tape::open(root.join("tape")).expect("capture refusal tape opens");
        fs::create_dir(tape.root().join("000000000001.plain"))
            .expect("fixture obstructs the active tape member");
        let record =
            vendor::codex::record("/bin/sh", &launch()).expect("fixture vendor record contract");
        let reports = Arc::new(Reports::default());
        let reporter: Arc<dyn CaptureReporter> = reports.clone();
        let observation = observation(lane, occupant, record, tape, 1024, 4, reporter)
            .expect("capture refusal observation");
        let running = spawn_observed(
            fixture_spec(
                "printf '{\"type\":\"item.completed\"}\\n'; exec /usr/bin/tail -f /dev/null"
                    .to_owned(),
            ),
            observation,
        )
        .expect("capture refusal child launches");
        let outcome = running.wait().expect("capture refusal child reaps");
        assert!(
            matches!(
                outcome.observation_failure(),
                Some(ObservationFailure::SinkRefused { detail, .. })
                    if detail.contains("tape refused record")
                        && detail.contains("append verbatim bytes")
            ),
            "unexpected observation outcome: {:?}",
            outcome.observation_failure(),
        );
        let failures = reports.failures.lock().expect("failure reports lock");
        assert_eq!(
            (failures.len(), failures[0].lane, failures[0].occupant),
            (1, lane, occupant)
        );
        assert!(
            reports
                .records
                .lock()
                .expect("record reports lock")
                .is_empty()
        );
        drop(failures);
        fs::remove_dir_all(root).expect("capture refusal cleanup");
    }

    fn fixture_spec(command: String) -> Spec {
        Spec {
            argv: Argv::new("/bin/sh", ["-c".to_owned(), command]).expect("fixture argv"),
            cwd: std::env::temp_dir(),
            environment: Environment::empty(),
            group: ProcessGroup::New,
            deadlines: Deadlines::new(
                Duration::from_secs(2),
                // The drain deadline is a failure threshold, not a wait. On a
                // healthy run the capture is already complete the first time
                // the wait looks at it, so none of this is spent however large
                // it is, and a capture that is merely late is answered when it
                // finishes rather than when the bound arrives. Only one that
                // has genuinely stopped reaches the bound, so it is sized for
                // a loaded machine: an observer starved by a parallel
                // workspace run is slow, not stopped.
                Duration::from_secs(10),
                Duration::from_millis(100),
            )
            .expect("fixture deadlines"),
            wait: WaitPolicy::new(
                "lane capture test",
                Cancellation::Never,
                RestartDisposition::Never,
            )
            .expect("fixture wait"),
            output: Output::new(
                StdinPolicy::Null,
                StreamPolicy::Capture { limit: 1024 },
                StreamPolicy::Capture { limit: 1024 },
            ),
            redaction: Redaction::All,
            grace: Duration::from_millis(20),
        }
    }

    fn launch() -> Launch {
        Launch {
            cwd: std::env::temp_dir(),
            prompt: "fixture".to_owned(),
            model: "fixture".to_owned(),
            effort: Effort::High,
            personality: None,
            fast_mode: Some(false),
            model_context_window: Some(1_050_000),
            auto_compact_window_tokens: Some(272_000),
            config_overlay: None,
            lsp: None,
            attribution: Attribution::Harness,
            hook: SteeringHook {
                command: "true".to_owned(),
            },
            steering_key: None,
            mcp: Some(McpServer {
                name: "fixture".to_owned(),
                command: "true".to_owned(),
                args: Vec::new(),
            }),
            resume: Resume::Fresh,
        }
    }

    fn uuid(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
            .parse()
            .expect("fixture UUID")
    }
}
