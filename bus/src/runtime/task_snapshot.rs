//! Incremental immutable task snapshots owned by the sole journal writer.

use std::collections::{BTreeMap, HashMap};
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
    indexes: Arc<Indexes>,
}

/// Creation route evidence normalized while resident task history advances.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskOrigin {
    /// Actor route candidate (`operator`, `spice`, session, or occupant
    /// identity); request projection validates its route grammar.
    Identity(String),
    /// A lane creation whose occupant is resolved from current residency.
    ResidencyIdentity(journal::crossing::CanonicalUuid),
}

/// Answers derived from the whole history, advanced once per commit so no
/// later read walks the history for them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Indexes {
    trust_surfaces: BTreeMap<String, journal::crossing::Digest>,
    terminals: BTreeMap<
        journal::crossing::TaskStamp,
        BTreeMap<journal::crossing::CanonicalUuid, journal::crossing::Seq>,
    >,
    claims: HashMap<
        journal::crossing::Actor,
        (journal::crossing::Seq, Option<journal::crossing::ClaimCell>),
    >,
    context_anchors: BTreeMap<
        (
            journal::crossing::CanonicalUuid,
            journal::crossing::CanonicalUuid,
        ),
        journal::crossing::Seq,
    >,
    explicit_releases: std::collections::BTreeSet<journal::crossing::Seq>,
    lane_occupants: BTreeMap<journal::crossing::CanonicalUuid, journal::crossing::CanonicalUuid>,
    lane_sessions: BTreeMap<
        (
            journal::crossing::CanonicalUuid,
            journal::crossing::CanonicalUuid,
        ),
        String,
    >,
    task_origins: BTreeMap<journal::crossing::TaskStamp, TaskOrigin>,
    duplicate_task_origins: std::collections::BTreeSet<journal::crossing::TaskStamp>,
    invalid_creator_teams: std::collections::BTreeSet<journal::crossing::TaskStamp>,
}

impl Indexes {
    fn advanced(&self, rows: &[journal::crossing::Envelope<journal::crossing::AnyFact>]) -> Self {
        let mut next = self.clone();
        for row in rows {
            if let Some(declared) = row
                .fact
                .read::<journal::crossing::body::LaneSessionDeclared>()
            {
                next.lane_sessions
                    .insert((declared.lane, declared.occupant), declared.identity);
            } else if let Some(started) = row.fact.read::<journal::crossing::body::LaneStarted>() {
                next.lane_occupants.insert(started.lane, started.occupant);
                next.context_anchors
                    .insert((started.lane, started.occupant), row.seq);
            } else if let Some(added) = row.fact.read::<journal::crossing::body::TaskAdded>() {
                let origin = match &row.actor {
                    journal::crossing::Actor::Operator => {
                        TaskOrigin::Identity("operator".to_owned())
                    }
                    journal::crossing::Actor::Spice => TaskOrigin::Identity("spice".to_owned()),
                    journal::crossing::Actor::Lane(lane) => next.lane_occupants.get(lane).map_or(
                        TaskOrigin::ResidencyIdentity(*lane),
                        |occupant| {
                            TaskOrigin::Identity(
                                next.lane_sessions
                                    .get(&(*lane, *occupant))
                                    .cloned()
                                    .unwrap_or_else(|| occupant.to_string()),
                            )
                        },
                    ),
                };
                if !matches!(row.actor, journal::crossing::Actor::Lane(_))
                    && added.creation.creator_team().is_some()
                {
                    next.invalid_creator_teams.insert(added.task.clone());
                }
                if next
                    .task_origins
                    .insert(added.task.clone(), origin)
                    .is_some()
                {
                    next.duplicate_task_origins.insert(added.task);
                }
            } else if let Some(pended) = row.fact.read::<journal::crossing::body::TrustPended>() {
                next.trust_surfaces.insert(pended.surface, pended.digest);
            } else if let Some(done) = row.fact.read::<journal::crossing::body::TaskDone>() {
                if let Some(terminal) = done.terminal {
                    next.terminals
                        .entry(done.task)
                        .or_default()
                        .insert(terminal.operation, row.seq);
                }
            } else if let Some(won) = row.fact.read::<journal::crossing::body::ClaimWon>() {
                next.claims.insert(won.actor, (row.seq, won.cell));
            } else if row
                .fact
                .read::<journal::crossing::body::ClaimFreed>()
                .is_some()
            {
                next.explicit_releases.insert(row.seq);
            } else if let Some(opened) = context_opening(&row.fact) {
                next.context_anchors.insert(opened, row.seq);
            }
        }
        next
    }
}

