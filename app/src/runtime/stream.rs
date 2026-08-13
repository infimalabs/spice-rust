//! Stream frames projected into capability-scoped Canvas runtime state.

use super::*;

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
        if admitted.scope() != self.accepted.scope() {
            return Vec::new();
        }
        let epoch = self.principal.epoch();
        match admitted.get() {
            StreamFrame::Facts { packed, .. } => self.apply_fact_page(admitted, packed, epoch),
            StreamFrame::Trust { packed, .. } => {
                self.trust_pages.push(self.page(admitted, packed.clone()));
                continuation(ViewSubject::Trust, epoch, packed)
            }
            StreamFrame::Messages {
                limit, messages, ..
            } => {
                self.message_limit = Some(*limit as usize);
                self.messages.clear();
                self.activities.clear();
                for message in messages {
                    self.observe_message(message);
                }
                Vec::new()
            }
            StreamFrame::Thread { packed, .. } => self.apply_thread_page(admitted, packed, epoch),
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
            StreamFrame::Message { message, .. } => {
                self.observe_message(message);
                Vec::new()
            }
            // Proof the socket answered, and nothing else. It reports no state,
            // so applying it changes none.
            StreamFrame::Pong { .. } => Vec::new(),
            StreamFrame::Refusal { refusal, .. } => {
                self.push_event(
                    "refusal",
                    refusal.reason().to_owned(),
                    refusal.explanation().to_owned(),
                );
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

    fn apply_fact_page(
        &mut self,
        admitted: &Admitted<StreamFrame>,
        packed: &Packed<wire::StreamFact>,
        epoch: CanonicalUuid,
    ) -> Vec<StreamDirective> {
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
            return continuation(ViewSubject::Facts, epoch, packed);
        }
        let page = project_page(packed);
        self.observe_fact_rows(admitted, &packed.rows, false);
        self.fact_pages.push(self.page(admitted, page));
        self.resume = Some(StreamCursor {
            epoch,
            seq: page_cursor,
        });
        let mut directives = continuation(ViewSubject::Facts, epoch, packed);
        if packed.overflow.is_none()
            && let Some(last) = packed.rows.last()
        {
            directives.push(self.begin_thread(epoch, last.seq()));
        }
        directives
    }

    fn apply_thread_page(
        &mut self,
        admitted: &Admitted<StreamFrame>,
        packed: &Packed<wire::StreamFact>,
        epoch: CanonicalUuid,
    ) -> Vec<StreamDirective> {
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

    fn observe_message(&mut self, message: &AgentMessage) {
        let (class, title) = match message.activity {
            AgentActivityKind::Prose => ("prose", "assistant"),
            AgentActivityKind::Tool => ("tool", "tool"),
        };
        self.push_activity(LaneActivity {
            lane: message.lane,
            class,
            title: title.to_owned(),
            detail: message.text.clone(),
            source: ActivitySource {
                lane: message.lane,
                locus: message.locus,
            },
        });
        if !message.activity.is_prose() && message.media.is_empty() {
            return;
        }
        let duplicate = self
            .messages
            .iter()
            .any(|existing| existing.lane == message.lane && existing.locus == message.locus);
        if duplicate {
            return;
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

    fn push_activity(&mut self, activity: LaneActivity) {
        if self
            .activities
            .iter()
            .any(|existing| existing.source == activity.source)
        {
            return;
        }
        // Keep the bounded arrival history, not only one row per lane. The
        // renderer takes the first matching row, while this history lets a
        // reconnect recognize an older identity replayed after newer work.
        self.activities.insert(0, activity);
        if let Some(limit) = self.message_limit {
            let lane = self.activities[0].lane;
            retain_first_for_lane(&mut self.activities, lane, limit, |activity| activity.lane);
        }
    }
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
