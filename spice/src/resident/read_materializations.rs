//! Generation-resident read models for Canvas control requests.
//!
//! Complete journal replay belongs to this background publisher. A request
//! observes one immutable generation only after a scalar authority barrier
//! proves that generation current.

use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant as Monotonic};

use bus::{Bus, BusReader};
use fold::TaskEffortInputs;
use wire::intent::EffortAnswer;
use wire::{Instant, Seq};

use super::RuntimePaths;

#[derive(Debug)]
pub(super) struct Generation {
    cursor: Seq,
    at: Instant,
    consent: Arc<consent::Queue>,
    #[cfg(test)]
    decoded_facts: usize,
    #[cfg(test)]
    effort_inputs: usize,
    #[cfg(test)]
    consent_rebuilt: bool,
}

impl Generation {
    pub(super) const fn cursor(&self) -> Seq {
        self.cursor
    }

    pub(super) fn consent(&self) -> &consent::Queue {
        self.consent.as_ref()
    }

    #[cfg(test)]
    pub(super) const fn decoded_facts(&self) -> usize {
        self.decoded_facts
    }

    #[cfg(test)]
    pub(super) const fn effort_inputs(&self) -> usize {
        self.effort_inputs
    }

    #[cfg(test)]
    pub(super) const fn consent_rebuilt(&self) -> bool {
        self.consent_rebuilt
    }
}

struct ReadState {
    generation: Arc<Generation>,
    consent: Arc<consent::Queue>,
    effort: TaskEffortInputs,
    effort_answer: Option<(Seq, Arc<EffortAnswer>)>,
}

#[derive(Debug)]
enum Published {
    Ready(Arc<Generation>),
    Fault(String),
}

#[derive(Debug)]
struct Publication {
    revision: u64,
    value: Published,
}

const CURRENT_WAIT: Duration = Duration::from_secs(10);

/// One coalescing background publisher and its latest immutable generation.
pub(super) struct CurrentReads {
    reader: BusReader,
    state: Arc<Mutex<ReadState>>,
    published: Arc<(Mutex<Publication>, Condvar)>,
    refresh: Option<SyncSender<()>>,
    worker: Option<JoinHandle<()>>,
    #[cfg(test)]
    request_barriers: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    effort_derivations: std::sync::atomic::AtomicU64,
}

