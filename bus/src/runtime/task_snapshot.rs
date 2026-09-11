//! Incremental immutable task snapshots owned by the sole journal writer.

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
}

#[derive(Debug, Eq, PartialEq)]
struct TaskFactChunk {
    prior: Option<Arc<TaskFactChunk>>,
    rows: Box<[journal::crossing::Envelope<journal::crossing::AnyFact>]>,
}

impl TaskFacts {
    fn from_selected(
        rows: Vec<journal::crossing::Envelope<journal::crossing::AnyFact>>,
        horizon: Option<journal::crossing::Seq>,
    ) -> Self {
        let len = rows.len();
        Self {
            tail: (!rows.is_empty()).then(|| {
                Arc::new(TaskFactChunk {
                    prior: None,
                    rows: rows.into_boxed_slice(),
                })
            }),
            len,
            horizon,
        }
    }

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

pub(super) fn load_task_snapshot(journal: &Journal) -> Result<TaskSnapshot, BusReadError> {
    journal
        .read_with(|read| {
            let kinds = resident_authority_kinds();
            Ok(TaskSnapshot {
                cursor: read.cursor().map_err(BusReadError::Head)?,
                facts: TaskFacts::from_selected(
                    read.selected_facts(&kinds, None)
                        .map_err(BusReadError::Head)?,
                    read.cursor().map_err(BusReadError::Head)?,
                ),
                tasks: Arc::from(read.task_heads().map_err(BusReadError::Head)?),
                trust: Arc::from(read.trust_heads().map_err(BusReadError::Head)?),
                residency: read.residency_head().map_err(BusReadError::Head)?,
                operator_steering: Arc::from(
                    read.pending_operator_steering()
                        .map_err(BusReadError::Head)?,
                ),
            })
        })
        .map_err(map_read_error)
}

pub(super) fn refresh_task_snapshot(
    journal: &Journal,
    cached: &mut Option<TaskSnapshot>,
    commit: JournalCommit,
) {
    let Some(current) = cached.take() else {
        return;
    };
    *cached = advance_task_snapshot(journal, &current, commit).ok();
}

fn advance_task_snapshot(
    journal: &Journal,
    current: &TaskSnapshot,
    commit: JournalCommit,
) -> Result<TaskSnapshot, BusReadError> {
    let first = journal::crossing::Seq::new(commit.first)
        .expect("a durable commit begins at a nonzero position");
    let last = journal::crossing::Seq::new(commit.last)
        .expect("a durable commit ends at a nonzero position");
    let (cursor, rows, changed, trust, residency, operator_steering) = journal
        .read_with(|read| {
            let cursor = read.cursor().map_err(BusReadError::Head)?;
            if cursor != Some(last) {
                return Err(BusReadError::Head(GuardError::Invalid(format!(
                    "task snapshot refresh expected cursor {last}, observed {cursor:?}"
                ))));
            }
            let kinds = resident_authority_kinds();
            let rows = read
                .selected_facts(&kinds, current.cursor)
                .map_err(BusReadError::Head)?;
            let changed = read
                .changed_task_heads(first, last)
                .map_err(BusReadError::Head)?;
            let trust = read.trust_heads().map_err(BusReadError::Head)?;
            let residency = read.residency_head().map_err(BusReadError::Head)?;
            let operator_steering = read
                .pending_operator_steering()
                .map_err(BusReadError::Head)?;
            Ok((cursor, rows, changed, trust, residency, operator_steering))
        })
        .map_err(map_read_error)?;
    let facts = current
        .facts
        .append_selected(rows, last)
        .map_err(BusReadError::Head)?;
    let tasks = if changed.is_empty() {
        current.tasks.clone()
    } else {
        let mut tasks = current.tasks.to_vec();
        for heads in changed {
            let stamp = &heads.statement().body().born;
            match tasks.binary_search_by(|current| current.statement().body().born.cmp(stamp)) {
                Ok(index) => tasks[index] = heads,
                Err(index) => tasks.insert(index, heads),
            }
        }
        Arc::from(tasks)
    };
    Ok(TaskSnapshot {
        cursor,
        facts,
        tasks,
        trust: Arc::from(trust),
        residency,
        operator_steering: Arc::from(operator_steering),
    })
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
