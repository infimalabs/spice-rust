//! Bounded bridge from lane capture workers into the resident event loop.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use lane::capture::{
    CaptureExit, CaptureFailure, CaptureRecord, CaptureReportError, CaptureReporter,
};
use tokio::sync::mpsc;
use wire::CanonicalUuid;

use super::supervisor::Notice;

const RECORD_CAPACITY: usize = 64;
const NOTICE_BYTES: usize = 4 * 1024;
/// Kernel queue the notice socket keeps while the resident is mid-transaction.
///
/// A supervisor sends one datagram per tape record and cannot wait: when the
/// queue is full the kernel refuses the send outright. Darwin's default receive
/// space is 4 KiB, seventeen record notices, and one journal commit outlasts
/// that on a busy lane, so most notices were refused before this queue grew.
const NOTICE_QUEUE_BYTES: usize = 1024 * 1024;
/// Notices one loop turn takes from the drained queue before it sweeps.
const NOTICE_BATCH: usize = 1024;

/// Resident-generation endpoint for tiny supervisor tape notices.
///
/// The socket is read by its own task so the kernel queue empties whenever the
/// event loop yields at all, not only when the loop reaches the notice arm.
/// Everything read waits in an unbounded in-process queue until the loop takes
/// a batch, so a burst that arrives during one journal commit is never lost.
pub(super) struct SupervisorReports {
    notices: mpsc::UnboundedReceiver<io::Result<Notice>>,
    drain: tokio::task::JoinHandle<()>,
    path: PathBuf,
}

impl SupervisorReports {
    /// Bind the notice door and start draining it. Requires a live runtime.
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
            socket2::SockRef::from(&socket)
                .set_recv_buffer_size(NOTICE_QUEUE_BYTES)
                .map_err(|error| {
                    super::environment_fault(
                        "lane::supervisor-capture-unavailable",
                        &error.to_string(),
                    )
                })?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|error| {
                super::environment_fault("lane::supervisor-capture-unavailable", &error.to_string())
            })?;
            let (sender, notices) = mpsc::unbounded_channel();
            let drain = tokio::spawn(drain_notices(socket, sender));
            Ok(Self {
                notices,
                drain,
                path,
            })
        }
    }

    /// The next notice, once one has been read off the socket.
    pub(super) async fn recv(&mut self) -> io::Result<Notice> {
        self.notices.recv().await.unwrap_or_else(|| {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the supervisor notice drain ended",
            ))
        })
    }

    /// One received notice plus everything already queued behind it, with the
    /// record notices a later record notice for the same occupant supersedes
    /// left out.
    ///
    /// The sweep a record notice triggers accounts and presents every record
    /// on that lane's tape up to its head, so of a run of record notices for
    /// one occupant only the last one needs to cross. Exit and failure notices
    /// keep their place in the order.
    pub(super) fn batch(&mut self, first: io::Result<Notice>) -> Vec<io::Result<Notice>> {
        let mut notices = vec![first];
        while notices.len() < NOTICE_BATCH {
            match self.notices.try_recv() {
                Ok(notice) => notices.push(notice),
                Err(_) => break,
            }
        }
        coalesce(notices)
    }
}

impl Drop for SupervisorReports {
    fn drop(&mut self) {
        self.drain.abort();
        let _ = fs::remove_file(&self.path);
    }
}

/// Read datagrams for as long as the resident loop keeps its receiver.
async fn drain_notices(
    socket: tokio::net::UnixDatagram,
    notices: mpsc::UnboundedSender<io::Result<Notice>>,
) {
    let mut buffer = vec![0_u8; NOTICE_BYTES];
    loop {
        let notice = match socket.recv(&mut buffer).await {
            Ok(received) => serde_json::from_slice(&buffer[..received])
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
            Err(error) => Err(error),
        };
        if notices.send(notice).is_err() {
            return;
        }
    }
}

/// Keep only the last record notice per occupant, and every other notice.
fn coalesce(notices: Vec<io::Result<Notice>>) -> Vec<io::Result<Notice>> {
    let mut kept = Vec::with_capacity(notices.len());
    let mut occupants: BTreeSet<(CanonicalUuid, CanonicalUuid)> = BTreeSet::new();
    for notice in notices.into_iter().rev() {
        if let Ok(Notice::Record { lane, occupant, .. }) = &notice
            && !occupants.insert((*lane, *occupant))
        {
            continue;
        }
        kept.push(notice);
    }
    kept.reverse();
    kept
}

pub(super) fn materialize(
    paths: &super::RuntimePaths,
    residency: &wire::ResidencyHead,
    notice: Notice,
) -> super::CommandResult<Option<CaptureRecord>> {
    let Notice::Record {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn record(lane: u8, occupant: u8, offset: u64) -> io::Result<Notice> {
        Ok(Notice::Record {
            lane: uuid(lane),
            occupant: uuid(occupant),
            stream: "stdout".to_owned(),
            boundary: "complete".to_owned(),
            locus: wire::TapeLocus {
                member: 1,
                offset,
                length: 1,
            },
        })
    }

    fn uuid(byte: u8) -> CanonicalUuid {
        format!("00000000-0000-4000-8000-0000000000{byte:02x}")
            .parse()
            .expect("fixture uuid")
    }

    fn spelled(notices: &[io::Result<Notice>]) -> Vec<String> {
        notices
            .iter()
            .map(|notice| match notice {
                Ok(Notice::Record {
                    occupant, locus, ..
                }) => {
                    format!("record {occupant} @{}", locus.offset)
                }
                Ok(Notice::Exited { occupant, .. }) => format!("exited {occupant}"),
                Ok(Notice::Failed { occupant, .. }) => format!("failed {occupant}"),
                Err(error) => format!("refused {error}"),
            })
            .collect()
    }

    #[test]
    fn a_batch_keeps_the_last_record_notice_per_occupant_and_every_other_notice() {
        let exited = Ok(Notice::Exited {
            lane: uuid(1),
            occupant: uuid(2),
        });
        let refused = Err(io::Error::new(io::ErrorKind::InvalidData, "garbled"));
        let batch = vec![
            record(1, 2, 0),
            record(1, 2, 10),
            record(3, 4, 0),
            refused,
            record(1, 2, 20),
            exited,
            record(3, 4, 10),
        ];

        assert_eq!(
            spelled(&coalesce(batch)),
            vec![
                "refused garbled".to_owned(),
                format!("record {} @20", uuid(2)),
                format!("exited {}", uuid(2)),
                format!("record {} @10", uuid(4)),
            ]
        );
    }
}
