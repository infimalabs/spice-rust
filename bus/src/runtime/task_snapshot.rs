//! Incremental immutable task snapshots owned by the sole journal writer.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::*;
/// Deterministic work performed by one live-allocation projection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AllocationWork {
    pub task_visits: usize,
    pub standing_lookups: usize,
}

/// Instrumented live-allocation access over one immutable snapshot.
pub struct AllocationRead<'a> {
    snapshot: &'a TaskSnapshot,
    task_visits: std::cell::Cell<usize>,
    standing_lookups: std::cell::Cell<usize>,
}

impl<'a> AllocationRead<'a> {
    pub(super) fn new(snapshot: &'a TaskSnapshot) -> Self {
        Self {
            snapshot,
            task_visits: std::cell::Cell::new(0),
            standing_lookups: std::cell::Cell::new(0),
        }
    }

    /// Visit each live allocation head exactly once.
    pub fn frontier(&self) -> impl ExactSizeIterator<Item = &'a TaskHeads> + '_ {
        self.snapshot.allocation_frontier.iter().inspect(|_| {
            self.task_visits
                .set(self.task_visits.get().saturating_add(1));
        })
    }

    /// Resolve one direct dependency edge.
    #[must_use]
    pub fn dependency_standing(
        &self,
        task: &journal::crossing::TaskStamp,
    ) -> Option<DependencyStanding> {
        self.standing_lookups
            .set(self.standing_lookups.get().saturating_add(1));
        self.snapshot.dependency_standing.get(task).copied()
    }

    #[must_use]
    pub fn work(&self) -> AllocationWork {
        AllocationWork {
            task_visits: self.task_visits.get(),
            standing_lookups: self.standing_lookups.get(),
        }
    }
}

/// Impossible live-allocation authority retained for a stable read fault.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AllocationFault {
    ActiveTerminal { task: journal::crossing::TaskStamp },
}

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

/// Persistent logarithmic completion-terminal lookup.
///
/// Each level is either empty or holds \(2^n\) insertions. Adding one terminal
/// merges occupied levels like a binary carry, so unrelated commits share the
/// whole index, insertion is amortized logarithmic, and lookup probes at most
/// one hash table per level.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TerminalKey {
    stamp: [u8; journal::crossing::TaskStamp::WIDTH],
    stamp_len: u8,
    operation: journal::crossing::CanonicalUuid,
}

impl TerminalKey {
    fn new(
        task: &journal::crossing::TaskStamp,
        operation: journal::crossing::CanonicalUuid,
    ) -> Self {
        let source = task.as_str().as_bytes();
        let mut stamp = [0; journal::crossing::TaskStamp::WIDTH];
        stamp[..source.len()].copy_from_slice(source);
        Self {
            stamp,
            stamp_len: u8::try_from(source.len()).expect("task stamp width fits in one byte"),
            operation,
        }
    }
}

type TerminalLevel = HashMap<TerminalKey, journal::crossing::Seq>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TerminalIndex {
    levels: Arc<[Option<Arc<TerminalLevel>>]>,
}

impl TerminalIndex {
    fn pushed(
        &self,
        task: &journal::crossing::TaskStamp,
        operation: journal::crossing::CanonicalUuid,
        seq: journal::crossing::Seq,
    ) -> Self {
        let mut levels = self.levels.to_vec();
        let mut carry = HashMap::from([(TerminalKey::new(task, operation), seq)]);
        let mut level = 0;
        loop {
            if level == levels.len() {
                levels.push(Some(Arc::new(carry)));
                break;
            }
            let Some(existing) = levels[level].take() else {
                levels[level] = Some(Arc::new(carry));
                break;
            };
            let mut merged = HashMap::with_capacity(existing.len() + carry.len());
            merged.extend(existing.iter().map(|(key, seq)| (*key, *seq)));
            // A repeated operation is impossible in valid authority, but the
            // newest durable position still wins if malformed history carries
            // one; lower levels are always newer than higher levels.
            merged.extend(carry);
            carry = merged;
            level += 1;
        }
        Self {
            levels: Arc::from(levels),
        }
    }

