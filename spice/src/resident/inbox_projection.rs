use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use bus::BusReader;
use wire::CanonicalUuid;

use super::{
    CommandEffects, CommandResult, RuntimePaths, environment_fault, steering, store_fault,
};

const PROJECTION_CAPACITY: usize = 64;

struct Queue {
    lanes: VecDeque<CanonicalUuid>,
    active: Option<CanonicalUuid>,
    closed: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}

pub(super) struct Completion {
    pub(super) lane: CanonicalUuid,
    pub(super) result: CommandResult<()>,
}

pub(super) struct Scheduler {
    shared: Arc<Shared>,
    completions: tokio::sync::mpsc::Receiver<Completion>,
    worker: Option<JoinHandle<()>>,
}

impl Scheduler {
    pub(super) fn start(
        reader: BusReader,
        paths: RuntimePaths,
        epoch: CanonicalUuid,
    ) -> CommandResult<Self> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                lanes: VecDeque::new(),
                active: None,
                closed: false,
            }),
            ready: Condvar::new(),
        });
        let (completed, completions) = tokio::sync::mpsc::channel(1);
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("spice-inbox-projection".to_owned())
            .spawn(move || run(worker_shared, completed, reader, paths, epoch))
            .map_err(|error| {
                environment_fault("steer::projection-worker-unavailable", &error.to_string())
            })?;
        Ok(Self {
            shared,
            completions,
            worker: Some(worker),
        })
    }

    pub(super) fn schedule(&self, effects: CommandEffects) -> CommandResult<()> {
        let Some(lane) = effects.inbox_projection else {
            return Ok(());
        };
        let mut queue =
            self.shared.queue.lock().map_err(|error| {
                store_fault("steer::projection-queue-poisoned", &error.to_string())
            })?;
        if queue.closed {
            return Err(environment_fault(
                "steer::projection-worker-closed",
                "the resident inbox projection worker is closed",
            ));
        }
        if queue.lanes.contains(&lane) {
            return Ok(());
        }
        let active_is_distinct = queue
            .active
            .is_some_and(|active| !queue.lanes.contains(&active));
        if queue.active != Some(lane)
            && queue.lanes.len() + usize::from(active_is_distinct) >= PROJECTION_CAPACITY
        {
            return Err(environment_fault(
                "steer::projection-capacity",
                &format!(
                    "the bounded inbox projection queue is full; run `spice steer repair {lane}`"
                ),
            ));
        }
        queue.lanes.push_back(lane);
        self.shared.ready.notify_one();
        Ok(())
    }

    pub(super) async fn recv(&mut self) -> CommandResult<Completion> {
        self.completions.recv().await.ok_or_else(|| {
            environment_fault(
                "steer::projection-worker-closed",
                "the resident inbox projection worker ended without a completion",
            )
        })
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        self.completions.close();
        if let Ok(mut queue) = self.shared.queue.lock() {
            queue.closed = true;
            queue.lanes.clear();
            self.shared.ready.notify_one();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run(
    shared: Arc<Shared>,
    completed: tokio::sync::mpsc::Sender<Completion>,
    reader: BusReader,
    paths: RuntimePaths,
    epoch: CanonicalUuid,
) {
    loop {
        let Some(lane) = next_lane(&shared) else {
            return;
        };
        let result = project(&reader, &paths, epoch, lane);
        if let Ok(mut queue) = shared.queue.lock() {
            queue.active = None;
            if queue.closed {
                return;
            }
        } else {
            return;
        }
        if completed
            .blocking_send(Completion { lane, result })
            .is_err()
        {
            return;
        }
    }
}

fn next_lane(shared: &Shared) -> Option<CanonicalUuid> {
    let mut queue = shared.queue.lock().ok()?;
    while queue.lanes.is_empty() && !queue.closed {
        queue = shared.ready.wait(queue).ok()?;
    }
    if queue.closed {
        return None;
    }
    let lane = queue.lanes.pop_front()?;
    queue.active = Some(lane);
    Some(lane)
}

fn project(
    reader: &BusReader,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
) -> CommandResult<()> {
    let residency = reader
        .read(|read| read.residency_head())
        .map_err(|error| store_fault("steer::residency-unreadable", &error.to_string()))?
        .ok_or_else(|| store_fault("steer::residency-missing", "resident authority is absent"))?;
    if residency.body().epoch != epoch {
        return Err(store_fault(
            "steer::residency-moved",
            &format!(
                "projection epoch {epoch} no longer owns resident epoch {}",
                residency.body().epoch
            ),
        ));
    }
    steering::publish_inbox_once(reader, paths, residency.body(), lane)
}
