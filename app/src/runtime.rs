//! Capability-scoped state used by the real Wasm canvas runtime.
//!
//! Every mutation enters through an admitted acceptance, response, or stream
//! frame. Stream facts contribute credential-free envelope metadata; accepted
//! steering replies and live steering facts contribute the operator records
//! rendered by conversation.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use wire::intent::{
    BoardAnswer, BoardTask, GlobalSettingsAnswer, InboxAnswer, LaneAnswer, LaneLiveness,
    TASK_RELATION_DEPTH_LIMIT, TASK_RELATION_NODE_LIMIT, TASK_RELATION_ROOT_LIMIT,
    TaskDetailAnswer, TaskRelationKind, TaskRelationNodeRef, TaskRelations, TaskRelationsAnswer,
    VendorCatalogAnswer,
};
use wire::{
    Accepted, Actor, AgentActivityKind, AgentMessage, CanonicalUuid, CommandKind, FactKind,
    MessageLocus, Outcome, Packed, Response, Seq, StreamCursor, StreamFrame, ViewSubject,
};

use crate::pane::{Principal, SteerPane};
use crate::socket::{Admitted, SessionScope};
use crate::watchband;

mod message_cards;
#[cfg(any(target_arch = "wasm32", test))]
mod publication;
mod stream;
mod work_history;
#[cfg(any(target_arch = "wasm32", rust_analyzer))]
pub(crate) use publication::CanvasSurface;
pub use stream::{StreamApplication, StreamDirective, StreamTerminal};
pub use work_history::{WorkHistoryRequest, WorkHistoryState, work_history_completion_cutoff};
pub(crate) type QuoteTasks = BTreeMap<String, Vec<wire::TaskHandle>>;

/// Whether a topology mutation response leaves Canvas owing an authoritative read.
///
/// Successful topology replies already carry the new table. A refusal or
/// fault carries no replacement table, so the browser must re-pull rather
/// than continue rendering the revision the rejected gesture observed.
#[cfg(any(target_arch = "wasm32", test))]
#[must_use]
pub(crate) const fn topology_mutation_requires_refresh<T>(outcome: &Outcome<T>) -> bool {
    !matches!(outcome, Outcome::Reply(_))
}

/// Board snapshot fields used by the canvas, excluding residency capabilities.
#[derive(Clone, PartialEq, Eq)]
pub struct BoardView {
    /// Finalized journal schema observed with this task snapshot.
    pub schema: u32,
    /// Greatest durable position covered by every task head.
    pub cursor: Seq,
    /// Current task evidence bundles.
    pub tasks: Vec<BoardTask>,
    /// Resident allocation order: the task offered next comes first.
    pub offer: Vec<wire::TaskStamp>,
    /// Resident steering-key origin index: the Work each quoted direction caused.
    pub(crate) quote_tasks: Rc<QuoteTasks>,
    /// Complete environment observed at the same board snapshot barrier.
    pub(crate) environment: wire::Environment,
    /// Current resident state.
    pub resident: wire::ResidentState,
}

impl From<&BoardAnswer> for BoardView {
    fn from(answer: &BoardAnswer) -> Self {
        let tasks = answer.tasks.clone();
        let offer = answer.offer.clone();
        let environment = answer.environment();
        Self {
            schema: answer.schema,
            cursor: answer.cursor,
            tasks,
            offer,
            quote_tasks: Rc::new(answer.origins.clone()),
            environment,
            resident: answer.residency.body.state,
        }
    }
}

impl BoardView {
    /// Pending public work only; hidden rows remain in `tasks` as evidence.
    pub fn active_public_tasks(&self) -> impl Iterator<Item = &BoardTask> {
        self.tasks.iter().filter(|task| task.is_active_public())
    }

    /// Whether the snapshot's environment satisfies one task's typed requirements.
    #[must_use]
    pub fn environment_satisfies(&self, task: &BoardTask) -> bool {
        task.statement
            .body
            .requirements
            .are_satisfied_by(&self.environment)
    }
}

/// Capability-free lane snapshot fields used by the canvas.
#[derive(Clone, PartialEq, Eq)]
pub struct LaneView {
    /// Resident generation that owns these rows.
    pub epoch: CanonicalUuid,
    /// Current resident state.
    pub state: wire::ResidentState,
    /// Monotonic authority for these exact team rows and policies.
    pub topology_revision: u64,
    /// Safe current lane rows.
    pub lanes: Vec<LaneRow>,
    /// One-shot resident deadline for re-reading the lane liveness snapshot.
    pub refresh_after_millis: Option<u32>,
    /// Durable renewal state keyed by the independently steerable member.
    renewals: BTreeMap<CanonicalUuid, wire::LaneRenewalRecord>,
}

impl LaneView {
    /// Current durable renewal state for one exact team member.
    #[must_use]
    pub fn renewal(
        &self,
        team: CanonicalUuid,
        lane: CanonicalUuid,
    ) -> Option<&wire::LaneRenewalRecord> {
        self.renewals
            .get(&lane)
            .filter(|renewal| renewal.team == team)
    }

    /// Resolve one independently steerable member anywhere in the topology.
    #[must_use]
    pub fn member(&self, lane: CanonicalUuid) -> Option<&LaneMember> {
        self.lanes.iter().find_map(|row| row.member(lane))
    }
}

/// One Canvas lane: a team and every independently steerable member in it.
#[derive(Clone, PartialEq, Eq)]
pub struct LaneRow {
    /// Stable team identity.
    pub lane: CanonicalUuid,
    /// Authoritative task-allocation lifetime for this team.
    pub subscription: wire::intent::TeamLife,
    /// Members in canonical resident seat order.
    pub members: Vec<LaneMember>,
}

