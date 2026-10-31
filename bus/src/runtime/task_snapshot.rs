//! Incremental immutable task snapshots owned by the sole journal writer.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use journal::crossing::rooms::RoomQuestionItem;

mod allocation;
mod disposition;
mod persistent_index;
mod review_authors;
mod selection;
mod store;

pub use review_authors::{AuthorshipFault, ClaimAuthor, FindingAuthors};
pub(super) use store::{
    TaskHeadStore, TaskSnapshotState, advance_task_snapshot, invalidate_task_snapshot,
};

use allocation::{advance as advance_allocation_index, rebuild as rebuild_allocation_index};
use review_authors::ReviewAuthors;
use selection::CandidateSelection;

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
        let key = TerminalKey::new(task, operation);
        self.levels.iter().find_map(|level| {
            level
                .as_ref()
                .and_then(|entries| entries.get(&key))
                .copied()
        })
    }
}

/// Each continuing session's asked Design items, keyed by lane and session.
type NoticedAsks = HashMap<
    (
        journal::crossing::CanonicalUuid,
        journal::crossing::rooms::DesignSession,
    ),
    Arc<HashSet<(journal::crossing::TaskStamp, Option<RoomQuestionItem>)>>,
>;

/// Each continuing session's recorded shadows by Design and statement head,
/// keyed by lane and session.
type DesignShadows = HashMap<
    (
        journal::crossing::CanonicalUuid,
        journal::crossing::rooms::DesignSession,
    ),
    HashSet<(journal::crossing::TaskStamp, journal::crossing::Seq)>,
>;

/// Answers derived from the whole history, advanced once per commit so no
/// later read walks the history for them. Each index that grows with history
/// is shared through `Arc` and copied only by the commit that changes it
/// (`Arc::make_mut`); maps bounded by lanes or surfaces stay plain.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Indexes {
    trust_surfaces: HashMap<String, journal::crossing::Digest>,
    terminals: TerminalIndex,
    claims:
        HashMap<journal::crossing::Actor, (journal::crossing::Seq, journal::crossing::ClaimCell)>,
    design_notices: Arc<
        HashMap<
            (
                journal::crossing::CanonicalUuid,
                journal::crossing::rooms::DesignSession,
            ),
            (
                journal::crossing::Seq,
                journal::crossing::rooms::DesignNoticed,
            ),
        >,
    >,
    /// Each continuing session's asked Design items; `None` is a whole-Design
    /// ask.
    noticed_asks: Arc<NoticedAsks>,
    /// Each continuing session's recorded shadows by Design and statement
    /// head, so one shortfall is recorded once per statement revision.
    design_shadows: Arc<DesignShadows>,
    room_publication_positions: Arc<
        HashMap<(journal::crossing::Actor, journal::crossing::TaskStamp), journal::crossing::Seq>,
    >,
    /// Each lane's newest contribution against one Design's question item.
    item_contributions: Arc<
        HashMap<
            (
                journal::crossing::CanonicalUuid,
                journal::crossing::TaskStamp,
                RoomQuestionItem,
            ),
            journal::crossing::Seq,
        >,
    >,
    /// Each lane's newest leave of one Design room.
    room_leaves: Arc<
        HashMap<
            (
                journal::crossing::CanonicalUuid,
                journal::crossing::TaskStamp,
            ),
            journal::crossing::Seq,
        >,
    >,
    context_anchors: Arc<
        HashMap<
            (
                journal::crossing::CanonicalUuid,
                journal::crossing::CanonicalUuid,
            ),
            journal::crossing::Seq,
        >,
    >,
    explicit_releases: Arc<HashMap<journal::crossing::TaskStamp, journal::crossing::Seq>>,
    /// Births by occupant, shared until a commit records another lane start.
    births: Arc<HashMap<journal::crossing::CanonicalUuid, BirthRecord>>,
    lane_occupants: HashMap<journal::crossing::CanonicalUuid, journal::crossing::CanonicalUuid>,
    lane_sessions: Arc<
        HashMap<
            (
                journal::crossing::CanonicalUuid,
                journal::crossing::CanonicalUuid,
            ),
            journal::crossing::rooms::DesignSession,
        >,
    >,
    occupant_session_declarations: Arc<
        HashMap<
            (
                journal::crossing::CanonicalUuid,
                journal::crossing::CanonicalUuid,
            ),
            journal::crossing::Seq,
        >,
    >,
    session_declarations: Arc<
        HashMap<
            (
                journal::crossing::CanonicalUuid,
                journal::crossing::rooms::DesignSession,
            ),
            journal::crossing::Seq,
        >,
    >,
    session_compactions: Arc<
        HashMap<
            (
                journal::crossing::CanonicalUuid,
                journal::crossing::rooms::DesignSession,
            ),
            journal::crossing::Seq,
        >,
    >,
    /// Origins of the allocation frontier's tasks, pruned to the live
    /// frontier on each build; shared until a commit adds or retires one.
    task_origins: Arc<HashMap<journal::crossing::TaskStamp, TaskOrigin>>,
    duplicate_task_origins: Arc<HashSet<journal::crossing::TaskStamp>>,
    invalid_creator_teams: Arc<HashSet<journal::crossing::TaskStamp>>,
    /// Every Design the current working-group partition holds.
    grouped_designs: Arc<HashSet<journal::crossing::TaskStamp>>,
    /// The reason each Design's collapse recorded, shared until a commit
    /// records another conclusion.
    collapse_reasons: Arc<HashMap<journal::crossing::TaskStamp, String>>,
    /// Each Plan's recorded disposition; see [`disposition`].
    plan_dispositions:
        Arc<HashMap<journal::crossing::TaskStamp, journal::crossing::body::PlanDisposition>>,
    /// Candidate-version precedence classifying outputs for selection waiting.
    candidate_selection: CandidateSelection,
    /// Pass-cycle evidence for every Design, shared until a commit records a
    /// fact the cycle reads.
    design_cycles: Arc<journal::crossing::rooms::DesignCycles>,
    /// Phase authors of every finding review, advanced by claim, completion,
    /// and review facts.
    review_authors: ReviewAuthors,
}

