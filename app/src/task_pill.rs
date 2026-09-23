//! Task-pill presentation derived from the shared Work projection.

use crate::work::{WorkStem, WorkStemIdentity};

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
    pub(crate) fn from_stem(stem: &WorkStem) -> Self {
        let counts = stem.counts();
        Self {
            identity: stem.identity().clone(),
            stem: stem.identity().stem().to_owned(),
            hidden: stem.identity().hidden(),
            ready: counts.ready(),
            active: counts.in_flight(),
            unavailable: counts.unavailable(),
        }
    }

    pub(crate) fn count(&self) -> String {
        format!("{}·{}·{}", self.ready, self.active, self.unavailable)
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn title(&self) -> String {
        format!(
            "{} ready, {} active or in flight, {} blocked or deferred across {}.*",
            self.ready, self.active, self.unavailable, self.stem
        )
    }
}
