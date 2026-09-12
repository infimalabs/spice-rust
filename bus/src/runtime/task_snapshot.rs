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
    Building(TaskSnapshotBuild),
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
            Self::Building(build) => Err(pending_error(build.facts.horizon())),
        }
    }
}

pub(super) struct TaskSnapshotBuild {
    facts: TaskFacts,
    cursor: Option<journal::crossing::Seq>,
    tasks: Vec<journal::TaskHeads>,
    task_after: Option<journal::crossing::TaskStamp>,
    trust: Vec<JournalHead<journal::crossing::TrustHead>>,
    trust_after: Option<journal::crossing::Digest>,
    stage: SnapshotStage,
    steps: u64,
}

#[derive(Clone, Copy)]
enum SnapshotStage {
    Facts,
    Tasks,
    Trust,
}

impl TaskSnapshotBuild {
    fn new(facts: TaskFacts) -> Self {
        Self {
            facts,
            cursor: None,
            tasks: Vec::new(),
            task_after: None,
            trust: Vec::new(),
            trust_after: None,
            stage: SnapshotStage::Facts,
            steps: 0,
        }
    }
}

pub(super) fn invalidate_task_snapshot(state: &mut TaskSnapshotState) {
    let facts = match state {
        TaskSnapshotState::Ready(snapshot) => Some(snapshot.facts.clone()),
        TaskSnapshotState::Building(build) => Some(build.facts.clone()),
        TaskSnapshotState::Empty | TaskSnapshotState::Fault(_) => None,
    };
    if let Some(facts) = facts {
        *state = TaskSnapshotState::Building(TaskSnapshotBuild::new(facts));
    }
}

pub(super) fn advance_task_snapshot(
    journal: &Journal,
    state: &mut TaskSnapshotState,
) -> Result<(), BusReadError> {
    let mut build = match std::mem::take(state) {
        TaskSnapshotState::Empty => TaskSnapshotBuild::new(TaskFacts::default()),
        TaskSnapshotState::Building(build) => build,
        ready @ TaskSnapshotState::Ready(_) => {
            *state = ready;
            return Ok(());
        }
        TaskSnapshotState::Fault(detail) => {
            *state = TaskSnapshotState::Fault(detail.clone());
            return Err(BusReadError::Head(GuardError::Invalid(detail.clone())));
        }
    };
    build.steps = build.steps.saturating_add(1);
    let step = match build.stage {
        SnapshotStage::Facts => advance_facts(journal, build),
        SnapshotStage::Tasks => advance_tasks(journal, build),
        SnapshotStage::Trust => advance_trust(journal, build),
    }
    .map_err(map_read_error);
    match step {
        Ok(BuildStep::Pending(build)) => {
            *state = TaskSnapshotState::Building(build);
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

fn advance_facts(
    journal: &Journal,
    mut build: TaskSnapshotBuild,
) -> Result<BuildStep, journal::ReadError<BusReadError>> {
    journal.read_with(|read| {
        let page = read
            .selected_fact_page(
                &resident_authority_kinds(),
                build.facts.horizon(),
                journal::FactReadBudget::RETAINED,
            )
            .map_err(BusReadError::Head)?;
        if let Some(horizon) = page.continuation.or(page.horizon) {
            build.facts = build
                .facts
                .append_selected(page.rows, horizon)
                .map_err(BusReadError::Head)?;
        }
        if page.continuation.is_none() {
            build.cursor = page.horizon;
            build.stage = SnapshotStage::Tasks;
        }
        Ok(BuildStep::Pending(build))
    })
}

fn advance_tasks(
    journal: &Journal,
    mut build: TaskSnapshotBuild,
) -> Result<BuildStep, journal::ReadError<BusReadError>> {
    journal.read_with(|read| {
        let page = read
            .task_head_page(build.task_after.as_ref(), journal::FactReadBudget::RETAINED)
            .map_err(BusReadError::Head)?;
        require_cursor(page.witness, build.cursor)?;
        build.tasks.extend(page.rows);
        build.task_after = page.continuation;
        if build.task_after.is_none() {
            build.stage = SnapshotStage::Trust;
        }
        Ok(BuildStep::Pending(build))
    })
}

fn advance_trust(
    journal: &Journal,
    mut build: TaskSnapshotBuild,
) -> Result<BuildStep, journal::ReadError<BusReadError>> {
    journal.read_with(|read| {
        let page = read
            .trust_head_page(
                build.trust_after.as_ref(),
                journal::FactReadBudget::RETAINED,
            )
            .map_err(BusReadError::Head)?;
        require_cursor(page.witness, build.cursor)?;
        build.trust.extend(page.rows);
        build.trust_after = page.continuation;
        if build.trust_after.is_some() {
            return Ok(BuildStep::Pending(build));
        }
        let residency = read.residency_head().map_err(BusReadError::Head)?;
        if read.cursor().map_err(BusReadError::Head)? != build.cursor {
            return Err(BusReadError::Head(GuardError::Invalid(
                "task snapshot authority moved during completion".to_owned(),
            )));
        }
        Ok(BuildStep::Ready(TaskSnapshot {
            cursor: build.cursor,
            facts: build.facts,
            tasks: Arc::from(build.tasks),
            trust: Arc::from(build.trust),
            residency,
            materialization_steps: build.steps,
        }))
    })
}

enum BuildStep {
    Pending(TaskSnapshotBuild),
    Ready(TaskSnapshot),
}

fn require_cursor(
    witness: journal::CurrentHeadWitness,
    expected: Option<journal::crossing::Seq>,
) -> Result<(), BusReadError> {
    (witness.cursor == expected).then_some(()).ok_or_else(|| {
        BusReadError::Head(GuardError::Invalid(
            "task snapshot authority moved between bounded pages".to_owned(),
        ))
    })
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