impl Indexes {
    fn replace_partition(&mut self, partition: journal::crossing::rooms::WorkingGroupPartition) {
        self.grouped_designs = Arc::new(
            partition
                .groups
                .into_iter()
                .flat_map(|group| group.anchors)
                .collect(),
        );
    }

    fn advanced(&self, rows: &[journal::crossing::Envelope<journal::crossing::AnyFact>]) -> Self {
        let mut next = self.clone();
        for row in rows {
            next.observe_publication(row);
            next.observe(row);
        }
        next
    }

    fn observe_publication(
        &mut self,
        row: &journal::crossing::Envelope<journal::crossing::AnyFact>,
    ) {
        let Some(room) = row.fact.room_record() else {
            return;
        };
        Arc::make_mut(&mut self.room_publication_positions)
            .insert((row.actor.clone(), room.clone()), row.seq);
        if let Some(contributed) = row.fact.read::<journal::crossing::rooms::RoomContributed>() {
            if let Some(item) = contributed.item {
                Arc::make_mut(&mut self.item_contributions)
                    .insert((contributed.lane, contributed.room, item), row.seq);
            }
        } else if let Some(left) = row.fact.read::<journal::crossing::rooms::RoomLeft>() {
            Arc::make_mut(&mut self.room_leaves).insert((left.lane, left.room), row.seq);
        }
    }

