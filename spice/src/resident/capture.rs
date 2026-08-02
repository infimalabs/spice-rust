//! Bounded bridge from lane capture workers into the resident event loop.

use std::sync::Arc;

use lane::capture::{CaptureFailure, CaptureRecord, CaptureReportError, CaptureReporter};
use tokio::sync::mpsc;

const RECORD_CAPACITY: usize = 64;

pub(super) struct Channels {
    pub(super) reporter: Arc<dyn CaptureReporter>,
    pub(super) records: mpsc::Receiver<CaptureRecord>,
    pub(super) failures: mpsc::UnboundedReceiver<CaptureFailure>,
}

pub(super) fn channels() -> Channels {
    let (records, record_receiver) = mpsc::channel(RECORD_CAPACITY);
    let (failures, failure_receiver) = mpsc::unbounded_channel();
    Channels {
        reporter: Arc::new(Reporter { records, failures }),
        records: record_receiver,
        failures: failure_receiver,
    }
}

struct Reporter {
    records: mpsc::Sender<CaptureRecord>,
    failures: mpsc::UnboundedSender<CaptureFailure>,
}

impl CaptureReporter for Reporter {
    fn record(&self, record: CaptureRecord) -> Result<(), CaptureReportError> {
        self.records.try_send(record).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => {
                CaptureReportError::new("resident capture report queue is full")
            }
            mpsc::error::TrySendError::Closed(_) => {
                CaptureReportError::new("resident capture report receiver is closed")
            }
        })
    }

    fn failed(&self, failure: CaptureFailure) {
        let _sent = self.failures.send(failure);
    }
}
