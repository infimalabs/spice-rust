//! The record a resident writes into the lock file it possesses.
//!
//! The kernel's answer about the lock says whether some resident is running.
//! It cannot say which one, and that distance is the whole difference between
//! dialling a live surface and dialling an address nobody serves: a resident
//! holds this lock from its first instant, while the discovery records the
//! previous resident published are still on disk and still name the previous
//! resident's generation.
//!
//! So a holder writes down which generation it is, and readers compare. The
//! grammar is stated once here because its two halves live far apart -- the
//! resident writes it from `up`, and clients parse it across a process
//! boundary and an integration-test boundary besides. A reader that restated
//! the grammar and then drifted from the writer would not fail loudly: it
//! would report every live resident as a stranger, and each refusal it raised
//! would look exactly like the defect this record exists to catch.

use wire::CanonicalUuid;

/// What a holder names before it has a generation to name.
///
/// The lock is taken before the epoch is minted, so the first record a
/// resident writes cannot carry one. The spelling is deliberately not a UUID:
/// no published discovery can ever carry it, so a reader comparing generations
/// reads the whole startup interval as the mismatch it is rather than as a
/// record it may trust.
const PENDING_EPOCH: &str = "pending";

/// The record a resident publishes while it possesses the lock.
pub fn holder_record(pid: u32, epoch: Option<CanonicalUuid>) -> String {
    match epoch {
        Some(epoch) => format!("pid={pid};epoch={epoch}"),
        None => format!("pid={pid};epoch={PENDING_EPOCH}"),
    }
}

/// The generation a holder record names, if it names a complete one.
///
/// Every other shape answers `None`, and every caller reads that answer the
/// same way: whoever holds this lock did not publish the record you are
/// holding. An empty record is a holder on its way out, which truncates before
/// it releases; a pending record is a holder that has not minted its
/// generation yet; anything unparsable is a holder this reader cannot vouch
/// for. None of the three is the publisher, so none of them earns a dial.
///
/// Only the first line is read. A holder rewrite pads its record out to the
/// span it replaces so no reader ever lands in an empty file, and that padding
/// is trailing filler, not a second field.
pub fn holder_epoch(record: &[u8]) -> Option<CanonicalUuid> {
    std::str::from_utf8(record)
        .ok()?
        .lines()
        .next()?
        .split(';')
        .find_map(|field| field.trim().strip_prefix("epoch="))?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use sill::{Acquisition, LockFile, LockName};

    use super::*;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    fn uuid(last: u8) -> CanonicalUuid {
        format!("00000000-0000-4000-8000-{last:012x}")
            .parse()
            .expect("canonical fixture UUID")
    }

    /// A record survives the framing the lock actually writes it through.
    ///
    /// The reader is paired against `sill`'s own rewrite rather than against a
    /// restated byte layout, because the layout is not this module's to state:
    /// the newline and the padding that keeps a contended reader out of an
    /// empty file are added underneath. A test that rebuilt them here would go
    /// on passing after `sill` changed how a record is landed, which is the
    /// one drift that leaves both halves of this grammar looking correct.
    ///
    /// The long record is written first on purpose. The short one that follows
    /// is the padded case, and a reader that stopped at the first newline is
    /// the only one that survives both.
    #[test]
    fn a_holder_record_round_trips_through_the_lock_that_frames_it() {
        let path = std::env::temp_dir().join(format!(
            "spice-holder-record-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let lock = LockFile::new(
            LockName::new("resident").expect("resident is a non-empty lock name"),
            &path,
        );
        let Acquisition::Acquired(possession) = lock.try_acquire().expect("fixture lock") else {
            panic!("a lock file nothing else has seen cannot be contended");
        };

        possession
            .rewrite_record(holder_record(4321, Some(uuid(1))).as_bytes())
            .expect("long holder record writes");
        assert_eq!(
            holder_epoch(&std::fs::read(&path).expect("long record reads")),
            Some(uuid(1))
        );

        possession
            .rewrite_record(holder_record(7, None).as_bytes())
            .expect("padded holder record writes");
        let padded = std::fs::read(&path).expect("padded record reads");
        assert!(
            padded.len() > holder_record(7, None).len() + 1,
            "the shorter record must land padded, or this case is the long one again"
        );
        assert_eq!(
            holder_epoch(&padded),
            None,
            "a holder that has not minted its generation names none"
        );

        drop(possession);
        std::fs::remove_file(&path).expect("fixture cleanup");
    }

    /// Nothing but a complete generation is read as one.
    ///
    /// The truncated record is what a holder leaves behind while it releases,
    /// and the pid-only record is what a future field order could look like
    /// from here. Both are refusals rather than trust, and the last case pins
    /// that the field is matched whole: a longer name ending in `epoch` is a
    /// different field and must not be mistaken for this one.
    #[test]
    fn no_partial_holder_record_names_a_generation() {
        assert_eq!(holder_epoch(b""), None);
        assert_eq!(holder_epoch(b"\n"), None);
        assert_eq!(holder_epoch(b"pid=7\n"), None);
        assert_eq!(holder_epoch(b"pid=7;epoch=\n"), None);
        assert_eq!(holder_epoch(b"pid=7;epoch=not-a-generation\n"), None);
        assert_eq!(holder_epoch(&[0xff, 0xfe]), None);
        assert_eq!(
            holder_epoch(format!("pid=7;prior-epoch={}\n", uuid(2)).as_bytes()),
            None,
            "a field whose name merely ends in the one under test is not it"
        );
        assert_eq!(
            holder_epoch(format!("pid=7;epoch={}\n", uuid(2)).as_bytes()),
            Some(uuid(2)),
            "the negative cases are only meaningful against a reader that still reads"
        );
    }
}