/// One independently steerable worktree and occupant inside a Canvas lane.
#[derive(Clone, PartialEq, Eq)]
pub struct LaneMember {
    pub lane: CanonicalUuid,
    pub name: Option<String>,
    pub worktree: String,
    pub speech: wire::SpeechPlayback,
    pub life: String,
    pub vendor: wire::Name,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub occupant: Option<CanonicalUuid>,
    pub session: Option<String>,
    pub disposition: String,
    /// Dynamic supervised live/quiet fold supplied by the resident.
    pub liveness: LaneLiveness,
    /// Whether a resumable session was ever declared for this seat.
    pub bound: bool,
    /// Whether the resident can seat an occupant here on an operator wake:
    /// a seat with a declared session resumes it, and a seat that was never
    /// started launches fresh. A started seat that lost its session cannot.
    pub wakeable: bool,
    /// Commands the lane started that it has yet to hear the ending of.
    pub jobs: Vec<wire::intent::LaneJob>,
}

/// What a lane tile says about its agent, ranked the way the operator scans.
///
/// Ported from the predecessor 0.31.0 UI, `spice/serve/static/app.lanes.js`:
/// `targetChoiceStatus` derives it, `targetChoiceStatusLabel` names it, and the
/// `targetChoiceStatusValues` list is the rank. That list carries nine states
/// where this wire now distinguishes five: the resident projects recently
/// heard and quiet supervised occupants as `working` and `running`, while
/// richer vendor phases remain absent. The ladder keeps the predecessor's
/// order and names only states the resident actually reports.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum LaneStatus {
    /// Direction is queued for this lane; the operator is being waited on.
    Queued,
    /// An occupant was heard from inside the quiet horizon, so what it is up
    /// to right now is known in detail.
    Working,
    /// An occupant holds the seat under supervision; nothing more is claimed.
    Running,
    /// Nobody is in the seat, but a command it started still runs or ended
    /// unheard, and its next occupant is owed the ending.
    Waiting,
    /// The seat has a declared session but nobody in it.
    Bound,
    /// The seat has never been occupied.
    Unbound,
}

impl LaneStatus {
    /// Stable machine spelling used by browser hooks and status styling.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Working => "working",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Bound => "bound",
            Self::Unbound => "unbound",
        }
    }

    /// The predecessor's exact wording for this state.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Queued => "steering queued",
            Self::Working => "agent working",
            Self::Running => "agent running",
            Self::Waiting => "agent waiting",
            Self::Bound => "agent bound",
            Self::Unbound => "agent unbound",
        }
    }
}

impl LaneMember {
    /// This member's status, given how much direction is queued for it.
    ///
    /// Pending direction outranks liveness, exactly as the predecessor ranks
    /// it: a lane holding unanswered steering is the one the operator has to
    /// look at, whether or not anything is running in it. A vacant seat that
    /// still owes its next occupant a command's ending is waiting, not cold.
    #[must_use]
    pub const fn status(&self, pending: usize) -> LaneStatus {
        if pending > 0 {
            LaneStatus::Queued
        } else {
            match self.liveness {
                LaneLiveness::Working => LaneStatus::Working,
                LaneLiveness::Running => LaneStatus::Running,
                LaneLiveness::Vacant if !self.jobs.is_empty() => LaneStatus::Waiting,
                LaneLiveness::Vacant if self.bound => LaneStatus::Bound,
                LaneLiveness::Vacant => LaneStatus::Unbound,
            }
        }
    }
}

impl LaneRow {
    /// The team host and main agent: canonical member zero at the right control edge.
    #[must_use]
    pub fn host(&self) -> Option<&LaneMember> {
        self.members.first()
    }

    /// Resolve one independently steerable member of this team.
    #[must_use]
    pub fn member(&self, lane: CanonicalUuid) -> Option<&LaneMember> {
        self.members.iter().find(|member| member.lane == lane)
    }

    /// Whether this row is a team the fleet renders rather than a lone agent.
    #[must_use]
    pub fn teamed(&self) -> bool {
        wire::head::teamed(self.lane, self.members.iter().map(|member| member.lane))
    }
}

impl From<&LaneAnswer> for LaneView {
    fn from(answer: &LaneAnswer) -> Self {
        let mut subscriptions = BTreeMap::new();
        for subscription in &answer.subscriptions {
            assert!(
                subscriptions
                    .insert(subscription.team, subscription.policy)
                    .is_none(),
                "an admitted lane answer names one policy per team"
            );
        }
        let mut renewals = BTreeMap::new();
        for renewal in &answer.renewals {
            assert!(
                renewals.insert(renewal.lane, renewal.clone()).is_none(),
                "an admitted lane answer names one renewal state per member"
            );
        }
        let mut teams = Vec::<LaneRow>::new();
        for lane in &answer.lanes {
            let member = LaneMember {
                lane: lane.lane,
                name: lane.name.clone(),
                worktree: lane.worktree.clone(),
                speech: lane.speech.clone(),
                life: lane.life.clone(),
                vendor: lane.vendor.clone(),
                model: lane.model.clone(),
                effort: lane.effort.clone(),
                occupant: lane.occupant,
                session: lane.session.clone(),
                disposition: lane.disposition.clone(),
                liveness: lane.liveness,
                bound: lane.bound,
                wakeable: lane.bound || lane.lineage.is_empty(),
                jobs: lane.jobs.clone(),
            };
            if let Some(team) = teams.iter_mut().find(|team| team.lane == lane.team) {
                team.members.push(member);
            } else {
                teams.push(LaneRow {
                    lane: lane.team,
                    subscription: *subscriptions
                        .get(&lane.team)
                        .expect("an admitted lane answer names every team policy"),
                    members: vec![member],
                });
            }
        }
        assert_eq!(
            subscriptions.len(),
            teams.len(),
            "an admitted lane answer names no orphan team policy"
        );
        Self {
            epoch: answer.epoch,
            state: answer.state,
            topology_revision: answer.topology_revision,
            lanes: teams,
            refresh_after_millis: answer.refresh_after_millis,
            renewals,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct ScopedValue<T> {
    scope: SessionScope,
    value: Option<T>,
}

impl<T> ScopedValue<T> {
    fn new(accepted: &Admitted<Accepted>) -> Self {
        Self {
            scope: accepted.scope(),
            value: None,
        }
    }

    fn get(&self, principal: &Principal) -> Option<&T> {
        (self.scope == principal.scope())
            .then_some(self.value.as_ref())
            .flatten()
    }

    fn replace(&mut self, scope: SessionScope, value: T) -> bool {
        if self.scope != scope {
            return false;
        }
        self.value = Some(value);
        true
    }
}

/// One agent-authored activity retained for a team status strip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneActivity {
    lane: CanonicalUuid,
    display_millis: i64,
    class: &'static str,
    title: String,
    detail: String,
    source: ActivitySource,
}

/// One durable lane ending rendered between transcript epochs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneLifecycleBoundary {
    seq: Seq,
    lane: CanonicalUuid,
    occupant: Option<CanonicalUuid>,
    observed: wire::Instant,
    cause: String,
}

impl LaneLifecycleBoundary {
    #[must_use]
    pub const fn seq(&self) -> Seq {
        self.seq
    }

