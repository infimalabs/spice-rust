//! Generation-resident read models for Canvas control requests.
//!
//! Complete journal replay belongs to this background publisher. A request
//! observes one immutable generation only after a scalar authority barrier
//! proves that generation current.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};

use bus::{Bus, BusReader};
use fold::{Store, TaskEffortReducer, task_effort_answer};
use wire::Seq;
use wire::intent::EffortAnswer;

use super::RuntimePaths;

#[derive(Debug)]
pub(super) struct Generation {
    cursor: Seq,
    consent: consent::Queue,
    effort: EffortAnswer,
}

impl Generation {
    pub(super) const fn cursor(&self) -> Seq {
        self.cursor
    }

    pub(super) const fn consent(&self) -> &consent::Queue {
        &self.consent
    }

    pub(super) const fn effort(&self) -> &EffortAnswer {
        &self.effort
    }
}

#[derive(Debug)]
enum Published {
    Ready(Arc<Generation>),
    Fault(String),
}

/// One coalescing background publisher and its latest immutable generation.
pub(super) struct CurrentReads {
    reader: BusReader,
    folds: PathBuf,
    policy: consent::Policy,
    published: Arc<RwLock<Published>>,
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
        let initial = Arc::new(build_generation(&reader, paths.folds.clone(), policy)?);
        let published = Arc::new(RwLock::new(Published::Ready(initial)));
        let (refresh, requests) = mpsc::sync_channel(1);
        let worker_reader = reader.clone();
        let worker_published = Arc::clone(&published);
        let folds = paths.folds.clone();
        let resident_policy = policy.clone();
        let worker_policy = resident_policy.clone();
        let worker = thread::Builder::new()
            .name("spice-read-materializer".to_owned())
            .spawn(move || {
                run_worker(
                    worker_reader,
                    folds,
                    worker_policy,
                    requests,
                    worker_published,
                );
            })
            .map_err(|error| format!("read materializer thread did not start: {error}"))?;
        Ok(Self {
            reader,
            folds: paths.folds.clone(),
            policy: resident_policy,
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
        let replacement = Arc::new(build_generation(
            &self.reader,
            self.folds.clone(),
            &self.policy,
        )?);
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
        let mut published = self
            .published
            .write()
            .map_err(|_| "read materializer publication lock is poisoned".to_owned())?;
        *published = Published::Ready(replacement);
        Ok(())
    }

    /// Capture the current immutable generation or fail on any generation gap.
    pub(super) fn current(&self) -> Result<Arc<Generation>, String> {
        #[cfg(test)]
        self.request_barriers
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let authority = self
            .reader
            .cursor()
            .map_err(|error| format!("authority cursor is unreadable: {error}"))?
            .ok_or_else(|| "running resident has no durable fact cursor".to_owned())?;
        let generation = match &*self
            .published
            .read()
            .map_err(|_| "read materializer publication lock is poisoned".to_owned())?
        {
            Published::Ready(generation) => Arc::clone(generation),
            Published::Fault(detail) => return Err(detail.clone()),
        };
        if generation.cursor != authority {
            return Err(format!(
                "read materialization is at {}, authority is at {authority}",
                generation.cursor
            ));
        }
        Ok(generation)
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
    folds: PathBuf,
    policy: consent::Policy,
    requests: Receiver<()>,
    published: Arc<RwLock<Published>>,
) {
    while requests.recv().is_ok() {
        loop {
            match requests.try_recv() {
                Ok(()) => {}
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        let replacement = match build_generation(&reader, folds.clone(), &policy) {
            Ok(generation) => Published::Ready(Arc::new(generation)),
            Err(error) => Published::Fault(error),
        };
        let Ok(mut current) = published.write() else {
            return;
        };
        *current = replacement;
    }
}

fn build_generation(
    reader: &BusReader,
    folds: PathBuf,
    policy: &consent::Policy,
) -> Result<Generation, String> {
    let snapshot = reader
        .fact_snapshot()
        .map_err(|error| format!("read materialization facts are unreadable: {error}"))?;
    let cursor = snapshot
        .horizon()
        .ok_or_else(|| "running resident has no durable fact horizon".to_owned())?;
    let consent = consent::Queue::from_snapshot(&snapshot, policy)
        .map_err(|error| format!("consent materialization is invalid: {error}"))?;
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
    Ok(Generation {
        cursor,
        consent,
        effort,
    })
}
