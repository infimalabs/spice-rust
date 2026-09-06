//! Capability-scoped state used by the real Wasm canvas runtime.
//!
//! Every mutation enters through an admitted acceptance, response, or stream
//! frame. Stream facts contribute credential-free envelope metadata; accepted
//! steering replies, live steering facts, and durable direction snapshots
//! contribute the operator records rendered by conversation.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use wire::intent::{
    BoardAnswer, BoardTask, ConsentPolicyAnswer, ConsentQueueAnswer, DirectiveAnswer,
    GlobalSettingsAnswer, InboxAnswer, LaneAnswer, LaneChangeAnswer, LaneLiveness,
};
use wire::{
    Accepted, Actor, AgentActivityKind, AgentMessage, CanonicalUuid, CommandKind, FactKind,
    MessageLocus, Origin, Outcome, Packed, Response, Seq, StreamCursor, StreamFrame, TrustView,
    ViewSubject,
};

use crate::pane::{GrantPane, Principal, ReadPane, SteerPane};
use crate::socket::{Admitted, SessionScope};
use crate::state::{BrowserState, ChainWalk};

mod message_cards;
#[cfg(any(target_arch = "wasm32", test))]
mod publication;
mod stream;
#[cfg(target_arch = "wasm32")]
pub(crate) use publication::CanvasSurface;
pub use stream::{FactRow, OperatorMessage, StreamApplication, StreamDirective, StreamTerminal};

const EVENT_LIMIT: usize = 32;

/// Maximum number of fact pages one long-lived Canvas tab retains.
pub const FACT_PAGE_LIMIT: usize = 16;

/// Maximum number of causal-thread pages one explicit walk may fetch and retain.
pub const THREAD_PAGE_LIMIT: usize = 16;

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
    /// Greatest durable position covered by every task head.
    pub cursor: Seq,
    /// Current actor-facing task evidence bundles.
    pub tasks: Vec<BoardTask>,
    /// Current resident state.
    pub resident: wire::ResidentState,
}

impl From<&BoardAnswer> for BoardView {
    fn from(answer: &BoardAnswer) -> Self {
        let tasks = answer
            .tasks
            .iter()
            .cloned()
            .map(BoardTask::into_actor_facing)
            .collect();
        Self {
            cursor: answer.cursor,
            tasks,
            resident: answer.residency.body.state,
        }
    }
}

impl BoardView {
    /// Pending public work only; hidden rows remain in `tasks` as evidence.
    pub fn active_public_tasks(&self) -> impl Iterator<Item = &BoardTask> {
        self.tasks.iter().filter(|task| task.is_active_public())
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
}

/// One Canvas lane: a lineage and every independently steerable member in it.
#[derive(Clone, PartialEq, Eq)]
pub struct LaneRow {
    /// Stable lineage identity.
    pub lane: CanonicalUuid,
    /// Authoritative task-subscription policy for this lineage.
    pub subscription: wire::intent::LineageLife,
    /// Members in canonical resident seat order.
    pub members: Vec<LaneMember>,
}

/// One independently steerable worktree and occupant inside a Canvas lane.
#[derive(Clone, PartialEq, Eq)]
pub struct LaneMember {
    pub lane: CanonicalUuid,
    pub name: Option<String>,
    pub worktree: String,
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
}

/// What a lane tile says about its agent, ranked the way the operator scans.
///
/// Ported from the predecessor 0.31.0 UI, `spice/serve/static/app.lanes.js`:
/// `targetChoiceStatus` derives it, `targetChoiceStatusLabel` names it, and the
/// `targetChoiceStatusValues` list is the rank. That list carries nine states
/// where this wire now distinguishes five: the resident projects recent and
/// quiet supervised occupants as `running` and `running-stale`, while richer
/// vendor phases remain absent. The ladder keeps the predecessor's order and
/// names only states the resident actually reports.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum LaneStatus {
    /// Direction is queued for this lane; the operator is being waited on.
    Queued,
    /// An occupant is live in the seat.
    Running,
    /// An occupant remains supervised but has been quiet for the stale horizon.
    RunningStale,
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
            Self::Running => "running",
            Self::RunningStale => "running-stale",
            Self::Bound => "bound",
            Self::Unbound => "unbound",
        }
    }

    /// The predecessor's exact wording for this state.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Queued => "steering queued",
            Self::Running => "agent running",
            Self::RunningStale => "agent running-stale",
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
    /// look at, whether or not anything is running in it.
    #[must_use]
    pub const fn status(&self, pending: usize) -> LaneStatus {
        if pending > 0 {
            LaneStatus::Queued
        } else {
            match self.liveness {
                LaneLiveness::Running => LaneStatus::Running,
                LaneLiveness::RunningStale => LaneStatus::RunningStale,
                LaneLiveness::Vacant if self.bound => LaneStatus::Bound,
                LaneLiveness::Vacant => LaneStatus::Unbound,
            }
        }
    }
}

