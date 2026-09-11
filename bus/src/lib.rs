//! Sole resident writer, durable commit notifications, and armed timers.

#![forbid(unsafe_code)]

pub mod timer;

#[cfg(not(target_arch = "wasm32"))]
mod runtime {
    use std::error::Error;
    use std::fmt;
    use std::sync::Arc;
    use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
    use std::thread::{self, JoinHandle};

    use journal::{
        CommitFault, FactSnapshot, GuardError, GuardRefusal, Journal, JournalHead, JournalWrite,
        SnapshotError, TaskHeads, Verification, VerifyError,
    };

    const REQUEST_CAPACITY: usize = 64;
    const BROADCAST_CAPACITY: usize = 64;
    type FactPage =
        journal::crossing::Packed<journal::crossing::Envelope<journal::crossing::AnyFact>>;
    type DirectionHeads = Vec<JournalHead<journal::crossing::SteeringHead>>;

    mod task_snapshot;
    pub use task_snapshot::TaskFacts;
    use task_snapshot::{load_task_snapshot, refresh_task_snapshot};

    /// One successful journal transaction, constructable only by the bus
    /// writer after durability.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct JournalCommit {
        first: u64,
        last: u64,
        cursor: journal::crossing::JournalCursor,
    }

    impl JournalCommit {
        fn from_applied(applied: journal::Applied) -> Self {
            Self::from_range(applied.first(), applied.last())
        }

        fn from_range(first: journal::crossing::Seq, last: journal::crossing::Seq) -> Self {
            Self {
                first: first.get(),
                last: last.get(),
                cursor: journal::crossing::JournalCursor::at(last),
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

        /// Monotone journal cursor made visible by this commit.
        #[must_use]
        pub const fn cursor(&self) -> journal::crossing::JournalCursor {
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
        facts: TaskFacts,
        tasks: Arc<[TaskHeads]>,
        trust: Arc<[JournalHead<journal::crossing::TrustHead>]>,
        residency: Option<JournalHead<journal::crossing::ResidencyHead>>,
        operator_steering: Arc<[JournalHead<journal::crossing::SteeringHead>]>,
    }

    /// Incremental resident authority and indexed pending steering observed at
    /// one writer-queue barrier.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct SteeringSnapshot {
        authority: TaskSnapshot,
        pending: Vec<JournalHead<journal::crossing::SteeringHead>>,
    }

    /// Cursor and trust heads observed inside one immutable authority snapshot.
    ///
    /// Fact pages are read separately through bounded primary-key ranges at
    /// this cursor, so opening Canvas never materializes the complete stream.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct CanvasHeads {
        cursor: Option<journal::crossing::Seq>,
        trust: Vec<JournalHead<journal::crossing::TrustHead>>,
    }

    /// One exact steering head with the immutable facts that created and most
    /// recently transitioned it.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct SteeringPresentation {
        head: JournalHead<journal::crossing::SteeringHead>,
        created: journal::crossing::Envelope<journal::crossing::AnyFact>,
        current: journal::crossing::Envelope<journal::crossing::AnyFact>,
    }

    impl SteeringPresentation {
        #[must_use]
        pub const fn head(&self) -> &JournalHead<journal::crossing::SteeringHead> {
            &self.head
        }

        #[must_use]
        pub const fn created(&self) -> &journal::crossing::Envelope<journal::crossing::AnyFact> {
            &self.created
        }

        #[must_use]
        pub const fn current(&self) -> &journal::crossing::Envelope<journal::crossing::AnyFact> {
            &self.current
        }
    }

    impl CanvasHeads {
        /// Last durable position visible to this head capture.
        #[must_use]
        pub const fn cursor(&self) -> Option<journal::crossing::Seq> {
            self.cursor
        }

        /// Current trust heads at this barrier, ordered by digest.
        #[must_use]
        pub fn trust(&self) -> &[JournalHead<journal::crossing::TrustHead>] {
            &self.trust
        }
    }

    impl TaskSnapshot {
        #[must_use]
        pub const fn cursor(&self) -> Option<journal::crossing::Seq> {
            self.cursor
        }

        /// Resident authority facts captured through the same board barrier.
        ///
        /// High-volume transcript-ingestion accounting is queried separately
        /// by origin and kind instead of being replayed into this cache.
        #[must_use]
        pub const fn facts(&self) -> &TaskFacts {
            &self.facts
        }

        #[must_use]
        pub fn tasks(&self) -> &[TaskHeads] {
            &self.tasks
        }

        /// Current trust heads captured at the same board barrier.
        #[must_use]
        pub fn trust(&self) -> &[JournalHead<journal::crossing::TrustHead>] {
            &self.trust
        }

        /// Closed environment derived from the heads captured at this barrier.
        #[must_use]
        pub fn environment(&self) -> journal::crossing::Environment {
            journal::crossing::Environment::journal_schema(journal::SCHEMA_VERSION)
                .expect("the compiled journal schema is positive")
                .with_trust_heads(self.trust.iter().map(|head| head.body().clone()))
        }

        #[must_use]
        pub fn residency(&self) -> Option<&JournalHead<journal::crossing::ResidencyHead>> {
            self.residency.as_ref()
        }

        /// Current live directions authored by the operator.
        #[must_use]
        pub fn operator_steering(&self) -> &[JournalHead<journal::crossing::SteeringHead>] {
            &self.operator_steering
        }
    }

    impl SteeringSnapshot {
        /// Incrementally retained facts, residency, and cursor at this barrier.
        #[must_use]
        pub const fn authority(&self) -> &TaskSnapshot {
            &self.authority
        }

        /// Live steering heads selected at the same barrier.
        #[must_use]
        pub fn pending(&self) -> &[JournalHead<journal::crossing::SteeringHead>] {
            &self.pending
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
        /// Read only the current durable cursor at one queued barrier.
        pub fn cursor(&self) -> Result<Option<journal::crossing::Seq>, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Cursor { answer })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
                .map_err(BusReadError::Head)
        }

        /// Read the complete fact stream at one queued barrier.
        ///
        /// This cloneable door exists for resident-owned background
        /// materializers. Interactive request paths should use bounded reads.
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

        /// Read the writer-owned incremental task snapshot at one queued barrier.
        pub fn task_snapshot(&self) -> Result<TaskSnapshot, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Tasks { answer })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
        }

        /// Resolve only the steering keys named by a bounded message window.
        pub fn steering_presentations(
            &self,
            keys: Vec<String>,
        ) -> Result<Vec<SteeringPresentation>, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::SteeringPresentations { keys, answer })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
                .map_err(BusReadError::Head)
        }

        /// Read one bounded newest-first page from a fact kind and origin.
        pub fn recent_origin_facts(
            &self,
            origin: journal::crossing::Origin,
            kind: journal::crossing::FactKind,
            before: Option<journal::crossing::Seq>,
            limit: usize,
        ) -> Result<Vec<journal::crossing::Envelope<journal::crossing::AnyFact>>, BusReadError>
        {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::RecentOriginFacts {
                    origin,
                    kind,
                    before,
                    limit,
                    answer,
                })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
                .map_err(BusReadError::Head)
        }

        /// Capture the Canvas cursor and trust heads at one queued barrier.
        pub fn canvas_heads(&self) -> Result<CanvasHeads, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::CanvasHeads { answer })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
        }

        /// Read one bounded primary-key fact page through a captured horizon.
        pub fn canvas_facts(
            &self,
            start: journal::FactPageStart,
            horizon: journal::crossing::Seq,
            limit: usize,
        ) -> Result<FactPage, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::CanvasFacts {
                    start,
                    horizon,
                    limit,
                    answer,
                })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
                .map_err(BusReadError::Head)
        }

        /// Read newest durable operator directions per lane at one queued barrier.
        pub fn canvas_directions(&self, limit: usize) -> Result<DirectionHeads, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::CanvasDirections { limit, answer })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
                .map_err(BusReadError::Head)
        }

        /// Read one bounded causal page directly through the thread index.
        pub fn canvas_thread(
            &self,
            position: journal::crossing::Seq,
            limit: usize,
        ) -> Result<Option<FactPage>, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::CanvasThread {
                    position,
                    limit,
                    answer,
                })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
                .map_err(BusReadError::Head)
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

        /// Run one bounded indexed read through the sole journal queue.
        ///
        /// The closure receives the same immutable SQLite snapshot used by
        /// journal guards. Unlike [`Self::fact_snapshot`], callers choose exact
        /// head or index queries and therefore do not decode unrelated facts.
        pub fn read<T, F>(&self, query: F) -> Result<T, BusReadError>
        where
            T: Send + 'static,
            F: for<'a> FnOnce(&journal::JournalRead<'a>) -> Result<T, GuardError> + Send + 'static,
        {
            self.transact(move |journal| {
                JournalTransaction::settled(journal.read(query).map_err(BusReadError::Head))
            })
            .map_err(BusReadError::Bus)?
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
            self.reader().fact_snapshot()
        }

        /// Read only the current durable cursor through the writer queue.
        pub fn cursor(&self) -> Result<Option<journal::crossing::Seq>, BusReadError> {
            self.reader().cursor()
        }

        /// Read board facts, task heads, and residency together at one barrier.
        pub fn task_snapshot(&self) -> Result<TaskSnapshot, BusReadError> {
            self.reader().task_snapshot()
        }

        /// Read incremental resident authority and pending steering at one barrier.
        ///
        /// The retained authority shares already-decoded fact chunks across
        /// requests. Interactive inbox polling therefore never rebuilds the
        /// complete journal merely to recover a lane's context anchor.
        pub fn steering_snapshot(
            &self,
            lane: Option<journal::crossing::CanonicalUuid>,
        ) -> Result<SteeringSnapshot, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Steering { lane, answer })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
        }

        /// Capture the canvas fact and trust planes at one authority barrier.
        pub fn canvas_heads(&self) -> Result<CanvasHeads, BusReadError> {
            self.reader().canvas_heads()
        }

        /// Read one bounded primary-key fact page through a captured horizon.
        pub fn canvas_facts(
            &self,
            start: journal::FactPageStart,
            horizon: journal::crossing::Seq,
            limit: usize,
        ) -> Result<FactPage, BusReadError> {
            self.reader().canvas_facts(start, horizon, limit)
        }

        /// Read newest durable operator directions per lane through the writer queue.
        pub fn canvas_directions(&self, limit: usize) -> Result<DirectionHeads, BusReadError> {
            self.reader().canvas_directions(limit)
        }

        /// Read one bounded causal page directly through the thread index.
        pub fn canvas_thread(
            &self,
            position: journal::crossing::Seq,
            limit: usize,
        ) -> Result<Option<FactPage>, BusReadError> {
            self.reader().canvas_thread(position, limit)
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
        Cursor {
            answer: SyncSender<Result<Option<journal::crossing::Seq>, GuardError>>,
        },
        Tasks {
            answer: SyncSender<Result<TaskSnapshot, BusReadError>>,
        },
        Steering {
            lane: Option<journal::crossing::CanonicalUuid>,
            answer: SyncSender<Result<SteeringSnapshot, BusReadError>>,
        },
        SteeringPresentations {
            keys: Vec<String>,
            answer: SyncSender<Result<Vec<SteeringPresentation>, GuardError>>,
        },
        RecentOriginFacts {
            origin: journal::crossing::Origin,
            kind: journal::crossing::FactKind,
            before: Option<journal::crossing::Seq>,
            limit: usize,
            answer: SyncSender<
                Result<Vec<journal::crossing::Envelope<journal::crossing::AnyFact>>, GuardError>,
            >,
        },
        CanvasHeads {
            answer: SyncSender<Result<CanvasHeads, BusReadError>>,
        },
        CanvasFacts {
            start: journal::FactPageStart,
            horizon: journal::crossing::Seq,
            limit: usize,
            answer: SyncSender<Result<FactPage, GuardError>>,
        },
        CanvasDirections {
            limit: usize,
            answer: SyncSender<Result<DirectionHeads, GuardError>>,
        },
        CanvasThread {
            position: journal::crossing::Seq,
            limit: usize,
            answer: SyncSender<Result<Option<FactPage>, GuardError>>,
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
        let mut task_snapshot = None;
        while let Ok(request) = requests.recv() {
            match request {
                Request::Commit { write, answer } => {
                    answer_commit(
                        &mut journal,
                        &mut task_snapshot,
                        &mut subscribers,
                        &broadcasts,
                        *write,
                        answer,
                    );
                }
                Request::Transaction { transaction } => {
                    answer_transaction(
                        &mut journal,
                        &mut task_snapshot,
                        &mut subscribers,
                        &broadcasts,
                        transaction,
                    );
                }
                Request::Subscribe { commits, ready } => {
                    subscribers.push(commits);
                    let _ = ready.send(());
                }
                Request::Snapshot { answer } => {
                    let _ = answer.send(journal.fact_snapshot());
                }
                Request::Cursor { answer } => {
                    let _ = answer.send(journal.read(|read| read.cursor()));
                }
                Request::Tasks { answer } => {
                    answer_tasks(&journal, &mut task_snapshot, answer);
                }
                Request::Steering { lane, answer } => {
                    let snapshot =
                        current_task_snapshot(&journal, &mut task_snapshot).and_then(|authority| {
                            journal
                                .read(|read| read.pending_steering(lane))
                                .map_err(BusReadError::Head)
                                .map(|pending| SteeringSnapshot { authority, pending })
                        });
                    let _ = answer.send(snapshot);
                }
                Request::SteeringPresentations { keys, answer } => {
                    let _ = answer.send(steering_presentations(&journal, keys));
                }
                Request::RecentOriginFacts {
                    origin,
                    kind,
                    before,
                    limit,
                    answer,
                } => {
                    let _ =
                        answer.send(journal.read(move |read| {
                            read.recent_origin_facts(&origin, kind, before, limit)
                        }));
                }
                Request::CanvasHeads { answer } => {
                    let _ = answer.send(canvas_heads(&journal));
                }
                Request::CanvasFacts {
                    start,
                    horizon,
                    limit,
                    answer,
                } => {
                    let _ = answer.send(journal.read(|read| read.fact_page(start, horizon, limit)));
                }
                Request::CanvasDirections { limit, answer } => {
                    let _ = answer.send(journal.read(|read| read.operator_steering(limit)));
                }
                Request::CanvasThread {
                    position,
                    limit,
                    answer,
                } => {
                    let _ = answer.send(journal.read(|read| read.causal_page(position, limit)));
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

    fn steering_presentations(
        journal: &Journal,
        keys: Vec<String>,
    ) -> Result<Vec<SteeringPresentation>, GuardError> {
        journal.read(move |read| {
            let mut presentations = Vec::new();
            for key in keys {
                let Some(head) = read.steering_key(&key)? else {
                    continue;
                };
                let created = read
                    .fact_page(
                        journal::FactPageStart::At(head.body().created),
                        head.body().created,
                        1,
                    )?
                    .rows
                    .into_iter()
                    .next()
                    .ok_or_else(|| {
                        GuardError::Invalid(format!("steering key {key} has no creating fact"))
                    })?;
                let current = read
                    .fact_page(
                        journal::FactPageStart::At(head.position()),
                        head.position(),
                        1,
                    )?
                    .rows
                    .into_iter()
                    .next()
                    .ok_or_else(|| {
                        GuardError::Invalid(format!("steering key {key} has no current fact"))
                    })?;
                presentations.push(SteeringPresentation {
                    head,
                    created,
                    current,
                });
            }
            Ok(presentations)
        })
    }

    fn answer_commit(
        journal: &mut Journal,
        task_snapshot: &mut Option<TaskSnapshot>,
        subscribers: &mut Vec<mpsc::Sender<JournalCommit>>,
        broadcasts: &tokio::sync::broadcast::Sender<JournalCommit>,
        write: JournalWrite,
        answer: SyncSender<CommitOutcome>,
    ) {
        let outcome = match journal.apply(write) {
            Ok(applied) => {
                let commit = JournalCommit::from_applied(applied);
                refresh_task_snapshot(journal, task_snapshot, commit);
                publish_commit(subscribers, broadcasts, commit);
                CommitOutcome::Committed(commit)
            }
            Err(journal::CommitError::Refused(refusal)) => CommitOutcome::Refused(refusal),
            Err(journal::CommitError::Fault(fault)) => CommitOutcome::Fault(fault),
        };
        let _ = answer.send(outcome);
    }

    fn answer_transaction(
        journal: &mut Journal,
        task_snapshot: &mut Option<TaskSnapshot>,
        subscribers: &mut Vec<mpsc::Sender<JournalCommit>>,
        broadcasts: &tokio::sync::broadcast::Sender<JournalCommit>,
        transaction: Box<dyn FnOnce(&mut Journal) -> TransactionCompletion + Send>,
    ) {
        let TransactionCompletion { commit, respond } = transaction(journal);
        if let Some(commit) = commit {
            refresh_task_snapshot(journal, task_snapshot, commit);
            publish_commit(subscribers, broadcasts, commit);
        }
        respond();
    }

    fn publish_commit(
        subscribers: &mut Vec<mpsc::Sender<JournalCommit>>,
        broadcasts: &tokio::sync::broadcast::Sender<JournalCommit>,
        commit: JournalCommit,
    ) {
        subscribers.retain(|subscriber| subscriber.send(commit).is_ok());
        let _ = broadcasts.send(commit);
    }

    fn answer_tasks(
        journal: &Journal,
        cached: &mut Option<TaskSnapshot>,
        answer: SyncSender<Result<TaskSnapshot, BusReadError>>,
    ) {
        let snapshot = current_task_snapshot(journal, cached);
        let _ = answer.send(snapshot);
    }

    fn current_task_snapshot(
        journal: &Journal,
        cached: &mut Option<TaskSnapshot>,
    ) -> Result<TaskSnapshot, BusReadError> {
        let snapshot = cached
            .clone()
            .map_or_else(|| load_task_snapshot(journal), Ok);
        if let Ok(snapshot) = &snapshot {
            *cached = Some(snapshot.clone());
        }
        snapshot
    }

    fn canvas_heads(journal: &Journal) -> Result<CanvasHeads, BusReadError> {
        journal
            .read_with(|read| {
                let cursor = read.cursor().map_err(BusReadError::Head)?;
                let trust = read.trust_heads().map_err(BusReadError::Head)?;
                Ok(CanvasHeads { cursor, trust })
            })
            .map_err(|error| match error {
                journal::ReadError::Journal(error) => BusReadError::Head(error),
                journal::ReadError::Answer(error) => error,
            })
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
    BroadcastSubscriber, Bus, BusError, BusReadError, BusReader, BusVerifyError, CanvasHeads,
    CommitOutcome, CommitSubscriber, JournalCommit, JournalTransaction, SteeringPresentation,
    SteeringSnapshot, TaskFacts, TaskSnapshot,
};