    fn get(
        &self,
        task: &journal::crossing::TaskStamp,
        operation: journal::crossing::CanonicalUuid,
    ) -> Option<journal::crossing::Seq> {
        self.get_with_probes(task, operation).0
    }

    fn get_with_probes(
        &self,
        task: &journal::crossing::TaskStamp,
        operation: journal::crossing::CanonicalUuid,
    ) -> (Option<journal::crossing::Seq>, usize) {
        let key = TerminalKey::new(task, operation);
        let mut probes = 0;
        for level in self.levels.iter() {
            probes += 1;
            if let Some(found) = level.as_ref().and_then(|entries| entries.get(&key)) {
                return (Some(*found), probes);
            }
        }
        (None, probes)
    }
}

/// Answers derived from the whole history, advanced once per commit so no
/// later read walks the history for them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Indexes {
    trust_surfaces: HashMap<String, journal::crossing::Digest>,
    terminals: TerminalIndex,
    claims: HashMap<
        journal::crossing::Actor,
        (journal::crossing::Seq, Option<journal::crossing::ClaimCell>),
    >,
    context_anchors: HashMap<
        (
            journal::crossing::CanonicalUuid,
            journal::crossing::CanonicalUuid,
        ),
        journal::crossing::Seq,
    >,
    explicit_releases: HashMap<journal::crossing::TaskStamp, journal::crossing::Seq>,
    lane_occupants: HashMap<journal::crossing::CanonicalUuid, journal::crossing::CanonicalUuid>,
    lane_sessions: HashMap<
        (
            journal::crossing::CanonicalUuid,
            journal::crossing::CanonicalUuid,
        ),
        String,
    >,
    task_origins: HashMap<journal::crossing::TaskStamp, TaskOrigin>,
    duplicate_task_origins: HashSet<journal::crossing::TaskStamp>,
    invalid_creator_teams: HashSet<journal::crossing::TaskStamp>,
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
                    next.terminals = next
                        .terminals
                        .pushed(&done.task, terminal.operation, row.seq);
                    next.retire_task_origin(&done.task);
                }
            } else if let Some(deleted) = row.fact.read::<journal::crossing::body::TaskDeleted>() {
                next.retire_task_origin(&deleted.task);
            } else if let Some(closed) = row.fact.read::<journal::crossing::body::TaskClosed>() {
                next.retire_task_origin(&closed.task);
            } else if let Some(won) = row.fact.read::<journal::crossing::body::ClaimWon>() {
                next.claims.insert(won.actor, (row.seq, won.cell));
            } else if let Some(freed) = row.fact.read::<journal::crossing::body::ClaimFreed>() {
                next.explicit_releases.insert(freed.task, row.seq);
            } else if let Some(opened) = context_opening(&row.fact) {
                next.context_anchors.insert(opened, row.seq);
            }
        }
        next
    }

    fn retire_task_origin(&mut self, task: &journal::crossing::TaskStamp) {
        self.task_origins.remove(task);
        self.duplicate_task_origins.remove(task);
        self.invalid_creator_teams.remove(task);
        self.explicit_releases.remove(task);
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
        self.indexes.terminals.get(task, operation)
    }

    /// Immutable terminal-index levels probed by the same lookup.
    ///
    /// This deterministic work metric protects completion replay from silently
    /// returning to a retained-history scan.
    #[must_use]
    pub fn completion_terminal_lookup_probes(
        &self,
        task: &journal::crossing::TaskStamp,
        operation: journal::crossing::CanonicalUuid,
    ) -> usize {
        self.indexes.terminals.get_with_probes(task, operation).1
    }

    /// The newest claim one actor won: its position and claim-time cell.
    #[must_use]
    pub fn latest_claim(
        &self,
        actor: &journal::crossing::Actor,
    ) -> Option<(journal::crossing::Seq, Option<journal::crossing::ClaimCell>)> {
        self.indexes.claims.get(actor).cloned()
    }

    /// Whether this task's current claim head was established by an explicit release.
    #[must_use]
    pub fn is_explicit_release(
        &self,
        task: &journal::crossing::TaskStamp,
        claim: journal::crossing::Seq,
    ) -> bool {
        self.indexes.explicit_releases.get(task) == Some(&claim)
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

    fn retain_allocation_origins(&mut self, frontier: &[journal::TaskHeads]) {
        let live = frontier
            .iter()
            .map(|heads| heads.statement().body().born.clone())
            .collect::<HashSet<_>>();
        let indexes = Arc::make_mut(&mut self.indexes);
        indexes.task_origins.retain(|task, _| live.contains(task));
        indexes
            .duplicate_task_origins
            .retain(|task| live.contains(task));
        indexes
            .invalid_creator_teams
            .retain(|task| live.contains(task));
        indexes
            .explicit_releases
            .retain(|task, _| live.contains(task));
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

/// Maximum changed-head generations retained before one complete view becomes
/// the next immutable base.
const TASK_HEAD_DELTA_CHUNKS_MAX: usize = 32;
/// Small stores may retain this many changed heads without premature churn.
const TASK_HEAD_DELTA_HEADS_MIN: usize = 64;

/// Persistent task heads with one shared complete-board materialization.
///
/// Snapshot clones share both the base/deltas and the generation's lazy cache.
/// A bounded delta count and head count ensure a later complete read never
/// replays process-lifetime history.
#[derive(Debug)]
pub(super) struct TaskHeadStore {
    base: Arc<[journal::TaskHeads]>,
    deltas: Arc<[Arc<[journal::TaskHeads]>]>,
    delta_heads: usize,
    len: usize,
    materialized: Arc<std::sync::OnceLock<Arc<[journal::TaskHeads]>>>,
    materialization_visits: Arc<std::sync::atomic::AtomicUsize>,
}

impl TaskHeadStore {
    fn from_base(tasks: Vec<journal::TaskHeads>) -> Self {
        let base: Arc<[journal::TaskHeads]> = Arc::from(tasks);
        let materialized = Arc::new(std::sync::OnceLock::new());
        materialized
            .set(Arc::clone(&base))
            .expect("a new task-head materialization is empty");
        Self {
            len: base.len(),
            base,
            deltas: Arc::from([]),
            delta_heads: 0,
            materialized,
            materialization_visits: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    fn advanced(&self, moved: Vec<journal::TaskHeads>, len: usize) -> Self {
        if moved.is_empty() {
            let mut carried = self.clone();
            carried.len = len;
            return carried;
        }
        let delta_heads = self.delta_heads.saturating_add(moved.len());
        if self.deltas.len() >= TASK_HEAD_DELTA_CHUNKS_MAX
            || delta_heads > self.len.max(TASK_HEAD_DELTA_HEADS_MIN)
        {
            let tasks = merge_task_head_sources([self.tasks(), moved.as_slice()], len);
            return Self::from_base(tasks);
        }
        let mut deltas = self.deltas.to_vec();
        deltas.push(Arc::from(moved));
        Self {
            base: Arc::clone(&self.base),
            deltas: Arc::from(deltas),
            delta_heads,
            len,
            materialized: Arc::new(std::sync::OnceLock::new()),
            materialization_visits: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    const fn len(&self) -> usize {
        self.len
    }

    pub(super) fn tasks(&self) -> &[journal::TaskHeads] {
        self.materialized
            .get_or_init(|| {
                let tasks = merge_task_head_sources(
                    std::iter::once(self.base.as_ref())
                        .chain(self.deltas.iter().map(|delta| delta.as_ref())),
                    self.len,
                );
                self.materialization_visits.store(
                    self.base.len().saturating_add(self.delta_heads),
                    std::sync::atomic::Ordering::Relaxed,
                );
                Arc::from(tasks)
            })
            .as_ref()
    }

    pub(crate) fn materialization_visits(&self) -> usize {
        self.materialization_visits
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[derive(Debug)]
struct TaskHeadMergeCursor<'a> {
    rows: &'a [journal::TaskHeads],
    position: usize,
    generation: usize,
}

impl<'a> TaskHeadMergeCursor<'a> {
    fn current(&self) -> &'a journal::TaskHeads {
        &self.rows[self.position]
    }

    fn stamp(&self) -> &'a journal::crossing::TaskStamp {
        &self.current().statement().body().born
    }
}

impl PartialEq for TaskHeadMergeCursor<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.stamp() == other.stamp()
            && self.generation == other.generation
            && self.position == other.position
    }
}

impl Eq for TaskHeadMergeCursor<'_> {}

impl Ord for TaskHeadMergeCursor<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .stamp()
            .cmp(self.stamp())
            .then_with(|| self.generation.cmp(&other.generation))
            .then_with(|| self.position.cmp(&other.position))
    }
}

impl PartialOrd for TaskHeadMergeCursor<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn merge_task_head_sources<'a>(
    sources: impl IntoIterator<Item = &'a [journal::TaskHeads]>,
    expected_len: usize,
) -> Vec<journal::TaskHeads> {
    let mut pending = std::collections::BinaryHeap::new();
    for (generation, rows) in sources.into_iter().enumerate() {
        debug_assert!(
            rows.windows(2)
                .all(|pair| { pair[0].statement().body().born < pair[1].statement().body().born })
        );
        if !rows.is_empty() {
            pending.push(TaskHeadMergeCursor {
                rows,
                position: 0,
                generation,
            });
        }
    }

    let mut tasks = Vec::with_capacity(expected_len);
    while let Some(cursor) = pending.pop() {
        let stamp = cursor.stamp();
        let mut winner = cursor.current();
        let mut winner_generation = cursor.generation;
        push_next_task_head(&mut pending, cursor);
        while pending.peek().is_some_and(|next| next.stamp() == stamp) {
            let duplicate = pending
                .pop()
                .expect("the matching task-head cursor is present");
            if duplicate.generation > winner_generation {
                winner = duplicate.current();
                winner_generation = duplicate.generation;
            }
            push_next_task_head(&mut pending, duplicate);
        }
        tasks.push(winner.clone());
    }
    debug_assert_eq!(tasks.len(), expected_len);
    tasks
}

