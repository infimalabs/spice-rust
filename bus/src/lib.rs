//! Sole resident writer, durable commit notifications, and armed timers.

#![forbid(unsafe_code)]

pub mod timer;

#[cfg(not(target_arch = "wasm32"))]
mod runtime {
    use std::error::Error;
    use std::fmt;
    use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
    use std::thread::{self, JoinHandle};

    use journal::{
        CommitFault, FactSnapshot, GuardError, GuardRefusal, Journal, JournalHead, JournalWrite,
        SnapshotError, TaskHeads, Verification, VerifyError,
    };

    const REQUEST_CAPACITY: usize = 64;
    const BROADCAST_CAPACITY: usize = 64;

    /// One successful journal transaction, constructable only by the bus
    /// writer after durability.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct JournalCommit {
        first: u64,
        last: u64,
        cursor: fold::Cursor,
    }

    impl JournalCommit {
        fn from_applied(applied: journal::Applied) -> Self {
            Self::from_range(applied.first(), applied.last())
        }

        fn from_range(first: journal::crossing::Seq, last: journal::crossing::Seq) -> Self {
            Self {
                first: first.get(),
                last: last.get(),
                cursor: fold::Cursor::at(last),
            }
        }

        /// First fact position committed by this transaction.
        #[must_use]
        pub const fn first(&self) -> u64 {
            self.first
        }

        /// Last fact position committed by this transaction.
        #[must_use]
        pub const fn last(&self) -> u64 {
            self.last
        }

        /// Monotone fold cursor made visible by this commit.
        #[must_use]
        pub const fn cursor(&self) -> fold::Cursor {
            self.cursor
        }
    }

    /// Typed result of one submitted guarded transaction.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum CommitOutcome {
        /// Every effect is durable and the cursor may be observed.
        Committed(JournalCommit),
        /// A precondition or proposal shape refused with zero writes.
        Refused(GuardRefusal),
        /// Storage or encoding faulted and rolled the transaction back.
        Fault(CommitFault),
    }

    /// Result and optional durable range from one sole-writer transaction.
    pub struct JournalTransaction<T> {
        result: T,
        committed: Option<(journal::crossing::Seq, journal::crossing::Seq)>,
    }

    impl<T> JournalTransaction<T> {
        /// A transaction that made no durable change.
        pub const fn settled(result: T) -> Self {
            Self {
                result,
                committed: None,
            }
        }

        /// A transaction that committed the exact supplied journal range.
        pub const fn committed(
            result: T,
            first: journal::crossing::Seq,
            last: journal::crossing::Seq,
        ) -> Self {
            Self {
                result,
                committed: Some((first, last)),
            }
        }
    }

    /// One queued, immutable view of the heads needed by board requests.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct TaskSnapshot {
        cursor: Option<journal::crossing::Seq>,
        facts: FactSnapshot,
        tasks: Vec<TaskHeads>,
        residency: Option<JournalHead<journal::crossing::ResidencyHead>>,
    }

    /// Facts, trust heads, and residency observed inside one immutable authority snapshot.
    ///
    /// Canvas views consume this instead of issuing independent reads whose
    /// cursors could disagree. The cursor, stream, and heads are all captured
    /// by the writer thread after every earlier queued transaction settled.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct CanvasSnapshot {
        cursor: Option<journal::crossing::Seq>,
        facts: FactSnapshot,
        trust: Vec<JournalHead<journal::crossing::TrustHead>>,
        residency: Option<JournalHead<journal::crossing::ResidencyHead>>,
    }

    impl CanvasSnapshot {
        /// Last durable position visible to the complete capture.
        #[must_use]
        pub const fn cursor(&self) -> Option<journal::crossing::Seq> {
            self.cursor
        }

        /// Complete fact stream at this barrier.
        #[must_use]
        pub const fn facts(&self) -> &FactSnapshot {
            &self.facts
        }

        /// Current trust heads at this barrier, ordered by digest.
        #[must_use]
        pub fn trust(&self) -> &[JournalHead<journal::crossing::TrustHead>] {
            &self.trust
        }

        /// Current resident and lane head at this same barrier.
        #[must_use]
        pub const fn residency(&self) -> Option<&JournalHead<journal::crossing::ResidencyHead>> {
            self.residency.as_ref()
        }
    }

    impl TaskSnapshot {
        #[must_use]
        pub const fn cursor(&self) -> Option<journal::crossing::Seq> {
            self.cursor
        }

        /// Complete fact stream captured at the same board barrier.
        #[must_use]
        pub const fn facts(&self) -> &FactSnapshot {
            &self.facts
        }

        #[must_use]
        pub fn tasks(&self) -> &[TaskHeads] {
            &self.tasks
        }

        #[must_use]
        pub fn residency(&self) -> Option<&JournalHead<journal::crossing::ResidencyHead>> {
            self.residency.as_ref()
        }
    }

    /// Receiver for post-durability commit broadcasts.
    #[derive(Debug)]
    pub struct CommitSubscriber {
        receiver: Receiver<JournalCommit>,
    }

    /// Async, bounded cursor broadcast for generation-resident consumers.
    ///
    /// Tokio's broadcast receiver names lag explicitly. A consumer that falls
    /// behind never receives a silently repaired sequence: it observes
    /// [`tokio::sync::broadcast::error::RecvError::Lagged`] and can resume from
    /// its own last admitted cursor.
    #[derive(Debug)]
    pub struct BroadcastSubscriber {
        receiver: tokio::sync::broadcast::Receiver<JournalCommit>,
    }

    impl BroadcastSubscriber {
        /// Wait for the next durable commit or a typed closed/lagged boundary.
        pub async fn recv(
            &mut self,
        ) -> Result<JournalCommit, tokio::sync::broadcast::error::RecvError> {
            self.receiver.recv().await
        }
    }

    /// Cloneable read/broadcast door into the one resident-owned bus.
    ///
    /// Cloning this handle does not create another journal connection or
    /// writer. Every snapshot request is serialized through the same queue as
    /// commits, and every subscription listens to that writer's one bounded
    /// post-durability broadcast.
    #[derive(Clone, Debug)]
    pub struct BusReader {
        requests: SyncSender<Request>,
        broadcasts: tokio::sync::broadcast::Sender<JournalCommit>,
    }

    impl BusReader {
        /// Capture facts and trust heads at one queued authority barrier.
        pub fn canvas_snapshot(&self) -> Result<CanvasSnapshot, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Canvas { answer })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
        }

        /// Subscribe to the writer's bounded post-durability cursor broadcast.
        #[must_use]
        pub fn subscribe(&self) -> BroadcastSubscriber {
            BroadcastSubscriber {
                receiver: self.broadcasts.subscribe(),
            }
        }
    }

    impl CommitSubscriber {
        /// Wait for the next durable commit notification.
        pub fn recv(&self) -> Result<JournalCommit, mpsc::RecvError> {
            self.receiver.recv()
        }

        /// Observe a pending notification without waiting.
        pub fn try_recv(&self) -> Result<JournalCommit, TryRecvError> {
            self.receiver.try_recv()
        }
    }

    /// Handle to the resident's bounded writer queue.
    #[derive(Debug)]
    pub struct Bus {
        requests: SyncSender<Request>,
        broadcasts: tokio::sync::broadcast::Sender<JournalCommit>,
        writer: Option<JoinHandle<()>>,
    }

    impl Bus {
        /// Move the only admitted writer connection onto one dedicated thread.
        ///
        /// # Errors
        ///
        /// Returns [`BusError::Start`] when the writer thread cannot be
        /// created.
        pub fn start(journal: Journal) -> Result<Self, BusError> {
            let (requests, receiver) = mpsc::sync_channel(REQUEST_CAPACITY);
            let (broadcasts, _) = tokio::sync::broadcast::channel(BROADCAST_CAPACITY);
            let writer_broadcasts = broadcasts.clone();
            let writer = thread::Builder::new()
                .name("spice-journal-writer".to_owned())
                .spawn(move || run_writer(journal, receiver, writer_broadcasts))
                .map_err(BusError::Start)?;
            Ok(Self {
                requests,
                broadcasts,
                writer: Some(writer),
            })
        }

        /// Clone a read/broadcast door into this exact bus generation.
        #[must_use]
        pub fn reader(&self) -> BusReader {
            BusReader {
                requests: self.requests.clone(),
                broadcasts: self.broadcasts.clone(),
            }
        }

        /// Submit one guarded transaction to the dedicated writer.
        pub fn commit(&self, write: JournalWrite) -> Result<CommitOutcome, BusError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Commit {
                    write: Box::new(write),
                    answer,
                })
                .map_err(|_| BusError::Closed)?;
            receiver.recv().map_err(|_| BusError::Closed)
        }

        /// Run one domain transaction through the sole journal writer.
        pub fn transact<T, F>(&self, transaction: F) -> Result<T, BusError>
        where
            T: Send + 'static,
            F: FnOnce(&mut Journal) -> JournalTransaction<T> + Send + 'static,
        {
            let (answer, receiver) = mpsc::sync_channel(1);
            let transaction = Box::new(move |journal: &mut Journal| {
                let transaction = transaction(journal);
                let commit = transaction
                    .committed
                    .map(|(first, last)| JournalCommit::from_range(first, last));
                let respond = Box::new(move || {
                    let _ = answer.send(transaction.result);
                });
                TransactionCompletion { commit, respond }
            });
            self.requests
                .send(Request::Transaction { transaction })
                .map_err(|_| BusError::Closed)?;
            receiver.recv().map_err(|_| BusError::Closed)
        }

        /// Subscribe to commits that become durable after registration.
        pub fn subscribe(&self) -> Result<CommitSubscriber, BusError> {
            let (commits, receiver) = mpsc::channel();
            let (ready, registered) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Subscribe { commits, ready })
                .map_err(|_| BusError::Closed)?;
            registered.recv().map_err(|_| BusError::Closed)?;
            Ok(CommitSubscriber { receiver })
        }

        /// Read the fact stream through the writer queue, after all earlier
        /// submissions have settled.
        pub fn fact_snapshot(&self) -> Result<FactSnapshot, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Snapshot { answer })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
                .map_err(BusReadError::Snapshot)
        }

        /// Read board facts, task heads, and residency together at one barrier.
        pub fn task_snapshot(&self) -> Result<TaskSnapshot, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Tasks { answer })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
        }

        /// Capture the canvas fact and trust planes at one authority barrier.
        pub fn canvas_snapshot(&self) -> Result<CanvasSnapshot, BusReadError> {
            self.reader().canvas_snapshot()
        }

        /// Read the current resident and lane head through the writer queue.
        pub fn residency_head(
            &self,
        ) -> Result<Option<JournalHead<journal::crossing::ResidencyHead>>, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Residency { answer })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
                .map_err(BusReadError::Head)
        }

        /// Read one executable trust head through the writer queue.
        pub fn trust_head(
            &self,
            digest: &journal::crossing::Digest,
        ) -> Result<Option<JournalHead<journal::crossing::TrustHead>>, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Trust {
                    digest: digest.clone(),
                    answer,
                })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
                .map_err(BusReadError::Head)
        }

        /// Read boundaries for the actively claimed lanes among `lanes`.
        ///
        /// One submission rather than a boundary read per lane, so every lane in
        /// a membership transition is measured at the same barrier. Reading them
        /// separately would let live capture advance between two lanes of the
        /// same move and record a split that never existed at any instant.
        pub fn lane_source_boundaries(
            &self,
            lanes: &[journal::crossing::CanonicalUuid],
        ) -> Result<Vec<journal::crossing::body::LaneSourceBoundary>, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::LaneSourceBoundaries {
                    lanes: lanes.to_vec(),
                    answer,
                })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
                .map_err(BusReadError::Head)
        }

        /// Read one lane's accounted transcript boundary through the writer queue.
        ///
        /// Going through the queue is what makes the boundary journal-authoritative
        /// rather than merely recent: it settles behind every earlier submission,
        /// so a lifecycle fact records a position no already-committed ingestion
        /// has moved past.
        pub fn source_boundary(
            &self,
            lane: &journal::crossing::CanonicalUuid,
        ) -> Result<journal::crossing::body::SourceBoundary, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::SourceBoundary {
                    lane: *lane,
                    answer,
                })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
                .map_err(BusReadError::Head)
        }

        /// Verify authority through the writer queue, after all earlier
        /// submissions have settled.
        pub fn verify(&self) -> Result<Verification, BusVerifyError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Verify { answer })
                .map_err(|_| BusVerifyError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusVerifyError::Bus(BusError::Closed))?
                .map_err(BusVerifyError::Journal)
        }

        /// Stop the writer after every request before the shutdown message.
        pub fn shutdown(mut self) -> Result<(), BusError> {
            self.stop()
        }

        fn stop(&mut self) -> Result<(), BusError> {
            let Some(writer) = self.writer.take() else {
                return Ok(());
            };
            let (answer, receiver) = mpsc::sync_channel(1);
            let sent = self.requests.send(Request::Shutdown { answer }).is_ok();
            let acknowledged = sent && receiver.recv().is_ok();
            writer.join().map_err(|_| BusError::WriterPanicked)?;
            if acknowledged {
                Ok(())
            } else {
                Err(BusError::Closed)
            }
        }
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.stop();
        }
    }

    enum Request {
        Commit {
            write: Box<JournalWrite>,
            answer: SyncSender<CommitOutcome>,
        },
        Transaction {
            transaction: Box<dyn FnOnce(&mut Journal) -> TransactionCompletion + Send>,
        },
        Subscribe {
            commits: mpsc::Sender<JournalCommit>,
            ready: SyncSender<()>,
        },
        Snapshot {
            answer: SyncSender<Result<FactSnapshot, SnapshotError>>,
        },
        Tasks {
            answer: SyncSender<Result<TaskSnapshot, BusReadError>>,
        },
        Canvas {
            answer: SyncSender<Result<CanvasSnapshot, BusReadError>>,
        },
        Residency {
            answer: SyncSender<
                Result<Option<JournalHead<journal::crossing::ResidencyHead>>, GuardError>,
            >,
        },
        Trust {
            digest: journal::crossing::Digest,
            answer:
                SyncSender<Result<Option<JournalHead<journal::crossing::TrustHead>>, GuardError>>,
        },
        SourceBoundary {
            lane: journal::crossing::CanonicalUuid,
            answer: SyncSender<Result<journal::crossing::body::SourceBoundary, GuardError>>,
        },
        LaneSourceBoundaries {
            lanes: Vec<journal::crossing::CanonicalUuid>,
            answer:
                SyncSender<Result<Vec<journal::crossing::body::LaneSourceBoundary>, GuardError>>,
        },
        Verify {
            answer: SyncSender<Result<Verification, VerifyError>>,
        },
        Shutdown {
            answer: SyncSender<()>,
        },
    }

    fn run_writer(
        mut journal: Journal,
        requests: Receiver<Request>,
        broadcasts: tokio::sync::broadcast::Sender<JournalCommit>,
    ) {
        let mut subscribers = Vec::<mpsc::Sender<JournalCommit>>::new();
        while let Ok(request) = requests.recv() {
            match request {
                Request::Commit { write, answer } => {
                    let outcome = match journal.apply(*write) {
                        Ok(applied) => {
                            let commit = JournalCommit::from_applied(applied);
                            subscribers.retain(|subscriber| subscriber.send(commit).is_ok());
                            let _ = broadcasts.send(commit);
                            CommitOutcome::Committed(commit)
                        }
                        Err(journal::CommitError::Refused(refusal)) => {
                            CommitOutcome::Refused(refusal)
                        }
                        Err(journal::CommitError::Fault(fault)) => CommitOutcome::Fault(fault),
                    };
                    let _ = answer.send(outcome);
                }
                Request::Transaction { transaction } => {
                    let TransactionCompletion { commit, respond } = transaction(&mut journal);
                    if let Some(commit) = commit {
                        subscribers.retain(|subscriber| subscriber.send(commit).is_ok());
                        let _ = broadcasts.send(commit);
                    }
                    respond();
                }
                Request::Subscribe { commits, ready } => {
                    subscribers.push(commits);
                    let _ = ready.send(());
                }
                Request::Snapshot { answer } => {
                    let _ = answer.send(journal.fact_snapshot());
                }
                Request::Tasks { answer } => {
                    let snapshot = journal
                        .read_with(|read| {
                            Ok(TaskSnapshot {
                                cursor: read.cursor().map_err(BusReadError::Head)?,
                                facts: read.fact_snapshot().map_err(BusReadError::Snapshot)?,
                                tasks: read.task_heads().map_err(BusReadError::Head)?,
                                residency: read.residency_head().map_err(BusReadError::Head)?,
                            })
                        })
                        .map_err(|error| match error {
                            journal::ReadError::Journal(error) => BusReadError::Head(error),
                            journal::ReadError::Answer(error) => error,
                        });
                    let _ = answer.send(snapshot);
                }
                Request::Canvas { answer } => {
                    let snapshot = journal
                        .read_with(|read| {
                            let cursor = read.cursor().map_err(BusReadError::Head)?;
                            let facts = read.fact_snapshot().map_err(BusReadError::Snapshot)?;
                            let trust = read.trust_heads().map_err(BusReadError::Head)?;
                            let residency = read.residency_head().map_err(BusReadError::Head)?;
                            Ok(CanvasSnapshot {
                                cursor,
                                facts,
                                trust,
                                residency,
                            })
                        })
                        .map_err(|error| match error {
                            journal::ReadError::Journal(error) => BusReadError::Head(error),
                            journal::ReadError::Answer(error) => error,
                        });
                    let _ = answer.send(snapshot);
                }
                Request::Residency { answer } => {
                    let _ = answer.send(journal.read(|read| read.residency_head()));
                }
                Request::Trust { digest, answer } => {
                    let _ = answer.send(journal.read(|read| read.trust_head(&digest)));
                }
                Request::SourceBoundary { lane, answer } => {
                    let _ = answer.send(journal.read(|read| read.source_boundary(&lane)));
                }
                Request::LaneSourceBoundaries { lanes, answer } => {
                    let _ = answer.send(journal.read(|read| read.lane_source_boundaries(&lanes)));
                }
                Request::Verify { answer } => {
                    let _ = answer.send(journal.verify());
                }
                Request::Shutdown { answer } => {
                    let _ = answer.send(());
                    break;
                }
            }
        }
    }

    struct TransactionCompletion {
        commit: Option<JournalCommit>,
        respond: Box<dyn FnOnce() + Send>,
    }

    /// The dedicated writer could not be reached or did not end normally.
    #[derive(Debug)]
    pub enum BusError {
        /// The operating system refused the writer thread.
        Start(std::io::Error),
        /// The writer queue is closed.
        Closed,
        /// The writer thread panicked while owning authority.
        WriterPanicked,
    }

    impl fmt::Display for BusError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Start(error) => write!(formatter, "bus writer did not start: {error}"),
                Self::Closed => formatter.write_str("bus writer is closed"),
                Self::WriterPanicked => formatter.write_str("bus writer panicked"),
            }
        }
    }

    impl Error for BusError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            match self {
                Self::Start(error) => Some(error),
                Self::Closed | Self::WriterPanicked => None,
            }
        }
    }

    /// A queued fact snapshot could not be returned.
    #[derive(Debug)]
    pub enum BusReadError {
        Bus(BusError),
        Snapshot(SnapshotError),
        Head(GuardError),
    }

    impl fmt::Display for BusReadError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Bus(error) => error.fmt(formatter),
                Self::Snapshot(error) => error.fmt(formatter),
                Self::Head(error) => error.fmt(formatter),
            }
        }
    }

    impl Error for BusReadError {}

    /// A queued authority verification could not be returned.
    #[derive(Debug)]
    pub enum BusVerifyError {
        Bus(BusError),
        Journal(VerifyError),
    }

    impl fmt::Display for BusVerifyError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Bus(error) => error.fmt(formatter),
                Self::Journal(error) => error.fmt(formatter),
            }
        }
    }

    impl Error for BusVerifyError {}
}

#[cfg(not(target_arch = "wasm32"))]
pub use runtime::{
    BroadcastSubscriber, Bus, BusError, BusReadError, BusReader, BusVerifyError, CanvasSnapshot,
    CommitOutcome, CommitSubscriber, JournalCommit, JournalTransaction, TaskSnapshot,
};