/// The lane and occupant whose context one fact opens: its start, a renewal,
/// or a compaction.
#[must_use]
pub fn context_opening(
    fact: &journal::crossing::AnyFact,
) -> Option<(
    journal::crossing::CanonicalUuid,
    journal::crossing::CanonicalUuid,
)> {
    use journal::crossing::body::{LaneCompacted, LaneRenewed, LaneStarted};
    if let Some(body) = fact.read::<LaneCompacted>() {
        Some((body.lane, body.occupant))
    } else if let Some(body) = fact.read::<LaneRenewed>() {
        Some((body.lane, body.occupant))
    } else {
        fact.read::<LaneStarted>()
            .map(|body| (body.lane, body.occupant))
    }
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
        let indexes = Arc::new(self.indexes.advanced(&rows));
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
            indexes,
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

    /// The fact at one durable position, if this history retains it.
    ///
    /// Only the chunk spanning the position is searched, so the walk pays
    /// for the chunks newer than it and one binary search.
    #[must_use]
    pub fn at(
        &self,
        seq: journal::crossing::Seq,
    ) -> Option<&journal::crossing::Envelope<journal::crossing::AnyFact>> {
        let mut cursor = self.tail.as_deref();
        while let Some(chunk) = cursor {
            if chunk.rows.first().is_some_and(|row| row.seq <= seq) {
                let index = chunk.rows.binary_search_by_key(&seq, |row| row.seq).ok()?;
                return Some(&chunk.rows[index]);
            }
            cursor = chunk.prior.as_deref();
        }
        None
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
        self.indexes.trust_surfaces.get(surface)
    }

    /// Position of the completion that landed one task operation's terminal.
    #[must_use]
    pub fn completion_terminal(
        &self,
        task: &journal::crossing::TaskStamp,
        operation: journal::crossing::CanonicalUuid,
    ) -> Option<journal::crossing::Seq> {
        self.indexes.terminals.get(task)?.get(&operation).copied()
    }

    /// The newest claim one actor won: its position and claim-time cell.
    #[must_use]
    pub fn latest_claim(
        &self,
        actor: &journal::crossing::Actor,
    ) -> Option<(journal::crossing::Seq, Option<journal::crossing::ClaimCell>)> {
        self.indexes.claims.get(actor).cloned()
    }

    /// Whether a claim head position was established by an explicit release.
    #[must_use]
    pub fn is_explicit_release(&self, claim: journal::crossing::Seq) -> bool {
        self.indexes.explicit_releases.contains(&claim)
    }

    /// Creation route indexed while task history advances.
    #[must_use]
    pub fn task_origin(&self, task: &journal::crossing::TaskStamp) -> Option<&TaskOrigin> {
        self.indexes.task_origins.get(task)
    }

    /// Whether one task has more than one creation fact.
    #[must_use]
    pub fn has_duplicate_task_origin(&self, task: &journal::crossing::TaskStamp) -> bool {
        self.indexes.duplicate_task_origins.contains(task)
    }

    /// Whether a non-lane creation improperly names a creator team.
    #[must_use]
    pub fn has_invalid_creator_team(&self, task: &journal::crossing::TaskStamp) -> bool {
        self.indexes.invalid_creator_teams.contains(task)
    }

    /// Vendor session identity declared for one lane occupant, if any.
    #[must_use]
    pub fn lane_session_identity(
        &self,
        lane: journal::crossing::CanonicalUuid,
        occupant: journal::crossing::CanonicalUuid,
    ) -> Option<&str> {
        self.indexes
            .lane_sessions
            .get(&(lane, occupant))
            .map(String::as_str)
    }
    /// The newest fact opening a context for one lane occupant.
    #[must_use]
    pub fn context_anchor(
        &self,
        lane: journal::crossing::CanonicalUuid,
        occupant: journal::crossing::CanonicalUuid,
    ) -> Option<journal::crossing::Seq> {
        self.indexes.context_anchors.get(&(lane, occupant)).copied()
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
    /// The tasks the last ready snapshot held, at the cursor it held them:
    /// this build reads only the heads that moved past it, so a commit costs
    /// what it changed rather than every task the journal remembers.
    prior: Option<(journal::crossing::Seq, Arc<[journal::TaskHeads]>)>,
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
            prior: None,
            trust: Vec::new(),
            trust_after: None,
            stage: SnapshotStage::Facts,
            steps: 0,
        }
    }
}

