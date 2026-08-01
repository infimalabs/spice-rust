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
        CommitFault, FactSnapshot, GuardRefusal, Journal, JournalWrite, SnapshotError,
        Verification, VerifyError,
    };

    const REQUEST_CAPACITY: usize = 64;

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
            Self {
                first: applied.first().get(),
                last: applied.last().get(),
                cursor: fold::Cursor::at(applied.last()),
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

    /// Receiver for post-durability commit broadcasts.
    #[derive(Debug)]
    pub struct CommitSubscriber {
        receiver: Receiver<JournalCommit>,
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
            let writer = thread::Builder::new()
                .name("spice-journal-writer".to_owned())
                .spawn(move || run_writer(journal, receiver))
                .map_err(BusError::Start)?;
            Ok(Self {
                requests,
                writer: Some(writer),
            })
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
        Subscribe {
            commits: mpsc::Sender<JournalCommit>,
            ready: SyncSender<()>,
        },
        Snapshot {
            answer: SyncSender<Result<FactSnapshot, SnapshotError>>,
        },
        Verify {
            answer: SyncSender<Result<Verification, VerifyError>>,
        },
        Shutdown {
            answer: SyncSender<()>,
        },
    }

    fn run_writer(mut journal: Journal, requests: Receiver<Request>) {
        let mut subscribers = Vec::<mpsc::Sender<JournalCommit>>::new();
        while let Ok(request) = requests.recv() {
            match request {
                Request::Commit { write, answer } => {
                    let outcome = match journal.apply(*write) {
                        Ok(applied) => {
                            let commit = JournalCommit::from_applied(applied);
                            subscribers.retain(|subscriber| subscriber.send(commit).is_ok());
                            CommitOutcome::Committed(commit)
                        }
                        Err(journal::CommitError::Refused(refusal)) => {
                            CommitOutcome::Refused(refusal)
                        }
                        Err(journal::CommitError::Fault(fault)) => CommitOutcome::Fault(fault),
                    };
                    let _ = answer.send(outcome);
                }
                Request::Subscribe { commits, ready } => {
                    subscribers.push(commits);
                    let _ = ready.send(());
                }
                Request::Snapshot { answer } => {
                    let _ = answer.send(journal.fact_snapshot());
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
    }

    impl fmt::Display for BusReadError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Bus(error) => error.fmt(formatter),
                Self::Snapshot(error) => error.fmt(formatter),
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
    Bus, BusError, BusReadError, BusVerifyError, CommitOutcome, CommitSubscriber, JournalCommit,
};
