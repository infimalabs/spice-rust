//! Capability-scoped state used by the real Wasm app runtime.
//!
//! Every mutation enters through an admitted acceptance, response, or stream
//! frame. Stream facts contribute credential-free envelope metadata; accepted
//! steering replies and live steering facts contribute the operator records
//! rendered by conversation.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use wire::intent::{
    BoardAnswer, BoardTask, ConsentQueueAnswer, GlobalSettingsAnswer, InboxAnswer, LaneAnswer,
    TASK_RELATION_DEPTH_LIMIT, TASK_RELATION_NODE_LIMIT, TASK_RELATION_ROOT_LIMIT,
    TaskRelationKind, TaskRelationNodeRef, TaskRelations, TaskRelationsAnswer, VendorCatalogAnswer,
};
use wire::principal::PrincipalDirectory;
use wire::rooms::RoomAuthorityStamp;
use wire::{
    Accepted, Actor, AgentActivityKind, AgentMessage, CanonicalUuid, Digest, FactKind,
    MessageLocus, Outcome, Packed, Response, Seq, StreamCursor, StreamFrame, TaskStamp,
    ViewSubject,
};

use crate::pane::Principal;
use crate::socket::Admitted;
use crate::watchband;

mod consent;
mod contribution_read;
mod conversation_text;
mod lanes;
mod member_control;
mod message_cards;
mod principal;
mod principals;
mod publication;
mod rooms;
mod scoped;
mod stale;
mod stream;
mod task_detail;
pub use consent::ConsentRuling;
pub use contribution_read::ContributionRead;
#[cfg(target_arch = "wasm32")]
pub(crate) use conversation_text::operator_destination_label;
pub use conversation_text::{ConversationText, ItemConcern};
pub use lanes::{LaneMember, LaneRow, LaneStatus, LaneView};
pub use member_control::{MemberCommand, MemberControlState, MemberRequest, MemberSettlement};
pub use publication::AppSurface;
pub use rooms::{
    CandidateWithdrawalRecord, Contribution, Conversation, ConversationProjection, Delivery,
    DesignStanding, OperatorItem, ParticipantReason, PreparationProjection, Relevance,
    RoomAnchorAuthority, RoomAnchorSettlement, RoomEvent, RoomEventKind, RoomParticipant,
    RoomPositioned, RoomProjection, RoomProjectionStatus, RoomState, StopRequest, associated,
    design_dependent_count,
};
use scoped::ScopedValue;
pub use stale::{Stale, stale_reads};
pub use stream::{StreamApplication, StreamDirective, StreamTerminal};
pub use task_detail::TaskDetailState;
pub(crate) type QuoteTasks = BTreeMap<String, Vec<wire::TaskHandle>>;

/// Whether a topology mutation response leaves the app owing an authoritative read.
///
/// Successful topology replies already carry the new table. A refusal or
/// fault carries no replacement table, so the browser must re-pull rather
/// than continue rendering the revision the rejected gesture observed.
#[cfg(any(target_arch = "wasm32", test))]
#[must_use]
pub(crate) const fn topology_mutation_requires_refresh<T>(outcome: &Outcome<T>) -> bool {
    !matches!(outcome, Outcome::Reply(_))
}

/// Board snapshot fields used by the app, excluding residency capabilities.
#[derive(Clone, PartialEq, Eq)]
pub struct BoardView {
    /// Finalized journal schema observed with this task snapshot.
    pub schema: u32,
    /// Greatest durable position covered by every task head.
    pub cursor: Seq,
    /// Current task evidence bundles rendered by the work surface.
    pub tasks: Vec<BoardTask>,
    /// Settled task evidence retained only because a saved group still anchors it.
    pub(crate) room_anchors: Vec<BoardTask>,
    /// Resident allocation order: the task offered next comes first.
    pub offer: Vec<wire::TaskStamp>,
    /// Rows the allocator holds back for an unsettled prerequisite or an
    /// unconcluded or unauthorizing controller, judged by the resident.
    pub blocked: BTreeSet<wire::TaskStamp>,
    /// Rows held by normal Plan/Design control or a prerequisite chain that
    /// still reaches advancing work, judged by the resident.
    pub workflow_waiting: BTreeSet<wire::TaskStamp>,
    /// Every open Design's pass-cycle state, judged by the resident.
    pub design_cycles: BTreeMap<wire::TaskStamp, wire::rooms::DesignCycle>,
    /// Resident steering-key origin index: the Work each quoted direction caused.
    pub(crate) quote_tasks: Rc<QuoteTasks>,
    /// Complete environment observed at the same board snapshot barrier.
    pub(crate) environment: wire::Environment,
}