    fn observe(&mut self, row: &journal::crossing::Envelope<journal::crossing::AnyFact>) {
        use journal::crossing::FactKind;

        let kind = row.fact.kind();
        if journal::crossing::rooms::DesignCycles::concerns(kind) {
            Arc::make_mut(&mut self.design_cycles).observe(row.seq, &row.actor, &row.fact);
        }
        match kind {
            FactKind::LaneSessionDeclared | FactKind::LaneStarted => self.observe_lane(row),
            FactKind::TaskAdded => self.observe_task_added(row),
            FactKind::DesignProposed
            | FactKind::PreparationDecided
            | FactKind::CandidateWithdrawn
            | FactKind::TrustPended => self.observe_selection(row),
            FactKind::WorkingGroupMoved
            | FactKind::WorkingGroupsArranged
            | FactKind::DesignNoticed
            | FactKind::DesignShadowed => self.observe_design(row),
            FactKind::TaskDone
            | FactKind::TaskDeleted
            | FactKind::TaskClosed
            | FactKind::TaskConcluded
            | FactKind::ClaimWon
            | FactKind::ClaimFreed => self.observe_task_authority(row),
            FactKind::ClaimCarried
            | FactKind::ClaimRebound
            | FactKind::ClaimLapsed
            | FactKind::TaskReviewed => self.observe_review_authors(row),
            FactKind::LaneCompacted => {
                self.observe_compaction(row);
                if let Some(opened) = context_opening(&row.fact) {
                    Arc::make_mut(&mut self.context_anchors).insert(opened, row.seq);
                }
            }
            _ => {
                if let Some(opened) = context_opening(&row.fact) {
                    Arc::make_mut(&mut self.context_anchors).insert(opened, row.seq);
                }
            }
        }
    }

    fn observe_lane(&mut self, row: &journal::crossing::Envelope<journal::crossing::AnyFact>) {
        if let Some(declared) = row
            .fact
            .read::<journal::crossing::body::LaneSessionDeclared>()
        {
            let session = journal::crossing::rooms::DesignSession {
                vendor: declared.vendor,
                identity: declared.identity,
            };
            Arc::make_mut(&mut self.lane_sessions)
                .insert((declared.lane, declared.occupant), session.clone());
            Arc::make_mut(&mut self.occupant_session_declarations)
                .insert((declared.lane, declared.occupant), row.seq);
            let first = (declared.lane, session);
            if !self.session_declarations.contains_key(&first) {
                Arc::make_mut(&mut self.session_declarations).insert(first, row.seq);
            }
            return;
        }
        let Some(started) = row.fact.read::<journal::crossing::body::LaneStarted>() else {
            return;
        };
        self.lane_occupants.insert(started.lane, started.occupant);
        Arc::make_mut(&mut self.context_anchors).insert((started.lane, started.occupant), row.seq);
        let births = Arc::make_mut(&mut self.births);
        let record = BirthRecord::recorded(
            births.get(&started.occupant).copied(),
            started.lane,
            row.seq,
        );
        births.insert(started.occupant, record);
    }

    fn observe_compaction(
        &mut self,
        row: &journal::crossing::Envelope<journal::crossing::AnyFact>,
    ) {
        let Some(compacted) = row.fact.read::<journal::crossing::body::LaneCompacted>() else {
            return;
        };
        let Some(session) = self
            .lane_sessions
            .get(&(compacted.lane, compacted.occupant))
            .cloned()
        else {
            return;
        };
        Arc::make_mut(&mut self.session_compactions).insert((compacted.lane, session), row.seq);
    }

    fn observe_task_added(
        &mut self,
        row: &journal::crossing::Envelope<journal::crossing::AnyFact>,
    ) {
        let Some(added) = row.fact.read::<journal::crossing::body::TaskAdded>() else {
            return;
        };
        let origin = match &row.actor {
            journal::crossing::Actor::Lane(lane) => self.lane_occupants.get(lane).map_or(
                TaskOrigin::ResidencyIdentity(*lane),
                |occupant| {
                    TaskOrigin::Identity(
                        self.lane_sessions.get(&(*lane, *occupant)).map_or_else(
                            || occupant.to_string(),
                            |session| session.identity.clone(),
                        ),
                    )
                },
            ),
            _ => TaskOrigin::Identity(row.actor.to_string()),
        };
        if !matches!(row.actor, journal::crossing::Actor::Lane(_))
            && added.creation.creator_team().is_some()
        {
            Arc::make_mut(&mut self.invalid_creator_teams).insert(added.task.clone());
        }
        if Arc::make_mut(&mut self.task_origins)
            .insert(added.task.clone(), origin)
            .is_some()
        {
            Arc::make_mut(&mut self.duplicate_task_origins).insert(added.task);
        }
    }