    #[must_use]
    pub const fn lane(&self) -> CanonicalUuid {
        self.lane
    }

    #[must_use]
    pub const fn occupant(&self) -> Option<CanonicalUuid> {
        self.occupant
    }

    #[must_use]
    pub const fn observed(&self) -> &wire::Instant {
        &self.observed
    }

    #[must_use]
    pub fn cause(&self) -> &str {
        &self.cause
    }
}

impl LaneActivity {
    /// Prose detail is the line the composer quote header would show for the
    /// same message: the first authored line after borrowed Markdown quotes,
    /// or the shared absent-preview label when every line is quoted. The
    /// ticker voices this detail as the agent's own words, so a reply that
    /// opens by quoting the operator must never surface as that quote. Every
    /// other activity kind keeps its text verbatim.
    fn from_message(message: &AgentMessage) -> Self {
        let (class, title) = match message.activity {
            AgentActivityKind::Prose => ("prose", "assistant"),
            AgentActivityKind::Failure => ("failure", "provider failure"),
            AgentActivityKind::Tool => ("tool", "tool"),
            AgentActivityKind::Reasoning => ("reasoning", "thinking"),
            AgentActivityKind::Turn => ("turn", "new turn"),
            AgentActivityKind::Compaction => ("compaction", "compaction"),
            AgentActivityKind::SignOut => ("sign-out", "signed out"),
        };
        let detail = match message.activity {
            AgentActivityKind::Prose => {
                watchband::quote_label(watchband::first_nonquoted_prose(&message.text).as_deref())
                    .to_owned()
            }
            _ => message.text.clone(),
        };
        Self {
            lane: message.lane,
            display_millis: agent_message_display_millis(message),
            class,
            title: title.to_owned(),
            detail,
            source: ActivitySource {
                lane: message.lane,
                locus: message.locus,
            },
        }
    }

    /// Independently steerable member that produced this activity.
    #[must_use]
    pub const fn lane(&self) -> CanonicalUuid {
        self.lane
    }

    /// Instant used only to place and age this activity on Canvas surfaces.
    ///
    /// A synthetic source instant remains an ordering identity rather than a
    /// vendor-authored claim. The retained [`wire::AgentMessage`] carries that
    /// provenance distinction; this projection is deliberately total so every
    /// visible activity can participate in one display chronology.
    #[must_use]
    pub const fn display_millis(&self) -> i64 {
        self.display_millis
    }

    #[must_use]
    pub const fn class(&self) -> &'static str {
        self.class
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

/// Total display chronology for one admitted agent message.
///
/// Spice observation time is preferred when it exists. Otherwise the source
/// instant supplies a stable display and ordering value, including when that
/// instant was synthesized. This fallback does not mutate `observed` or claim
/// that a synthetic instant came from the vendor; those facts remain explicit
/// on the retained wire message.
#[must_use]
pub(crate) fn agent_message_display_millis(message: &wire::AgentMessage) -> i64 {
    message.display_millis()
}
/// Stable browser identity for one visible agent card.
///
/// A compaction reaches Canvas first as a live announcement and later as the
/// vendor's preserved record. Those representations have different source
/// loci but one vendor boundary identity, so they must reconcile through one
/// DOM and Mosaic key rather than remove and reinsert the divider.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn agent_message_card_identity(message: &wire::AgentMessage) -> String {
    let Some(boundary) = message
        .boundary
        .as_ref()
        .filter(|_| message.activity == AgentActivityKind::Compaction)
    else {
        return format!(
            "agent-{}-{}-{}-{}-{}",
            message.lane,
            message.locus.member,
            message.locus.offset,
            message.locus.length,
            message.locus.event,
        );
    };
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut identity = format!("agent-{}-compaction-", message.lane);
    identity.reserve(boundary.id.len().saturating_mul(2));
    for byte in boundary.id.bytes() {
        identity.push(char::from(HEX[usize::from(byte >> 4)]));
        identity.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    identity
}

/// Whether two wire records are representations of one visible compaction.
#[cfg(any(target_arch = "wasm32", test))]
#[must_use]
pub(crate) fn same_compaction_card(left: &wire::AgentMessage, right: &wire::AgentMessage) -> bool {
    left.activity == AgentActivityKind::Compaction
        && right.activity == AgentActivityKind::Compaction
        && left.lane == right.lane
        && left
            .boundary
            .as_ref()
            .zip(right.boundary.as_ref())
            .is_some_and(|(left, right)| left.id == right.id)
}

/// Durable identity used to ignore a repeated live activity without moving it
/// back to the front of the status chronology.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ActivitySource {
    lane: CanonicalUuid,
    locus: MessageLocus,
}
/// Exact identity of one bounded on-demand relationship read.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskRelationsKey {
    roots: Vec<wire::TaskStamp>,
    relation: TaskRelationKind,
    depth: u8,
    node_limit: u16,
}

impl TaskRelationsKey {
    #[must_use]
    pub fn new(
        mut roots: Vec<wire::TaskStamp>,
        relation: TaskRelationKind,
        depth: u8,
        node_limit: u16,
    ) -> Option<Self> {
        roots.sort();
        roots.dedup();
        (!roots.is_empty()
            && roots.len() <= TASK_RELATION_ROOT_LIMIT
            && depth <= TASK_RELATION_DEPTH_LIMIT
            && usize::from(node_limit) >= roots.len()
            && node_limit <= TASK_RELATION_NODE_LIMIT)
            .then_some(Self {
                roots,
                relation,
                depth,
                node_limit,
            })
    }

