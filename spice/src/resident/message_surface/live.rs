//! Disposable presentation of durable tape, independent of authority mutation.
//!
//! Notices are wake-ups. The same decoder and projector serve this read-only
//! path and journal catch-up; their shared position makes overlapping delivery
//! idempotent without losing records whose individual notices never arrived.

use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

use super::*;

type Wake = (CanonicalUuid, CanonicalUuid);

#[derive(Clone)]
pub(in crate::resident) struct LiveMessages {
    worker: Arc<Worker>,
    state: Arc<Mutex<MessageState>>,
}

struct Worker {
    wake: Option<mpsc::Sender<Wake>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.wake.take();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            eprintln!("canvas live message worker panicked");
        }
    }
}

struct LiveReader {
    journal: journal::JournalReader,
    paths: RuntimePaths,
    epoch: CanonicalUuid,
    publisher: canvas::stream::MessagePublisher,
}

impl LiveMessages {
    pub(in crate::resident) fn new(
        paths: &RuntimePaths,
        epoch: CanonicalUuid,
        publisher: canvas::stream::MessagePublisher,
        state: MessageState,
    ) -> CommandResult<Self> {
        let journal = journal::JournalReader::open(&paths.journal).map_err(|error| {
            store_fault("canvas::message-reader-unavailable", &error.to_string())
        })?;
        let reader = LiveReader {
            journal,
            paths: paths.clone(),
            epoch,
            publisher,
        };
        let state = Arc::new(Mutex::new(state));
        let presentation = Arc::clone(&state);
        let (wake, records) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("spice-live-messages".to_owned())
            .spawn(move || {
                while let Ok(first) = records.recv() {
                    let mut batch = vec![first];
                    let mut seen = BTreeSet::from([first]);
                    for record in records
                        .try_iter()
                        .take(super::super::capture::NOTICE_BATCH - 1)
                    {
                        if seen.insert(record) {
                            batch.push(record);
                        }
                    }
                    for (lane, occupant) in batch {
                        if let Err(error) = reader.present(lane, occupant, &presentation) {
                            eprintln!("canvas live message unavailable: {error:?}");
                        }
                    }
                }
            })
            .map_err(|error| {
                super::super::environment_fault(
                    "canvas::message-worker-unavailable",
                    &error.to_string(),
                )
            })?;
        Ok(Self {
            worker: Arc::new(Worker {
                wake: Some(wake),
                thread: Some(thread),
            }),
            state,
        })
    }

    pub(in crate::resident) fn state(&self) -> &Mutex<MessageState> {
        &self.state
    }

    pub(in crate::resident) fn notice(
        &self,
        notice: &super::super::supervisor::Notice,
    ) -> CommandResult<()> {
        use super::super::supervisor::Notice;
        match notice {
            Notice::Record { lane, occupant, .. }
            | Notice::Exited { lane, occupant }
            | Notice::Failed { lane, occupant, .. } => self.wake(*lane, *occupant),
            Notice::Asked { .. } => Ok(()),
        }
    }

    pub(in crate::resident) fn wake(
        &self,
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
    ) -> CommandResult<()> {
        self.worker
            .wake
            .as_ref()
            .expect("the live owner retains its sender")
            .send((lane, occupant))
            .map_err(|error| store_fault("canvas::message-worker-unavailable", &error.to_string()))
    }
}

