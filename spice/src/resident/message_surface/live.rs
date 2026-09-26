//! Event-loop-owned presentation of durable tape after compaction reconciliation.
//!
//! Capture callbacks only enqueue wake identities. The resident loop drains one
//! cohort, reconciles session compactions, and then publishes every projected
//! message as one transcript generation.

use std::sync::Arc;

use super::*;

pub(in crate::resident) type Wake = (CanonicalUuid, CanonicalUuid);

#[derive(Clone)]
pub(in crate::resident) struct LiveMessages {
    wake: tokio::sync::mpsc::UnboundedSender<Wake>,
    state: Arc<Mutex<MessageState>>,
}

pub(in crate::resident) struct LiveMessageWakes {
    wakes: tokio::sync::mpsc::UnboundedReceiver<Wake>,
    reader: LiveReader,
}

struct LiveReader {
    journal: journal::JournalReader,
    paths: RuntimePaths,
    epoch: CanonicalUuid,
}

impl LiveMessages {
    pub(in crate::resident) fn new(
        paths: &RuntimePaths,
        epoch: CanonicalUuid,
        state: MessageState,
    ) -> CommandResult<(Self, LiveMessageWakes)> {
        let journal = journal::JournalReader::open(&paths.journal).map_err(|error| {
            store_fault("canvas::message-reader-unavailable", &error.to_string())
        })?;
        let (wake, wakes) = tokio::sync::mpsc::unbounded_channel();
        let messages = Self {
            wake,
            state: Arc::new(Mutex::new(state)),
        };
        let wakes = LiveMessageWakes {
            wakes,
            reader: LiveReader {
                journal,
                paths: paths.clone(),
                epoch,
            },
        };
        Ok((messages, wakes))
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
        self.wake
            .send((lane, occupant))
            .map_err(|error| store_fault("canvas::message-worker-unavailable", &error.to_string()))
    }
}

impl LiveMessageWakes {
    pub(in crate::resident) async fn recv(&mut self) -> Option<Vec<Wake>> {
        let first = self.wakes.recv().await?;
        let mut wakes = vec![first];
        let mut seen = BTreeSet::from([first]);
        while wakes.len() < super::super::capture::NOTICE_BATCH {
            let Ok(wake) = self.wakes.try_recv() else {
                break;
            };
            if seen.insert(wake) {
                wakes.push(wake);
            }
        }
        Some(wakes)
    }

    pub(in crate::resident) fn present(
        &self,
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        state: &Mutex<MessageState>,
        publisher: &impl AgentMessagePublisher,
    ) -> CommandResult<()> {
        self.reader.present(lane, occupant, state, publisher)
    }
}

impl LiveReader {
    fn present(
        &self,
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        state: &Mutex<MessageState>,
        publisher: &impl AgentMessagePublisher,
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
                    publisher,
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
