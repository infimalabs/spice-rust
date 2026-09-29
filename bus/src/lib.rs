//! Sole resident writer, durable commit notifications, and armed timers.

#![forbid(unsafe_code)]

pub mod timer;

#[cfg(not(target_arch = "wasm32"))]
mod runtime {
    use std::collections::VecDeque;
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
    /// Greatest number of steering keys one presentation request can resolve.
    pub const MAX_STEERING_PRESENTATION_KEYS: usize = 64;
    type FactPage =
        journal::crossing::Packed<journal::crossing::Envelope<journal::crossing::AnyFact>>;

    mod task_snapshot;
    pub use task_snapshot::{TaskFacts, context_opening};
    use task_snapshot::{TaskSnapshotState, advance_task_snapshot, invalidate_task_snapshot};
    mod transcript_sessions;
    use transcript_sessions::TranscriptSessionState;
    pub use transcript_sessions::{TranscriptSession, TranscriptSessions};

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
        materialization_steps: u64,
    }

    /// Incremental resident authority, one bounded pending-steering page, and
    /// an optional recent retired-steering window observed at one
    /// writer-queue barrier.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct SteeringSnapshot {
        authority: TaskSnapshot,
        pending: journal::SteeringPage,
        retired: Vec<journal::RetiredSteering>,
    }

    /// Cursor, trust, and residency heads observed inside one immutable authority snapshot.
    ///
    /// Fact pages are read separately through bounded primary-key ranges at
    /// this cursor, so opening Canvas never materializes the complete stream.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct CanvasHeads {
        cursor: Option<journal::crossing::Seq>,
        trust: Arc<[JournalHead<journal::crossing::TrustHead>]>,
        residency: Option<JournalHead<journal::crossing::ResidencyHead>>,
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
        /// Read one key's presentation context from an already-admitted snapshot.
        pub fn read(
            read: &journal::JournalRead<'_>,
            key: &str,
        ) -> Result<Option<Self>, GuardError> {
            let Some(head) = read.steering_key(key)? else {
                return Ok(None);
            };
            let fact = |position| {
                read.fact_page(
                    journal::FactPageStart::At(position),
                    position,
                    journal::FactReadBudget::new(1, journal::MAX_RETAINED_FACT_PAGE_BYTES)
                        .expect("one steering context fact is a repository budget"),
                )?
                .rows
                .into_iter()
                .next()
                .ok_or_else(|| {
                    GuardError::Invalid(format!(
                        "steering key {key} has no context fact at {position}"
                    ))
                })
            };
            let created = fact(head.body().created)?;
            let current = fact(head.position())?;
            Ok(Some(Self {
                head,
                created,
                current,
            }))
        }

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

        /// Current resident and lane authority at the same durable barrier.
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

        /// Number of bounded retained-fact pages applied for this generation.
        #[must_use]
        pub const fn materialization_steps(&self) -> u64 {
            self.materialization_steps
        }
    }

    impl SteeringSnapshot {
        /// Incrementally retained facts, residency, and cursor at this barrier.
        #[must_use]
        pub const fn authority(&self) -> &TaskSnapshot {
            &self.authority
        }

        /// Bounded live steering page selected at the same barrier.
        #[must_use]
        pub const fn pending(&self) -> &journal::SteeringPage {
            &self.pending
        }

        /// Consume the snapshot and return its retired-steering window.
        #[must_use]
        pub fn into_retired(self) -> Vec<journal::RetiredSteering> {
            self.retired
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
        /// Resolve a transcript author through this writer's incremental session index.
        pub fn transcript_session(
            &self,
            lane: journal::crossing::CanonicalUuid,
            source: journal::crossing::CanonicalUuid,
            member: u64,
            offset: u64,
        ) -> Result<Option<TranscriptSession>, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::TranscriptSession {
                    lane,
                    source,
                    member,
                    offset,
                    answer,
                })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
                .map_err(BusReadError::Head)
        }

        /// Read only the current durable cursor at one queued barrier.
        pub fn cursor(&self) -> Result<Option<journal::crossing::Seq>, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Read(JournalRead::Cursor { answer }))
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
                .send(Request::Read(JournalRead::Snapshot { answer }))
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

        /// Read one bounded retained-kind page through the writer queue.
        pub fn selected_fact_page(
            &self,
            kinds: Vec<journal::crossing::FactKind>,
            after: Option<journal::crossing::Seq>,
            budget: journal::FactReadBudget,
        ) -> Result<journal::SelectedFactPage, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            let transaction = Box::new(move |journal: &mut Journal| {
                let result =
                    journal.read(move |read| read.selected_fact_page(&kinds, after, budget));
                let respond = Box::new(move || {
                    let _ = answer.send(result);
                });
                TransactionCompletion {
                    commit: None,
                    respond,
                }
            });
            self.requests
                .send(Request::Transaction { transaction })
                .map_err(|_| BusReadError::Bus(BusError::Closed))?;
            receiver
                .recv()
                .map_err(|_| BusReadError::Bus(BusError::Closed))?
                .map_err(BusReadError::Head)
        }

        /// Resolve only the steering keys named by one fixed-size message batch.
        ///
        /// # Errors
        ///
        /// Refuses a batch larger than [`MAX_STEERING_PRESENTATION_KEYS`].
        pub fn steering_presentations(
            &self,
            keys: Vec<String>,
        ) -> Result<Vec<SteeringPresentation>, BusReadError> {
            if keys.len() > MAX_STEERING_PRESENTATION_KEYS {
                return Err(BusReadError::Head(GuardError::Invalid(format!(
                    "steering presentation batch has {} keys, maximum is {MAX_STEERING_PRESENTATION_KEYS}",
                    keys.len()
                ))));
            }
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Read(JournalRead::SteeringPresentations {
                    keys,
                    answer,
                }))
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
            budget: journal::FactReadBudget,
        ) -> Result<journal::SelectedFactPage, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Read(JournalRead::RecentOriginFacts {
                    origin,
                    kind,
                    before,
                    budget,
                    answer,
                }))
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
            budget: journal::FactReadBudget,
        ) -> Result<FactPage, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Read(JournalRead::CanvasFacts {
                    start,
                    horizon,
                    budget,
                    answer,
                }))
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
            budget: journal::FactReadBudget,
        ) -> Result<Option<FactPage>, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Read(JournalRead::CanvasThread {
                    position,
                    budget,
                    answer,
                }))
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

        /// Read incremental resident authority and one pending-steering page.
        ///
        /// The retained authority shares already-decoded fact chunks across
        /// requests. Interactive inbox polling therefore never rebuilds the
        /// complete journal merely to recover a lane's context anchor. A
        /// nonzero `retired` also reads that many most recently retired
        /// directives per lane at the same barrier.
        pub fn steering_snapshot(
            &self,
            lane: Option<journal::crossing::CanonicalUuid>,
            start: journal::FactPageStart,
            retired: usize,
        ) -> Result<SteeringSnapshot, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Steering {
                    lane,
                    start,
                    retired,
                    answer,
                })
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
            budget: journal::FactReadBudget,
        ) -> Result<FactPage, BusReadError> {
            self.reader().canvas_facts(start, horizon, budget)
        }

        /// Read one bounded causal page directly through the thread index.
        pub fn canvas_thread(
            &self,
            position: journal::crossing::Seq,
            budget: journal::FactReadBudget,
        ) -> Result<Option<FactPage>, BusReadError> {
            self.reader().canvas_thread(position, budget)
        }

        /// Read the current resident and lane head through the writer queue.
        pub fn residency_head(
            &self,
        ) -> Result<Option<JournalHead<journal::crossing::ResidencyHead>>, BusReadError> {
            let (answer, receiver) = mpsc::sync_channel(1);
            self.requests
                .send(Request::Read(JournalRead::Residency { answer }))
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
                .send(Request::Read(JournalRead::Trust {
                    digest: digest.clone(),
                    answer,
                }))
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
                .send(Request::Read(JournalRead::LaneSourceBoundaries {
                    lanes: lanes.to_vec(),
                    answer,
                }))
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
                .send(Request::Read(JournalRead::SourceBoundary {
                    lane: *lane,
                    answer,
                }))
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
                .send(Request::Read(JournalRead::Verify { answer }))
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
        TranscriptSession {
            lane: journal::crossing::CanonicalUuid,
            source: journal::crossing::CanonicalUuid,
            member: u64,
            offset: u64,
            answer: SyncSender<Result<Option<TranscriptSession>, GuardError>>,
        },
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
        Read(JournalRead),
        Tasks {
            answer: SyncSender<Result<TaskSnapshot, BusReadError>>,
        },
        Steering {
            lane: Option<journal::crossing::CanonicalUuid>,
            start: journal::FactPageStart,
            retired: usize,
            answer: SyncSender<Result<SteeringSnapshot, BusReadError>>,
        },
        CanvasHeads {
            answer: SyncSender<Result<CanvasHeads, BusReadError>>,
        },
        Shutdown {
            answer: SyncSender<()>,
        },
    }

    /// Reads answered from the journal alone, owing nothing to writer state.
    enum JournalRead {
        Snapshot {
            answer: SyncSender<Result<FactSnapshot, SnapshotError>>,
        },
        Cursor {
            answer: SyncSender<Result<Option<journal::crossing::Seq>, GuardError>>,
        },
        SteeringPresentations {
            keys: Vec<String>,
            answer: SyncSender<Result<Vec<SteeringPresentation>, GuardError>>,
        },
        RecentOriginFacts {
            origin: journal::crossing::Origin,
            kind: journal::crossing::FactKind,
            before: Option<journal::crossing::Seq>,
            budget: journal::FactReadBudget,
            answer: SyncSender<Result<journal::SelectedFactPage, GuardError>>,
        },
        CanvasFacts {
            start: journal::FactPageStart,
            horizon: journal::crossing::Seq,
            budget: journal::FactReadBudget,
            answer: SyncSender<Result<FactPage, GuardError>>,
        },
        CanvasThread {
            position: journal::crossing::Seq,
            budget: journal::FactReadBudget,
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
    }

    fn run_writer(
        mut journal: Journal,
        requests: Receiver<Request>,
        broadcasts: tokio::sync::broadcast::Sender<JournalCommit>,
    ) {
        let mut subscribers = Vec::<mpsc::Sender<JournalCommit>>::new();
        let mut task_snapshot = TaskSnapshotState::default();
        let mut transcript_sessions = TranscriptSessionState::default();
        let mut snapshot_waiters = SnapshotWaiters::default();
        while let Some(request) = next_request(
            &journal,
            &requests,
            &mut task_snapshot,
            &mut snapshot_waiters,
        ) {
            match request {
                Request::TranscriptSession {
                    lane,
                    source,
                    member,
                    offset,
                    answer,
                } => {
                    let _ = answer
                        .send(transcript_sessions.lookup(&journal, lane, source, member, offset));
                }
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
                Request::Read(read) => answer_journal_read(&journal, read),
                Request::Tasks { answer } => {
                    answer_tasks(&task_snapshot, &mut snapshot_waiters.tasks, answer);
                }
                Request::Steering {
                    lane,
                    start,
                    retired,
                    answer,
                } => {
                    let _ = answer.send(steering_snapshot(
                        &journal,
                        &task_snapshot,
                        lane,
                        start,
                        retired,
                    ));
                }
                Request::CanvasHeads { answer } => {
                    let _ = answer.send(canvas_heads(&task_snapshot));
                }
                Request::Shutdown { answer } => {
                    let _ = answer.send(());
                    break;
                }
            }
        }
    }

    fn answer_journal_read(journal: &Journal, read: JournalRead) {
        match read {
            JournalRead::Snapshot { answer } => {
                let _ = answer.send(journal.fact_snapshot());
            }
            JournalRead::Cursor { answer } => {
                let _ = answer.send(journal.read(|read| read.cursor()));
            }
            JournalRead::SteeringPresentations { keys, answer } => {
                let _ = answer.send(steering_presentations(journal, keys));
            }
            JournalRead::RecentOriginFacts {
                origin,
                kind,
                before,
                budget,
                answer,
            } => {
                let _ = answer.send(
                    journal
                        .read(move |read| read.recent_origin_facts(&origin, kind, before, budget)),
                );
            }
            JournalRead::CanvasFacts {
                start,
                horizon,
                budget,
                answer,
            } => {
                let _ = answer.send(journal.read(|read| read.fact_page(start, horizon, budget)));
            }
            JournalRead::CanvasThread {
                position,
                budget,
                answer,
            } => {
                let _ = answer.send(journal.read(|read| read.causal_page(position, budget)));
            }
            JournalRead::Residency { answer } => {
                let _ = answer.send(journal.read(|read| read.residency_head()));
            }
            JournalRead::Trust { digest, answer } => {
                let _ = answer.send(journal.read(|read| read.trust_head(&digest)));
            }
            JournalRead::SourceBoundary { lane, answer } => {
                let _ = answer.send(journal.read(|read| read.source_boundary(&lane)));
            }
            JournalRead::LaneSourceBoundaries { lanes, answer } => {
                let _ = answer.send(journal.read(|read| read.lane_source_boundaries(&lanes)));
            }
            JournalRead::Verify { answer } => {
                let _ = answer.send(journal.verify());
            }
        }
    }

    #[derive(Default)]
    struct SnapshotWaiters {
        tasks: Vec<SyncSender<Result<TaskSnapshot, BusReadError>>>,
        requests: VecDeque<Request>,
    }

    fn next_request(
        journal: &Journal,
        requests: &Receiver<Request>,
        task_snapshot: &mut TaskSnapshotState,
        waiters: &mut SnapshotWaiters,
    ) -> Option<Request> {
        loop {
            if !task_snapshot.needs_step() {
                if let Some(request) = waiters.requests.pop_front() {
                    return Some(request);
                }
                return requests.recv().ok();
            }
            let _ = advance_task_snapshot(journal, task_snapshot);
            answer_waiting_tasks(task_snapshot, &mut waiters.tasks);
            match request_during_snapshot(requests, &mut waiters.requests) {
                Ok(Some(request)) => return Some(request),
                Ok(None) => {}
                Err(()) => return None,
            }
        }
    }

    fn request_during_snapshot(
        requests: &Receiver<Request>,
        retained: &mut VecDeque<Request>,
    ) -> Result<Option<Request>, ()> {
        match requests.try_recv() {
            Ok(request)
                if request_needs_task_snapshot(&request) && retained.len() < REQUEST_CAPACITY =>
            {
                retained.push_back(request);
                Ok(None)
            }
            Ok(request) => Ok(Some(request)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(()),
        }
    }

    fn request_needs_task_snapshot(request: &Request) -> bool {
        matches!(
            request,
            Request::Steering { .. } | Request::CanvasHeads { .. }
        )
    }

    fn steering_presentations(
        journal: &Journal,
        keys: Vec<String>,
    ) -> Result<Vec<SteeringPresentation>, GuardError> {
        if keys.len() > MAX_STEERING_PRESENTATION_KEYS {
            return Err(GuardError::Invalid(format!(
                "steering presentation batch has {} keys, maximum is {MAX_STEERING_PRESENTATION_KEYS}",
                keys.len()
            )));
        }
        journal.read(move |read| {
            keys.iter()
                .filter_map(|key| SteeringPresentation::read(read, key).transpose())
                .collect()
        })
    }

    fn answer_commit(
        journal: &mut Journal,
        task_snapshot: &mut TaskSnapshotState,
        subscribers: &mut Vec<mpsc::Sender<JournalCommit>>,
        broadcasts: &tokio::sync::broadcast::Sender<JournalCommit>,
        write: JournalWrite,
        answer: SyncSender<CommitOutcome>,
    ) {
        let outcome = match journal.apply(write) {
            Ok(applied) => {
                let commit = JournalCommit::from_applied(applied);
                invalidate_task_snapshot(task_snapshot);
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
        task_snapshot: &mut TaskSnapshotState,
        subscribers: &mut Vec<mpsc::Sender<JournalCommit>>,
        broadcasts: &tokio::sync::broadcast::Sender<JournalCommit>,
        transaction: Box<dyn FnOnce(&mut Journal) -> TransactionCompletion + Send>,
    ) {
        let TransactionCompletion { commit, respond } = transaction(journal);
        if let Some(commit) = commit {
            invalidate_task_snapshot(task_snapshot);
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
        state: &TaskSnapshotState,
        waiters: &mut Vec<SyncSender<Result<TaskSnapshot, BusReadError>>>,
        answer: SyncSender<Result<TaskSnapshot, BusReadError>>,
    ) {
        if state.needs_step() {
            if waiters.len() < REQUEST_CAPACITY {
                waiters.push(answer);
            } else {
                let _ = answer.send(state.current());
            }
        } else {
            let _ = answer.send(state.current());
        }
    }

    fn answer_waiting_tasks(
        state: &TaskSnapshotState,
        waiters: &mut Vec<SyncSender<Result<TaskSnapshot, BusReadError>>>,
    ) {
        if state.needs_step() {
            return;
        }
        for answer in waiters.drain(..) {
            let _ = answer.send(state.current());
        }
    }

    fn current_task_snapshot(state: &TaskSnapshotState) -> Result<TaskSnapshot, BusReadError> {
        state.current()
    }

    fn steering_snapshot(
        journal: &Journal,
        state: &TaskSnapshotState,
        lane: Option<journal::crossing::CanonicalUuid>,
        start: journal::FactPageStart,
        retired: usize,
    ) -> Result<SteeringSnapshot, BusReadError> {
        current_task_snapshot(state).and_then(|authority| {
            journal
                .read(|read| {
                    Ok((
                        read.pending_steering(lane, start)?,
                        read.retired_steering(lane, retired)?,
                    ))
                })
                .map_err(BusReadError::Head)
                .map(|(pending, retired)| SteeringSnapshot {
                    authority,
                    pending,
                    retired,
                })
        })
    }

    fn canvas_heads(state: &TaskSnapshotState) -> Result<CanvasHeads, BusReadError> {
        current_task_snapshot(state).map(|snapshot| CanvasHeads {
            cursor: snapshot.cursor,
            trust: snapshot.trust.clone(),
            residency: snapshot.residency.clone(),
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
    CommitOutcome, CommitSubscriber, JournalCommit, JournalTransaction,
    MAX_STEERING_PRESENTATION_KEYS, SteeringPresentation, SteeringSnapshot, TaskFacts,
    TaskSnapshot, TranscriptSession, TranscriptSessions, context_opening,
};