    #[must_use]
    pub fn request(&self) -> TaskRelations {
        TaskRelations {
            roots: self.roots.clone(),
            relations: vec![self.relation],
            depth: self.depth,
            node_limit: self.node_limit,
        }
    }

    #[must_use]
    pub fn roots(&self) -> &[wire::TaskStamp] {
        &self.roots
    }

    #[must_use]
    pub const fn relation(&self) -> TaskRelationKind {
        self.relation
    }
}

/// Capability-scoped state for one request-identity-keyed relationship read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskRelationsState {
    Loading,
    Available(Box<TaskRelationsAnswer>),
    Refreshing(Box<TaskRelationsAnswer>),
    Unavailable {
        reason: String,
        detail: Option<String>,
    },
}

impl TaskRelationsState {
    /// Last admitted answer, retained while its typed family refreshes.
    #[must_use]
    pub fn answer(&self) -> Option<&TaskRelationsAnswer> {
        match self {
            Self::Available(answer) | Self::Refreshing(answer) => Some(answer),
            Self::Loading | Self::Unavailable { .. } => None,
        }
    }

    #[must_use]
    pub const fn refreshing(&self) -> bool {
        matches!(self, Self::Refreshing(_))
    }
}

/// Capability-scoped state for one on-demand task detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskDetailState {
    /// A request is queued or in flight without prior admitted detail.
    Loading,
    /// Current task evidence and its ordered durable notes.
    Available(Rc<TaskDetailAnswer>),
    /// Last admitted evidence retained while a changed task is refreshed.
    Refreshing(Rc<TaskDetailAnswer>),
    /// The resident refused or could not serve the selected task.
    Unavailable {
        /// Stable resident reason.
        reason: String,
        /// Human diagnostic detail, when the resident supplied it.
        detail: Option<String>,
    },
}

impl TaskDetailState {
    /// Last admitted detail, retained during an in-flight refresh.
    #[must_use]
    pub fn answer(&self) -> Option<&TaskDetailAnswer> {
        match self {
            Self::Available(answer) | Self::Refreshing(answer) => Some(answer),
            Self::Loading | Self::Unavailable { .. } => None,
        }
    }

    #[must_use]
    pub const fn refreshing(&self) -> bool {
        matches!(self, Self::Refreshing(_))
    }
}

/// All state rendered by one capability-scoped canvas.
///
/// Deliberately not `Debug`: even though scopes redact themselves, snapshots
/// should never gain an accidental whole-runtime debug path.
#[derive(PartialEq, Eq)]
pub struct CanvasRuntime {
    accepted: Admitted<Accepted>,
    principal: Principal,
    board: ScopedValue<BoardView>,
    lanes: ScopedValue<LaneView>,
    task_details: BTreeMap<wire::TaskStamp, TaskDetailState>,
    task_relations: BTreeMap<TaskRelationsKey, TaskRelationsState>,
    task_relations_revision: u64,
    work_history: WorkHistoryState,
    lane_position: Option<Seq>,
    inbox: ScopedValue<InboxAnswer>,
    settings: ScopedValue<GlobalSettingsAnswer>,
    vendors: ScopedValue<VendorCatalogAnswer>,
    settings_position: Option<Seq>,
    message_cards: message_cards::MessageCards,
    /// Exact lane-local frontier each independently paged member can extend.
    message_history: BTreeMap<CanonicalUuid, wire::MessageHistoryCursor>,
    /// Admitted response completions for explicit same-cursor retries.
    message_history_completions: BTreeMap<CanonicalUuid, u64>,
    /// Lanes whose older history ended under the current snapshot; a live
    /// generation still reporting their retained edge cannot reopen them.
    message_history_exhausted: BTreeSet<CanonicalUuid>,
    /// Last transcript generation admitted atomically.
    message_generation: Option<u64>,
    /// A same-scope subscription snapshot merges into retained hydrated cards.
    message_reconnect_pending: bool,
    /// Re-arms member-local paging after a same-scope subscription refresh.
    message_history_revision: u64,
    quota: Option<Rc<wire::QuotaGeneration>>,
    resume: Option<StreamCursor>,
    terminal: Option<StreamTerminal>,
    activities: Vec<LaneActivity>,
    lifecycle_boundaries: Vec<LaneLifecycleBoundary>,
}

impl CanvasRuntime {
    /// Begin empty for the principal named by an admitted acceptance.
    #[must_use]
    pub fn new(accepted: &Admitted<Accepted>) -> Self {
        Self {
            accepted: accepted.clone(),
            principal: Principal::admitted(accepted),
            board: ScopedValue::new(accepted),
            lanes: ScopedValue::new(accepted),
            task_details: BTreeMap::new(),
            task_relations: BTreeMap::new(),
            task_relations_revision: 0,
            work_history: WorkHistoryState::default(),
            lane_position: None,
            inbox: ScopedValue::new(accepted),
            settings: ScopedValue::new(accepted),
            vendors: ScopedValue::new(accepted),
            settings_position: None,
            message_cards: message_cards::MessageCards::default(),
            message_history: BTreeMap::new(),
            message_history_completions: BTreeMap::new(),
            message_history_exhausted: BTreeSet::new(),
            message_generation: None,
            message_reconnect_pending: false,
            message_history_revision: 0,
            quota: None,
            resume: None,
            terminal: None,
            activities: Vec::new(),
            lifecycle_boundaries: Vec::new(),
        }
    }
    /// Start a changed resident scope without blanking the operator's mounted
    /// transcript while its replacement snapshots settle.
    ///
    /// Only two operator capabilities may share this presentation cache. A
    /// downgrade to a lane principal starts empty, so data visible under the
    /// old capability never crosses an authority boundary. The first message
    /// snapshot merges by durable card identity, retaining deep history and
    /// allocation identity while replacing content the new generation changed.
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    pub(crate) fn rehydrate(accepted: &Admitted<Accepted>, previous: &Self) -> Self {
        let mut next = Self::new(accepted);
        if previous.principal.actor() == &Actor::Operator
            && next.principal.actor() == &Actor::Operator
        {
            next.message_cards = previous.message_cards.clone();
            next.message_reconnect_pending = true;
        }
        next
    }

