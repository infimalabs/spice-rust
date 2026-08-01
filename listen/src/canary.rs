use std::collections::BTreeSet;

use crate::{Decoded, EventKind};

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
}
