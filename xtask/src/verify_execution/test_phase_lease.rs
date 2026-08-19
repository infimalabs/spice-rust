//! Progress-honest authority for the verifier's two long test branches.

use std::fmt;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Bounds {
    pub(super) inactivity: Duration,
    pub(super) absolute: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Event {
    WorkspacePrebuildTerminal,
    ActionStart { ordinal: usize },
    ActionAdvance { ordinal: usize, cursor: u64 },
    ActionTerminal { ordinal: usize },
}

impl Event {
    const fn is_advance(self) -> bool {
        matches!(self, Self::ActionAdvance { .. })
    }
}

impl fmt::Display for Event {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkspacePrebuildTerminal => formatter.write_str("workspace-prebuild-terminal"),
            Self::ActionStart { ordinal } => write!(formatter, "action-{ordinal}-start"),
            Self::ActionAdvance { ordinal, cursor } => {
                write!(formatter, "action-{ordinal}-advance-{cursor}")
            }
            Self::ActionTerminal { ordinal } => write!(formatter, "action-{ordinal}-terminal"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Expiry {
    Inactivity,
    Absolute,
}

impl fmt::Display for Expiry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inactivity => formatter.write_str("inactivity"),
            Self::Absolute => formatter.write_str("absolute"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Evidence {
    pub(super) generation: u64,
    pub(super) reason: Event,
    pub(super) inactivity_remaining: Duration,
    pub(super) absolute_remaining: Duration,
}

pub(super) struct Lease {
    bounds: Bounds,
    absolute_deadline: Instant,
    last_progress: Option<Instant>,
    last_event: Option<Event>,
    generation: u64,
    reported_generation: u64,
}

impl Lease {
    pub(super) fn new(started: Instant, bounds: Bounds) -> Self {
        Self {
            bounds,
            absolute_deadline: started + bounds.absolute,
            last_progress: None,
            last_event: None,
            generation: 0,
            reported_generation: 0,
        }
    }

    pub(super) fn renew(&mut self, observed: Instant, event: Event) {
        debug_assert!(
            self.last_progress
                .is_none_or(|previous| observed >= previous),
            "test-phase progress observations are monotonic",
        );
        self.last_progress = Some(observed);
        self.last_event = Some(event);
        self.generation = self.generation.saturating_add(1);
    }

    pub(super) fn expiry(&self, observed: Instant, complete: bool) -> Option<Expiry> {
        if complete {
            return None;
        }
        let last_progress = self.last_progress?;
        if observed >= self.absolute_deadline {
            return Some(Expiry::Absolute);
        }
        if observed.saturating_duration_since(last_progress) >= self.bounds.inactivity {
            return Some(Expiry::Inactivity);
        }
        None
    }

    pub(super) fn take_evidence(
        &mut self,
        observed: Instant,
        report_advance: bool,
    ) -> Option<Evidence> {
        if self.generation == self.reported_generation {
            return None;
        }
        let reason = self.last_event?;
        if reason.is_advance() && !report_advance {
            return None;
        }
        self.reported_generation = self.generation;
        let last_progress = self.last_progress?;
        Some(Evidence {
            generation: self.generation,
            reason,
            inactivity_remaining: self
                .bounds
                .inactivity
                .saturating_sub(observed.saturating_duration_since(last_progress)),
            absolute_remaining: self.absolute_deadline.saturating_duration_since(observed),
        })
    }

    pub(super) fn current_evidence(&self, observed: Instant) -> Option<Evidence> {
        let last_progress = self.last_progress?;
        Some(Evidence {
            generation: self.generation,
            reason: self.last_event?,
            inactivity_remaining: self
                .bounds
                .inactivity
                .saturating_sub(observed.saturating_duration_since(last_progress)),
            absolute_remaining: self.absolute_deadline.saturating_duration_since(observed),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INACTIVITY: Duration = Duration::from_secs(10);
    const ABSOLUTE: Duration = Duration::from_secs(30);

    fn lease(started: Instant) -> Lease {
        Lease::new(
            started,
            Bounds {
                inactivity: INACTIVITY,
                absolute: ABSOLUTE,
            },
        )
    }

    #[test]
    fn cold_prebuild_does_not_consume_an_inactive_test_lease() {
        let started = Instant::now();
        let mut lease = lease(started);
        assert_eq!(lease.expiry(started + Duration::from_secs(20), false), None);

        lease.renew(
            started + Duration::from_secs(20),
            Event::WorkspacePrebuildTerminal,
        );
        assert_eq!(lease.expiry(started + Duration::from_secs(29), false), None);
        assert_eq!(
            lease.expiry(started + Duration::from_secs(30), false),
            Some(Expiry::Absolute),
            "the global absolute bound still includes prebuild time",
        );
    }

    #[test]
    fn distinct_advancement_renews_inactivity_but_not_the_absolute_bound() {
        let started = Instant::now();
        let mut lease = lease(started);
        lease.renew(started, Event::WorkspacePrebuildTerminal);
        lease.renew(
            started + Duration::from_secs(9),
            Event::ActionAdvance {
                ordinal: 9,
                cursor: 1,
            },
        );
        assert_eq!(lease.expiry(started + Duration::from_secs(18), false), None);
        lease.renew(
            started + Duration::from_secs(18),
            Event::ActionTerminal { ordinal: 9 },
        );
        assert_eq!(lease.expiry(started + Duration::from_secs(27), false), None);
        assert_eq!(
            lease.expiry(started + ABSOLUTE, false),
            Some(Expiry::Absolute),
        );
    }

    #[test]
    fn a_silent_active_branch_expires_at_the_inactivity_boundary() {
        let started = Instant::now();
        let mut lease = lease(started);
        lease.renew(started, Event::WorkspacePrebuildTerminal);
        assert_eq!(
            lease.expiry(started + INACTIVITY - Duration::from_nanos(1), false),
            None
        );
        assert_eq!(
            lease.expiry(started + INACTIVITY, false),
            Some(Expiry::Inactivity),
        );
        assert_eq!(
            lease.expiry(started + INACTIVITY, true),
            None,
            "a completed test phase cannot time out",
        );
    }

    #[test]
    fn duplicate_observation_is_not_a_new_evidence_generation() {
        let started = Instant::now();
        let mut lease = lease(started);
        lease.renew(started, Event::WorkspacePrebuildTerminal);
        let first = lease
            .take_evidence(started, false)
            .expect("activation publishes one generation");
        assert_eq!(first.generation, 1);
        assert_eq!(lease.take_evidence(started, false), None);

        lease.renew(
            started + Duration::from_secs(1),
            Event::ActionAdvance {
                ordinal: 8,
                cursor: 1,
            },
        );
        assert_eq!(
            lease.take_evidence(started + Duration::from_secs(1), false),
            None,
            "raw advancement is retained until the bounded report interval",
        );
        assert_eq!(
            lease
                .take_evidence(started + Duration::from_secs(1), true)
                .expect("bounded advancement report")
                .generation,
            2,
        );
        assert_eq!(
            lease.take_evidence(started + Duration::from_secs(1), true),
            None
        );
    }
}