    /// Adopt a reconnect. Returns true when a changed capability or epoch
    /// cleared the entire runtime before the new principal can render.
    ///
    /// Facts published while the socket was down are never streamed, and a
    /// read in flight at the drop never answers, so every retained task read
    /// is stale: details keep their answer while [`Self::task_detail_reads`]
    /// are re-read, and relationship lenses re-arm the demand hook.
    pub fn reconnect(&mut self, accepted: &Admitted<Accepted>) -> bool {
        if self.accepted.scope() == accepted.scope() {
            self.accepted = accepted.clone();
            self.principal = Principal::admitted(accepted);
            self.message_reconnect_pending = true;
            self.terminal = None;
            if self.work_history.active() {
                self.evict_work_history(true);
            }
            self.task_details.values_mut().for_each(refresh_task_detail);
            self.task_relations
                .retain(|_, state| refresh_task_relations(state, false));
            self.task_relations_revision = self.task_relations_revision.saturating_add(1);
            false
        } else {
            *self = Self::new(accepted);
            true
        }
    }

    /// Whether an acceptance belongs to the authority this runtime may retain.
    #[must_use]
    pub fn same_scope(&self, accepted: &Admitted<Accepted>) -> bool {
        self.accepted.scope() == accepted.scope()
    }

    /// Whether every current snapshot needed for one coherent Canvas is present.
    #[must_use]
    pub fn is_hydrated(&self) -> bool {
        self.board().is_some()
            && self.lanes().is_some()
            && self.inbox().is_some()
            && self.settings().is_some()
            && self.vendor_catalog().is_some()
    }

    #[must_use]
    pub const fn principal(&self) -> &Principal {
        &self.principal
    }

    /// Subjects visible to this admitted principal.
    #[must_use]
    pub fn subjects(&self) -> Vec<ViewSubject> {
        let mut subjects = vec![ViewSubject::Facts];
        if self.principal.actor() == &Actor::Operator {
            subjects.push(ViewSubject::Messages);
            subjects.push(ViewSubject::Quota);
        }
        subjects
    }

    /// Whether the principal's steer pane offers this command.
    #[must_use]
    pub fn offers(&self, kind: CommandKind) -> bool {
        SteerPane::over(&self.principal).offers(kind)
    }

    #[must_use]
    pub const fn resume(&self) -> Option<StreamCursor> {
        self.resume
    }

    #[must_use]
    pub const fn terminal(&self) -> Option<StreamTerminal> {
        self.terminal
    }

    /// Exact member-local cursor for the next retained history page.
    #[must_use]
    pub fn message_history_target(
        &self,
        lane: CanonicalUuid,
    ) -> Option<wire::MessageHistoryCursor> {
        self.message_history.get(&lane).copied()
    }
    /// Admitted completion generation for one member-local history request.
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    #[must_use]
    pub(crate) fn message_history_completion(&self, lane: CanonicalUuid) -> u64 {
        self.message_history_completions
            .get(&lane)
            .copied()
            .unwrap_or(0)
    }

    /// Subscription generation owning the current member-local frontiers.
    #[must_use]
    pub const fn message_history_revision(&self) -> u64 {
        self.message_history_revision
    }

    #[must_use]
    pub fn board(&self) -> Option<&BoardView> {
        self.board.get(&self.principal)
    }

    /// On-demand detail state retained only for this admitted capability scope.
    #[must_use]
    pub fn task_detail(&self, task: &wire::TaskStamp) -> Option<&TaskDetailState> {
        self.task_details.get(task)
    }

    /// Every retained task detail, each of which a reconnect must read again.
    pub fn task_detail_reads(&self) -> impl Iterator<Item = &wire::TaskStamp> {
        self.task_details.keys()
    }

    /// Mark one selected detail as requested.
    pub fn begin_task_detail(&mut self, task: wire::TaskStamp) {
        self.task_details.insert(task, TaskDetailState::Loading);
    }

    /// Retain one affected detail while its replacement is requested.
    pub fn invalidate_task_detail(&mut self, task: &wire::TaskStamp) -> bool {
        let Some(detail) = self.task_details.get_mut(task) else {
            return false;
        };
        refresh_task_detail(detail);
        true
    }

    /// One exact bounded relationship read retained for this capability scope.
    #[must_use]
    pub fn task_relations(&self, key: &TaskRelationsKey) -> Option<&TaskRelationsState> {
        self.task_relations.get(key)
    }

    /// Whether one exact relationship projection needs a demand read.
    #[must_use]
    pub fn task_relations_needs_request(&self, key: &TaskRelationsKey) -> bool {
        matches!(
            self.task_relations.get(key),
            None | Some(TaskRelationsState::Refreshing(_))
        )
    }

    /// Mark one request identity as queued or in flight without discarding admitted geometry.
    pub fn begin_task_relations(&mut self, key: TaskRelationsKey) {
        if !matches!(
            self.task_relations.get(&key),
            Some(TaskRelationsState::Refreshing(_))
        ) {
            self.task_relations.insert(key, TaskRelationsState::Loading);
        }
    }

    /// Retain cached families while a stream-triggered replacement is requested.
    ///
    /// A failed read holds no geometry to keep, so it is dropped rather than
    /// parked as loading: the demand hook then reads it again.
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    pub(crate) fn invalidate_task_relations(&mut self, relations: &BTreeSet<TaskRelationKind>) {
        let mut invalidated = false;
        self.task_relations.retain(|key, state| {
            if !relations.contains(&key.relation) {
                return true;
            }
            invalidated = true;
            refresh_task_relations(state, true)
        });
        if invalidated {
            self.task_relations_revision = self.task_relations_revision.saturating_add(1);
        }
    }

