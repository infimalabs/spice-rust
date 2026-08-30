//! Bounded bridge from lane capture workers into the resident event loop.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use lane::capture::{
    CaptureExit, CaptureFailure, CaptureRecord, CaptureReportError, CaptureReporter,
};
use tokio::sync::mpsc;

const RECORD_CAPACITY: usize = 64;
const NOTICE_BYTES: usize = 4 * 1024;

/// Resident-generation endpoint for tiny supervisor tape notices.
pub(super) struct SupervisorReports {
    socket: tokio::net::UnixDatagram,
    path: PathBuf,
    buffer: [u8; NOTICE_BYTES],
}

impl SupervisorReports {
    pub(super) fn bind(paths: &super::RuntimePaths) -> super::CommandResult<Self> {
        #[cfg(not(unix))]
        {
            let _ = paths;
            return Err(super::environment_fault(
                "lane::supervisor-capture-unsupported",
                "detached supervisor capture requires Unix datagrams",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};

            let path = super::supervisor::capture_path(paths);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_socket() => {
                    fs::remove_file(&path).map_err(|error| {
                        super::environment_fault(
                            "lane::supervisor-capture-unavailable",
                            &error.to_string(),
                        )
                    })?;
                }
                Ok(_) => {
                    return Err(super::environment_fault(
                        "lane::supervisor-capture-invalid",
                        &format!("{} is not a Unix socket", path.display()),
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(super::environment_fault(
                        "lane::supervisor-capture-unavailable",
                        &error.to_string(),
                    ));
                }
            }
            let socket = tokio::net::UnixDatagram::bind(&path).map_err(|error| {
                super::environment_fault("lane::supervisor-capture-unavailable", &error.to_string())
            })?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|error| {
                super::environment_fault("lane::supervisor-capture-unavailable", &error.to_string())
            })?;
            Ok(Self {
                socket,
                path,
                buffer: [0; NOTICE_BYTES],
            })
        }
    }

    pub(super) async fn recv(&mut self) -> io::Result<super::supervisor::Notice> {
        let received = self.socket.recv(&mut self.buffer).await?;
        serde_json::from_slice(&self.buffer[..received])
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

impl Drop for SupervisorReports {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) fn materialize(
    paths: &super::RuntimePaths,
    residency: &wire::ResidencyHead,
    notice: super::supervisor::Notice,
) -> super::CommandResult<Option<CaptureRecord>> {
    let super::supervisor::Notice::Record {
        lane,
        occupant,
        stream,
        boundary,
        locus,
    } = notice
    else {
        return Ok(None);
    };
    let Some(row) = residency
        .lanes
        .iter()
        .find(|row| row.lane == lane && row.occupant == Some(occupant))
    else {
        return Ok(None);
    };
    let Some(vendor) = super::ingest_vendor(&row.vendor) else {
        return Ok(None);
    };
    let stream = match stream.as_str() {
        "stdout" => vendor::RecordStream::Stdout,
        "stderr" => vendor::RecordStream::Stderr,
        other => {
            return Err(super::store_fault(
                "lane::supervisor-capture-invalid",
                &format!("unknown record stream {other:?}"),
            ));
        }
    };
    let boundary = match boundary.as_str() {
        "complete" => sill::RecordBoundary::Complete,
        "terminal" => sill::RecordBoundary::Terminal,
        "truncated" => sill::RecordBoundary::Truncated,
        other => {
            return Err(super::store_fault(
                "lane::supervisor-capture-invalid",
                &format!("unknown record boundary {other:?}"),
            ));
        }
    };
    let locus = tape::Locus::new(locus.member, locus.offset, locus.length);
    let tape =
        tape::Tape::open_existing(paths.tape.join(lane.to_string()).join(row.vendor.as_str()))
            .map_err(|error| {
                super::store_fault("lane::supervisor-capture-unavailable", &error.to_string())
            })?;
    let bytes = tape.read(locus).map_err(|error| {
        super::store_fault("lane::supervisor-capture-unavailable", &error.to_string())
    })?;
    let decoded = listen::decode(vendor.dialect(), &bytes, locus).map_err(|error| {
        super::store_fault("lane::supervisor-capture-invalid", &error.to_string())
    })?;
    Ok(Some(CaptureRecord {
        lane,
        occupant,
        stream,
        boundary,
        locus,
        declaration: None,
        decoded,
    }))
}

pub(super) struct Channels {
    pub(super) reporter: Arc<dyn CaptureReporter>,
    pub(super) records: mpsc::Receiver<CaptureRecord>,
    pub(super) failures: mpsc::UnboundedReceiver<CaptureFailure>,
    pub(super) exits: mpsc::UnboundedReceiver<CaptureExit>,
}

pub(super) fn channels() -> Channels {
    let (records, record_receiver) = mpsc::channel(RECORD_CAPACITY);
    let (failures, failure_receiver) = mpsc::unbounded_channel();
    let (exits, exit_receiver) = mpsc::unbounded_channel();
    Channels {
        reporter: Arc::new(Reporter {
            records,
            failures,
            exits,
        }),
        records: record_receiver,
        failures: failure_receiver,
        exits: exit_receiver,
    }
}

struct Reporter {
    records: mpsc::Sender<CaptureRecord>,
    failures: mpsc::UnboundedSender<CaptureFailure>,
    exits: mpsc::UnboundedSender<CaptureExit>,
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

    fn exited(&self, exit: CaptureExit) {
        let _sent = self.exits.send(exit);
    }
}