impl LaneRow {
    /// The lineage host, which is the first member in canonical seat order.
    #[must_use]
    pub fn host(&self) -> Option<&LaneMember> {
        self.members.first()
    }

    /// Resolve one independently steerable member of this lineage.
    #[must_use]
    pub fn member(&self, lane: CanonicalUuid) -> Option<&LaneMember> {
        self.members.iter().find(|member| member.lane == lane)
    }
}

impl From<&LaneAnswer> for LaneView {
    fn from(answer: &LaneAnswer) -> Self {
        let mut subscriptions = BTreeMap::new();
        for subscription in &answer.subscriptions {
            assert!(
                subscriptions
                    .insert(subscription.lineage_id, subscription.policy)
                    .is_none(),
                "an admitted lane answer names one policy per lineage"
            );
        }
        let mut lineages = Vec::<LaneRow>::new();
        for lane in &answer.lanes {
            let member = LaneMember {
                lane: lane.lane,
                name: lane.name.clone(),
                worktree: lane.worktree.clone(),
                life: lane.life.clone(),
                vendor: lane.vendor.clone(),
                model: lane.model.clone(),
                effort: lane.effort.clone(),
                occupant: lane.occupant,
                session: lane.session.clone(),
                disposition: lane.disposition.clone(),
                liveness: lane.liveness,
                bound: lane.bound,
            };
            if let Some(lineage) = lineages
                .iter_mut()
                .find(|lineage| lineage.lane == lane.lineage_id)
            {
                lineage.members.push(member);
            } else {
                lineages.push(LaneRow {
                    lane: lane.lineage_id,
                    subscription: *subscriptions
                        .get(&lane.lineage_id)
                        .expect("an admitted lane answer names every lineage policy"),
                    members: vec![member],
                });
            }
        }
        assert_eq!(
            subscriptions.len(),
            lineages.len(),
            "an admitted lane answer names no orphan lineage policy"
        );
        Self {
            epoch: answer.epoch,
            state: answer.state,
            topology_revision: answer.topology_revision,
            lanes: lineages,
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

/// One visible chronological outcome retained from an admitted frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeEvent {
    class: &'static str,
    title: String,
    detail: String,
}

/// One agent-authored activity retained for a lineage status strip.
///
/// Command outcomes deliberately remain [`RuntimeEvent`]s: they have their own
/// chronology pane, while this type is restricted to work performed by a
/// seated lane. Keeping the two chronologies distinct prevents a board, lane,
/// or inbox snapshot receipt from displacing the work the operator is watching.
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
    fn from_message(message: &AgentMessage) -> Self {
        let (class, title) = match message.activity {
            AgentActivityKind::Prose => ("prose", "assistant"),
            AgentActivityKind::Tool => ("tool", "tool"),
            AgentActivityKind::Compaction => ("compaction", "compaction"),
        };
        Self {
            lane: message.lane,
            display_millis: agent_message_display_millis(message),
            class,
            title: title.to_owned(),
            detail: message.text.clone(),
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

/// Durable identity used to ignore a repeated live activity without moving it
/// back to the front of the status chronology.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ActivitySource {
    lane: CanonicalUuid,
    locus: MessageLocus,
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
    lane_position: Option<Seq>,
    inbox: ScopedValue<InboxAnswer>,
    consent: ScopedValue<ConsentQueueAnswer>,
    policy: ScopedValue<ConsentPolicyAnswer>,
    settings: ScopedValue<GlobalSettingsAnswer>,
    settings_position: Option<Seq>,
    fact_pages: Vec<BrowserState<FactRow>>,
    trust_pages: Vec<BrowserState<TrustView>>,
    trust_resume: Option<StreamCursor>,
    message_cards: message_cards::MessageCards,
    operator_messages: Vec<OperatorMessage>,
    /// Exact oldest admitted card each independently paged member can extend.
    message_history: BTreeMap<CanonicalUuid, MessageLocus>,
    /// A same-scope subscription snapshot merges into retained hydrated cards.
    message_reconnect_pending: bool,
    /// Re-arms member-local paging after a same-scope subscription refresh.
    message_history_revision: u64,
    thread_pages: Vec<BrowserState<FactRow>>,
    facts: BrowserState<FactRow>,
    thread: BrowserState<FactRow>,
    thread_start: Option<Seq>,
    thread_next: Option<Seq>,
    resume: Option<StreamCursor>,
    terminal: Option<StreamTerminal>,
    events: Vec<RuntimeEvent>,
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
            lane_position: None,
            inbox: ScopedValue::new(accepted),
            consent: ScopedValue::new(accepted),
            policy: ScopedValue::new(accepted),
            settings: ScopedValue::new(accepted),
            settings_position: None,
            fact_pages: Vec::new(),
            trust_pages: Vec::new(),
            trust_resume: None,
            message_cards: message_cards::MessageCards::default(),
            operator_messages: Vec::new(),
            message_history: BTreeMap::new(),
            message_reconnect_pending: false,
            message_history_revision: 0,
            thread_pages: Vec::new(),
            facts: BrowserState::new(accepted),
            thread: BrowserState::new(accepted),
            thread_start: None,
            thread_next: None,
            resume: None,
            terminal: None,
            events: Vec::new(),
            activities: Vec::new(),
            lifecycle_boundaries: Vec::new(),
        }
    }

    /// Adopt a reconnect. Returns true when a changed capability or epoch
    /// cleared the entire runtime before the new principal can render.
    pub fn reconnect(&mut self, accepted: &Admitted<Accepted>) -> bool {
        if self.accepted.scope() == accepted.scope() {
            self.accepted = accepted.clone();
            self.principal = Principal::admitted(accepted);
            self.facts.reconnect(accepted);
            self.thread.reconnect(accepted);
            for page in &mut self.fact_pages {
                page.reconnect(accepted);
            }
            for page in &mut self.trust_pages {
                page.reconnect(accepted);
            }
            for page in &mut self.thread_pages {
                page.reconnect(accepted);
            }
            self.message_reconnect_pending = true;
            self.terminal = None;
            false
        } else {
            *self = Self::new(accepted);
            true
        }
    }

    #[must_use]
    pub const fn principal(&self) -> &Principal {
        &self.principal
    }

    /// Subjects visible to this admitted principal.
    #[must_use]
    pub fn subjects(&self) -> Vec<ViewSubject> {
        let mut subjects = vec![ViewSubject::Facts];
        if GrantPane::over(&self.principal).is_some() {
            subjects.push(ViewSubject::Directions);
            subjects.push(ViewSubject::Trust);
            subjects.push(ViewSubject::Messages);
        }
        subjects
    }

    /// Whether the principal's steer pane offers this command.
    #[must_use]
    pub fn offers(&self, kind: CommandKind) -> bool {
        SteerPane::over(&self.principal).offers(kind)
    }

    /// Whether this runtime has an admitted operator grant pane.
    #[must_use]
    pub fn grants(&self) -> bool {
        GrantPane::over(&self.principal).is_some()
    }

    #[must_use]
    pub const fn resume(&self) -> Option<StreamCursor> {
        self.resume
    }

    #[must_use]
    pub const fn terminal(&self) -> Option<StreamTerminal> {
        self.terminal
    }

    /// Exact causal cursor whose response can extend the current walk.
    #[must_use]
    pub fn pending_thread(&self) -> Option<StreamCursor> {
        self.thread_next.map(|seq| StreamCursor {
            epoch: self.principal.epoch(),
            seq,
        })
    }

    /// Exact member-local cursor for the next retained history page.
    #[must_use]
    pub fn message_history_target(&self, lane: CanonicalUuid) -> Option<MessageLocus> {
        self.message_history.get(&lane).copied()
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

    #[must_use]
    pub fn lanes(&self) -> Option<&LaneView> {
        self.lanes.get(&self.principal)
    }

    #[must_use]
    pub fn inbox(&self) -> Option<&InboxAnswer> {
        self.inbox.get(&self.principal)
    }

    #[must_use]
    pub fn consent(&self) -> Option<&ConsentQueueAnswer> {
        self.consent.get(&self.principal)
    }

    #[must_use]
    pub fn policy(&self) -> Option<&ConsentPolicyAnswer> {
        self.policy.get(&self.principal)
    }

    /// Authoritative global settings, absent until the initial read lands.
    #[must_use]
    pub fn settings(&self) -> Option<&GlobalSettingsAnswer> {
        self.settings.get(&self.principal)
    }

    #[must_use]
    pub fn events(&self) -> &[RuntimeEvent] {
        &self.events
    }

    /// Newest retained command outcome for the chronology pane.
    #[must_use]
    pub fn newest_event(&self) -> Option<&RuntimeEvent> {
        self.events.first()
    }

    /// Newest retained activity produced by any member of `lineage`.
    ///
    /// Activity is retained in arrival order across the resident's captured
    /// transcript feed. Filtering that one chronology by membership is
    /// what makes a fused team show whichever of its agents acted last.
    #[must_use]
    pub fn newest_lineage_activity(&self, lineage: &LaneRow) -> Option<&LaneActivity> {
        self.activities
            .iter()
            .find(|activity| lineage.member(activity.lane).is_some())
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

    #[must_use]
    pub fn fact_pages(&self) -> Vec<ReadPane<'_, FactRow>> {
        self.fact_pages
            .iter()
            .filter_map(|state| ReadPane::over(&self.principal, state))
            .collect()
    }

    /// Durable operator-authored direction retained in command acceptance order.
    #[must_use]
    pub fn operator_messages(&self) -> &[OperatorMessage] {
        &self.operator_messages
    }

    #[must_use]
    pub fn trust_pages(&self) -> Vec<ReadPane<'_, TrustView>> {
        self.trust_pages
            .iter()
            .filter_map(|state| ReadPane::over(&self.principal, state))
            .collect()
    }

    /// Newest retained agent-authored transcript messages in creation order.
    #[must_use]
    pub fn messages(&self) -> &[Rc<AgentMessage>] {
        self.message_cards.entries()
    }

    #[must_use]
    pub fn thread_pages(&self) -> Vec<ReadPane<'_, FactRow>> {
        self.thread_pages
            .iter()
            .filter_map(|state| ReadPane::over(&self.principal, state))
            .collect()
    }

