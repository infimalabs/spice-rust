//! Stream frames projected into capability-scoped Canvas runtime state.

use super::*;

const OPERATOR_MESSAGE_LIMIT: usize = 256;

/// One operator-authored direction projected from durable steering authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorMessage {
    pub(crate) authored: wire::Instant,
    pub(crate) directive: CanonicalUuid,
    pub(crate) key: String,
    pub(crate) lane: CanonicalUuid,
    pub(crate) text: String,
    pub(crate) attachments: Vec<wire::Media>,
}

impl OperatorMessage {
    #[must_use]
    pub const fn authored(&self) -> &wire::Instant {
        &self.authored
    }

    #[must_use]
    pub const fn directive(&self) -> CanonicalUuid {
        self.directive
    }

    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    #[must_use]
    pub const fn lane(&self) -> CanonicalUuid {
        self.lane
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn attachments(&self) -> &[wire::Media] {
        &self.attachments
    }
}

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

/// Writer work and render invalidation produced by one admitted stream frame.
///
/// Directives and visible mutation are independent. A replayed packed page can
/// still owe its continuation while leaving the runtime exactly as it stood;
/// carrying both results prevents the browser from using "writer work exists"
/// as an accidental proxy for "redraw the surface".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamApplication {
    directives: Vec<StreamDirective>,
    changed: bool,
}

impl StreamApplication {
    fn new(directives: Vec<StreamDirective>, changed: bool) -> Self {
        Self {
            directives,
            changed,
        }
    }

    /// Immediate requests the socket writer must carry.
    #[must_use]
    pub fn directives(&self) -> &[StreamDirective] {
        &self.directives
    }

    /// Whether any state a future render can observe changed.
    #[must_use]
    pub const fn changed(&self) -> bool {
        self.changed
    }

    /// Consume the application and carry its writer work onward.
    #[must_use]
    pub fn into_directives(self) -> Vec<StreamDirective> {
        self.directives
    }
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

impl CanvasRuntime {
    /// Apply one admitted stream frame and return immediate writer work.
    pub fn apply_stream(&mut self, admitted: &Admitted<StreamFrame>) -> Vec<StreamDirective> {
        self.apply_stream_frame(admitted).into_directives()
    }

    /// Apply one admitted stream frame and name whether it warrants a redraw.
    pub fn apply_stream_frame(&mut self, admitted: &Admitted<StreamFrame>) -> StreamApplication {
        if admitted.scope() != self.accepted.scope() {
            return StreamApplication::new(Vec::new(), false);
        }
        let epoch = self.principal.epoch();
        match admitted.get() {
            StreamFrame::Facts { packed, .. } => self.apply_fact_page(admitted, packed, epoch),
            StreamFrame::Directions {
                limit, directions, ..
            } => self.apply_direction_snapshot(*limit as usize, directions),
            StreamFrame::Trust { packed, .. } => self.apply_trust_page(admitted, packed, epoch),
            StreamFrame::Messages {
                limit, messages, ..
            } => {
                let limit = *limit as usize;
                let changed = !self.message_snapshot_matches(limit, messages);
                self.message_limit = Some(limit);
                if changed {
                    self.messages.clear();
                    self.activities.clear();
                    for message in messages {
                        self.observe_message(message);
                    }
                }
                StreamApplication::new(Vec::new(), changed)
            }
            StreamFrame::Thread { packed, .. } => self.apply_thread_page(admitted, packed, epoch),
            StreamFrame::Fact { fact, .. } => {
                let operator_changed = self.observe_operator_rows(std::slice::from_ref(fact));
                if self
                    .resume
                    .is_some_and(|resume| resume.epoch == epoch && resume.seq >= fact.seq())
                {
                    return StreamApplication::new(Vec::new(), operator_changed);
                }
                self.observe_fact_rows(admitted, std::slice::from_ref(fact), false);
                self.resume = Some(StreamCursor {
                    epoch,
                    seq: fact.seq(),
                });
                if !operator_visible_fact(fact.kind()) {
                    return StreamApplication::new(Vec::new(), false);
                }
                let packed = Packed {
                    rows: vec![FactRow::from(fact)],
                    overflow: None,
                    horizon: fact.seq(),
                };
                let page = self.page(admitted, packed);
                push_bounded_page(&mut self.fact_pages, page, FACT_PAGE_LIMIT);
                self.prepare_thread(fact.seq());
                StreamApplication::new(Vec::new(), true)
            }
            StreamFrame::Message { message, .. } => {
                let changed = self.observe_message(message);
                StreamApplication::new(Vec::new(), changed)
            }
            // Proof the socket answered, and nothing else. It reports no state,
            // so applying it changes none.
            StreamFrame::Pong { .. } => StreamApplication::new(Vec::new(), false),
            StreamFrame::Refusal { refusal, .. } => {
                self.push_event(
                    "refusal",
                    refusal.reason().to_owned(),
                    refusal.explanation().to_owned(),
                );
                StreamApplication::new(Vec::new(), true)
            }
            StreamFrame::Fault { fault, .. } => {
                self.push_event("fault", fault.reason.clone(), fault.detail.clone());
                StreamApplication::new(Vec::new(), true)
            }
            StreamFrame::Overflow { after, dropped, .. } => {
                let terminal = StreamTerminal::Overflow {
                    after: *after,
                    dropped: *dropped,
                };
                let changed = self.resume != Some(*after) || self.terminal != Some(terminal);
                if changed {
                    self.resume = Some(*after);
                    self.terminal = Some(terminal);
                    self.push_event(
                        "overflow",
                        "Canvas stream overflow".to_owned(),
                        format!("{dropped} updates omitted; resume after {}", after.seq),
                    );
                }
                StreamApplication::new(vec![StreamDirective::Reconnect], changed)
            }
            StreamFrame::Closing { after, .. } => {
                let terminal = StreamTerminal::Closing { after: *after };
                let changed = self.resume != Some(*after) || self.terminal != Some(terminal);
                if changed {
                    self.resume = Some(*after);
                    self.terminal = Some(terminal);
                    self.push_event(
                        "closing",
                        "Resident generation closing".to_owned(),
                        format!("resume after {}", after.seq),
                    );
                }
                StreamApplication::new(vec![StreamDirective::Reconnect], changed)
            }
        }
    }