fn push_next_task_head<'a>(
    pending: &mut std::collections::BinaryHeap<TaskHeadMergeCursor<'a>>,
    mut cursor: TaskHeadMergeCursor<'a>,
) {
    cursor.position += 1;
    if cursor.position < cursor.rows.len() {
        pending.push(cursor);
    }
}

impl Clone for TaskHeadStore {
    fn clone(&self) -> Self {
        Self {
            base: Arc::clone(&self.base),
            deltas: Arc::clone(&self.deltas),
            delta_heads: self.delta_heads,
            len: self.len,
            materialized: Arc::clone(&self.materialized),
            materialization_visits: Arc::clone(&self.materialization_visits),
        }
    }
}

impl PartialEq for TaskHeadStore {
    fn eq(&self, other: &Self) -> bool {
        self.tasks() == other.tasks()
    }
}

impl Eq for TaskHeadStore {}

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
    /// Last ready generation. Changed task bundles are applied to its
    /// persistent head store and compact allocation indexes.
    prior: Option<TaskSnapshot>,
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
            snapshot.cursor.map(|_| snapshot.clone()),
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
        if let Some(prior) = &build.prior {
            if read.cursor().map_err(BusReadError::Head)? != build.cursor {
                return Err(BusReadError::Head(GuardError::Invalid(
                    "task snapshot authority moved between bounded pages".to_owned(),
                )));
            }
            let since = prior
                .cursor()
                .expect("a carried task snapshot has a durable cursor");
            if let Some(cursor) = build.cursor.filter(|cursor| *cursor > since) {
                let first = journal::crossing::Seq::new(since.get() + 1)
                    .expect("a position past a position is not zero");
                build.tasks = read
                    .changed_task_heads(first, cursor)
                    .map_err(BusReadError::Head)?;
            }
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
        let task_head_update_visits = build.tasks.len();
        let task_count = build.prior.as_ref().map_or(build.tasks.len(), |prior| {
            let since = prior
                .cursor()
                .expect("a carried task snapshot has a durable cursor");
            prior.tasks.len()
                + build
                    .tasks
                    .iter()
                    .filter(|heads| heads.statement().body().queue_origin > since)
                    .count()
        });
        let allocation = match &build.prior {
            Some(prior) => advance_allocation_index(read, prior, &build.tasks),
            None => rebuild_allocation_index(&build.tasks, &build.facts),
        }
        .map_err(BusReadError::Head)?;
        build.facts.retain_allocation_origins(&allocation.frontier);
        let tasks = match &build.prior {
            Some(prior) => prior
                .tasks
                .advanced(std::mem::take(&mut build.tasks), task_count),
            None => TaskHeadStore::from_base(std::mem::take(&mut build.tasks)),
        };
        Ok(BuildStep::Ready(TaskSnapshot {
            cursor: build.cursor,
            facts: build.facts,
            tasks,
            allocation_frontier: allocation.frontier,
            dependency_standing: allocation.standing,
            dependency_references: allocation.references,
            legacy_recent_claim_cells: allocation.legacy_recent_claim_cells,
            allocation_faults: allocation.faults,
            trust: Arc::from(build.trust),
            residency,
            materialization_steps: build.steps,
            task_head_update_visits,
        }))
    })
}

