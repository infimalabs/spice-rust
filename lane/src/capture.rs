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
        let locus = self
            .tape
            .tee(bytes)
            .map_err(|error| ObserveError::new(format!("tape refused record: {error}")))?;
        let decoded = self
            .record
            .decode(self.record.stream(), bytes, locus)
            .map_err(|error| ObserveError::new(format!("decode contract failed: {error}")))?
            .expect("the selected record stream always decodes");
        self.reporter
            .record(CaptureRecord {
                lane: self.lane,
                occupant: self.occupant,
                stream: self.record.stream(),
                boundary: observed.boundary(),
                locus,
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
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{CaptureFailure, CaptureRecord, CaptureReportError, CaptureReporter, observation};
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
        assert!(matches!(records[0].decoded, Decoded::Events(_)));
        assert!(matches!(
            &records[1].decoded,
            Decoded::Residue(residue) if residue.kind == ResidueKind::Truncated
        ));
        assert!(reports.failures.lock().expect("failures lock").is_empty());
        drop(records);
        fs::remove_dir_all(root).expect("capture fixture cleanup");
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
                Duration::from_millis(100),
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
            fast_mode: false,
            attribution: Attribution::Harness,
            hook: SteeringHook {
                command: "true".to_owned(),
            },
            steering_key: None,
            mcp: McpServer {
                name: "fixture".to_owned(),
                command: "true".to_owned(),
                args: Vec::new(),
            },
            resume: Resume::Fresh,
        }
    }

    fn uuid(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
            .parse()
            .expect("fixture UUID")
    }
}
