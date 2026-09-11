//! Incremental immutable task snapshots owned by the sole journal writer.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::*;

/// Persistent task-authority history shared by queued task snapshots.
///
/// Each durable commit contributes one immutable chunk. Cloning a task
/// snapshot therefore shares all prior decoded facts instead of copying or
/// decoding the complete journal again.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TaskFacts {
    tail: Option<Arc<TaskFactChunk>>,
    len: usize,
    horizon: Option<journal::crossing::Seq>,
    trust_surfaces: Arc<BTreeMap<String, journal::crossing::Digest>>,
}

#[derive(Debug, Eq, PartialEq)]
struct TaskFactChunk {
    prior: Option<Arc<TaskFactChunk>>,
    rows: Box<[journal::crossing::Envelope<journal::crossing::AnyFact>]>,
}

impl TaskFacts {
    fn append_selected(
        &self,
        rows: Vec<journal::crossing::Envelope<journal::crossing::AnyFact>>,
        horizon: journal::crossing::Seq,
    ) -> Result<Self, GuardError> {
        if self.horizon.is_some_and(|current| horizon <= current)
            || rows.windows(2).any(|pair| pair[0].seq >= pair[1].seq)
            || rows.iter().any(|row| {
                self.horizon.is_some_and(|current| row.seq <= current) || row.seq > horizon
            })
        {
            return Err(GuardError::Invalid(format!(
                "task snapshot append after {:?} through {horizon} received {:?}..={:?}",
                self.horizon,
                rows.first().map(|row| row.seq),
                rows.last().map(|row| row.seq)
            )));
        }
        let added = rows.len();
        let trust_surfaces = advance_trust_surfaces(&self.trust_surfaces, &rows);
        Ok(Self {
            tail: if rows.is_empty() {
                self.tail.clone()
            } else {
                Some(Arc::new(TaskFactChunk {
                    prior: self.tail.clone(),
                    rows: rows.into_boxed_slice(),
                }))
            },
            len: self.len.saturating_add(added),
            horizon: Some(horizon),
            trust_surfaces,
        })
    }

    /// Facts in ascending durable position without flattening shared chunks.
    #[must_use]
    pub fn iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = &journal::crossing::Envelope<journal::crossing::AnyFact>>
    {
        let mut chunks = Vec::new();
        let mut cursor = self.tail.as_deref();
        while let Some(chunk) = cursor {
            chunks.push(chunk.rows.as_ref());
            cursor = chunk.prior.as_deref();
        }
        chunks.reverse();
        chunks.into_iter().flat_map(<[_]>::iter)
    }

    /// Facts strictly newer than one already admitted cursor.
    ///
    /// The walk begins at the newest immutable chunk and stops as soon as a
    /// chunk ends at or before `cursor`. A consumer that keeps up with commits
    /// therefore touches only the committed delta, independent of retained
    /// history.
    #[must_use]
    pub fn after(
        &self,
        cursor: Option<journal::crossing::Seq>,
    ) -> impl DoubleEndedIterator<Item = &journal::crossing::Envelope<journal::crossing::AnyFact>>
    {
        let mut chunks = Vec::new();
        let mut tail = self.tail.as_deref();
        while let Some(chunk) = tail {
            let newer = chunk
                .rows
                .last()
                .is_some_and(|row| cursor.is_none_or(|cursor| row.seq > cursor));
            if !newer {
                break;
            }
            chunks.push(chunk.rows.as_ref());
            tail = chunk.prior.as_deref();
        }
        chunks.reverse();
        chunks
            .into_iter()
            .flat_map(<[_]>::iter)
            .filter(move |row| cursor.is_none_or(|cursor| row.seq > cursor))
    }

    /// Number of decoded facts shared by this snapshot.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether this history contains no facts.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Journal horizon through which this authority projection is complete.
    #[must_use]
    pub const fn horizon(&self) -> Option<journal::crossing::Seq> {
        self.horizon
    }

    /// Latest trust digest pended for one named executable surface.
    #[must_use]
    pub fn latest_trust_digest(&self, surface: &str) -> Option<&journal::crossing::Digest> {
        self.trust_surfaces.get(surface)
    }
}

fn advance_trust_surfaces(
    current: &Arc<BTreeMap<String, journal::crossing::Digest>>,
    rows: &[journal::crossing::Envelope<journal::crossing::AnyFact>],
) -> Arc<BTreeMap<String, journal::crossing::Digest>> {
    let mut surfaces = current.as_ref().clone();
    for row in rows {
        if let Some(pended) = row.fact.read::<journal::crossing::body::TrustPended>() {
            surfaces.insert(pended.surface, pended.digest);
        }
    }
    Arc::new(surfaces)
}

