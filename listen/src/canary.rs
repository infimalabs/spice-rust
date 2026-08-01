use std::collections::BTreeSet;

use crate::{Decoded, EventKind};
use wire::{CanonicalUuid, Instant, body};

/// One expected event kind absent from the inspected observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Alarm {
    /// Missing kind.
    pub kind: EventKind,
}

/// Pure vitality inspection over decoded records.
pub struct Canary;

impl Canary {
    /// Report every expected kind not present in the observations.
    ///
    /// Residue remains available to the caller and does not prevent alarms for
    /// other kinds.
    #[must_use]
    pub fn inspect(expected: &[EventKind], observations: &[Decoded]) -> Vec<Alarm> {
        let seen: BTreeSet<_> = observations
            .iter()
            .flat_map(Decoded::events)
            .map(|event| event.kind())
            .collect();
        let mut already_reported = BTreeSet::new();

        expected
            .iter()
            .copied()
            .filter(|kind| !seen.contains(kind))
            .filter(|kind| already_reported.insert(*kind))
            .map(|kind| Alarm { kind })
            .collect()
    }

    /// Produce alarm fact bodies for missing kinds once their window is due.
    ///
    /// The caller supplies both instants so replay and tests never consult an
    /// ambient clock. Duplicate expectations collapse through [`Self::inspect`].
    #[must_use]
    pub fn due_alarms(
        lane: CanonicalUuid,
        expected: &[EventKind],
        observations: &[Decoded],
        window: Instant,
        now: Instant,
    ) -> Vec<body::CanaryAlarmed> {
        if now < window {
            return Vec::new();
        }

        Self::inspect(expected, observations)
            .into_iter()
            .map(|alarm| body::CanaryAlarmed {
                lane,
                expected: alarm.kind.as_str().to_owned(),
                window,
            })
            .collect()
    }

    /// Produce contradiction evidence when activity has no decoded context.
    #[must_use]
    pub const fn contradiction(
        lane: CanonicalUuid,
        activity: u64,
        decoded: u64,
    ) -> Option<body::CanaryContradicted> {
        if activity == 0 || decoded != 0 {
            return None;
        }

        Some(body::CanaryContradicted {
            lane,
            activity,
            decoded,
        })
    }
}