    fn observe_selection(&mut self, row: &journal::crossing::Envelope<journal::crossing::AnyFact>) {
        if let Some(retained) = row.fact.read::<journal::crossing::rooms::DesignProposed>() {
            self.candidate_selection.retain(retained.candidate);
        } else if let Some(decided) = row
            .fact
            .read::<journal::crossing::rooms::PreparationDecided>()
        {
            self.candidate_selection.decide(
                decided.decision.outcome.candidate(),
                decided.decision.decision,
            );
        } else if let Some(withdrawn) = row
            .fact
            .read::<journal::crossing::rooms::CandidateWithdrawn>()
        {
            self.candidate_selection
                .withdraw(&withdrawn.withdrawal.candidate);
        } else if let Some(pended) = row.fact.read::<journal::crossing::body::TrustPended>() {
            self.trust_surfaces.insert(pended.surface, pended.digest);
        }
    }

    fn observe_design(&mut self, row: &journal::crossing::Envelope<journal::crossing::AnyFact>) {
        if let Some(moved) = row
            .fact
            .read::<journal::crossing::rooms::WorkingGroupMoved>()
        {
            self.replace_partition(moved.partition);
        } else if let Some(arranged) = row
            .fact
            .read::<journal::crossing::rooms::WorkingGroupsArranged>()
        {
            self.replace_partition(arranged.partition);
        } else if let Some(noticed) = row.fact.read::<journal::crossing::rooms::DesignNoticed>() {
            let session_key = (noticed.lane, noticed.session.clone());
            let asked = Arc::make_mut(&mut self.noticed_asks)
                .entry(session_key.clone())
                .or_default();
            let asked = Arc::make_mut(asked);
            if noticed.asks.is_empty() {
                asked.insert((noticed.design.clone(), None));
            }
            for ask in &noticed.asks {
                asked.insert((noticed.design.clone(), Some(ask.item.clone())));
            }
            Arc::make_mut(&mut self.design_notices).insert(session_key, (row.seq, noticed));
        } else if let Some(shadowed) = row.fact.read::<journal::crossing::rooms::DesignShadowed>() {
            Arc::make_mut(&mut self.design_shadows)
                .entry((shadowed.lane, shadowed.session))
                .or_default()
                .insert((shadowed.design, shadowed.statement));
        }
    }

    fn observe_task_authority(
        &mut self,
        row: &journal::crossing::Envelope<journal::crossing::AnyFact>,
    ) {
        if let Some(done) = row.fact.read::<journal::crossing::body::TaskDone>() {
            self.review_authors.complete(&row.actor, &done.task);
            self.observe_done(row, done);
        } else if let Some(deleted) = row.fact.read::<journal::crossing::body::TaskDeleted>() {
            self.retire_task(&deleted.task);
        } else if let Some(closed) = row.fact.read::<journal::crossing::body::TaskClosed>() {
            self.retire_task(&closed.task);
        } else if let Some(concluded) = row.fact.read::<journal::crossing::body::TaskConcluded>() {
            self.retire_task(&concluded.task);
            if let journal::crossing::body::DesignConclusion::Collapsed { reason } =
                concluded.conclusion
            {
                Arc::make_mut(&mut self.collapse_reasons).insert(concluded.task, reason);
            }
        } else if let Some(won) = row.fact.read::<journal::crossing::body::ClaimWon>() {
            self.review_authors
                .claim(won.task.clone(), won.lane, won.occupant);
            self.claims.insert(won.actor, (row.seq, won.cell));
        } else if let Some(freed) = row.fact.read::<journal::crossing::body::ClaimFreed>() {
            self.review_authors.release(&freed.task);
            Arc::make_mut(&mut self.explicit_releases).insert(freed.task, row.seq);
        }
    }

