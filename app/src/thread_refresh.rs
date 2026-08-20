//! Bounded coalescing state for live causal-thread refreshes.

use wire::StreamCursor;

/// One armed refresh deadline and the newest cursor it must request.
#[derive(Debug, Default)]
pub(crate) struct ThreadRefresh {
    cursor: Option<StreamCursor>,
    armed: bool,
}

impl ThreadRefresh {
    /// Retain the newest cursor and report whether this burst needs a timer.
    pub(crate) fn observe(&mut self, cursor: StreamCursor) -> bool {
        if self
            .cursor
            .is_none_or(|current| current.epoch != cursor.epoch || current.seq < cursor.seq)
        {
            self.cursor = Some(cursor);
        }
        if self.armed {
            false
        } else {
            self.armed = true;
            true
        }
    }

    /// Settle the armed deadline and return its one newest request cursor.
    pub(crate) fn take_due(&mut self) -> Option<StreamCursor> {
        self.armed = false;
        self.cursor.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor(epoch: &str, seq: u64) -> StreamCursor {
        StreamCursor {
            epoch: epoch.parse().expect("fixture epoch is canonical"),
            seq: wire::Seq::new(seq).expect("fixture sequence is nonzero"),
        }
    }

    #[test]
    fn one_burst_arms_once_and_releases_only_its_newest_cursor() {
        let epoch = "018f82d4-6f59-7d06-92e3-bce9131163c3";
        let mut refresh = ThreadRefresh::default();

        assert!(refresh.observe(cursor(epoch, 41)));
        assert!(!refresh.observe(cursor(epoch, 43)));
        assert!(!refresh.observe(cursor(epoch, 42)));
        assert_eq!(refresh.take_due(), Some(cursor(epoch, 43)));
        assert_eq!(refresh.take_due(), None);
    }

    #[test]
    fn a_later_burst_rearms_after_the_prior_deadline_settles() {
        let epoch = "018f82d4-6f59-7d06-92e3-bce9131163c3";
        let mut refresh = ThreadRefresh::default();

        assert!(refresh.observe(cursor(epoch, 41)));
        assert_eq!(refresh.take_due(), Some(cursor(epoch, 41)));
        assert!(refresh.observe(cursor(epoch, 42)));
        assert_eq!(refresh.take_due(), Some(cursor(epoch, 42)));
    }
}