    /// Drop one unavailable lens so the demand hook reads it again.
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    pub(crate) fn retry_task_relations(&mut self, key: &TaskRelationsKey) -> bool {
        if !matches!(
            self.task_relations.get(key),
            Some(TaskRelationsState::Unavailable { .. })
        ) {
            return false;
        }
        self.task_relations.remove(key);
        self.task_relations_revision = self.task_relations_revision.saturating_add(1);
        true
    }

    /// Generation that re-arms the active demand hook after cache eviction.
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    #[must_use]
    pub(crate) const fn task_relations_revision(&self) -> u64 {
        self.task_relations_revision
    }

    #[must_use]
    pub fn lanes(&self) -> Option<&LaneView> {
        self.lanes.get(&self.principal)
    }

    #[must_use]
    pub fn inbox(&self) -> Option<&InboxAnswer> {
        self.inbox.get(&self.principal)
    }

    /// Authoritative global settings, absent until the initial read lands.
    #[must_use]
    pub fn settings(&self) -> Option<&GlobalSettingsAnswer> {
        self.settings.get(&self.principal)
    }

    /// Vendors this resident generation can seat, absent until the read lands.
    #[must_use]
    pub fn vendor_catalog(&self) -> Option<&VendorCatalogAnswer> {
        self.vendors.get(&self.principal)
    }

    /// Latest monotonic redacted provider quota generation.
    #[must_use]
    pub fn quota(&self) -> Option<&wire::QuotaGeneration> {
        self.quota.as_deref()
    }

    /// Newest retained activity produced by any member of `team`.
    ///
    /// Activity is retained in arrival order across the resident's captured
    /// transcript feed. Filtering that one chronology by membership is
    /// what makes a fused team show whichever of its agents acted last.
    #[must_use]
    pub fn newest_team_activity(&self, team: &LaneRow) -> Option<&LaneActivity> {
        self.activities
            .iter()
            .find(|activity| team.member(activity.lane).is_some())
    }

    /// Newest retained activity produced by one independently steerable lane.
    #[must_use]
    pub fn newest_lane_activity(&self, lane: CanonicalUuid) -> Option<&LaneActivity> {
        self.activities
            .iter()
            .find(|activity| activity.lane == lane)
    }

    /// Retained lane activity, newest first.
    #[must_use]
    pub fn activities(&self) -> &[LaneActivity] {
        &self.activities
    }

    /// Durable lane endings in journal chronology.
    #[must_use]
    pub fn lifecycle_boundaries(&self) -> &[LaneLifecycleBoundary] {
        &self.lifecycle_boundaries
    }

    /// Newest retained agent-authored transcript messages in creation order.
    #[must_use]
    pub fn messages(&self) -> &[Rc<AgentMessage>] {
        self.message_cards.entries()
    }

    /// Newest prose `lane` authored among the retained messages, with the line
    /// its composer header shows. Reads the creation-ordered store, so history
    /// pages merged behind the head never displace it.
    #[must_use]
    pub fn latest_lane_prose(&self, lane: CanonicalUuid) -> Option<(&AgentMessage, String)> {
        crate::watchband::latest_lane_prose(lane, self.messages().iter().map(Rc::as_ref))
    }

    pub fn apply_board(&mut self, admitted: &Admitted<Response<BoardAnswer>>) {
        self.apply_response(admitted, |runtime, answer| {
            runtime
                .board
                .replace(admitted.scope(), BoardView::from(answer));
            runtime.work_history.invalidate(false);
        });
    }
    /// Apply one correlated detail response without crossing capability scope.
    pub fn apply_task_detail(
        &mut self,
        task: &wire::TaskStamp,
        admitted: &Admitted<Response<TaskDetailAnswer>>,
    ) {
        if admitted.scope() != self.accepted.scope() {
            return;
        }
        let state = match &admitted.get().outcome {
            Outcome::Reply(reply) if reply.body.task.statement.body.born == *task => {
                TaskDetailState::Available(Rc::new(reply.body.clone()))
            }
            Outcome::Reply(_) => TaskDetailState::Unavailable {
                reason: "task::detail-identity-mismatch".to_owned(),
                detail: Some("the detail answer named a different durable task".to_owned()),
            },
            Outcome::Refusal(refusal) => {
                let diagnostic = refusal.diagnostic();
                TaskDetailState::Unavailable {
                    reason: diagnostic
                        .map_or("task::detail-refused", wire::RefusalDiagnostic::reason)
                        .to_owned(),
                    detail: diagnostic
                        .and_then(wire::RefusalDiagnostic::detail)
                        .map(str::to_owned),
                }
            }
            Outcome::Fault(fault) => TaskDetailState::Unavailable {
                reason: fault.reason.clone(),
                detail: Some(fault.detail.clone()),
            },
        };
        self.task_details.insert(task.clone(), state);
    }
    /// Apply one correlated relationship answer without crossing request identity.
    pub fn apply_task_relations(
        &mut self,
        key: &TaskRelationsKey,
        admitted: &Admitted<Response<TaskRelationsAnswer>>,
    ) {
        if admitted.scope() != self.accepted.scope() {
            return;
        }
        let state = match &admitted.get().outcome {
            Outcome::Reply(reply) if task_relations_match(key, &reply.body) => {
                TaskRelationsState::Available(Box::new(reply.body.clone()))
            }
            Outcome::Reply(_) => TaskRelationsState::Unavailable {
                reason: "task::relations-identity-mismatch".to_owned(),
                detail: Some(
                    "the relationship answer did not match its roots, typed family, or node identities"
                        .to_owned(),
                ),
            },
            Outcome::Refusal(refusal) => {
                let diagnostic = refusal.diagnostic();
                TaskRelationsState::Unavailable {
                    reason: diagnostic
                        .map_or("task::relations-refused", wire::RefusalDiagnostic::reason)
                        .to_owned(),
                    detail: diagnostic
                        .and_then(wire::RefusalDiagnostic::detail)
                        .map(str::to_owned),
                }
            }
            Outcome::Fault(fault) => TaskRelationsState::Unavailable {
                reason: fault.reason.clone(),
                detail: Some(fault.detail.clone()),
            },
        };
        self.task_relations.insert(key.clone(), state);
    }