struct AllocationIndex {
    frontier: Arc<[journal::TaskHeads]>,
    references: Arc<HashMap<journal::crossing::TaskStamp, usize>>,
    standing: Arc<HashMap<journal::crossing::TaskStamp, DependencyStanding>>,
    legacy_recent_claim_cells: Arc<HashMap<journal::crossing::Actor, journal::crossing::ClaimCell>>,
    faults: Arc<[AllocationFault]>,
}

fn rebuild_allocation_index(
    tasks: &[journal::TaskHeads],
    facts: &TaskFacts,
) -> Result<AllocationIndex, GuardError> {
    let mut frontier = Vec::new();
    let mut references = HashMap::new();
    let mut faults = Vec::new();
    let mut prior = None;
    for heads in tasks {
        let task = heads.statement().body().born.clone();
        if prior.as_ref().is_some_and(|prior| prior >= &task) {
            return Err(GuardError::Invalid(
                "task snapshot heads are not in unique task-stamp order".to_owned(),
            ));
        }
        prior = Some(task.clone());
        match task_standing(heads) {
            DependencyStanding::Open => {
                add_dependencies(
                    &mut references,
                    heads.statement().body().dependencies.iter(),
                );
                frontier.push(heads.clone());
            }
            DependencyStanding::Completed | DependencyStanding::Deleted => {
                if actively_claimed(heads) {
                    faults.push(AllocationFault::ActiveTerminal { task });
                }
            }
        }
    }
    let mut standing = HashMap::with_capacity(references.len());
    for heads in tasks {
        let task = &heads.statement().body().born;
        if references.contains_key(task) {
            standing.insert(task.clone(), task_standing(heads));
        }
    }
    let mut legacy_recent_claim_cells = HashMap::new();
    let missing = facts
        .indexes
        .claims
        .iter()
        .filter(|&(_, (_, cell))| cell.is_none())
        .map(|(actor, (claimed, _))| ((actor.clone(), *claimed), actor.clone()))
        .collect::<HashMap<_, _>>();
    if !missing.is_empty() {
        for heads in tasks {
            let Some(claim) = heads.claim() else {
                continue;
            };
            if let Some(actor) = missing.get(&(claim.body().actor.clone(), claim.body().claimed)) {
                legacy_recent_claim_cells.insert(actor.clone(), claim.body().cell.clone());
            }
        }
    }
    Ok(AllocationIndex {
        frontier: Arc::from(frontier),
        references: Arc::new(references),
        standing: Arc::new(standing),
        legacy_recent_claim_cells: Arc::new(legacy_recent_claim_cells),
        faults: Arc::from(faults),
    })
}