    #[must_use]
    pub fn thread_walk(&self) -> Option<ChainWalk> {
        self.thread_start.map(|start| self.thread.walk(start))
    }

    pub fn apply_board(&mut self, admitted: &Admitted<Response<BoardAnswer>>) {
        self.apply_response(admitted, |runtime, answer| {
            runtime
                .board
                .replace(admitted.scope(), BoardView::from(answer));
            (
                "Board snapshot".to_owned(),
                format!("cursor {}", answer.cursor),
            )
        });
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
            (
                "Lane snapshot".to_owned(),
                format!("{} lanes", answer.lanes.len()),
            )
        });
        matches!(&admitted.get().outcome, Outcome::Reply(_))
    }

    pub fn apply_inbox(&mut self, admitted: &Admitted<Response<InboxAnswer>>) {
        self.apply_response(admitted, |runtime, answer| {
            runtime.inbox.replace(admitted.scope(), answer.clone());
            let pending = answer
                .lanes
                .iter()
                .map(|lane| lane.pending.len())
                .sum::<usize>();
            ("Steering inbox".to_owned(), format!("{pending} pending"))
        });
    }

    pub fn apply_consent(&mut self, admitted: &Admitted<Response<ConsentQueueAnswer>>) {
        self.apply_response(admitted, |runtime, answer| {
            runtime.consent.replace(admitted.scope(), answer.clone());
            (
                "Consent queue".to_owned(),
                format!("{} pending", answer.pending.len()),
            )
        });
    }

    pub fn apply_policy(&mut self, admitted: &Admitted<Response<ConsentPolicyAnswer>>) {
        self.apply_response(admitted, |runtime, answer| {
            runtime.policy.replace(admitted.scope(), answer.clone());
            (
                "Consent policy".to_owned(),
                format!("{} rules", answer.rules.len()),
            )
        });
    }

    /// Apply a global settings read or mutation without allowing older authority backwards.
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
            if position.is_some_and(|next| {
                runtime
                    .settings_position
                    .is_none_or(|current| next >= current)
            }) {
                runtime.settings_position = position;
                runtime.settings.replace(admitted.scope(), *answer);
            }
            (
                "Global settings".to_owned(),
                format!("fast mode {}", if answer.fast_mode { "on" } else { "off" }),
            )
        });
    }

    /// Record the resident's authoritative result for one lane seating request.
    pub fn apply_lane_change(&mut self, admitted: &Admitted<Response<LaneChangeAnswer>>) {
        self.apply_response(admitted, |_runtime, answer| {
            (
                "Agent seated".to_owned(),
                format!(
                    "{} moved from {} to {}",
                    answer.lane, answer.from, answer.to
                ),
            )
        });
    }

    pub fn apply_note<T>(&mut self, admitted: &Admitted<Response<T>>, task: &str) {
        self.apply_response(admitted, |_runtime, _answer| {
            (
                format!("Note appended to {task}"),
                "reply admitted".to_owned(),
            )
        });
    }

    pub fn apply_decision<T>(&mut self, admitted: &Admitted<Response<T>>) {
        self.apply_response(admitted, |_runtime, _answer| {
            ("Consent decision".to_owned(), "reply admitted".to_owned())
        });
    }

    /// Record the receipt for direction the operator sent from a lane tile.
    ///
    /// The acknowledgment key is the operator's handle on the directive from
    /// here on, so it is the detail rather than the opaque directive identity.
    pub fn apply_steer(
        &mut self,
        admitted: &Admitted<Response<DirectiveAnswer>>,
        lane: CanonicalUuid,
    ) {
        self.apply_response(admitted, |_runtime, answer| {
            (
                format!("Steered lane {lane}"),
                format!(
                    "key {} {}",
                    answer.key,
                    disposition_label(answer.disposition)
                ),
            )
        });
    }

    /// Retain one accepted operator submission and its steering receipt atomically.
    pub fn apply_steer_submission(
        &mut self,
        admitted: &Admitted<Response<DirectiveAnswer>>,
        lane: CanonicalUuid,
        text: String,
        attachments: Vec<wire::Media>,
    ) {
        if admitted.scope() == self.accepted.scope()
            && let Outcome::Reply(reply) = &admitted.get().outcome
            && reply.body.disposition == wire::Disposition::Live
        {
            self.retain_operator_message(
                reply.body.authored,
                reply.body.directive,
                reply.body.key.clone(),
                lane,
                text,
                attachments,
            );
        }
        self.apply_steer(admitted, lane);
    }

    /// Record one response and hand back the chronology row it produced.
    ///
    /// The row is returned rather than only pushed because a caller may need
    /// the failing arms too, and reading them back off the one match here is
    /// what keeps a second interpretation of the same outcome from existing.
    /// `None` means the response belonged to another scope and moved nothing.
    fn apply_response<T>(
        &mut self,
        admitted: &Admitted<Response<T>>,
        reply: impl FnOnce(&mut Self, &T) -> (String, String),
    ) -> Option<RuntimeEvent> {
        if admitted.scope() != self.accepted.scope() {
            return None;
        }
        match &admitted.get().outcome {
            Outcome::Reply(answer) => {
                let (title, detail) = reply(self, &answer.body);
                self.push_event("reply", title, detail);
            }
            Outcome::Refusal(refusal) => {
                self.push_event(
                    "refusal",
                    refusal.reason().to_owned(),
                    refusal.explanation().to_owned(),
                );
            }
            Outcome::Fault(fault) => {
                self.push_event("fault", fault.reason.clone(), fault.detail.clone());
            }
        }
        self.newest_event().cloned()
    }
}

