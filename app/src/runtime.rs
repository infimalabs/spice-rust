//! Capability-scoped state used by the real Wasm canvas runtime.
//!
//! Every mutation enters through an admitted acceptance, response, or stream
//! frame. Stream facts are already credential-free; the canvas projects only
//! the envelope metadata its panes render and retains no kind-specific body.

use wire::intent::{
    BoardAnswer, BoardTask, ConsentPolicyAnswer, ConsentQueueAnswer, InboxAnswer, LaneAnswer,
};
use wire::{
    Accepted, Actor, CanonicalUuid, CommandKind, FactKind, Origin, Outcome, Packed, Response, Seq,
    StreamCursor, StreamFrame, TrustView, ViewSubject,
};

use crate::pane::{GrantPane, Principal, ReadPane, SteerPane};
use crate::socket::{Admitted, SessionScope};
use crate::state::{BrowserState, ChainWalk};

const EVENT_LIMIT: usize = 32;

/// One fact envelope projected to fields that are safe and useful to render.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FactRow {
    seq: Seq,
    actor: Actor,
    origin: Origin,
    cause: Option<Seq>,
    kind: FactKind,
}

impl FactRow {
    /// Durable journal position.
    #[must_use]
    pub const fn seq(&self) -> Seq {
        self.seq
    }

    /// Principal that authored the fact.
    #[must_use]
    pub const fn actor(&self) -> &Actor {
        &self.actor
    }

    /// Root attribution carried by the envelope.
    #[must_use]
    pub const fn origin(&self) -> &Origin {
        &self.origin
    }

    /// Immediately provoking position, absent at a root.
    #[must_use]
    pub const fn cause(&self) -> Option<Seq> {
        self.cause
    }

    /// Closed fact discriminant.
    #[must_use]
    pub const fn kind(&self) -> FactKind {
        self.kind
    }
}