impl From<&BoardAnswer> for BoardView {
    fn from(answer: &BoardAnswer) -> Self {
        let (room_anchors, tasks) = answer
            .tasks
            .iter()
            .cloned()
            .partition(|task| !task.workflow.body.status.is_pending());
        let offer = answer.offer.clone();
        let environment = answer.environment();
        Self {
            schema: answer.schema,
            cursor: answer.cursor,
            tasks,
            room_anchors,
            offer,
            blocked: answer.blocked.iter().cloned().collect(),
            workflow_waiting: answer.workflow_waiting.iter().cloned().collect(),
            design_cycles: answer.design_cycles.clone(),
            quote_tasks: Rc::new(answer.origins.clone()),
            environment,
        }
    }
}

impl BoardView {
    /// Whether `task` is a Design the operator's decision holds.
    #[cfg_attr(
        all(not(test), not(target_arch = "wasm32")),
        expect(
            dead_code,
            reason = "only the wasm32 work and attention views ask whether a decision holds a Design"
        )
    )]
    pub(crate) fn awaiting_selection(&self, task: &wire::TaskStamp) -> bool {
        self.design_cycles.get(task) == Some(&wire::rooms::DesignCycle::AwaitingDecision)
    }

    /// Pending public work only; hidden rows remain in `tasks` as evidence.
    pub fn active_public_tasks(&self) -> impl Iterator<Item = &BoardTask> {
        self.tasks.iter().filter(|task| task.is_active())
    }

    /// The row for `task`, pending or retained as a room anchor.
    #[must_use]
    pub fn row(&self, task: &wire::TaskStamp) -> Option<&BoardTask> {
        self.tasks
            .iter()
            .chain(&self.room_anchors)
            .find(|row| row.statement.body.born == *task)
    }

    /// Whether the snapshot's environment satisfies one task's typed requirements.
    #[must_use]
    pub fn environment_satisfies(&self, task: &BoardTask) -> bool {
        task.statement
            .body
            .requirements
            .are_satisfied_by(&self.environment)
    }

    /// Whether one typed requirement is satisfied by this snapshot's exact
    /// environment.
    #[must_use]
    pub fn requirement_satisfied(&self, requirement: &wire::EnvironmentRequirement) -> bool {
        self.environment.satisfies(requirement)
    }

    /// Current authority head for one exact executable digest.
    #[must_use]
    pub fn trust_head(&self, digest: &Digest) -> Option<&wire::TrustHead> {
        self.environment.trust_head(digest)
    }

    /// Whether current authority can perform this digest's exact
    /// pended-to-granted transition.
    #[must_use]
    pub fn trust_grant_ready(&self, digest: &Digest) -> bool {
        self.trust_head(digest).is_some_and(|head| {
            head.state == wire::TrustState::Pended
                && head
                    .worktree
                    .as_deref()
                    .is_some_and(|worktree| !worktree.is_empty())
        })
    }
}

/// One agent-authored activity retained for a team status strip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneActivity {
    author: CanonicalUuid,
    lane: CanonicalUuid,
    display_millis: i64,
    kind: AgentActivityKind,
    title: String,
    detail: String,
    text_bytes: usize,
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

/// One occupancy transition used to date the current member presence.
///
/// This is separate from [`LaneLifecycleBoundary`]: every start and ending
/// matters to presence, while the conversation stream intentionally renders
/// only noteworthy lifecycle cards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanePresenceTransition {
    seq: Seq,
    lane: CanonicalUuid,
    occupant: Option<CanonicalUuid>,
    renewed: bool,
    observed: wire::Instant,
}

impl LanePresenceTransition {
    #[must_use]
    pub const fn lane(&self) -> CanonicalUuid {
        self.lane
    }

    #[must_use]
    pub const fn occupant(&self) -> Option<CanonicalUuid> {
        self.occupant
    }