/// The operator-facing spelling of a directive disposition.
///
/// The match is exhaustive, so a new retirement state must state its spelling
/// rather than reach the operator as a debug rendering.
const fn disposition_label(disposition: wire::Disposition) -> &'static str {
    match disposition {
        wire::Disposition::Live => "live",
        wire::Disposition::Acked => "acked",
        wire::Disposition::Declined => "declined",
        wire::Disposition::Lapsed => "lapsed",
        wire::Disposition::DeadLettered => "deadlettered",
        wire::Disposition::Canceled => "canceled",
    }
}

/// Which cached reads a batch of admitted facts has invalidated.
///
/// The stream carries facts, not snapshots: it advances the journal cursor and
/// leaves every read the surface is drawn from exactly as stale as it was.
/// That was survivable while the four axes were the whole surface and the
/// operator was reading history, and it is not survivable for the watchband,
/// which claims to show what a lane is doing right now. Uncorrected, the shell
/// reports itself online with a current cursor while a tile shows a directive
/// two events old — worse than showing nothing, because the staleness is
/// invisible.
///
/// This lives beside [`CanvasRuntime::apply_board`], [`CanvasRuntime::apply_lanes`],
/// [`CanvasRuntime::apply_inbox`] and [`CanvasRuntime::apply_consent`] — one field per read it names — rather than
/// in the socket loop that acts on it, so it can be tested off `wasm32`.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stale {
    board: bool,
    lanes: bool,
    inbox: bool,
    consent: bool,
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
            }
            // A claim binds a task to a lane, so both sides of that join move.
            Some(("claim", _)) => {
                self.board = true;
                self.lanes = true;
            }
            Some(("consent", _)) => self.consent = true,
            _ => {}
        }
    }

    /// Whether the board read went stale.
    #[must_use]
    pub const fn board(self) -> bool {
        self.board
    }

    /// Whether the lane read went stale.
    #[must_use]
    pub const fn lanes(self) -> bool {
        self.lanes
    }

    /// Whether the steering inbox read went stale.
    #[must_use]
    pub const fn inbox(self) -> bool {
        self.inbox
    }

    /// Whether the consent queue read went stale.
    #[must_use]
    pub const fn consent(self) -> bool {
        self.consent
    }

    /// Whether anything needs re-reading at all.
    #[must_use]
    pub const fn any(self) -> bool {
        self.board || self.lanes || self.inbox || self.consent
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
        StreamFrame::Fact { fact, .. } => stale.widen(fact.kind()),
        StreamFrame::Facts { packed, .. } if packed.overflow.is_none() => {
            for row in &packed.rows {
                stale.widen(row.kind());
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