    fn apply_direction_snapshot(
        &mut self,
        limit: usize,
        directions: &[wire::OperatorDirection],
    ) -> StreamApplication {
        let mut projected = directions
            .iter()
            .map(|direction| OperatorMessage {
                authored: direction.authored,
                directive: direction.directive,
                key: direction.key.clone(),
                lane: direction.lane,
                text: direction.text.clone(),
                attachments: direction.attachments.clone(),
            })
            .collect::<Vec<_>>();
        trim_operator_messages(&mut projected, limit);
        let changed = self.operator_messages != projected;
        if changed {
            self.operator_messages = projected;
        }
        StreamApplication::new(Vec::new(), changed)
    }

    fn apply_fact_page(
        &mut self,
        admitted: &Admitted<StreamFrame>,
        packed: &Packed<wire::StreamFact>,
        epoch: CanonicalUuid,
    ) -> StreamApplication {
        let operator_changed = self.observe_operator_rows(&packed.rows);
        let page_cursor = packed
            .overflow
            .and_then(|_| packed.rows.last().map(wire::StreamFact::seq))
            .unwrap_or(packed.horizon);
        // A same-scope replacement may extend retained history, but a packed
        // page at or below its resume cursor is replay rather than a new page.
        if self
            .resume
            .is_some_and(|resume| resume.epoch == epoch && resume.seq >= page_cursor)
        {
            return StreamApplication::new(
                continuation(ViewSubject::Facts, epoch, packed),
                operator_changed,
            );
        }
        let page = project_page(packed);
        let visible_tail = page.rows.last().map(FactRow::seq);
        self.observe_fact_rows(admitted, &packed.rows, false);
        let changed = !page.rows.is_empty();
        if changed {
            let page = self.page(admitted, page);
            push_bounded_page(&mut self.fact_pages, page, FACT_PAGE_LIMIT);
        }
        self.resume = Some(StreamCursor {
            epoch,
            seq: page_cursor,
        });
        let mut directives = continuation(ViewSubject::Facts, epoch, packed);
        if packed.overflow.is_none()
            && let Some(seq) = visible_tail
        {
            directives.push(self.begin_thread(epoch, seq));
        }
        StreamApplication::new(directives, changed)
    }

    fn apply_trust_page(
        &mut self,
        admitted: &Admitted<StreamFrame>,
        packed: &Packed<wire::TrustView>,
        epoch: CanonicalUuid,
    ) -> StreamApplication {
        let page_cursor = packed
            .overflow
            .and_then(|_| packed.rows.last().map(|row| row.position))
            .unwrap_or(packed.horizon);
        let directives = continuation(ViewSubject::Trust, epoch, packed);
        if self
            .trust_resume
            .is_some_and(|resume| resume.epoch == epoch && resume.seq >= page_cursor)
        {
            return StreamApplication::new(directives, false);
        }
        let page = self.page(admitted, packed.clone());
        self.trust_pages.push(page);
        self.trust_resume = Some(StreamCursor {
            epoch,
            seq: page_cursor,
        });
        StreamApplication::new(directives, true)
    }