    /// Apply a lane table only when its authority position does not move backwards.
    pub fn apply_lanes(&mut self, admitted: &Admitted<Response<LaneAnswer>>) -> bool {
        if admitted.scope() != self.accepted.scope() {
            return false;
        }
        let position = match &admitted.get().outcome {
            Outcome::Reply(reply) => match reply.position {
                wire::ReplyPosition::Committed(range) => Some(range.last),
                wire::ReplyPosition::Snapshot(cursor) => Some(cursor),
                wire::ReplyPosition::Direct {} => return false,
            },
            Outcome::Refusal(_) | Outcome::Fault(_) => None,
        };
        if position
            .is_some_and(|position| self.lane_position.is_some_and(|current| position < current))
        {
            return false;
        }
        if let Some(position) = position {
            self.lane_position = Some(position);
        }
        self.apply_response(admitted, |runtime, answer| {
            runtime
                .lanes
                .replace(admitted.scope(), LaneView::from(answer));
        });
        matches!(&admitted.get().outcome, Outcome::Reply(_))
    }

    pub fn apply_inbox(&mut self, admitted: &Admitted<Response<InboxAnswer>>) {
        self.apply_response(admitted, |runtime, answer| {
            runtime.inbox.replace(admitted.scope(), answer.clone());
        });
    }

    /// Apply a global settings read or mutation without allowing older authority backwards.
    pub fn apply_settings(&mut self, admitted: &Admitted<Response<GlobalSettingsAnswer>>) -> bool {
        let position = match &admitted.get().outcome {
            Outcome::Reply(reply) => match reply.position {
                wire::ReplyPosition::Committed(range) => Some(range.last),
                wire::ReplyPosition::Snapshot(cursor) => Some(cursor),
                wire::ReplyPosition::Direct {} => None,
            },

            Outcome::Refusal(_) | Outcome::Fault(_) => None,
        };
        let mut applied = false;
        self.apply_response(admitted, |runtime, answer| {
            if position.is_some_and(|next| {
                runtime
                    .settings_position
                    .is_none_or(|current| next >= current)
            }) {
                runtime.settings_position = position;
                applied = runtime.settings.replace(admitted.scope(), answer.clone());
            }
        });
        applied
    }

    /// Apply the resident's vendor catalogue read.
    pub fn apply_vendor_catalog(&mut self, admitted: &Admitted<Response<VendorCatalogAnswer>>) {
        self.apply_response(admitted, |runtime, answer| {
            runtime.vendors.replace(admitted.scope(), answer.clone());
        });
    }

    fn apply_response<T>(
        &mut self,
        admitted: &Admitted<Response<T>>,
        reply: impl FnOnce(&mut Self, &T),
    ) {
        if admitted.scope() != self.accepted.scope() {
            return;
        }
        if let Outcome::Reply(answer) = &admitted.get().outcome {
            reply(self, &answer.body);
        }
    }
}

fn task_relations_match(key: &TaskRelationsKey, answer: &TaskRelationsAnswer) -> bool {
    if answer.roots != key.roots {
        return false;
    }
    let identities = answer
        .nodes
        .iter()
        .map(wire::intent::TaskRelationNode::identity)
        .collect::<BTreeSet<_>>();
    if identities.len() != answer.nodes.len()
        || !key
            .roots
            .iter()
            .all(|task| identities.contains(&TaskRelationNodeRef::Task { task: task.clone() }))
    {
        return false;
    }
    answer.edges.iter().all(|edge| {
        edge.relation == key.relation
            && identities.contains(&edge.from)
            && identities.contains(&edge.to)
    }) && answer.truncation.as_ref().is_none_or(|truncation| {
        truncation
            .frontier
            .iter()
            .all(|frontier| frontier.edge.relation == key.relation)
    })
}

/// Keep a stale detail's answer on screen while its replacement is read.
fn refresh_task_detail(detail: &mut TaskDetailState) {
    *detail = match std::mem::replace(detail, TaskDetailState::Loading) {
        TaskDetailState::Available(answer) | TaskDetailState::Refreshing(answer) => {
            TaskDetailState::Refreshing(answer)
        }
        TaskDetailState::Loading | TaskDetailState::Unavailable { .. } => TaskDetailState::Loading,
    };
}

/// Mark one stale lens for re-reading; false drops it for the demand hook.
///
/// A lens without geometry is dropped rather than parked as loading, unless
/// its read is still in flight and will answer: a failed read would otherwise
/// never be read again, as would one lost with a closed socket.
fn refresh_task_relations(state: &mut TaskRelationsState, in_flight_answers: bool) -> bool {
    match std::mem::replace(state, TaskRelationsState::Loading) {
        TaskRelationsState::Available(answer) | TaskRelationsState::Refreshing(answer) => {
            *state = TaskRelationsState::Refreshing(answer);
            true
        }
        TaskRelationsState::Loading => in_flight_answers,
        TaskRelationsState::Unavailable { .. } => false,
    }
}

/// Which cached reads a batch of admitted facts has invalidated.
///
/// The stream carries facts, not snapshots: it advances the journal cursor and
/// leaves every read the surface is drawn from exactly as stale as it was.
/// That is not survivable for the watchband, which claims to show what a lane
/// is doing right now. Uncorrected, the shell
/// reports itself online with a current cursor while a tile shows a directive
/// two events old — worse than showing nothing, because the staleness is
/// invisible.
///
/// This lives beside [`CanvasRuntime::apply_board`], [`CanvasRuntime::apply_lanes`],
/// [`CanvasRuntime::apply_inbox`] — one field per read it names — rather than
/// in the socket loop that acts on it, so it can be tested off `wasm32`.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Stale {
    board: bool,
    lanes: bool,
    inbox: bool,
    task_details: BTreeSet<wire::TaskStamp>,
    task_relation_kinds: BTreeSet<TaskRelationKind>,
}

