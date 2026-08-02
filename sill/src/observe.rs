use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// Child output stream selected for structured observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum OutputStream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

impl fmt::Display for OutputStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        })
    }
}

/// Why an observed record was delivered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordBoundary {
    /// A newline completed the record. The delivered bytes include that newline.
    Complete,
    /// End of stream closed a final record without a newline.
    Terminal,
}

/// One byte-exact selected-stream record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservedRecord<'a> {
    bytes: &'a [u8],
    boundary: RecordBoundary,
}

impl<'a> ObservedRecord<'a> {
    #[cfg(unix)]
    pub(crate) const fn new(bytes: &'a [u8], boundary: RecordBoundary) -> Self {
        Self { bytes, boundary }
    }

    /// Exact child bytes, including the newline for a complete record.
    #[must_use]
    pub const fn bytes(self) -> &'a [u8] {
        self.bytes
    }

    /// Delimiter or terminal boundary that closed these bytes.
    #[must_use]
    pub const fn boundary(self) -> RecordBoundary {
        self.boundary
    }
}

/// A selected-record sink refused one record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObserveError {
    detail: String,
}

impl ObserveError {
    /// Name a sink refusal without exposing a sink-specific error type to SILL.
    #[must_use]
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }

    /// Stable diagnostic supplied by the sink.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for ObserveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl Error for ObserveError {}

/// Terminal failure of the selected-stream observation path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObservationFailure {
    /// The selected child pipe could not be read.
    Reader {
        stream: OutputStream,
        detail: String,
    },
    /// One newline-delimited record exceeded its admitted memory bound.
    FramingOverflow {
        stream: OutputStream,
        limit: usize,
        observed: usize,
    },
    /// The bounded handoff queue filled before the sink accepted prior records.
    SinkBackpressure {
        stream: OutputStream,
        capacity: usize,
        deadline: Duration,
    },
    /// The selected-record sink refused durable handling or interpretation.
    SinkRefused {
        stream: OutputStream,
        detail: String,
    },
    /// The sink worker or callback disappeared unexpectedly.
    ObserverLost { stream: OutputStream },
}

impl ObservationFailure {
    /// Selected stream whose evidence path failed.
    #[must_use]
    pub const fn stream(&self) -> OutputStream {
        match self {
            Self::Reader { stream, .. }
            | Self::FramingOverflow { stream, .. }
            | Self::SinkBackpressure { stream, .. }
            | Self::SinkRefused { stream, .. }
            | Self::ObserverLost { stream } => *stream,
        }
    }
}

impl fmt::Display for ObservationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reader { stream, detail } => {
                write!(formatter, "reading selected {stream} failed: {detail}")
            }
            Self::FramingOverflow {
                stream,
                limit,
                observed,
            } => write!(
                formatter,
                "selected {stream} record exceeded its {limit}-byte frame limit at {observed} bytes"
            ),
            Self::SinkBackpressure {
                stream,
                capacity,
                deadline,
            } => write!(
                formatter,
                concat!(
                    "selected {} observer exceeded its {}-record queue ",
                    "or {:?} delivery deadline"
                ),
                stream, capacity, deadline,
            ),
            Self::SinkRefused { stream, detail } => {
                write!(
                    formatter,
                    "selected {stream} observer refused a record: {detail}"
                )
            }
            Self::ObserverLost { stream } => {
                write!(formatter, "selected {stream} observer was lost")
            }
        }
    }
}

/// Sink for byte-exact selected-stream records.
///
/// `observe` is called by a bounded worker, never by the pipe reader. A blocked
/// sink therefore reaches the process spec's output-drain deadline or fills a
/// finite handoff queue and fails supervision instead of allowing the child
/// pipe or resident memory to grow without bound.
pub trait RecordObserver: Send + Sync + 'static {
    /// Accept one complete or terminal record.
    ///
    /// # Errors
    ///
    /// Returning an error terminates the observed process group.
    fn observe(&self, record: ObservedRecord<'_>) -> Result<(), ObserveError>;

    /// Receive the single terminal observation failure, after bounded cleanup.
    fn failed(&self, _failure: &ObservationFailure) {}
}

/// One bounded selected-stream observation contract.
pub struct Observation {
    stream: OutputStream,
    frame_limit: usize,
    queue_capacity: usize,
    #[cfg_attr(not(unix), allow(dead_code))]
    observer: Arc<dyn RecordObserver>,
}

impl Observation {
    /// Select one captured stream and admit explicit framing and queue bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationSpecError`] when either bound is zero.
    pub fn new(
        stream: OutputStream,
        frame_limit: usize,
        queue_capacity: usize,
        observer: Arc<dyn RecordObserver>,
    ) -> Result<Self, ObservationSpecError> {
        if frame_limit == 0 {
            return Err(ObservationSpecError::ZeroFrameLimit);
        }
        if queue_capacity == 0 {
            return Err(ObservationSpecError::ZeroQueueCapacity);
        }
        Ok(Self {
            stream,
            frame_limit,
            queue_capacity,
            observer,
        })
    }

    #[cfg(unix)]
    pub(crate) const fn stream(&self) -> OutputStream {
        self.stream
    }

    #[cfg(unix)]
    pub(crate) const fn frame_limit(&self) -> usize {
        self.frame_limit
    }

    #[cfg(unix)]
    pub(crate) const fn queue_capacity(&self) -> usize {
        self.queue_capacity
    }

    #[cfg(unix)]
    pub(crate) fn observer(&self) -> Arc<dyn RecordObserver> {
        Arc::clone(&self.observer)
    }
}

impl fmt::Debug for Observation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Observation")
            .field("stream", &self.stream)
            .field("frame_limit", &self.frame_limit)
            .field("queue_capacity", &self.queue_capacity)
            .finish_non_exhaustive()
    }
}

/// Invalid selected-stream observation bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationSpecError {
    /// A record frame must retain at least one byte.
    ZeroFrameLimit,
    /// The reader-to-sink handoff must admit at least one record.
    ZeroQueueCapacity,
}

impl fmt::Display for ObservationSpecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ZeroFrameLimit => "selected-stream frame limit must be nonzero",
            Self::ZeroQueueCapacity => "selected-stream queue capacity must be nonzero",
        })
    }
}

impl Error for ObservationSpecError {}
