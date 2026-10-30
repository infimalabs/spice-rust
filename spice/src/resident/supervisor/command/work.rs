//! Worker lifetime, including withdrawn tools whose work has not returned.

use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};

use super::Driver;

pub(super) struct Worker<W: Write + Send> {
    pub(super) driver: Arc<Driver<W>>,
}

impl<W: Write + Send> Drop for Worker<W> {
    fn drop(&mut self) {
        self.driver.workers.fetch_sub(1, Ordering::AcqRel);
    }
}

impl<W: Write + Send + 'static> Driver<W> {
    /// The guard belongs to the spawned closure; failed spawning drops it too.
    /// Withdrawing a call does not prematurely end its worker's lifetime.
    pub(super) fn worker(self: &Arc<Self>) -> Worker<W> {
        self.workers.fetch_add(1, Ordering::AcqRel);
        Worker {
            driver: Arc::clone(self),
        }
    }

    /// Capture is already joined, so no further tool calls can be admitted.
    /// Closing input also prevents kicks from reactivating this dead occupant.
    pub(in crate::resident::supervisor) fn quiescent(&self) -> bool {
        self.close();
        self.workers.load(Ordering::Acquire) == 0
            && self
                .asks
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty()
            && self
                .retunes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty()
    }
}
