//! The no-construction disposition each task's latest completion recorded, so
//! review admission reads it with one lookup.

use std::sync::Arc;

use journal::crossing::TaskStamp;
use journal::crossing::body::PlanDisposition;

use super::{Indexes, TaskFacts};

impl Indexes {
    /// Record `task`'s completion disposition; a completion without one
    /// retires the earlier record.
    pub(super) fn observe_disposition(
        &mut self,
        task: &TaskStamp,
        disposition: Option<PlanDisposition>,
    ) {
        if let Some(disposition) = disposition {
            Arc::make_mut(&mut self.plan_dispositions).insert(task.clone(), disposition);
        } else if self.plan_dispositions.contains_key(task) {
            Arc::make_mut(&mut self.plan_dispositions).remove(task);
        }
    }
}

impl TaskFacts {
    /// The no-construction disposition a task's latest completion recorded.
    #[must_use]
    pub fn plan_disposition(&self, task: &TaskStamp) -> Option<&PlanDisposition> {
        self.indexes.plan_dispositions.get(task)
    }
}
