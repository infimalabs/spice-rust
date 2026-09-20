//! Durable selected-record capture beneath one supervised lane occupant.

use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use journal::crossing::CanonicalUuid;
use listen::{Decoded, Locus};
use sill::{
    Observation, ObservationFailure, ObservationSpecError, ObserveError, ObservedRecord,
    OutputStream, RecordBoundary, RecordObserver,
};
use tape::Tape;
use vendor::{Record, RecordStream};

#[cfg(test)]
mod tests;

/// Allow two durable tee syncs and scheduler contention without treating the
/// process-exit drain window as a per-record I/O budget. Thirty seconds is a
/// fail-closed bound for a wedged sink, not a delay on healthy delivery.
const SINK_DELIVERY: Duration = Duration::from_secs(30);

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

    /// Nonblocking attribution while record delivery is stalled.
    fn stage(&self) -> &'static str {
        "notice"
    }

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
    let transport = (record.dialect() == listen::Dialect::Omp)
        .then(|| Mutex::new(vendor::omp::frame::FrameDecoder::default()));
    Observation::new(
        stream,
        frame_limit,
        queue_capacity,
        SINK_DELIVERY,
        Arc::new(LaneObserver {
            lane,
            occupant,
            record,
            tape,
            reporter,
            stage: AtomicU8::new(DeliveryStage::Decode as u8),
            transport,
        }),
    )
}

#[repr(u8)]
enum DeliveryStage {
    Tee,
    Decode,
    Report,
}

struct LaneObserver {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    record: Record,
    tape: Tape,
    reporter: Arc<dyn CaptureReporter>,
    stage: AtomicU8,
    transport: Option<Mutex<vendor::omp::frame::FrameDecoder>>,
}

impl RecordObserver for LaneObserver {
    fn observe(&self, observed: ObservedRecord<'_>) -> Result<(), ObserveError> {
        self.receive(observed.bytes(), observed.boundary())
    }

    fn finish(&self) -> Result<(), ObserveError> {
        if let Some(transport) = &self.transport {
            transport
                .lock()
                .map_err(|_| ObserveError::new("OMP transport lock poisoned"))?
                .finish()
                .map_err(|error| ObserveError::new(error.to_string()))?;
        }
        Ok(())
    }

    fn stage(&self) -> &'static str {
        match self.stage.load(Ordering::Relaxed) {
            stage if stage == DeliveryStage::Tee as u8 => "tee sync",
            stage if stage == DeliveryStage::Decode as u8 => "decode",
            _ => self.reporter.stage(),
        }
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

impl LaneObserver {
    fn receive(&self, bytes: &[u8], boundary: RecordBoundary) -> Result<(), ObserveError> {
        self.stage
            .store(DeliveryStage::Decode as u8, Ordering::Relaxed);
        let bytes = if let Some(transport) = &self.transport {
            let Some(bytes) = transport
                .lock()
                .map_err(|_| ObserveError::new("OMP transport lock poisoned"))?
                .push(bytes)
                .map_err(|error| ObserveError::new(error.to_string()))?
            else {
                return Ok(());
            };
            bytes
        } else {
            Cow::Borrowed(bytes)
        };
        let boundary = if matches!(bytes, Cow::Owned(_)) {
            RecordBoundary::Complete
        } else {
            boundary
        };
        self.capture(&bytes, boundary)
    }

    fn capture(&self, bytes: &[u8], boundary: RecordBoundary) -> Result<(), ObserveError> {
        if self.record.restated(self.record.stream(), bytes) {
            // What this partial carries arrives again, whole, in the frame
            // that ends it; keeping every partial grows the tape by the
            // square of what the occupant said.
            return Ok(());
        }
        self.stage
            .store(DeliveryStage::Tee as u8, Ordering::Relaxed);
        let locus = self
            .tape
            .tee(bytes)
            .map_err(|error| ObserveError::new(format!("tape refused record: {error}")))?;
        if boundary == RecordBoundary::Truncated {
            // A truncated head carries no newline of its own; delimiting it on
            // the tape keeps every later record newline-addressable.
            self.tape
                .tee(b"\n")
                .map_err(|error| ObserveError::new(format!("tape refused delimiter: {error}")))?;
        }
        self.stage
            .store(DeliveryStage::Decode as u8, Ordering::Relaxed);
        let decoded = self
            .record
            .decode(self.record.stream(), bytes, locus)
            .map_err(|error| ObserveError::new(format!("decode contract failed: {error}")))?
            .expect("the selected record stream always decodes");
        let declaration = self
            .record
            .session_declaration(&decoded)
            .map_err(|error| ObserveError::new(format!("session declaration refused: {error}")))?;
        self.stage
            .store(DeliveryStage::Report as u8, Ordering::Relaxed);
        self.reporter
            .record(CaptureRecord {
                lane: self.lane,
                occupant: self.occupant,
                stream: self.record.stream(),
                boundary,
                locus,
                declaration,
                decoded,
            })
            .map_err(|error| ObserveError::new(format!("record reporter refused: {error}")))
    }
}