    fn apply_thread_page(
        &mut self,
        admitted: &Admitted<StreamFrame>,
        packed: &Packed<wire::StreamFact>,
        epoch: CanonicalUuid,
    ) -> StreamApplication {
        let Some(first) = packed.rows.first().map(wire::StreamFact::seq) else {
            return StreamApplication::new(Vec::new(), false);
        };
        if self.thread_next != Some(first) {
            return StreamApplication::new(Vec::new(), false);
        }
        if self.thread_start.is_none() {
            self.thread_start = Some(first);
        }
        self.observe_fact_rows(admitted, &packed.rows, true);
        let page = self.page(admitted, project_page(packed));
        push_bounded_page(&mut self.thread_pages, page, THREAD_PAGE_LIMIT);
        let directives = match packed.overflow {
            None => {
                self.thread_next = None;
                Vec::new()
            }
            Some(_) if self.thread_pages.len() == THREAD_PAGE_LIMIT => {
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
        };
        StreamApplication::new(directives, true)
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
        for row in rows {
            let state = if thread {
                &mut self.thread
            } else {
                &mut self.facts
            };
            state.observe_stream(admitted, row.seq(), row.origin().clone(), row.cause());
        }
    }

    fn observe_operator_rows(&mut self, rows: &[wire::StreamFact]) -> bool {
        let mut changed = false;
        for row in rows {
            if row.actor() == &Actor::Operator
                && let Some(sent) = row.read::<wire::body::SteerSent>()
            {
                changed |= self.retain_operator_message(
                    *row.at(),
                    sent.directive,
                    sent.key,
                    sent.lane,
                    sent.text,
                    sent.attachments,
                );
            }
        }
        changed
    }

    fn observe_message(&mut self, message: &AgentMessage) -> bool {
        let (class, title) = match message.activity {
            AgentActivityKind::Prose => ("prose", "assistant"),
            AgentActivityKind::Tool => ("tool", "tool"),
            AgentActivityKind::Compaction => ("compaction", "compaction"),
        };
        let activity_changed = self.push_activity(LaneActivity {
            lane: message.lane,
            display_millis: agent_message_display_millis(message),
            class,
            title: title.to_owned(),
            detail: message.text.clone(),
            source: ActivitySource {
                lane: message.lane,
                locus: message.locus,
            },
        });
        if !visible_message_card(message) {
            return activity_changed;
        }
        if self.message_limit == Some(0) {
            return activity_changed;
        }
        let duplicate = self
            .messages
            .iter()
            .any(|existing| existing.lane == message.lane && existing.locus == message.locus);
        if duplicate {
            return activity_changed;
        }
        self.messages.push(message.clone());
        if let Some(limit) = self.message_limit
            && self
                .messages
                .iter()
                .filter(|existing| existing.lane == message.lane)
                .count()
                > limit
            && let Some(oldest) = self
                .messages
                .iter()
                .position(|existing| existing.lane == message.lane)
        {
            self.messages.remove(oldest);
        }
        true
    }

    pub(crate) fn retain_operator_message(
        &mut self,
        authored: wire::Instant,
        directive: CanonicalUuid,
        key: String,
        lane: CanonicalUuid,
        text: String,
        attachments: Vec<wire::Media>,
    ) -> bool {
        let message = OperatorMessage {
            authored,
            directive,
            key,
            lane,
            text,
            attachments,
        };
        if self
            .operator_messages
            .iter()
            .any(|existing| existing.directive == message.directive)
        {
            return false;
        }
        let lane = message.lane;
        self.operator_messages.push(message);
        trim_operator_lane(&mut self.operator_messages, lane, OPERATOR_MESSAGE_LIMIT);
        true
    }

    /// Whether one snapshot projects to the exact message and activity slices
    /// already retained, without cloning any of their payload bodies.
    fn message_snapshot_matches(&self, limit: usize, incoming: &[AgentMessage]) -> bool {
        let mut messages = Vec::<&AgentMessage>::new();
        let mut activities = Vec::<&AgentMessage>::new();
        for message in incoming {
            if !activities
                .iter()
                .any(|existing| existing.lane == message.lane && existing.locus == message.locus)
            {
                activities.insert(0, message);
                retain_first_for_lane(&mut activities, message.lane, limit, |row| row.lane);
            }
            if visible_message_card(message)
                && !messages.iter().any(|existing| {
                    existing.lane == message.lane && existing.locus == message.locus
                })
            {
                messages.push(message);
                if messages
                    .iter()
                    .filter(|existing| existing.lane == message.lane)
                    .count()
                    > limit
                    && let Some(oldest) = messages
                        .iter()
                        .position(|existing| existing.lane == message.lane)
                {
                    messages.remove(oldest);
                }
            }
        }
        let activity_count = activities.len();
        self.messages.iter().eq(messages)
            && self
                .activities
                .iter()
                .zip(activities)
                .all(|(retained, projected)| retained.matches_message(projected))
            && self.activities.len() == activity_count
    }

    fn begin_thread(&mut self, epoch: CanonicalUuid, seq: Seq) -> StreamDirective {
        self.prepare_thread(seq);
        StreamDirective::Thread {
            cursor: StreamCursor { epoch, seq },
        }
    }

    fn prepare_thread(&mut self, seq: Seq) {
        self.thread = BrowserState::new(&self.accepted);
        self.thread_pages.clear();
        self.thread_start = None;
        self.thread_next = Some(seq);
    }

    pub(super) fn push_event(&mut self, class: &'static str, title: String, detail: String) {
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

    fn push_activity(&mut self, activity: LaneActivity) -> bool {
        if self
            .activities
            .iter()
            .any(|existing| existing.source == activity.source)
        {
            return false;
        }
        if self.message_limit == Some(0) {
            return false;
        }
        // Keep the bounded arrival history, not only one row per lane. The
        // renderer takes the first matching row, while this history lets a
        // reconnect recognize an older identity replayed after newer work.
        self.activities.insert(0, activity);
        if let Some(limit) = self.message_limit {
            let lane = self.activities[0].lane;
            retain_first_for_lane(&mut self.activities, lane, limit, |activity| activity.lane);
        }
        true
    }
}

fn trim_operator_messages(messages: &mut Vec<OperatorMessage>, limit: usize) {
    let lanes = messages
        .iter()
        .map(|message| message.lane)
        .collect::<BTreeSet<_>>();
    for lane in lanes {
        trim_operator_lane(messages, lane, limit);
    }
}

fn trim_operator_lane(messages: &mut Vec<OperatorMessage>, lane: CanonicalUuid, limit: usize) {
    while messages
        .iter()
        .filter(|message| message.lane == lane)
        .count()
        > limit
    {
        let oldest = messages
            .iter()
            .enumerate()
            .filter(|(_, message)| message.lane == lane)
            .min_by_key(|(_, message)| message.authored.get().as_millisecond())
            .map(|(index, _)| index)
            .expect("a lane above its bound has an oldest operator message");
        messages.remove(oldest);
    }
}

impl LaneActivity {
    fn matches_message(&self, message: &AgentMessage) -> bool {
        let (class, title) = match message.activity {
            AgentActivityKind::Prose => ("prose", "assistant"),
            AgentActivityKind::Tool => ("tool", "tool"),
            AgentActivityKind::Compaction => ("compaction", "compaction"),
        };
        self.lane == message.lane
            && self.class == class
            && self.title == title
            && self.detail == message.text
            && self.source
                == (ActivitySource {
                    lane: message.lane,
                    locus: message.locus,
                })
    }
}

fn visible_message_card(message: &AgentMessage) -> bool {
    matches!(
        message.activity,
        AgentActivityKind::Prose | AgentActivityKind::Compaction
    ) || !message.media.is_empty()
}

/// Keep the first bounded rows for one lane without evicting sibling-lane history.
fn retain_first_for_lane<T>(
    rows: &mut Vec<T>,
    lane: CanonicalUuid,
    limit: usize,
    row_lane: impl Fn(&T) -> CanonicalUuid,
) {
    let mut seen = 0;
    rows.retain(|row| {
        if row_lane(row) != lane {
            return true;
        }
        seen += 1;
        seen <= limit
    });
}

fn project_page(packed: &Packed<wire::StreamFact>) -> Packed<FactRow> {
    Packed {
        rows: packed
            .rows
            .iter()
            .filter(|fact| operator_visible_fact(fact.kind()))
            .map(FactRow::from)
            .collect(),
        overflow: packed.overflow,
        horizon: packed.horizon,
    }
}

/// Whether one durable fact is useful in the operator's event chronology.
///
/// Transcript ingestion receipts prove that a capture reader consumed one
/// source byte range. They remain durable and continue advancing the stream
/// cursor and causal-header window, but rendering every receipt overwhelms the
/// facts about tasks, claims, lanes, and steering that the chronology exists
/// to expose. The transcript message stream is the operator-facing projection
/// of the captured content.
fn operator_visible_fact(kind: FactKind) -> bool {
    kind != FactKind::JournalIngested
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

fn push_bounded_page<T>(pages: &mut Vec<BrowserState<T>>, page: BrowserState<T>, limit: usize) {
    pages.push(page);
    if pages.len() > limit {
        pages.remove(0);
    }
}