    /// Whether the occupant took over from another one rather than starting
    /// in a vacant seat.
    #[must_use]
    pub const fn renewed(&self) -> bool {
        self.renewed
    }

    #[must_use]
    pub fn display_millis(&self) -> i64 {
        self.observed.get().as_millisecond()
    }
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
        let title = match message.activity {
            AgentActivityKind::Prose => "assistant",
            AgentActivityKind::Failure => "provider failure",
            AgentActivityKind::Tool => "tool",
            AgentActivityKind::Reasoning => "thinking",
            AgentActivityKind::Turn => "new turn",
            AgentActivityKind::Compaction => "compaction",
            AgentActivityKind::Session => "fresh session",
            AgentActivityKind::SignOut => "signed out",
            AgentActivityKind::Direction => "operator direction",
        };
        let detail = match message.activity {
            AgentActivityKind::Prose => {
                watchband::quote_label(watchband::first_nonquoted_prose(&message.text).as_deref())
                    .to_owned()
            }
            _ => message.text.clone(),
        };
        Self {
            author: message.author,
            lane: message.lane,
            display_millis: agent_message_display_millis(message),
            kind: message.activity,
            title: title.to_owned(),
            detail,
            text_bytes: message.text.len(),
            source: ActivitySource {
                lane: message.lane,
                locus: message.locus,
            },
        }
    }

    /// Occupant that produced this activity.
    #[must_use]
    pub const fn author(&self) -> CanonicalUuid {
        self.author
    }

    /// Independently steerable member that produced this activity.
    #[must_use]
    pub const fn lane(&self) -> CanonicalUuid {
        self.lane
    }

    /// Instant used only to place and age this activity on app surfaces.
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
    pub const fn kind(&self) -> AgentActivityKind {
        self.kind
    }

    #[must_use]
    pub const fn class(&self) -> &'static str {
        match self.kind {
            AgentActivityKind::Prose => "prose",
            AgentActivityKind::Failure => "failure",
            AgentActivityKind::Tool => "tool",
            AgentActivityKind::Reasoning => "reasoning",
            AgentActivityKind::Turn => "turn",
            AgentActivityKind::Compaction => "compaction",
            AgentActivityKind::Session => "session",
            AgentActivityKind::SignOut => "sign-out",
            AgentActivityKind::Direction => "direction",
        }
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// Full retained message length used to weight the activity trace.
    #[must_use]
    pub const fn text_bytes(&self) -> usize {
        self.text_bytes
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
/// A compaction reaches the app first as a live announcement and later as the
/// vendor's preserved record. Those representations have different source
/// loci but one vendor boundary identity, so they must reconcile through one
/// DOM and Mosaic key rather than remove and reinsert the divider.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn agent_message_card_identity(message: &wire::AgentMessage) -> String {
    // A direction card stands for one directive whichever delivery row it
    // was cut from, and one row can carry several directives.
    if message.activity == AgentActivityKind::Direction
        && let [context] = message.directive_contexts.as_slice()
        && let wire::QuoteTarget::Steering { key, .. } = &context.quote_target
    {
        return format!("agent-{}-direction-{key}", message.lane);
    }
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
/// Exact identity of one bounded optional relationship enrichment read.
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

/// Capability-scoped state for one request-identity-keyed enrichment read.
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
    #[must_use]
    pub fn answer(&self) -> Option<&TaskRelationsAnswer> {
        match self {
            Self::Available(answer) | Self::Refreshing(answer) => Some(answer),
            Self::Loading | Self::Unavailable { .. } => None,
        }
    }
}

/// Browser-local progress for an actionable environmental blocker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockerActionState {
    Pending,
    Approved,
    Failed(String),
}

