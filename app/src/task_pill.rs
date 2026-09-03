//! Visibility-aware task-pill aggregation shared with the browser chrome.

use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct TaskPill {
    pub(crate) stem: String,
    pub(crate) hidden: bool,
    pub(crate) ready: usize,
    pub(crate) active: usize,
    pub(crate) unavailable: usize,
}

impl TaskPill {
    pub(crate) fn count(&self) -> String {
        if self.hidden {
            (self.ready + self.active + self.unavailable).to_string()
        } else {
            format!("{}·{}·{}", self.ready, self.active, self.unavailable)
        }
    }

    pub(crate) fn title(&self) -> String {
        if self.hidden {
            return format!("{} deferred or system tasks", self.count());
        }
        format!(
            "{} ready, {} active or in flight, {} blocked or deferred across {}.*",
            self.ready, self.active, self.unavailable, self.stem
        )
    }
}

#[derive(Clone, Copy)]
pub(crate) enum TaskPillAvailability {
    Ready,
    Active,
    Unavailable,
}

#[derive(Default)]
pub(crate) struct TaskPillIndex(BTreeMap<(bool, String), TaskPill>);

impl TaskPillIndex {
    pub(crate) fn record(
        &mut self,
        hidden: bool,
        stem: String,
        availability: TaskPillAvailability,
    ) {
        let pill = self
            .0
            .entry((hidden, stem.clone()))
            .or_insert_with(|| TaskPill {
                stem,
                hidden,
                ..TaskPill::default()
            });
        match availability {
            TaskPillAvailability::Ready => pill.ready += 1,
            TaskPillAvailability::Active => pill.active += 1,
            TaskPillAvailability::Unavailable => pill.unavailable += 1,
        }
    }

    pub(crate) fn into_pills(self) -> Vec<TaskPill> {
        self.0.into_values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_by_visibility_then_stem_and_aggregates_same_stem_per_class() {
        let mut pills = TaskPillIndex::default();
        for (hidden, stem, availability) in [
            (true, "alpha", TaskPillAvailability::Active),
            (false, "zeta", TaskPillAvailability::Ready),
            (true, "oops", TaskPillAvailability::Ready),
            (false, "oops", TaskPillAvailability::Active),
            (true, "alpha", TaskPillAvailability::Unavailable),
            (false, "zeta", TaskPillAvailability::Unavailable),
        ] {
            pills.record(hidden, stem.to_owned(), availability);
        }
        let pills = pills.into_pills();

        assert_eq!(
            pills
                .iter()
                .map(|pill| (pill.hidden, pill.stem.as_str()))
                .collect::<Vec<_>>(),
            [
                (false, "oops"),
                (false, "zeta"),
                (true, "alpha"),
                (true, "oops"),
            ]
        );
        assert_eq!(
            pills
                .iter()
                .map(|pill| (pill.ready, pill.active, pill.unavailable))
                .collect::<Vec<_>>(),
            [(0, 1, 0), (1, 0, 1), (0, 1, 1), (1, 0, 0)]
        );
        assert_eq!(pills[0].count(), "0·1·0");
        assert_eq!(pills[1].count(), "1·0·1");
        assert_eq!(
            pills[1].title(),
            "1 ready, 0 active or in flight, 1 blocked or deferred across zeta.*"
        );
        assert_eq!(pills[2].count(), "2");
        assert_eq!(pills[2].title(), "2 deferred or system tasks");
        assert_eq!(pills[3].count(), "1");
    }
}