impl Stale {
    /// Widen this set by whatever one admitted fact invalidated.
    ///
    /// The mapping is read off the kind's own wire spelling rather than
    /// declared as a second table over [`FactKind`]. The vocabulary already
    /// namespaces every kind — `steer::sent`, `lane::seated`, `task::added`,
    /// `claim::won` — and that prefix is exactly the question "which read went
    /// stale". Matching the variants one by one would be a second registry to
    /// hold in step with the first, and the failure mode when it drifted would
    /// be a new kind that silently refreshes nothing.
    pub fn widen(&mut self, kind: FactKind) {
        match kind.as_str().split_once("::") {
            Some(("steer", _)) => self.inbox = true,
            // The inbox is keyed on the lane roster, not only on the steer
            // ledger: the resident builds one row per seated lane and carries
            // that lane's occupant on it, so a lane that arrives, departs or
            // changes occupant moves the inbox even though no directive was
            // sent. Widening only `lanes` here left a newly seated lane with
            // no inbox row in the browser, and the console reads its residency
            // witness out of that row -- so the first direction sent to a lane
            // seated from the canvas was dropped before it reached the wire.
            Some(("lane", _)) => {
                self.lanes = true;
                self.inbox = true;
            }
            Some(("task", _)) => {
                self.board = true;
                self.task_relation_kinds.extend([
                    TaskRelationKind::Origin,
                    TaskRelationKind::Dependency,
                    TaskRelationKind::Review,
                    TaskRelationKind::Lifecycle,
                    TaskRelationKind::Claim,
                ]);
            }
            // A claim binds a task to a lane, so both sides of that join move.
            Some(("claim", _)) => {
                self.board = true;
                self.lanes = true;
                self.task_relation_kinds.insert(TaskRelationKind::Claim);
            }
            _ => {}
        }
    }
    fn widen_fact(&mut self, fact: &wire::StreamFact) {
        self.widen(fact.kind());
        if let Some(task) = fact.task() {
            self.task_details.insert(task);
        }
    }

    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    pub(crate) fn retain_task_details(&mut self, mut retain: impl FnMut(&wire::TaskStamp) -> bool) {
        self.task_details.retain(|task| retain(task));
    }
    /// Mark one selected task detail for a demand-driven re-read.
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    pub(crate) fn invalidate_task_detail_read(&mut self, task: wire::TaskStamp) {
        self.task_details.insert(task);
    }
    /// Evict changed typed families; the active demand hook alone re-requests.
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    pub(crate) fn resolve_task_relations(&mut self, runtime: &mut CanvasRuntime, changed: bool) {
        if changed {
            runtime.invalidate_task_relations(&self.task_relation_kinds);
        }
        self.task_relation_kinds.clear();
    }

    /// Hold terminal paging until a changed task snapshot has been refreshed.
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    pub(crate) fn resolve_work_history(&self, runtime: &mut CanvasRuntime, changed: bool) {
        if changed && self.board {
            runtime.invalidate_work_history();
        }
    }

    /// Affected open task details, in durable identity order.
    pub fn task_details(&self) -> impl Iterator<Item = &wire::TaskStamp> {
        self.task_details.iter()
    }

    /// Re-read a quiet occupant when a live message may have resumed activity.
    ///
    /// The resident still decides liveness; replayed messages cannot make a
    /// process working locally. Rows already heard from need no redundant read.
    pub fn invalidate_quiet_lane(&mut self, view: Option<&LaneView>, lane: CanonicalUuid) {
        self.lanes |= view.is_some_and(|view| {
            view.lanes.iter().any(|team| {
                team.member(lane)
                    .is_some_and(|member| member.liveness == LaneLiveness::Running)
            })
        });
    }

    /// Whether the board read went stale.
    #[must_use]
    pub const fn board(&self) -> bool {
        self.board
    }

    /// Whether the lane read went stale.
    #[must_use]
    pub const fn lanes(&self) -> bool {
        self.lanes
    }

    /// Whether the steering inbox read went stale.
    #[must_use]
    pub const fn inbox(&self) -> bool {
        self.inbox
    }

    /// Whether anything needs re-reading at all.
    #[must_use]
    pub fn any(&self) -> bool {
        self.board
            || self.lanes
            || self.inbox
            || !self.task_details.is_empty()
            || !self.task_relation_kinds.is_empty()
    }
}

/// The reads one admitted stream frame invalidated.
///
/// Two frames carry facts and both count. [`StreamFrame::Fact`] is the live
/// push an established subscription receives one event at a time, and it is the
/// case the watchband exists for: it is always the tail, so it always widens.
/// [`StreamFrame::Facts`] is the paged backlog, and only its last page counts —
/// a subscription walks its history as a run of pages each carrying facts that
/// would widen this set, so honouring every page would issue a round of reads
/// per page of history to learn a state the final page already implies.
///
/// [`StreamFrame::Thread`] carries facts too and is deliberately absent: a
/// thread page is a causal walk backwards through history, not an advance, so
/// nothing it contains says a current read is out of date.
#[must_use]
pub fn stale_reads(admitted: &Admitted<StreamFrame>) -> Stale {
    let mut stale = Stale::default();
    match admitted.get() {
        StreamFrame::Fact { fact, .. } => stale.widen_fact(fact),
        StreamFrame::Facts { packed, .. } if packed.overflow.is_none() => {
            for row in &packed.rows {
                stale.widen_fact(row);
            }
        }
        _ => {}
    }
    stale
}

#[cfg(test)]
mod topology_tests {
    use super::*;

    #[test]
    fn a_stale_topology_refusal_requires_an_authoritative_refresh() {
        let stale = Outcome::<()>::Refusal(wire::Refusal::bare(
            wire::RefusalDiagnostic::new(
                "lane::topology-moved",
                Some("topology_revision".to_owned()),
                Some("7".to_owned()),
                Some("8".to_owned()),
            )
            .with_detail("read the lane table"),
        ));
        assert!(topology_mutation_requires_refresh(&stale));

        let current = Outcome::Reply(wire::Reply {
            position: wire::ReplyPosition::Direct {},
            body: (),
        });
        assert!(!topology_mutation_requires_refresh(&current));
    }
}