fn advance_allocation_index(
    read: &journal::JournalRead<'_>,
    prior: &TaskSnapshot,
    moved: &[journal::TaskHeads],
) -> Result<AllocationIndex, GuardError> {
    if moved.is_empty() {
        return Ok(AllocationIndex {
            frontier: Arc::clone(&prior.allocation_frontier),
            references: Arc::clone(&prior.dependency_references),
            standing: Arc::clone(&prior.dependency_standing),
            legacy_recent_claim_cells: Arc::clone(&prior.legacy_recent_claim_cells),
            faults: Arc::clone(&prior.allocation_faults),
        });
    }
    let mut frontier = prior.allocation_frontier.to_vec();
    let mut references = (*prior.dependency_references).clone();
    let mut standing = (*prior.dependency_standing).clone();
    let mut faults = prior.allocation_faults.to_vec();
    let mut resolve = std::collections::HashSet::new();
    for heads in moved {
        let task = heads.statement().body().born.clone();
        faults.retain(|fault| fault.task() != &task);
        if let Ok(index) = frontier.binary_search_by(|kept| kept.statement().body().born.cmp(&task))
        {
            let old = frontier.remove(index);
            remove_dependencies(
                &mut references,
                &mut standing,
                old.statement().body().dependencies.iter(),
            );
        }
        match task_standing(heads) {
            DependencyStanding::Open => {
                for dependency in &heads.statement().body().dependencies {
                    if references.get(dependency).copied().unwrap_or(0) == 0 {
                        resolve.insert(dependency.clone());
                    }
                }
                add_dependencies(
                    &mut references,
                    heads.statement().body().dependencies.iter(),
                );
                let index = frontier
                    .binary_search_by(|kept| kept.statement().body().born.cmp(&task))
                    .unwrap_or_else(|index| index);
                frontier.insert(index, heads.clone());
            }
            DependencyStanding::Completed | DependencyStanding::Deleted => {
                if actively_claimed(heads) {
                    faults.push(AllocationFault::ActiveTerminal { task: task.clone() });
                }
            }
        }
        if references.contains_key(&task) {
            resolve.insert(task);
        }
    }
    faults.sort_by(|left, right| left.task().cmp(right.task()));
    for task in resolve {
        if !references.contains_key(&task) {
            standing.remove(&task);
            continue;
        }
        let heads = match moved.binary_search_by(|heads| heads.statement().body().born.cmp(&task)) {
            Ok(index) => Some(moved[index].clone()),
            Err(_) => read.task_heads_for(&task)?,
        };
        match heads {
            Some(heads) => {
                standing.insert(task, task_standing(&heads));
            }
            None => {
                standing.remove(&task);
            }
        }
    }
    Ok(AllocationIndex {
        frontier: Arc::from(frontier),
        references: Arc::new(references),
        standing: Arc::new(standing),
        legacy_recent_claim_cells: Arc::clone(&prior.legacy_recent_claim_cells),
        faults: Arc::from(faults),
    })
}

