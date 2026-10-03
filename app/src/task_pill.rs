//! Task-pill presentation derived from the shared Work projection.

use crate::work::{WorkCounts, WorkProjection, WorkRail, WorkStemIdentity};

pub(crate) struct TaskPill {
    pub(crate) identity: WorkStemIdentity,
    pub(crate) stem: String,
    pub(crate) hidden: bool,
    pub(crate) ready: usize,
    pub(crate) active: usize,
    pub(crate) unavailable: usize,
}

impl TaskPill {
    #[must_use]
    pub(crate) fn new(identity: &WorkStemIdentity, counts: WorkCounts) -> Self {
        Self {
            identity: identity.clone(),
            stem: identity.stem().to_owned(),
            hidden: identity.hidden(),
            ready: counts.ready(),
            active: counts.in_flight(),
            unavailable: counts.unavailable(),
        }
    }

    /// One pill per stem with open work riding `rail`, in stem order.
    #[must_use]
    pub(crate) fn for_rail(work: &WorkProjection<'_>, rail: WorkRail) -> Vec<Self> {
        work.rail_stems(rail)
            .into_iter()
            .map(|(identity, counts)| Self::new(identity, counts))
            .collect()
    }

    pub(crate) fn count(&self) -> String {
        format!("{}·{}·{}", self.ready, self.active, self.unavailable)
    }

    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    pub(crate) fn title(&self) -> String {
        format!(
            "{}: {} ready, {} active or in flight, {} blocked or deferred",
            self.identity, self.ready, self.active, self.unavailable
        )
    }
}