pub(super) fn invalidate_task_snapshot(state: &mut TaskSnapshotState) {
    let carried = match state {
        TaskSnapshotState::Ready(snapshot) => Some((
            snapshot.facts.clone(),
            snapshot
                .cursor
                .map(|cursor| (cursor, Arc::clone(&snapshot.tasks))),
        )),
        TaskSnapshotState::Building(build) => Some((build.facts.clone(), build.prior.clone())),
        TaskSnapshotState::Empty | TaskSnapshotState::Fault(_) => None,
    };
    if let Some((facts, prior)) = carried {
        let mut build = TaskSnapshotBuild::new(facts);
        build.prior = prior;
        *state = TaskSnapshotState::Building(build);
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
                None,
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
        if let Some((since, prior)) = build.prior.take() {
            if read.cursor().map_err(BusReadError::Head)? != build.cursor {
                return Err(BusReadError::Head(GuardError::Invalid(
                    "task snapshot authority moved between bounded pages".to_owned(),
                )));
            }
            build.tasks = match build.cursor.filter(|cursor| *cursor > since) {
                Some(cursor) => {
                    let first = journal::crossing::Seq::new(since.get() + 1)
                        .expect("a position past a position is not zero");
                    let moved = read
                        .changed_task_heads(first, cursor)
                        .map_err(BusReadError::Head)?;
                    merged(&prior, moved)
                }
                None => prior.to_vec(),
            };
            build.stage = SnapshotStage::Trust;
            return Ok(BuildStep::Pending(build));
        }
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

/// The prior tasks with each moved bundle replacing its stamp's, or taking
/// its place in stamp order when the task is new; both inputs arrive sorted.
fn merged(prior: &[journal::TaskHeads], moved: Vec<journal::TaskHeads>) -> Vec<journal::TaskHeads> {
    let mut tasks = prior.to_vec();
    for heads in moved {
        let stamp = &heads.statement().body().born;
        match tasks.binary_search_by(|kept| kept.statement().body().born.cmp(stamp)) {
            Ok(index) => tasks[index] = heads,
            Err(index) => tasks.insert(index, heads),
        }
    }
    tasks
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
        let (allocation_frontier, dependency_standing, claim_head_cells) =
            allocation_index(&build.tasks).map_err(BusReadError::Head)?;
        Ok(BuildStep::Ready(TaskSnapshot {
            cursor: build.cursor,
            facts: build.facts,
            tasks: Arc::from(build.tasks),
            allocation_frontier: Arc::from(allocation_frontier),
            dependency_standing: Arc::new(dependency_standing),
            claim_head_cells: Arc::new(claim_head_cells),
            trust: Arc::from(build.trust),
            residency,
            materialization_steps: build.steps,
        }))
    })
}
fn allocation_index(
    tasks: &[journal::TaskHeads],
) -> Result<
    (
        Vec<usize>,
        BTreeMap<journal::crossing::TaskStamp, DependencyStanding>,
        BTreeMap<(journal::crossing::Actor, journal::crossing::Seq), journal::crossing::ClaimCell>,
    ),
    GuardError,
> {
    let mut frontier = Vec::new();
    let mut standing = BTreeMap::new();
    let mut claim_head_cells = BTreeMap::new();
    let mut prior = None;
    for (index, heads) in tasks.iter().enumerate() {
        let task = heads.statement().body().born.clone();
        if prior.as_ref().is_some_and(|prior| prior >= &task) {
            return Err(GuardError::Invalid(
                "task snapshot heads are not in unique task-stamp order".to_owned(),
            ));
        }
        prior = Some(task.clone());
        let state = match heads.workflow().body().status.as_str() {
            "completed" => DependencyStanding::Completed,
            "deleted" => DependencyStanding::Deleted,
            _ => DependencyStanding::Open,
        };
        standing.insert(task, state);
        let actively_claimed = heads.claim().is_some_and(|claim| {
            claim.body().disposition == journal::crossing::ClaimDisposition::Active
        });
        if let Some(claim) = heads.claim() {
            claim_head_cells.insert(
                (claim.body().actor.clone(), claim.body().claimed),
                claim.body().cell.clone(),
            );
        }
        if state == DependencyStanding::Open || actively_claimed {
            frontier.push(index);
        }
    }
    Ok((frontier, standing, claim_head_cells))
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