/// All state rendered by one capability-scoped app.
///
/// Deliberately not `Debug`: even though scopes redact themselves, snapshots
/// should never gain an accidental whole-runtime debug path.
#[derive(PartialEq, Eq)]
pub struct AppRuntime {
    accepted: Admitted<Accepted>,
    principal: Principal,
    board: ScopedValue<BoardView>,
    lanes: ScopedValue<LaneView>,
    task_details: task_detail::TaskDetails,
    task_relations: BTreeMap<TaskRelationsKey, TaskRelationsState>,
    blocker_actions: BTreeMap<Digest, BlockerActionState>,
    task_relations_revision: u64,
    /// Text of lane-authored publications the operator inbox shows; a
    /// publication's bytes never change, so a settled read stands.
    conversation_texts: BTreeMap<RoomAuthorityStamp, ConversationText>,
    /// Resolved sources of contributions in shown Designs.
    contribution_reads: BTreeMap<RoomAuthorityStamp, ContributionRead>,
    /// Designs the open working-group consoles show, which read their
    /// contributions' sources alongside the task-detail-shown Designs.
    console_designs: BTreeSet<TaskStamp>,
    member_controls: member_control::MemberControls,
    lane_position: Option<Seq>,
    inbox: ScopedValue<InboxAnswer>,
    /// Operator consent requests, read whenever a `consent::` fact streams.
    consent: ScopedValue<ConsentQueueAnswer>,
    consent_rulings: consent::ConsentRulings,
    /// The latest connected-readable principal display directory.
    principals: ScopedValue<PrincipalDirectory>,
    /// The latest operator-only settings read.
    settings: ScopedValue<GlobalSettingsAnswer>,
    /// The latest principal listing, or why the resident refused it to this
    /// principal; read whenever a `principal::` fact streams.
    principal_listing: ScopedValue<Result<wire::principal::PrincipalsAnswer, String>>,
    vendors: ScopedValue<VendorCatalogAnswer>,
    /// Newest journal position that set global fast mode, and its value.
    fast_mode_at: Option<(Seq, bool)>,
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
    rooms: rooms::RoomProjection,
    quota: Option<Rc<wire::QuotaGeneration>>,
    resume: Option<StreamCursor>,
    terminal: Option<StreamTerminal>,
    activities: Vec<LaneActivity>,
    lifecycle_boundaries: Vec<LaneLifecycleBoundary>,
    presence_transitions: Vec<LanePresenceTransition>,
}

/// The lane authority position a reply carries; a direct reply carries none.
const fn lane_reply_seq(position: wire::ReplyPosition) -> Option<Seq> {
    match position {
        wire::ReplyPosition::Committed(range) => Some(range.last),
        wire::ReplyPosition::Snapshot(cursor) => Some(cursor),
        wire::ReplyPosition::Direct {} => None,
    }
}