fn add_dependencies<'a>(
    references: &mut HashMap<journal::crossing::TaskStamp, usize>,
    dependencies: impl Iterator<Item = &'a journal::crossing::TaskStamp>,
) {
    for dependency in dependencies {
        *references.entry(dependency.clone()).or_default() += 1;
    }
}

fn remove_dependencies<'a>(
    references: &mut HashMap<journal::crossing::TaskStamp, usize>,
    standing: &mut HashMap<journal::crossing::TaskStamp, DependencyStanding>,
    dependencies: impl Iterator<Item = &'a journal::crossing::TaskStamp>,
) {
    for dependency in dependencies {
        let Some(count) = references.get_mut(dependency) else {
            continue;
        };
        *count -= 1;
        if *count == 0 {
            references.remove(dependency);
            standing.remove(dependency);
        }
    }
}

fn task_standing(heads: &journal::TaskHeads) -> DependencyStanding {
    match heads.workflow().body().status.as_str() {
        "completed" => DependencyStanding::Completed,
        "deleted" => DependencyStanding::Deleted,
        _ => DependencyStanding::Open,
    }
}

fn actively_claimed(heads: &journal::TaskHeads) -> bool {
    heads.claim().is_some_and(|claim| {
        claim.body().disposition == journal::crossing::ClaimDisposition::Active
    })
}

impl AllocationFault {
    fn task(&self) -> &journal::crossing::TaskStamp {
        match self {
            Self::ActiveTerminal { task } => task,
        }
    }
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
