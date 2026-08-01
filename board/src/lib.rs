//! Tasks and coordination as checked views over journal heads.

#![forbid(unsafe_code)]

use std::fmt;

#[cfg(not(target_arch = "wasm32"))]
pub mod allocate;
pub mod apply;
pub mod claim;
pub mod graph;
pub mod handout;
pub mod next;
pub mod oops;
pub mod phase;
#[cfg(not(target_arch = "wasm32"))]
pub mod possession;
pub mod task;
pub mod urgency;

pub use apply::{
    ApplyError, ApplyReceipt, AuthorityTransaction, Preview, PreviewRefusal, apply, digest_of,
};
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    schema_version: u32,
    cursor: fold::Cursor,
    entries: Vec<Entry>,
}

impl Snapshot {
    /// Admit a snapshot only for the journal schema this build owns.
    pub fn new(
        schema_version: u32,
        cursor: fold::Cursor,
        entries: Vec<Entry>,
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

    /// Reconstruct one owned board answer from authoritative journal heads.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read(journal: &journal::Journal) -> Result<Self, SnapshotReadError> {
        let (cursor, heads) = journal
            .read(|read| Ok((read.cursor()?, read.task_heads()?)))
            .map_err(SnapshotReadError::Journal)?;
        let entries = heads
            .into_iter()
            .map(|heads| {
                let (statement, workflow, claim, lease) = heads.into_bodies();
                let handle = statement.handle.clone();
                Entry::new(statement, workflow, claim, lease)
                    .map_err(|source| SnapshotReadError::Task { handle, source })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let cursor = cursor.map_or_else(fold::Cursor::empty, fold::Cursor::at);
        Ok(Self {
            schema_version: journal::SCHEMA_VERSION,
            cursor,
            entries,
        })
    }

    /// Journal schema that supplied the heads.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Greatest journal position covered by this answer.
    #[must_use]
    pub const fn cursor(&self) -> fold::Cursor {
        self.cursor
    }

    /// Checked current head bundles.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
}

/// Why authoritative journal heads could not become one board snapshot.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub enum SnapshotReadError {
    /// The journal could not return one complete checked head answer.
    Journal(journal::GuardError),
    /// One storage-valid head bundle violated board policy.
    Task {
        handle: wire::TaskHandle,
        source: TaskError,
    },
}

#[cfg(not(target_arch = "wasm32"))]
impl fmt::Display for SnapshotReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Journal(error) => {
                write!(formatter, "board snapshot journal read failed: {error}")
            }
            Self::Task { handle, source } => {
                write!(
                    formatter,
                    "board snapshot task {handle} is invalid: {source}"
                )
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl std::error::Error for SnapshotReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Journal(error) => Some(error),
            Self::Task { source, .. } => Some(source),
        }
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