impl AppRuntime {
    /// Begin empty for the principal named by an admitted acceptance.
    #[must_use]
    pub fn new(accepted: &Admitted<Accepted>) -> Self {
        Self {
            accepted: accepted.clone(),
            principal: Principal::admitted(accepted),
            board: ScopedValue::new(accepted),
            lanes: ScopedValue::new(accepted),
            task_details: task_detail::TaskDetails::default(),
            task_relations: BTreeMap::new(),
            blocker_actions: BTreeMap::new(),
            task_relations_revision: 0,
            conversation_texts: BTreeMap::new(),
            contribution_reads: BTreeMap::new(),
            console_designs: BTreeSet::new(),
            member_controls: member_control::MemberControls::default(),
            lane_position: None,
            inbox: ScopedValue::new(accepted),
            consent: ScopedValue::new(accepted),
            consent_rulings: consent::ConsentRulings::new(),
            principals: ScopedValue::new(accepted),
            settings: ScopedValue::new(accepted),
            principal_listing: ScopedValue::new(accepted),
            vendors: ScopedValue::new(accepted),
            fast_mode_at: None,
            message_cards: message_cards::MessageCards::default(),
            message_history: BTreeMap::new(),
            message_history_completions: BTreeMap::new(),
            message_history_exhausted: BTreeSet::new(),
            message_generation: None,
            message_reconnect_pending: false,
            message_history_revision: 0,
            rooms: rooms::RoomProjection::default(),
            quota: None,
            resume: None,
            terminal: None,
            activities: Vec::new(),
            lifecycle_boundaries: Vec::new(),
            presence_transitions: Vec::new(),
        }
    }
    /// Start a changed resident scope without blanking the operator's mounted
    /// transcript while its replacement snapshots settle.
    ///
    /// Only two capabilities of one human principal share this presentation
    /// cache; any other change of principal starts empty, so data never crosses
    /// an authority boundary. The first message snapshot merges by durable card
    /// identity, retaining deep history and allocation identity while
    /// replacing content the new generation changed.
    pub(crate) fn rehydrate(accepted: &Admitted<Accepted>, previous: &Self) -> Self {
        let mut next = Self::new(accepted);
        if matches!(next.principal.actor(), Actor::Human(_))
            && previous.principal.actor() == next.principal.actor()
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
    /// read in flight at the drop never answers. Room authority restarts
    /// from the new subscription's snapshot. Retained task detail and typed
    /// relationship enrichment keep admitted evidence while replacement reads run.
    pub fn reconnect(&mut self, accepted: &Admitted<Accepted>) -> bool {
        if self.accepted.scope() == accepted.scope() {
            self.accepted = accepted.clone();
            self.principal = Principal::admitted(accepted);
            self.message_reconnect_pending = true;
            self.terminal = None;
            self.rooms = rooms::RoomProjection::default();
            self.reconnect_consent_rulings();
            self.abandon_member_commands();
            self.reconnect_task_details();
            self.task_relations
                .retain(|_, state| refresh_task_relations(state, false));
            self.task_relations_revision = self.task_relations_revision.saturating_add(1);
            self.forget_conversation_text_reads();
            self.forget_contribution_reads();
            false
        } else {
            *self = Self::new(accepted);
            true
        }
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

    /// Last transcript generation admitted atomically; absent until the
    /// first retained message snapshot lands.
    #[must_use]
    pub const fn message_generation(&self) -> Option<u64> {
        self.message_generation
    }

    #[must_use]
    pub fn board(&self) -> Option<&BoardView> {
        self.board.get(&self.principal)
    }

    /// Room, group, and preparation authority from this subscription's room
    /// snapshot and the live facts after it.
    #[must_use]
    pub const fn room_projection(&self) -> &RoomProjection {
        &self.rooms
    }

    /// Current progress for one exact trust digest.
    #[must_use]
    pub fn blocker_action(&self, digest: &Digest) -> Option<&BlockerActionState> {
        self.blocker_actions.get(digest)
    }

    /// Reserve one exact, current trust requirement for approval.
    pub fn begin_trust_grant(
        &mut self,
        task: &wire::TaskStamp,
        revision: Seq,
        digest: &Digest,
    ) -> bool {
        if !self.trust_grant_is_ready(task, revision, digest)
            || matches!(
                self.blocker_actions.get(digest),
                Some(BlockerActionState::Pending | BlockerActionState::Approved)
            )
        {
            return false;
        }
        self.blocker_actions
            .insert(digest.clone(), BlockerActionState::Pending);
        true
    }

    /// Whether the same statement revision still requires this exact digest
    /// and current authority can perform its pended-to-granted transition.
    #[must_use]
    pub fn trust_grant_is_ready(
        &self,
        task: &wire::TaskStamp,
        revision: Seq,
        digest: &Digest,
    ) -> bool {
        self.board().is_some_and(|board| {
            board.trust_grant_ready(digest)
                && board.tasks.iter().any(|row| {
                    row.statement.body.born == *task
                        && row.statement.body.revision == revision
                        && row
                            .statement
                            .body
                            .requirements
                            .as_slice()
                            .iter()
                            .any(|requirement| {
                                matches!(
                                    requirement,
                                    wire::EnvironmentRequirement::TrustGrant { value }
                                        if value == digest
                                            && !board.requirement_satisfied(requirement)
                                )
                            })
                })
        })
    }

    /// Settle one approval while the board refresh catches up.
    pub fn settle_trust_grant(&mut self, digest: Digest, issue: Option<String>) {
        let state = issue.map_or(BlockerActionState::Approved, BlockerActionState::Failed);
        self.blocker_actions.insert(digest, state);
    }

    #[must_use]
    pub fn task_relations(&self, key: &TaskRelationsKey) -> Option<&TaskRelationsState> {
        self.task_relations.get(key)
    }

    #[must_use]
    pub fn task_relations_needs_request(&self, key: &TaskRelationsKey) -> bool {
        matches!(
            self.task_relations.get(key),
            None | Some(TaskRelationsState::Refreshing(_))
        )
    }

    pub fn begin_task_relations(&mut self, key: TaskRelationsKey) {
        if !matches!(
            self.task_relations.get(&key),
            Some(TaskRelationsState::Refreshing(_))
        ) {
            self.task_relations.insert(key, TaskRelationsState::Loading);
        }
    }

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

    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    #[must_use]
    pub(crate) const fn task_relations_revision(&self) -> u64 {
        self.task_relations_revision
    }

    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    fn invalidate_task_relations(&mut self, changes: &[TaskRelationInvalidation]) {
        invalidate_task_relation_states(
            &mut self.task_relations,
            &mut self.task_relations_revision,
            changes,
        );
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

    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    /// Retained card that produced one projected activity, when hydrated.
    #[must_use]
    pub(crate) fn activity_message(&self, activity: &LaneActivity) -> Option<&AgentMessage> {
        self.messages().iter().map(Rc::as_ref).find(|message| {
            message.lane == activity.source.lane && message.locus == activity.source.locus
        })
    }

    /// Durable lane endings in journal chronology.
    #[must_use]
    pub fn lifecycle_boundaries(&self) -> &[LaneLifecycleBoundary] {
        &self.lifecycle_boundaries
    }

    /// Newest retained occupancy transition for one independently steerable lane.
    #[must_use]
    pub fn newest_lane_presence_transition(
        &self,
        lane: CanonicalUuid,
    ) -> Option<&LanePresenceTransition> {
        self.presence_transitions
            .iter()
            .rev()
            .find(|transition| transition.lane == lane)
    }

    /// Retained occupancy transitions of one lane in journal order.
    pub fn lane_presence_transitions(
        &self,
        lane: CanonicalUuid,
    ) -> impl Iterator<Item = &LanePresenceTransition> {
        self.presence_transitions
            .iter()
            .filter(move |transition| transition.lane == lane)
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
            let board = BoardView::from(answer);
            runtime.blocker_actions.retain(|digest, _| {
                board.trust_grant_ready(digest)
                    && board.tasks.iter().any(|task| {
                        task.statement
                            .body
                            .requirements
                            .as_slice()
                            .iter()
                            .any(|requirement| {
                                matches!(
                                    requirement,
                                    wire::EnvironmentRequirement::TrustGrant { value }
                                        if value == digest
                                            && !board.requirement_satisfied(requirement)
                                )
                            })
                    })
            });
            runtime.board.replace(admitted.scope(), board);
        });
    }
    /// Apply one correlated relationship answer without crossing request identity.
    ///
    /// Returns whether the visible relationship content changed. Snapshot
    /// advancement alone is retained without scheduling a Yew publication.
    pub fn apply_task_relations(
        &mut self,
        key: &TaskRelationsKey,
        admitted: &Admitted<Response<TaskRelationsAnswer>>,
    ) -> bool {
        if admitted.scope() != self.accepted.scope() {
            return false;
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
        let changed = !matches!(
            (&state, self.task_relations.get(key).and_then(TaskRelationsState::answer)),
            (TaskRelationsState::Available(next), Some(current))
                if task_relations_content_eq(current, next)
        );
        self.task_relations.insert(key.clone(), state);
        changed
    }

    /// Apply a lane table only when its authority position does not move backwards.
    pub fn apply_lanes(&mut self, admitted: &Admitted<Response<LaneAnswer>>) -> bool {
        if admitted.scope() != self.accepted.scope() {
            return false;
        }
        let position = match &admitted.get().outcome {
            Outcome::Reply(reply) => match lane_reply_seq(reply.position) {
                Some(position) => Some(position),
                None => return false,
            },
            Outcome::Refusal(_) | Outcome::Fault(_) => None,
        };
        if position.is_some_and(|position| self.lane_superseded(position)) {
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

    /// Whether a lane reply for this scope is older than the lane authority
    /// already held, so a newer read has answered for it.
    pub fn lane_reply_superseded(&self, admitted: &Admitted<Response<LaneAnswer>>) -> bool {
        let Outcome::Reply(reply) = &admitted.get().outcome else {
            return false;
        };
        admitted.scope() == self.accepted.scope()
            && lane_reply_seq(reply.position).is_some_and(|position| self.lane_superseded(position))
    }

    fn lane_superseded(&self, position: Seq) -> bool {
        self.lane_position.is_some_and(|current| position < current)
    }

    /// Apply a global settings read or mutation. Fast mode never moves behind
    /// a newer journal position already observed.
    pub fn apply_settings(&mut self, admitted: &Admitted<Response<GlobalSettingsAnswer>>) {
        let position = match &admitted.get().outcome {
            Outcome::Reply(reply) => match reply.position {
                wire::ReplyPosition::Committed(range) => Some(range.last),
                wire::ReplyPosition::Snapshot(cursor) => Some(cursor),
                wire::ReplyPosition::Direct {} => None,
            },

            Outcome::Refusal(_) | Outcome::Fault(_) => None,
        };
        self.apply_response(admitted, |runtime, answer| {
            let Some(position) = position else {
                return;
            };
            let mut answer = answer.clone();
            match runtime.fast_mode_at {
                Some((newer, fast_mode)) if newer > position => answer.fast_mode = fast_mode,
                _ => runtime.fast_mode_at = Some((position, answer.fast_mode)),
            }
            runtime.settings.replace(admitted.scope(), answer);
        });
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

fn task_relations_content_eq(left: &TaskRelationsAnswer, right: &TaskRelationsAnswer) -> bool {
    left.roots == right.roots
        && left.nodes == right.nodes
        && left.edges == right.edges
        && left.truncation == right.truncation
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
struct TaskRelationInvalidation {
    kind: FactKind,
    task: wire::TaskStamp,
    dependency_targets: Vec<wire::TaskStamp>,
    origin_target: Option<wire::TaskStamp>,
}

impl TaskRelationInvalidation {
    fn from_fact(fact: &wire::StreamFact) -> Option<Self> {
        let kind = fact.kind();
        if kind.task_relation_kinds().is_empty() {
            return None;
        }
        if let Some(authorized) = fact.read::<wire::rooms::ExecutionAuthorized>() {
            return Some(Self {
                kind,
                task: authorized.authorization.candidate.controller,
                dependency_targets: authorized.authorization.selected_outputs,
                origin_target: None,
            });
        }
        let task = fact.task()?;
        let dependency_field = match kind {
            FactKind::TaskAdded => Some("depends"),
            FactKind::TaskEdited => Some("dependencies"),
            _ => None,
        };
        let dependency_targets = dependency_field
            .and_then(|field| fact.body().get(field))
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .map(|dependency| {
                dependency
                    .as_str()
                    .expect("validated task relationship targets are strings")
                    .parse()
                    .expect("validated task relationship targets are task stamps")
            })
            .collect();
        let origin_target = (kind == FactKind::TaskAdded)
            .then(|| fact.origin().as_str().parse::<wire::TaskProvenance>().ok())
            .flatten()
            .and_then(|origin| origin.parent().cloned());
        Some(Self {
            kind,
            task,
            dependency_targets,
            origin_target,
        })
    }

    #[cfg(any(target_arch = "wasm32", rust_analyzer, test))]
    fn affects(&self, key: &TaskRelationsKey, state: &TaskRelationsState) -> bool {
        if !self.kind.task_relation_kinds().contains(&key.relation) {
            return false;
        }
        let contains = |task: &wire::TaskStamp| {
            state.answer().map_or_else(
                || key.roots.contains(task),
                |answer| {
                    answer.nodes.iter().any(|node| {
                        matches!(
                            node,
                            wire::intent::TaskRelationNode::Task { task: node_task, .. }
                                if node_task == task
                        )
                    })
                },
            )
        };
        contains(&self.task)
            || match key.relation {
                TaskRelationKind::Dependency => self.dependency_targets.iter().any(contains),
                TaskRelationKind::Origin => self.origin_target.as_ref().is_some_and(contains),
                TaskRelationKind::Review
                | TaskRelationKind::Lifecycle
                | TaskRelationKind::Claim => false,
            }
    }
}

#[cfg(any(target_arch = "wasm32", rust_analyzer, test))]
fn invalidate_task_relation_states(
    states: &mut BTreeMap<TaskRelationsKey, TaskRelationsState>,
    revision: &mut u64,
    changes: &[TaskRelationInvalidation],
) {
    let mut invalidated = false;
    states.retain(|key, state| {
        if !changes.iter().any(|change| change.affects(key, state)) {
            return true;
        }
        invalidated = true;
        refresh_task_relations(state, true)
    });
    if invalidated {
        *revision = revision.saturating_add(1);
    }
}

#[cfg(test)]
mod topology_tests;

#[cfg(test)]
mod task_relation_tests;