impl Drop for TaskFacts {
    fn drop(&mut self) {
        let mut tail = self.tail.take();
        while let Some(chunk) = tail {
            let Ok(TaskFactChunk { prior, rows: _ }) = Arc::try_unwrap(chunk) else {
                break;
            };
            tail = prior;
        }
    }
}

#[derive(Default)]
pub(super) enum TaskSnapshotState {
    #[default]
    Empty,
    Building {
        facts: TaskFacts,
        steps: u64,
    },
    Ready(TaskSnapshot),
    Fault(String),
}

impl TaskSnapshotState {
    pub(super) const fn needs_step(&self) -> bool {
        matches!(self, Self::Empty | Self::Building { .. })
    }

    pub(super) fn current(&self) -> Result<TaskSnapshot, BusReadError> {
        match self {
            Self::Ready(snapshot) => Ok(snapshot.clone()),
            Self::Fault(detail) => Err(BusReadError::Head(GuardError::Invalid(detail.clone()))),
            Self::Empty => Err(pending_error(None)),
            Self::Building { facts, .. } => Err(pending_error(facts.horizon())),
        }
    }
}

pub(super) fn invalidate_task_snapshot(state: &mut TaskSnapshotState) {
    if let TaskSnapshotState::Ready(snapshot) = state {
        *state = TaskSnapshotState::Building {
            facts: snapshot.facts.clone(),
            steps: 0,
        };
    }
}

pub(super) fn advance_task_snapshot(
    journal: &Journal,
    state: &mut TaskSnapshotState,
) -> Result<(), BusReadError> {
    let (facts, steps) = match state {
        TaskSnapshotState::Empty => (TaskFacts::default(), 0),
        TaskSnapshotState::Building { facts, steps } => (facts.clone(), *steps),
        TaskSnapshotState::Ready(_) => return Ok(()),
        TaskSnapshotState::Fault(detail) => {
            return Err(BusReadError::Head(GuardError::Invalid(detail.clone())));
        }
    };
    let after = facts.horizon();
    let step = journal
        .read_with(|read| {
            let kinds = resident_authority_kinds();
            let page = read
                .selected_fact_page(&kinds, after, journal::FactReadBudget::RETAINED)
                .map_err(BusReadError::Head)?;
            let complete_through = page.continuation.or(page.horizon);
            let facts = match complete_through {
                Some(horizon) => facts
                    .append_selected(page.rows, horizon)
                    .map_err(BusReadError::Head)?,
                None => facts,
            };
            if page.continuation.is_some() {
                return Ok(BuildStep::Pending(facts));
            }
            Ok(BuildStep::Ready(TaskSnapshot {
                cursor: page.horizon,
                facts,
                tasks: Arc::from(read.task_heads().map_err(BusReadError::Head)?),
                trust: Arc::from(read.trust_heads().map_err(BusReadError::Head)?),
                residency: read.residency_head().map_err(BusReadError::Head)?,
                materialization_steps: steps.saturating_add(1),
            }))
        })
        .map_err(map_read_error);
    match step {
        Ok(BuildStep::Pending(facts)) => {
            *state = TaskSnapshotState::Building {
                facts,
                steps: steps.saturating_add(1),
            };
            Ok(())
        }
        Ok(BuildStep::Ready(snapshot)) => {
            *state = TaskSnapshotState::Ready(snapshot);
            Ok(())
        }
        Err(error) => {
            *state = TaskSnapshotState::Fault(error.to_string());
            Err(error)
        }
    }
}

enum BuildStep {
    Pending(TaskFacts),
    Ready(TaskSnapshot),
}

fn pending_error(through: Option<journal::crossing::Seq>) -> BusReadError {
    BusReadError::Head(GuardError::Invalid(format!(
        "task snapshot materialization is pending after {through:?}"
    )))
}

/// Facts consumed by resident decisions and bounded presentation recovery.
///
/// Transcript ingestion is high-volume accounting, not resident authority.
/// Its consumers use the origin-and-kind index directly, so the shared
/// authority cache never replays it. Keeping every other registered kind here
/// preserves the existing TaskFacts contract as the vocabulary grows.
fn resident_authority_kinds() -> Vec<journal::crossing::FactKind> {
    journal::crossing::FactKind::ALL
        .iter()
        .copied()
        .filter(|kind| *kind != journal::crossing::FactKind::JournalIngested)
        .collect()
}

fn map_read_error(error: journal::ReadError<BusReadError>) -> BusReadError {
    match error {
        journal::ReadError::Journal(error) => BusReadError::Head(error),
        journal::ReadError::Answer(error) => error,
    }
}
