//! Stream frames projected into capability-scoped Canvas runtime state.

use super::*;

#[cfg(test)]
use super::message_cards::trim_agent_message_lane;

const OPERATOR_MESSAGE_LIMIT: usize = 256;
const LIFECYCLE_BOUNDARY_LIMIT: usize = 256;

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
            StreamFrame::Facts { packed, .. } => self.apply_fact_page(packed, epoch),
            StreamFrame::Directions {
                limit, directions, ..
            } => self.apply_direction_snapshot(*limit as usize, directions),
            StreamFrame::Trust { .. } | StreamFrame::Thread { .. } => {
                StreamApplication::new(Vec::new(), false)
            }
            StreamFrame::Messages {
                limit,
                messages,
                more,
                ..
            } => self.apply_message_snapshot(*limit as usize, messages, more),
            StreamFrame::MessageHistory {
                lane,
                before,
                more,
                messages,
                ..
            } => self.apply_message_history(*lane, *before, messages, *more),
            StreamFrame::Fact { fact, .. } => {
                let operator_changed = self.observe_operator_rows(std::slice::from_ref(fact));
                let settings_changed = self.observe_settings_rows(std::slice::from_ref(fact));
                if self
                    .resume
                    .is_some_and(|resume| resume.epoch == epoch && resume.seq >= fact.seq())
                {
                    return StreamApplication::new(
                        Vec::new(),
                        operator_changed || settings_changed,
                    );
                }
                self.observe_lifecycle_boundary(fact);
                self.resume = Some(StreamCursor {
                    epoch,
                    seq: fact.seq(),
                });
                StreamApplication::new(Vec::new(), true)
            }
            StreamFrame::Message { message, .. } => {
                let changed = self.observe_message(message);
                StreamApplication::new(Vec::new(), changed)
            }
            // Proof the socket answered, and nothing else. It reports no state,
            // so applying it changes none.
            StreamFrame::Pong { .. } => StreamApplication::new(Vec::new(), false),
            StreamFrame::Refusal { .. } | StreamFrame::Fault { .. } => {
                StreamApplication::new(Vec::new(), false)
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
                }
                StreamApplication::new(vec![StreamDirective::Reconnect], changed)
            }
            StreamFrame::Closing { after, .. } => {
                let terminal = StreamTerminal::Closing { after: *after };
                let changed = self.resume != Some(*after) || self.terminal != Some(terminal);
                if changed {
                    self.resume = Some(*after);
                    self.terminal = Some(terminal);
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
        let before = self.operator_messages.clone();
        for direction in directions {
            self.retain_operator_message(
                direction.authored,
                direction.directive,
                direction.key.clone(),
                direction.lane,
                direction.text.clone(),
                direction.attachments.clone(),
            );
        }
        trim_operator_messages(&mut self.operator_messages, limit);
        StreamApplication::new(Vec::new(), self.operator_messages != before)
    }

    fn apply_fact_page(
        &mut self,
        packed: &Packed<wire::StreamFact>,
        epoch: CanonicalUuid,
    ) -> StreamApplication {
        let operator_changed = self.observe_operator_rows(&packed.rows);
        let settings_changed = self.observe_settings_rows(&packed.rows);
        let page_cursor = packed
            .overflow
            .and_then(|_| packed.rows.last().map(wire::StreamFact::seq))
            .unwrap_or(packed.horizon);
        if self
            .resume
            .is_some_and(|resume| resume.epoch == epoch && resume.seq >= page_cursor)
        {
            return StreamApplication::new(
                continuation(ViewSubject::Facts, epoch, packed),
                operator_changed || settings_changed,
            );
        }
        for row in &packed.rows {
            self.observe_lifecycle_boundary(row);
        }
        self.resume = Some(StreamCursor {
            epoch,
            seq: page_cursor,
        });
        StreamApplication::new(continuation(ViewSubject::Facts, epoch, packed), true)
    }

    fn observe_settings_rows(&mut self, rows: &[wire::StreamFact]) -> bool {
        let mut changed = false;
        for row in rows {
            let Some(body) = row.read::<wire::body::GlobalSettingsChanged>() else {
                continue;
            };
            if self
                .settings_position
                .is_some_and(|current| current >= row.seq())
            {
                continue;
            }
            self.settings_position = Some(row.seq());
            changed |= self.settings.replace(
                self.accepted.scope(),
                GlobalSettingsAnswer {
                    fast_mode: body.fast_mode,
                },
            );
        }
        changed
    }

    fn observe_lifecycle_boundary(&mut self, row: &wire::StreamFact) {
        let boundary = match row.kind() {
            FactKind::LaneStarted => row
                .read::<wire::body::LaneStarted>()
                .filter(|body| body.fresh_session == Some(true))
                .map(|body| {
                    (
                        body.lane,
                        Some(body.occupant),
                        "started fresh session".to_owned(),
                    )
                }),
            FactKind::LaneExited => row
                .read::<wire::body::LaneExited>()
                .filter(|body| body.code != 0)
                .map(|body| {
                    (
                        body.lane,
                        Some(body.occupant),
                        format!("exited with code {}", body.code),
                    )
                }),
            FactKind::LaneTimedOut => row.read::<wire::body::LaneTimedOut>().map(|body| {
                (
                    body.lane,
                    Some(body.occupant),
                    format!("runtime timed out; code {}", body.code),
                )
            }),
            FactKind::LaneCancelled => row.read::<wire::body::LaneCancelled>().map(|body| {
                (
                    body.lane,
                    Some(body.occupant),
                    format!("supervision cancelled; code {}", body.code),
                )
            }),
            FactKind::LaneSignalled => row.read::<wire::body::LaneSignalled>().map(|body| {
                (
                    body.lane,
                    Some(body.occupant),
                    format!("ended by external signal {}", body.signal),
                )
            }),
            FactKind::LaneDrained => None,
            FactKind::LaneKilled => row.read::<wire::body::LaneKilled>().map(|body| {
                (
                    body.lane,
                    Some(body.occupant),
                    "ended by resident signal".to_owned(),
                )
            }),
            FactKind::LaneDegraded => row
                .read::<wire::body::LaneDegraded>()
                .map(|body| (body.lane, None, "supervision degraded".to_owned())),
            _ => None,
        };
        let Some((lane, occupant, cause)) = boundary else {
            return;
        };
        if self
            .lifecycle_boundaries
            .iter()
            .any(|boundary| boundary.seq == row.seq())
        {
            return;
        }
        self.lifecycle_boundaries.push(LaneLifecycleBoundary {
            seq: row.seq(),
            lane,
            occupant,
            observed: *row.at(),
            cause,
        });
        self.lifecycle_boundaries
            .sort_by_key(|boundary| (boundary.observed, boundary.seq));
        if self.lifecycle_boundaries.len() > LIFECYCLE_BOUNDARY_LIMIT {
            let discard = self.lifecycle_boundaries.len() - LIFECYCLE_BOUNDARY_LIMIT;
            self.lifecycle_boundaries.drain(..discard);
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
        let activity_changed = self.push_activity(LaneActivity::from_message(message));
        let card_changed = self.message_cards.merge(std::iter::once(message));
        activity_changed || card_changed
    }

    fn apply_message_snapshot(
        &mut self,
        limit: usize,
        messages: &[AgentMessage],
        more: &[wire::MessageHistoryCursor],
    ) -> StreamApplication {
        let next_history = message_history_frontiers(more);
        let history_changed = self.message_history != next_history;
        let reconnect = std::mem::take(&mut self.message_reconnect_pending);
        let cards_changed = if reconnect {
            self.message_cards.reconnect_snapshot(limit, messages)
        } else {
            self.message_cards.snapshot(limit, messages)
        };
        let activities_changed = self.apply_activity_snapshot(limit, messages);
        self.message_history_completions
            .retain(|lane, _| next_history.contains_key(lane));
        self.message_history = next_history;
        if reconnect {
            self.message_history_revision = self.message_history_revision.saturating_add(1);
        }
        StreamApplication::new(
            Vec::new(),
            cards_changed || activities_changed || history_changed || reconnect,
        )
    }

    fn apply_message_history(
        &mut self,
        lane: CanonicalUuid,
        before: Option<MessageLocus>,
        messages: &[AgentMessage],
        more: bool,
    ) -> StreamApplication {
        if self.message_history.get(&lane).map(|cursor| cursor.before) != Some(before) {
            return StreamApplication::new(Vec::new(), false);
        }
        let changed = self
            .message_cards
            .merge(messages.iter().filter(|message| message.lane == lane));
        // Source history advances across records the card projection omits.
        let next = messages
            .iter()
            .filter(|message| message.lane == lane)
            .min_by_key(|message| message.chronology_key())
            .map(|message| message.locus)
            .filter(|next| Some(*next) != before);
        let previous = self.message_history.get(&lane).copied();
        if more {
            self.message_history.insert(
                lane,
                wire::MessageHistoryCursor {
                    lane,
                    before: next.or(before),
                },
            );
            let completion = self.message_history_completions.entry(lane).or_default();
            *completion = completion.saturating_add(1);
        } else {
            self.message_history.remove(&lane);
            self.message_history_completions.remove(&lane);
        }
        StreamApplication::new(
            Vec::new(),
            changed || more || self.message_history.get(&lane).copied() != previous,
        )
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

    fn apply_activity_snapshot(&mut self, limit: usize, incoming: &[AgentMessage]) -> bool {
        let mut activities = Vec::<&AgentMessage>::new();
        for message in incoming {
            if !activities
                .iter()
                .any(|existing| existing.lane == message.lane && existing.locus == message.locus)
            {
                activities.insert(0, message);
                retain_first_for_lane(&mut activities, message.lane, limit, |row| row.lane);
            }
        }
        let next = activities
            .into_iter()
            .map(LaneActivity::from_message)
            .collect::<Vec<_>>();
        let changed = self.activities != next;
        self.activities = next;
        changed
    }

    fn push_activity(&mut self, activity: LaneActivity) -> bool {
        if let Some(index) = self
            .activities
            .iter()
            .position(|existing| existing.source == activity.source)
        {
            if self.activities[index] == activity {
                return false;
            }
            self.activities[index] = activity;
            return true;
        }
        if self.message_cards.limit() == Some(0) {
            return false;
        }
        // Keep the bounded arrival history, not only one row per lane. The
        // renderer takes the first matching row, while this history lets a
        // reconnect recognize an older identity replayed after newer work.
        self.activities.insert(0, activity);
        if let Some(limit) = self.message_cards.limit() {
            let lane = self.activities[0].lane;
            retain_first_for_lane(&mut self.activities, lane, limit, |activity| activity.lane);
        }
        true
    }
}

fn message_history_frontiers(
    more: &[wire::MessageHistoryCursor],
) -> BTreeMap<CanonicalUuid, wire::MessageHistoryCursor> {
    more.iter().map(|cursor| (cursor.lane, *cursor)).collect()
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

#[cfg(test)]
mod message_retention_tests {
    use super::*;

    const QUIET_HISTORY_LENGTH: u64 = 40;

    fn message(lane: CanonicalUuid, offset: u64) -> AgentMessage {
        AgentMessage {
            lane,
            author: "018f82d4-6f59-7d06-92e3-bce9131163ef"
                .parse()
                .expect("fixture author"),
            locus: MessageLocus {
                member: 1,
                offset,
                length: 1,
                event: 0,
            },
            authored: "2030-04-12T18:44:09.271Z".parse().expect("fixture instant"),
            authored_synthetic: false,
            observed: None,
            activity: AgentActivityKind::Prose,
            final_answer: false,
            text: offset.to_string(),
            badges: Default::default(),
            automatic_speech: None,
            unavailable_tool_terminals: Vec::new(),
            boundary: None,
            media: Vec::new(),
        }
    }

    #[test]
    fn equal_time_message_chronology_uses_the_complete_admitted_identity() {
        let lane_a = "018f82d4-6f59-7d06-92e3-bce9131163e5"
            .parse()
            .expect("lane a");
        let lane_b = "018f82d4-6f59-7d06-92e3-bce9131163e6"
            .parse()
            .expect("lane b");
        let mut offset_ten = message(lane_a, 10);
        offset_ten.locus.length = 2;
        let mut longer_same_offset = offset_ten.clone();
        longer_same_offset.locus.length = 3;
        let mut later_event = longer_same_offset.clone();
        later_event.locus.event = 1;
        let mut messages = [
            message(lane_b, 1),
            later_event,
            longer_same_offset,
            offset_ten,
            message(lane_a, 2),
        ];

        messages.sort_by_key(AgentMessage::chronology_key);

        assert_eq!(
            messages
                .iter()
                .map(|message| (
                    message.lane,
                    message.locus.offset,
                    message.locus.length,
                    message.locus.event,
                ))
                .collect::<Vec<_>>(),
            vec![
                (lane_a, 2, 1, 0),
                (lane_a, 10, 2, 0),
                (lane_a, 10, 3, 0),
                (lane_a, 10, 3, 1),
                (lane_b, 1, 1, 0),
            ]
        );
    }

    #[test]
    fn reasoning_presence_is_a_thinking_activity_like_a_tool_moment() {
        let lane = "018f82d4-6f59-7d06-92e3-bce9131163e5"
            .parse()
            .expect("lane");
        let mut narration = message(lane, 7);
        narration.activity = AgentActivityKind::Reasoning;
        narration.text = "Checking the gate selection first.".to_owned();
        let activity = LaneActivity::from_message(&narration);
        assert_eq!(activity.class(), "reasoning");
        assert_eq!(activity.title(), "thinking");
        assert_eq!(activity.detail(), "Checking the gate selection first.");

        let mut wordless = narration.clone();
        wordless.text = String::new();
        let activity = LaneActivity::from_message(&wordless);
        assert_eq!(activity.title(), "thinking");
        assert!(activity.detail().is_empty());
        let tool = LaneActivity::from_message(&AgentMessage {
            activity: AgentActivityKind::Tool,
            ..narration
        });
        assert_eq!((tool.class(), tool.title()), ("tool", "tool"));
    }

    #[test]
    fn browser_retention_uses_the_same_compaction_floor_as_the_snapshot() {
        let busy_lane = "018f82d4-6f59-7d06-92e3-bce9131163e5"
            .parse()
            .expect("busy lane");
        let quiet_lane = "018f82d4-6f59-7d06-92e3-bce9131163e6"
            .parse()
            .expect("quiet lane");
        let mut messages = Vec::new();
        for offset in 0..300 {
            let mut activity = message(busy_lane, offset);
            if matches!(offset, 20 | 80 | 220) {
                activity.activity = AgentActivityKind::Compaction;
            }
            messages.push(Rc::new(activity));
            if offset < QUIET_HISTORY_LENGTH {
                messages.push(Rc::new(message(quiet_lane, offset)));
            }
            trim_agent_message_lane(&mut messages, busy_lane, 256);
        }

        let busy = messages
            .iter()
            .filter(|message| message.lane == busy_lane)
            .collect::<Vec<_>>();
        assert_eq!(busy.len(), 220);
        assert_eq!(busy[0].locus.offset, 80);
        assert_eq!(busy[0].activity, AgentActivityKind::Compaction);
        assert_eq!(busy.last().expect("busy tail").locus.offset, 299);
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.lane == quiet_lane)
                .count(),
            usize::try_from(QUIET_HISTORY_LENGTH).expect("quiet history length fits"),
        );
    }
}
