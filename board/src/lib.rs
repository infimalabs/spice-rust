//! Tasks and coordination as checked views over journal heads.

#![forbid(unsafe_code)]

use std::fmt;

pub mod apply;
pub mod claim;
pub mod graph;
pub mod handout;
pub mod next;
pub mod oops;
pub mod phase;
pub mod task;
pub mod urgency;

pub use apply::{ApplyError, ApplyReceipt, AuthorityTransaction, Preview, PreviewRefusal, apply};
pub use claim::{Active, ClaimRefusal};
pub use graph::{Diagram, View};
pub use next::{Selection, select};
pub use phase::{PhaseRefusal, Transition, TransitionKind};
pub use task::{Dependency, Entry, Family, Project, TaskError};
pub use urgency::Urgency;

/// One read barrier over current journal task heads.
///
/// The schema identity comes from `journal`, completeness is stated with the
/// shared fold cursor, and rows are checked [`Entry`] values. This is a
/// disposable answer; none of it can update authority.
#[derive(Clone, Copy, Debug)]
pub struct Snapshot<'a> {
    schema_version: u32,
    cursor: fold::Cursor,
    entries: &'a [Entry],
}

impl<'a> Snapshot<'a> {
    /// Admit a snapshot only for the journal schema this build owns.
    pub fn new(
        schema_version: u32,
        cursor: fold::Cursor,
        entries: &'a [Entry],
    ) -> Result<Self, SnapshotRefusal> {
        if schema_version != journal::SCHEMA_VERSION {
            return Err(SnapshotRefusal {
                found: schema_version,
                current: journal::SCHEMA_VERSION,
            });
        }
        Ok(Self {
            schema_version,
            cursor,
            entries,
        })
    }

    /// Journal schema that supplied the heads.
    #[must_use]
    pub const fn schema_version(self) -> u32 {
        self.schema_version
    }

    /// Greatest journal position covered by this answer.
    #[must_use]
    pub const fn cursor(self) -> fold::Cursor {
        self.cursor
    }

    /// Checked current head bundles.
    #[must_use]
    pub const fn entries(self) -> &'a [Entry] {
        self.entries
    }
}

/// Snapshot came from a journal schema this build does not own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotRefusal {
    pub found: u32,
    pub current: u32,
}

impl fmt::Display for SnapshotRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "board snapshot schema {} does not match current journal schema {}",
            self.found, self.current
        )
    }
}

impl std::error::Error for SnapshotRefusal {}
