//! Generation-resident read models for Canvas control requests.
//!
//! Complete journal replay belongs to this background publisher. A request
//! observes one immutable generation only after a scalar authority barrier
//! proves that generation current.

use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bus::{Bus, BusReader};
use fold::{Store, TaskEffortInputs, TaskEffortReducer, task_effort_answer};
use wire::Seq;
use wire::intent::EffortAnswer;

use super::RuntimePaths;

#[derive(Debug)]
pub(super) struct Generation {
    cursor: Seq,
    consent: Arc<consent::Queue>,
    effort: Arc<EffortAnswer>,
    #[cfg(test)]
    decoded_facts: usize,
    #[cfg(test)]
    effort_inputs: usize,
    #[cfg(test)]
    consent_rebuilt: bool,
    #[cfg(test)]
    effort_rebuilt: bool,
}

impl Generation {
    pub(super) const fn cursor(&self) -> Seq {
        self.cursor
    }

    pub(super) fn consent(&self) -> &consent::Queue {
        self.consent.as_ref()
    }

    pub(super) fn effort(&self) -> &EffortAnswer {
        self.effort.as_ref()
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

    #[cfg(test)]
    pub(super) const fn effort_rebuilt(&self) -> bool {
        self.effort_rebuilt
    }
}

struct ReadState {
    generation: Arc<Generation>,
    consent: Arc<consent::Queue>,
    effort: TaskEffortInputs,
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
}

impl CurrentReads {
    pub(super) fn start(
        bus: &Bus,
        paths: &RuntimePaths,
        policy: &consent::Policy,
    ) -> Result<Self, String> {
        let reader = bus.reader();
        let (initial, state) = build_initial(&reader, &paths.folds, policy)?;
        let initial = Arc::new(initial);
        let state = Arc::new(Mutex::new(ReadState {
            generation: Arc::clone(&initial),
            consent: state.0,
            effort: state.1,
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
        let deadline = Instant::now() + CURRENT_WAIT;
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
            let remaining = deadline.saturating_duration_since(Instant::now());
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

    #[cfg(test)]
    pub(super) fn request_barriers(&self) -> u64 {
        self.request_barriers
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
    let advanced_effort = state
        .effort
        .advance_and_answer(
            packed.rows.iter(),
            Some(authority),
            packed.rows.last().map(|row| row.at),
        )
        .map_err(|error| format!("task-effort materialization failed: {error}"))?;
    #[cfg(test)]
    let effort_rebuilt = advanced_effort.is_some();
    let effort = advanced_effort.map_or_else(|| Arc::clone(&state.generation.effort), Arc::new);
    let generation = Arc::new(Generation {
        cursor: authority,
        consent: Arc::clone(&consent),
        effort,
        #[cfg(test)]
        decoded_facts: packed.rows.len(),
        #[cfg(test)]
        effort_inputs: state.effort.len(),
        #[cfg(test)]
        consent_rebuilt,
        #[cfg(test)]
        effort_rebuilt,
    });
    state.consent = consent;
    state.generation = Arc::clone(&generation);
    Ok(generation)
}

fn build_initial(
    reader: &BusReader,
    folds: &std::path::Path,
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
    let mut store =
        Store::open(folds).map_err(|error| format!("task-effort store is unavailable: {error}"))?;
    let materialization = store
        .read_or_rebuild_snapshot(&snapshot, &TaskEffortReducer)
        .map_err(|error| format!("task-effort materialization failed: {error}"))?
        .materialization;
    if materialization.cursor.position() != Some(cursor) {
        return Err(format!(
            "task-effort materialization is at {:?}, authority is at {cursor}",
            materialization.cursor.position()
        ));
    }
    let effort = task_effort_answer(&materialization)
        .map_err(|error| format!("task-effort payload is invalid: {error}"))?;
    Ok((
        Generation {
            cursor,
            consent: Arc::new(consent.clone()),
            effort: Arc::new(effort),
            #[cfg(test)]
            decoded_facts: snapshot.facts().len(),
            #[cfg(test)]
            effort_inputs: effort_inputs.len(),
            #[cfg(test)]
            consent_rebuilt: true,
            #[cfg(test)]
            effort_rebuilt: true,
        },
        (Arc::new(consent), effort_inputs),
    ))
}