impl CurrentReads {
    pub(super) fn start(
        bus: &Bus,
        _paths: &RuntimePaths,
        policy: &consent::Policy,
    ) -> Result<Self, String> {
        let reader = bus.reader();
        let (initial, state) = build_initial(&reader, policy)?;
        let initial = Arc::new(initial);
        let state = Arc::new(Mutex::new(ReadState {
            generation: Arc::clone(&initial),
            consent: state.0,
            effort: state.1,
            effort_answer: None,
        }));
        let published = Arc::new((
            Mutex::new(Publication {
                revision: 0,
                value: Published::Ready(initial),
            }),
            Condvar::new(),
        ));
        let (refresh, requests) = mpsc::sync_channel(1);
        let worker_reader = reader.clone();
        let worker_published = Arc::clone(&published);
        let worker_state = Arc::clone(&state);
        let worker = thread::Builder::new()
            .name("spice-read-materializer".to_owned())
            .spawn(move || {
                run_worker(worker_reader, requests, worker_state, worker_published);
            })
            .map_err(|error| format!("read materializer thread did not start: {error}"))?;
        Ok(Self {
            reader,
            state,
            published,
            refresh: Some(refresh),
            worker: Some(worker),
            #[cfg(test)]
            request_barriers: std::sync::atomic::AtomicU64::new(0),
            #[cfg(test)]
            effort_derivations: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Coalesce any number of durable commits into one current rebuild.
    pub(super) fn refresh(&self) -> Result<(), String> {
        let Some(refresh) = &self.refresh else {
            return Err("read materializer is stopped".to_owned());
        };
        match refresh.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => Ok(()),
            Err(TrySendError::Disconnected(())) => {
                Err("read materializer worker is unavailable".to_owned())
            }
        }
    }

    /// Settle startup reconciliation before Canvas discovery becomes visible.
    pub(super) fn synchronize_before_publish(&self) -> Result<(), String> {
        if self.current().is_ok() {
            return Ok(());
        }
        let replacement = advance_generation(&self.reader, &self.state)?;
        let authority = self
            .reader
            .cursor()
            .map_err(|error| format!("authority cursor is unreadable: {error}"))?;
        if authority != Some(replacement.cursor) {
            return Err(format!(
                "authority moved to {authority:?} while read materialization settled at {}",
                replacement.cursor
            ));
        }
        let (published, changed) = &*self.published;
        let mut published = published
            .lock()
            .map_err(|_| "read materializer publication lock is poisoned".to_owned())?;
        published.revision = published.revision.wrapping_add(1);
        published.value = Published::Ready(replacement);
        drop(published);
        changed.notify_all();
        Ok(())
    }

    /// Capture the current immutable generation after a bounded stale-cache join.
    pub(super) fn current(&self) -> Result<Arc<Generation>, String> {
        #[cfg(test)]
        self.request_barriers
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let deadline = Monotonic::now() + CURRENT_WAIT;
        loop {
            let authority = self
                .reader
                .cursor()
                .map_err(|error| format!("authority cursor is unreadable: {error}"))?
                .ok_or_else(|| "running resident has no durable fact cursor".to_owned())?;
            let (published, changed) = &*self.published;
            let current = published
                .lock()
                .map_err(|_| "read materializer publication lock is poisoned".to_owned())?;
            match &current.value {
                Published::Ready(generation) if generation.cursor == authority => {
                    return Ok(Arc::clone(generation));
                }
                Published::Fault(detail) => return Err(detail.clone()),
                Published::Ready(_) => {}
            }

            let revision = current.revision;
            self.refresh()?;
            let remaining = deadline.saturating_duration_since(Monotonic::now());
            if remaining.is_zero() {
                return Err(format!(
                    "read materialization did not reach authority {authority} within {}s",
                    CURRENT_WAIT.as_secs()
                ));
            }
            let (current, timeout) = changed
                .wait_timeout_while(current, remaining, |state| state.revision == revision)
                .map_err(|_| "read materializer publication lock is poisoned".to_owned())?;
            if timeout.timed_out() && current.revision == revision {
                return Err(format!(
                    "read materialization did not reach authority {authority} within {}s",
                    CURRENT_WAIT.as_secs()
                ));
            }
        }
    }

    /// Derive task effort only for an explicit effort read.
    ///
    /// Normal commits advance the retained reducer inputs alongside the cheap
    /// current generation, but never walk or serialize the accumulated effort
    /// history. A repeated read at the same barrier reuses its one answer.
    pub(super) fn effort(&self) -> Result<(Seq, Arc<EffortAnswer>), String> {
        loop {
            let generation = self.current()?;
            let mut state = self
                .state
                .lock()
                .map_err(|_| "read materializer state lock is poisoned".to_owned())?;
            if state.generation.cursor != generation.cursor {
                drop(state);
                continue;
            }
            if let Some((cursor, answer)) = &state.effort_answer
                && *cursor == generation.cursor
            {
                return Ok((*cursor, Arc::clone(answer)));
            }
            let answer = state
                .effort
                .answer(Some(generation.cursor), Some(generation.at))
                .map_err(|error| format!("task-effort materialization failed: {error}"))?;
            let answer = Arc::new(answer);
            state.effort_answer = Some((generation.cursor, Arc::clone(&answer)));
            #[cfg(test)]
            self.effort_derivations
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok((generation.cursor, answer));
        }
    }

    #[cfg(test)]
    pub(super) fn request_barriers(&self) -> u64 {
        self.request_barriers
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(super) fn effort_derivations(&self) -> u64 {
        self.effort_derivations
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Drop for CurrentReads {
    fn drop(&mut self) {
        drop(self.refresh.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_worker(
    reader: BusReader,
    requests: Receiver<()>,
    state: Arc<Mutex<ReadState>>,
    published: Arc<(Mutex<Publication>, Condvar)>,
) {
    while requests.recv().is_ok() {
        while let Ok(()) = requests.try_recv() {}
        let replacement = match advance_generation(&reader, &state) {
            Ok(generation) => Published::Ready(generation),
            Err(error) => Published::Fault(error),
        };
        let (current, changed) = &*published;
        let Ok(mut current) = current.lock() else {
            return;
        };
        current.revision = current.revision.wrapping_add(1);
        current.value = replacement;
        drop(current);
        changed.notify_all();
    }
}

fn advance_generation(
    reader: &BusReader,
    state: &Arc<Mutex<ReadState>>,
) -> Result<Arc<Generation>, String> {
    let mut state = state
        .lock()
        .map_err(|_| "read materializer state lock is poisoned".to_owned())?;
    let authority = reader
        .cursor()
        .map_err(|error| format!("authority cursor is unreadable: {error}"))?
        .ok_or_else(|| "running resident has no durable fact cursor".to_owned())?;
    let admitted = state.generation.cursor;
    if authority < admitted {
        return Err(format!(
            "authority cursor {authority} precedes admitted read generation {admitted}"
        ));
    }
    if authority == admitted {
        return Ok(Arc::clone(&state.generation));
    }

    let count = usize::try_from(authority.get() - admitted.get())
        .map_err(|_| "committed read-materialization delta exceeds this platform".to_owned())?;
    let packed = reader
        .canvas_facts(
            journal::FactPageStart::After(Some(admitted)),
            authority,
            count,
        )
        .map_err(|error| format!("read materialization delta is unreadable: {error}"))?;
    if packed.overflow.is_some()
        || packed.rows.len() != count
        || packed.rows.first().map(|row| row.seq.get()) != Some(admitted.get() + 1)
        || packed.rows.last().map(|row| row.seq) != Some(authority)
    {
        return Err(format!(
            "read materialization expected committed range {}..={authority}, received {:?}..={:?}",
            admitted.get() + 1,
            packed.rows.first().map(|row| row.seq),
            packed.rows.last().map(|row| row.seq)
        ));
    }

    let consent_rebuilt = packed
        .rows
        .iter()
        .any(|row| consent::Queue::observes(row.fact.kind()));
    let consent = if consent_rebuilt {
        let mut consent = state.consent.as_ref().clone();
        consent
            .advance(packed.rows.iter())
            .map_err(|error| format!("consent materialization is invalid: {error}"))?;
        Arc::new(consent)
    } else {
        Arc::clone(&state.consent)
    };
    state.effort.advance(packed.rows.iter());
    let generation = Arc::new(Generation {
        cursor: authority,
        at: packed
            .rows
            .last()
            .expect("a non-empty generation delta has a last fact")
            .at,
        consent: Arc::clone(&consent),
        #[cfg(test)]
        decoded_facts: packed.rows.len(),
        #[cfg(test)]
        effort_inputs: state.effort.len(),
        #[cfg(test)]
        consent_rebuilt,
    });
    state.consent = consent;
    state.effort_answer = None;
    state.generation = Arc::clone(&generation);
    Ok(generation)
}

fn build_initial(
    reader: &BusReader,
    policy: &consent::Policy,
) -> Result<(Generation, (Arc<consent::Queue>, TaskEffortInputs)), String> {
    let snapshot = reader
        .fact_snapshot()
        .map_err(|error| format!("read materialization facts are unreadable: {error}"))?;
    let cursor = snapshot
        .horizon()
        .ok_or_else(|| "running resident has no durable fact horizon".to_owned())?;
    let consent = consent::Queue::from_snapshot(&snapshot, policy)
        .map_err(|error| format!("consent materialization is invalid: {error}"))?;
    let effort_inputs = TaskEffortInputs::from_snapshot(&snapshot);
    Ok((
        Generation {
            cursor,
            at: snapshot
                .facts()
                .last()
                .expect("a snapshot with a horizon has a last fact")
                .at,
            consent: Arc::new(consent.clone()),
            #[cfg(test)]
            decoded_facts: snapshot.facts().len(),
            #[cfg(test)]
            effort_inputs: effort_inputs.len(),
            #[cfg(test)]
            consent_rebuilt: true,
        },
        (Arc::new(consent), effort_inputs),
    ))
}