impl LiveReader {
    fn present(
        &self,
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        state: &Mutex<MessageState>,
    ) -> CommandResult<()> {
        let head = self
            .journal
            .read(|read| read.residency_head())
            .map_err(|error| {
                store_fault("canvas::message-residency-unavailable", &error.to_string())
            })?;
        let Some(head) = head.filter(|head| head.body().epoch == self.epoch) else {
            return Ok(());
        };
        let Some(row) = head
            .body()
            .lanes
            .iter()
            .find(|row| row.lane == lane && row.occupant == Some(occupant))
        else {
            return Ok(());
        };
        let Some(vendor) = ingest_vendor(&row.vendor) else {
            return Ok(());
        };
        // A successor starts at its own durable session declaration, never at
        // the predecessor's unaccounted tail. Before any declaration, only a
        // lane's first occupant can own a tape beginning at the origin.
        let admitted_start = match row.session.as_ref() {
            Some(session) if session.occupant == occupant => listen::Position {
                member: session.locus.member,
                offset: session.locus.offset,
            },
            _ if row.lineage.as_slice() == [occupant] => listen::Position::start(),
            _ => return Ok(()),
        };
        let root = self
            .paths
            .tape
            .join(lane.to_string())
            .join(row.vendor.as_str());
        // An ending may precede the first selected record and therefore have no tape.
        if !root.exists() {
            return Ok(());
        }
        let source = tape::Tape::open_existing(&root)
            .and_then(|tape| tape.source_id())
            .map_err(|error| store_fault("canvas::message-tape-unavailable", &error.to_string()))?;
        let identity = source
            .as_str()
            .parse::<CanonicalUuid>()
            .map_err(|error| store_fault("canvas::message-source-invalid", &error.to_string()))?;
        let mut state = lock_state(state)?;
        let checkpoint = listen::Checkpoint {
            source,
            next: state
                .presented
                .get(&(lane, occupant, Some(identity)))
                .copied()
                .map_or(admitted_start, |position| position.max(admitted_start)),
        };
        let scan = listen::scan(&root, vendor.dialect(), Some(&checkpoint))
            .map_err(|error| store_fault("canvas::message-tape-unavailable", &error.to_string()))?;
        // The tape scan is outside the writer. Revalidate its exact lane
        // authority after taking the immutable record set, so a renewal
        // during the scan cannot attribute the successor's bytes to this seat.
        let current = self
            .journal
            .read(|read| read.residency_head())
            .map_err(|error| {
                store_fault("canvas::message-residency-unavailable", &error.to_string())
            })?;
        if !current.is_some_and(|head| {
            head.body().epoch == self.epoch
                && head.body().lanes.iter().any(|current| current == row)
        }) {
            return Ok(());
        }
        let observed = wire::Instant::new(jiff::Timestamp::now())
            .map_err(|error| store_fault("canvas::message-clock-invalid", &error.to_string()))?;
        let worktree = row
            .session
            .as_ref()
            .filter(|session| session.occupant == occupant)
            .map(|session| Path::new(&session.worktree));
        for record in scan.records {
            let next = record.next.next;
            if let Some(record) =
                sweep::scanned_capture_record(lane, occupant, vendor::RecordStream::Stdout, record)
            {
                let directives = self.directives(&record)?;
                publish_agent_messages_in(
                    &self.publisher,
                    &record,
                    &mut state,
                    directives.as_ref(),
                    worktree,
                    Some(identity),
                    Some(observed),
                )?;
            }
            // Complete residue also advances this disposable view, never the
            // journal's independent, guarded accounting checkpoint.
            state
                .presented
                .insert((lane, occupant, Some(identity)), next);
        }
        Ok(())
    }

    fn directives(
        &self,
        record: &lane::capture::CaptureRecord,
    ) -> CommandResult<Option<PresentationDirectives>> {
        let keys = steering_keys(
            record
                .decoded
                .events()
                .iter()
                .filter_map(|event| match event {
                    listen::Event::Prose(prose) if prose.role == listen::ProseRole::Assistant => {
                        Some(prose.text.as_str())
                    }
                    _ => None,
                }),
        );
        if keys.is_empty() {
            return Ok(None);
        }
        let rows = self
            .journal
            .read(|read| {
                keys.iter()
                    .filter_map(|key| {
                        bus::SteeringPresentation::read(read, &key.to_string()).transpose()
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(|error| {
                store_fault("canvas::message-steering-unavailable", &error.to_string())
            })?;
        PresentationDirectives::from_presentations(&rows)
            .map(Some)
            .map_err(|error| store_fault("canvas::message-steering-invalid", &error))
    }
}