    fn observe_done(
        &mut self,
        row: &journal::crossing::Envelope<journal::crossing::AnyFact>,
        done: journal::crossing::body::TaskDone,
    ) {
        let journal::crossing::body::TaskDone {
            task,
            terminal,
            disposition,
            ..
        } = done;
        self.observe_disposition(&task, disposition);
        if let Some(terminal) = terminal {
            self.terminals = self.terminals.pushed(&task, terminal.operation, row.seq);
            // A phase advance also records a terminal answer, but its task stays
            // allocatable in the next phase and keeps its creation origin.
            if terminal.status == journal::crossing::body::TaskCompletionStatus::Completed {
                self.retire_task(&task);
            }
        }
    }

    /// Forget a retired task's origin records, copying only the indexes that
    /// still name it.
    fn retire_task(&mut self, task: &journal::crossing::TaskStamp) {
        if self.task_origins.contains_key(task) {
            Arc::make_mut(&mut self.task_origins).remove(task);
        }
        if self.duplicate_task_origins.contains(task) {
            Arc::make_mut(&mut self.duplicate_task_origins).remove(task);
        }
        if self.invalid_creator_teams.contains(task) {
            Arc::make_mut(&mut self.invalid_creator_teams).remove(task);
        }
        if self.explicit_releases.contains_key(task) {
            Arc::make_mut(&mut self.explicit_releases).remove(task);
        }
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

    /// The decision on the latest unwithdrawn candidate version naming one
    /// output, or `None` while that version is open or none ruled on it. An
    /// older verdict stays in room history but no longer classifies an output
    /// a later open candidate re-proposes.
    #[must_use]
    pub fn preparation_decision(
        &self,
        output: &journal::crossing::TaskStamp,
    ) -> Option<&journal::crossing::rooms::DecisionRef> {
        self.indexes.candidate_selection.decision(output)
    }

    /// The newest claim one actor won: its position and claim-time cell.
    #[must_use]
    pub fn latest_claim(
        &self,
        actor: &journal::crossing::Actor,
    ) -> Option<(journal::crossing::Seq, journal::crossing::ClaimCell)> {
        self.indexes.claims.get(actor).cloned()
    }

    /// Newest durable Design invitation recorded for this continuing session.
    #[must_use]
    pub fn latest_design_notice(
        &self,
        lane: journal::crossing::CanonicalUuid,
        session: &journal::crossing::rooms::DesignSession,
    ) -> Option<(
        journal::crossing::Seq,
        &journal::crossing::rooms::DesignNoticed,
    )> {
        self.indexes
            .design_notices
            .get(&(lane, session.clone()))
            .map(|(at, notice)| (*at, notice))
    }

    /// Whether this continuing session was already asked about this Design's
    /// `item`, or about the whole Design when `item` is absent.
    #[must_use]
    pub fn design_was_asked_in_session(
        &self,
        lane: journal::crossing::CanonicalUuid,
        session: &journal::crossing::rooms::DesignSession,
        design: &journal::crossing::TaskStamp,
        item: Option<&RoomQuestionItem>,
    ) -> bool {
        self.indexes
            .noticed_asks
            .get(&(lane, session.clone()))
            .is_some_and(|asked| asked.contains(&(design.clone(), item.cloned())))
    }

    /// Whether this continuing session was already asked about this Design at
    /// all: about any of its items or about the whole Design.
    #[must_use]
    pub fn design_was_noticed_in_session(
        &self,
        lane: journal::crossing::CanonicalUuid,
        session: &journal::crossing::rooms::DesignSession,
        design: &journal::crossing::TaskStamp,
    ) -> bool {
        self.indexes
            .noticed_asks
            .get(&(lane, session.clone()))
            .is_some_and(|asked| asked.iter().any(|(asked, _)| asked == design))
    }

    /// Whether this continuing session already recorded a shadow for this
    /// Design at this statement head.
    #[must_use]
    pub fn design_was_shadowed_in_session(
        &self,
        lane: journal::crossing::CanonicalUuid,
        session: &journal::crossing::rooms::DesignSession,
        design: &journal::crossing::TaskStamp,
        statement: journal::crossing::Seq,
    ) -> bool {
        self.indexes
            .design_shadows
            .get(&(lane, session.clone()))
            .is_some_and(|shadowed| shadowed.contains(&(design.clone(), statement)))
    }

    /// Whether `lane` answered an ask about `room`'s `item` after `after`: a
    /// contribution against that item, or a leave of the room. A whole-Design
    /// ask, with no item, is answered by any record the lane makes there.
    #[must_use]
    pub fn design_ask_answered_after(
        &self,
        lane: journal::crossing::CanonicalUuid,
        room: &journal::crossing::TaskStamp,
        item: Option<&RoomQuestionItem>,
        after: journal::crossing::Seq,
    ) -> bool {
        let later = |at: Option<&journal::crossing::Seq>| at.is_some_and(|at| *at > after);
        let Some(item) = item else {
            return later(
                self.indexes
                    .room_publication_positions
                    .get(&(journal::crossing::Actor::Lane(lane), room.clone())),
            );
        };
        later(
            self.indexes
                .item_contributions
                .get(&(lane, room.clone(), item.clone())),
        ) || later(self.indexes.room_leaves.get(&(lane, room.clone())))
    }

    /// Whether `lane` answered an ask about `room`'s `item` during this
    /// continuing session, as [`Self::design_ask_answered_after`] judges.
    #[must_use]
    pub fn design_ask_answered_in_session(
        &self,
        lane: journal::crossing::CanonicalUuid,
        session: &journal::crossing::rooms::DesignSession,
        room: &journal::crossing::TaskStamp,
        item: Option<&RoomQuestionItem>,
    ) -> bool {
        self.indexes
            .session_declarations
            .get(&(lane, session.clone()))
            .is_some_and(|declared| self.design_ask_answered_after(lane, room, item, *declared))
    }

    /// Each charged question item's series for `design`, if any item was
    /// charged.
    #[must_use]
    pub fn design_items(
        &self,
        design: &journal::crossing::TaskStamp,
    ) -> Option<&journal::crossing::rooms::DesignItems> {
        self.indexes.design_cycles.items(design)
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

    /// Most recently born occupant recorded for one logical lane, whether or
    /// not the current residency still seats that occupant.
    #[must_use]
    pub fn latest_lane_occupant(
        &self,
        lane: journal::crossing::CanonicalUuid,
    ) -> Option<journal::crossing::CanonicalUuid> {
        self.indexes.lane_occupants.get(&lane).copied()
    }

    /// Newest compaction recorded for one continuing vendor session.
    #[must_use]
    pub fn session_compaction(
        &self,
        lane: journal::crossing::CanonicalUuid,
        session: &journal::crossing::rooms::DesignSession,
    ) -> Option<journal::crossing::Seq> {
        self.indexes
            .session_compactions
            .get(&(lane, session.clone()))
            .copied()
    }

    /// Vendor session declared for one lane occupant, if any.
    #[must_use]
    pub fn lane_session(
        &self,
        lane: journal::crossing::CanonicalUuid,
        occupant: journal::crossing::CanonicalUuid,
    ) -> Option<&journal::crossing::rooms::DesignSession> {
        self.indexes.lane_sessions.get(&(lane, occupant))
    }

    /// Exact declaration that bound this lane occupant to its vendor session.
    #[must_use]
    pub fn lane_session_declaration(
        &self,
        lane: journal::crossing::CanonicalUuid,
        occupant: journal::crossing::CanonicalUuid,
    ) -> Option<journal::crossing::Seq> {
        self.indexes
            .occupant_session_declarations
            .get(&(lane, occupant))
            .copied()
    }

    /// Exact vendor session identity declared for one lane occupant, if any.
    #[must_use]
    pub fn lane_session_identity(
        &self,
        lane: journal::crossing::CanonicalUuid,
        occupant: journal::crossing::CanonicalUuid,
    ) -> Option<&str> {
        self.lane_session(lane, occupant)
            .map(|session| session.identity.as_str())
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

    /// The address of one occupant on its lane, derived from the one
    /// `lane::started` fact recording its birth there.
    ///
    /// A compaction or renewal never moves it: only the birth counts.
    pub fn occupant_address(
        &self,
        lane: journal::crossing::CanonicalUuid,
        occupant: journal::crossing::CanonicalUuid,
    ) -> Result<journal::crossing::OccupantAddress, AddressRefusal> {
        BirthRecord::address(self.birth_record(occupant), lane, occupant)
    }

    /// The address each of `lanes` offers, keyed by lane: a seated
    /// occupant's birth address, or why that lane has none.
    #[must_use]
    pub fn seat_addresses(
        &self,
        lanes: &[journal::crossing::LaneResidency],
    ) -> std::collections::BTreeMap<journal::crossing::CanonicalUuid, journal::crossing::SeatAddress>
    {
        super::occupant_address::seat_addresses(lanes, |occupant| self.birth_record(occupant))
    }

    pub(super) fn birth_record(
        &self,
        occupant: journal::crossing::CanonicalUuid,
    ) -> Option<BirthRecord> {
        self.indexes.births.get(&occupant).copied()
    }

    /// Current task identities the working-group partition holds.
    pub fn room_anchor_tasks(
        &self,
    ) -> impl ExactSizeIterator<Item = &journal::crossing::TaskStamp> {
        self.indexes.grouped_designs.iter()
    }

    /// Whether the current working-group partition holds `task`.
    #[must_use]
    pub fn grouped(&self, task: &journal::crossing::TaskStamp) -> bool {
        self.indexes.grouped_designs.contains(task)
    }

    /// One open Design's pass-cycle evidence, judging maturity against
    /// `statement`, its current statement head.
    #[must_use]
    pub fn design(
        &self,
        statement: &journal::crossing::TaskHead,
    ) -> journal::crossing::rooms::DesignCycleFacts {
        self.indexes
            .design_cycles
            .facts(&statement.born, &statement.acceptance)
    }

    /// The reason the operator's collapse of Design `task` recorded.
    #[must_use]
    pub fn collapse_reason(&self, task: &journal::crossing::TaskStamp) -> Option<&str> {
        self.indexes.collapse_reasons.get(task).map(String::as_str)
    }

    /// Keep only the allocation frontier's tasks in the origin indexes. An
    /// index naming no retired task stays shared with the prior snapshot.
    fn retain_allocation_origins(&mut self, frontier: &[journal::TaskHeads]) {
        let live = frontier
            .iter()
            .map(|heads| heads.statement().body().born.clone())
            .collect::<HashSet<_>>();
        let indexes = Arc::make_mut(&mut self.indexes);
        if indexes.task_origins.keys().any(|task| !live.contains(task)) {
            Arc::make_mut(&mut indexes.task_origins).retain(|task, _| live.contains(task));
        }
        if indexes
            .duplicate_task_origins
            .iter()
            .any(|task| !live.contains(task))
        {
            Arc::make_mut(&mut indexes.duplicate_task_origins).retain(|task| live.contains(task));
        }
        if indexes
            .invalid_creator_teams
            .iter()
            .any(|task| !live.contains(task))
        {
            Arc::make_mut(&mut indexes.invalid_creator_teams).retain(|task| live.contains(task));
        }
        if indexes
            .explicit_releases
            .keys()
            .any(|task| !live.contains(task))
        {
            Arc::make_mut(&mut indexes.explicit_releases).retain(|task, _| live.contains(task));
        }
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
