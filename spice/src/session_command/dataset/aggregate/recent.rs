//! The newest authored prompts and final answers a summary page reads.

use super::super::super::model::MessageRecord;

/// The newest authored prompts and final answers at or before `end`, oldest
/// first, and how many the scan offered.
pub(crate) struct Recent {
    limit: usize,
    end: Option<String>,
    pub(super) rows: Vec<MessageRecord>,
    pub(super) offered: usize,
}

impl Recent {
    pub(crate) const NONE: Self = Self {
        limit: 0,
        end: None,
        rows: Vec::new(),
        offered: 0,
    };

    pub(crate) fn new(limit: usize, end: Option<String>) -> Self {
        Self {
            limit,
            end,
            rows: Vec::with_capacity(limit.saturating_add(1)),
            offered: 0,
        }
    }

    /// Retain `row` in time order when it falls at or before `end`, keeping
    /// at most `limit` of the newest.
    pub(super) fn offer(&mut self, row: MessageRecord) {
        if self.limit == 0 || self.end.as_ref().is_some_and(|end| row.ts > *end) {
            return;
        }
        self.offered += 1;
        let at = self.rows.partition_point(|retained| {
            (&retained.ts, &retained.source, &retained.side) <= (&row.ts, &row.source, &row.side)
        });
        self.rows.insert(at, row);
        if self.rows.len() > self.limit {
            self.rows.remove(0);
        }
    }
}