impl From<&wire::StreamFact> for FactRow {
    fn from(fact: &wire::StreamFact) -> Self {
        Self {
            seq: fact.seq(),
            actor: fact.actor().clone(),
            origin: fact.origin().clone(),
            cause: fact.cause(),
            kind: fact.kind(),
        }
    }
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
        let mut tasks = answer.tasks.clone();
        for task in &mut tasks {
            task.redact_hidden_title();
        }
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

/// Lane snapshot fields used by the canvas, excluding capabilities and session
/// authority.
#[derive(Clone, PartialEq, Eq)]
pub struct LaneView {
    /// Resident generation that owns these rows.
    pub epoch: CanonicalUuid,
    /// Current resident state.
    pub state: wire::ResidentState,
    /// Safe current lane rows.
    pub lanes: Vec<LaneRow>,
}

/// One safe current lane row.
#[derive(Clone, PartialEq, Eq)]
pub struct LaneRow {
    pub lane: CanonicalUuid,
    pub worktree: String,
    pub life: String,
    pub vendor: wire::Name,
    pub occupant: Option<CanonicalUuid>,
    pub disposition: String,
}

impl From<&LaneAnswer> for LaneView {
    fn from(answer: &LaneAnswer) -> Self {
        Self {
            epoch: answer.epoch,
            state: answer.state,
            lanes: answer
                .lanes
                .iter()
                .map(|lane| LaneRow {
                    lane: lane.lane,
                    worktree: lane.worktree.clone(),
                    life: lane.life.clone(),
                    vendor: lane.vendor.clone(),
                    occupant: lane.occupant,
                    disposition: lane.disposition.clone(),
                })
                .collect(),
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

impl RuntimeEvent {
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

/// A terminal stream boundary that remains distinct from command outcomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamTerminal {
    /// Broadcast retention was exceeded.
    Overflow { after: StreamCursor, dropped: u64 },
    /// The resident generation shut down cleanly.
    Closing { after: StreamCursor },
}

/// An event-driven request the socket writer must send next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamDirective {
    /// Continue a packed subject at its exact producer cursor.
    Continue {
        subject: ViewSubject,
        cursor: StreamCursor,
    },
    /// Request or continue one causal thread.
    Thread { cursor: StreamCursor },
    /// Close this socket and reconnect after the retained cursor.
    Reconnect,
}

/// All state rendered by one capability-scoped canvas.
///
/// Deliberately not `Debug`: even though scopes redact themselves, snapshots
/// should never gain an accidental whole-runtime debug path.
#[derive(Clone, PartialEq, Eq)]
pub struct CanvasRuntime {
    accepted: Admitted<Accepted>,
    principal: Principal,
    board: ScopedValue<BoardView>,
    lanes: ScopedValue<LaneView>,
    inbox: ScopedValue<InboxAnswer>,
    consent: ScopedValue<ConsentQueueAnswer>,
    policy: ScopedValue<ConsentPolicyAnswer>,
    fact_pages: Vec<BrowserState<FactRow>>,
    trust_pages: Vec<BrowserState<TrustView>>,
    thread_pages: Vec<BrowserState<FactRow>>,
    facts: BrowserState<FactRow>,
    thread: BrowserState<FactRow>,
    thread_start: Option<Seq>,
    thread_next: Option<Seq>,
    resume: Option<StreamCursor>,
    terminal: Option<StreamTerminal>,
    events: Vec<RuntimeEvent>,
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
            inbox: ScopedValue::new(accepted),
            consent: ScopedValue::new(accepted),
            policy: ScopedValue::new(accepted),
            fact_pages: Vec::new(),
            trust_pages: Vec::new(),
            thread_pages: Vec::new(),
            facts: BrowserState::new(accepted),
            thread: BrowserState::new(accepted),
            thread_start: None,
            thread_next: None,
            resume: None,
            terminal: None,
            events: Vec::new(),
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
            subjects.push(ViewSubject::Trust);
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

    #[must_use]
    pub fn events(&self) -> &[RuntimeEvent] {
        &self.events
    }

    #[must_use]
    pub fn fact_pages(&self) -> Vec<ReadPane<'_, FactRow>> {
        self.fact_pages
            .iter()
            .filter_map(|state| ReadPane::over(&self.principal, state))
            .collect()
    }

    #[must_use]
    pub fn trust_pages(&self) -> Vec<ReadPane<'_, TrustView>> {
        self.trust_pages
            .iter()
            .filter_map(|state| ReadPane::over(&self.principal, state))
            .collect()
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

    /// Apply one admitted stream frame and return immediate writer work.
    pub fn apply_stream(&mut self, admitted: &Admitted<StreamFrame>) -> Vec<StreamDirective> {
        if admitted.scope() != self.accepted.scope() {
            return Vec::new();
        }
        let epoch = self.principal.epoch();
        match admitted.get() {
            StreamFrame::Facts { packed, .. } => {
                let page = project_page(packed);
                self.observe_fact_rows(admitted, &packed.rows, false);
                self.fact_pages.push(self.page(admitted, page));
                self.resume = Some(StreamCursor {
                    epoch,
                    seq: packed
                        .overflow
                        .and_then(|_| packed.rows.last().map(wire::StreamFact::seq))
                        .unwrap_or(packed.horizon),
                });
                let mut directives = continuation(ViewSubject::Facts, epoch, packed);
                if packed.overflow.is_none()
                    && let Some(last) = packed.rows.last()
                {
                    directives.push(self.begin_thread(epoch, last.seq()));
                }
                directives
            }
            StreamFrame::Trust { packed, .. } => {
                self.trust_pages.push(self.page(admitted, packed.clone()));
                continuation(ViewSubject::Trust, epoch, packed)
            }
            StreamFrame::Thread { packed, .. } => {
                let Some(first) = packed.rows.first().map(wire::StreamFact::seq) else {
                    return Vec::new();
                };
                if self.thread_next != Some(first) {
                    return Vec::new();
                }
                if self.thread_start.is_none() {
                    self.thread_start = Some(first);
                }
                self.observe_fact_rows(admitted, &packed.rows, true);
                self.thread_pages
                    .push(self.page(admitted, project_page(packed)));
                match packed.overflow {
                    None => {
                        self.thread_next = None;
                        Vec::new()
                    }
                    Some(overflow) => {
                        self.thread_next = Some(overflow.continuation);
                        vec![StreamDirective::Thread {
                            cursor: StreamCursor {
                                epoch,
                                seq: overflow.continuation,
                            },
                        }]
                    }
                }
            }
            StreamFrame::Fact { fact, .. } => {
                self.observe_fact_rows(admitted, std::slice::from_ref(fact), false);
                let packed = Packed {
                    rows: vec![FactRow::from(fact)],
                    overflow: None,
                    horizon: fact.seq(),
                };
                self.fact_pages.push(self.page(admitted, packed));
                self.resume = Some(StreamCursor {
                    epoch,
                    seq: fact.seq(),
                });
                vec![self.begin_thread(epoch, fact.seq())]
            }
            StreamFrame::Refusal { refusal, .. } => {
                self.push_event("refusal", refusal.reason.clone(), refusal.remedy.clone());
                Vec::new()
            }
            StreamFrame::Fault { fault, .. } => {
                self.push_event("fault", fault.reason.clone(), fault.detail.clone());
                Vec::new()
            }
            StreamFrame::Overflow { after, dropped, .. } => {
                self.resume = Some(*after);
                self.terminal = Some(StreamTerminal::Overflow {
                    after: *after,
                    dropped: *dropped,
                });
                self.push_event(
                    "overflow",
                    "Canvas stream overflow".to_owned(),
                    format!("{dropped} updates omitted; resume after {}", after.seq),
                );
                vec![StreamDirective::Reconnect]
            }
            StreamFrame::Closing { after, .. } => {
                self.resume = Some(*after);
                self.terminal = Some(StreamTerminal::Closing { after: *after });
                self.push_event(
                    "closing",
                    "Resident generation closing".to_owned(),
                    format!("resume after {}", after.seq),
                );
                vec![StreamDirective::Reconnect]
            }
        }
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

    pub fn apply_lanes(&mut self, admitted: &Admitted<Response<LaneAnswer>>) {
        self.apply_response(admitted, |runtime, answer| {
            runtime
                .lanes
                .replace(admitted.scope(), LaneView::from(answer));
            (
                "Lane snapshot".to_owned(),
                format!("{} lanes", answer.lanes.len()),
            )
        });
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

    fn apply_response<T>(
        &mut self,
        admitted: &Admitted<Response<T>>,
        reply: impl FnOnce(&mut Self, &T) -> (String, String),
    ) {
        if admitted.scope() != self.accepted.scope() {
            return;
        }
        match &admitted.get().outcome {
            Outcome::Reply(answer) => {
                let (title, detail) = reply(self, &answer.body);
                self.push_event("reply", title, detail);
            }
            Outcome::Refusal(refusal) => {
                self.push_event("refusal", refusal.reason.clone(), refusal.remedy.clone());
            }
            Outcome::Fault(fault) => {
                self.push_event("fault", fault.reason.clone(), fault.detail.clone());
            }
        }
    }

    fn page<T: Clone>(
        &self,
        admitted: &Admitted<StreamFrame>,
        packed: Packed<T>,
    ) -> BrowserState<T> {
        let mut state = BrowserState::new(&self.accepted);
        let replaced = state.replace_stream(admitted, packed);
        debug_assert!(replaced, "runtime page and stream frame share a scope");
        state
    }

    fn observe_fact_rows(
        &mut self,
        admitted: &Admitted<StreamFrame>,
        rows: &[wire::StreamFact],
        thread: bool,
    ) {
        let state = if thread {
            &mut self.thread
        } else {
            &mut self.facts
        };
        for row in rows {
            state.observe_stream(admitted, row.seq(), row.origin().clone(), row.cause());
        }
    }

    fn begin_thread(&mut self, epoch: CanonicalUuid, seq: Seq) -> StreamDirective {
        self.thread = BrowserState::new(&self.accepted);
        self.thread_pages.clear();
        self.thread_start = None;
        self.thread_next = Some(seq);
        StreamDirective::Thread {
            cursor: StreamCursor { epoch, seq },
        }
    }

    fn push_event(&mut self, class: &'static str, title: String, detail: String) {
        self.events.insert(
            0,
            RuntimeEvent {
                class,
                title,
                detail,
            },
        );
        self.events.truncate(EVENT_LIMIT);
    }
}

fn project_page(packed: &Packed<wire::StreamFact>) -> Packed<FactRow> {
    Packed {
        rows: packed.rows.iter().map(FactRow::from).collect(),
        overflow: packed.overflow,
        horizon: packed.horizon,
    }
}

fn continuation<T>(
    subject: ViewSubject,
    epoch: CanonicalUuid,
    packed: &Packed<T>,
) -> Vec<StreamDirective> {
    packed.overflow.map_or_else(Vec::new, |overflow| {
        vec![StreamDirective::Continue {
            subject,
            cursor: StreamCursor {
                epoch,
                seq: overflow.continuation,
            },
        }]
    })
}
