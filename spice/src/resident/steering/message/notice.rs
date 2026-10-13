//! What a packet's author and the resident diagnostic hear about one
//! publication or refusal.

use super::*;

pub(super) fn refusal_detail(text: &str, refusal: &steer::ack::scan::MessageRefusal) -> String {
    use steer::ack::scan::MessageRefusal;
    match refusal {
        MessageRefusal::Unterminated => "the selector list is not closed by a colon".to_owned(),
        MessageRefusal::Selectors { verb, count } => {
            format!("{} names exactly one selector, not {count}", verb.marker())
        }
        MessageRefusal::Recipient { span, error } => {
            format!(
                "`{}` is neither an occupant address nor `{}`: {error}",
                &text[span.clone()],
                steer::ack::scan::message::OPERATOR,
            )
        }
        MessageRefusal::Audience { span } => {
            format!("`{}` names no audience to ask", &text[span.clone()])
        }
        MessageRefusal::Result { span, error } => {
            format!(
                "`{}` is not a question result handle: {error}",
                &text[span.clone()]
            )
        }
        MessageRefusal::Context { span, .. } => {
            format!("`#{}` is not a task handle", &text[span.clone()])
        }
        MessageRefusal::DirectionAudience { span } => format!(
            "`{}` names a direction, which concerns only the operator: write `#KEY`",
            &text[span.clone()]
        ),
        MessageRefusal::Repeated { audience, .. } => {
            format!("{audience} is named more than once")
        }
        MessageRefusal::RepeatedContext { context, .. } => {
            format!("#{context} names a room another room-only route already names")
        }
        MessageRefusal::EmptyBody => "the packet has no body".to_owned(),
    }
}

/// Tell the recipients, or the author and the diagnostic, what one packet did.
pub(super) fn report(
    paths: &RuntimePaths,
    record: &AuthoredRecord<'_>,
    locus: &TranscriptLocus,
    header: &str,
    outcome: &Result<Published, Unpublished>,
    refusals: Refusals,
    diagnostic: &mut dyn io::Write,
) {
    let teller = Teller {
        paths,
        record,
        locus,
        header,
        refusals,
    };
    match outcome {
        Ok(Published::Sent {
            lanes,
            queued,
            unreached,
            rooms,
            conversed,
            held_room,
        }) => {
            for lane in lanes {
                notify_inbox(paths, *lane);
            }
            if !rooms.is_empty() || conversed.is_some() || !queued.is_empty() {
                teller.published(
                    &Recorded {
                        rooms,
                        conversed: conversed.as_ref(),
                        queued,
                        reached: lanes.len(),
                        held_room: *held_room,
                    },
                    diagnostic,
                );
            }
            if !unreached.is_empty() {
                teller.tell(UNAVAILABLE, &unreached.join("; "), lanes.len(), diagnostic);
            }
        }
        Ok(Published::Already) | Err(Unpublished::Withheld | Unpublished::Failed(_)) => {}
        Err(Unpublished::Refused { reason, detail }) => {
            teller.tell(reason, detail, 0, diagnostic);
        }
    }
}

/// Where one packet's outcome is told.
struct Teller<'a> {
    paths: &'a RuntimePaths,
    record: &'a AuthoredRecord<'a>,
    locus: &'a TranscriptLocus,
    header: &'a str,
    refusals: Refusals,
}

/// What one published packet did beyond its deliveries.
#[derive(Clone, Copy)]
struct Recorded<'a> {
    rooms: &'a [TaskHandle],
    conversed: Option<&'a Conversed>,
    queued: &'a [OccupantAddress],
    reached: usize,
    held_room: bool,
}

impl Teller<'_> {
    /// Stable identity of the packet's source locus.
    fn packet(&self) -> String {
        let locus = self.locus;
        format!(
            "{}:{}:{}:{}:{}",
            locus.member, locus.offset, locus.event, locus.line, locus.column
        )
    }

    /// Record the refusal and, under [`Refusals::Notify`], tell the author
    /// privately, with how many recipients the packet did reach.
    fn tell(&self, reason: &str, detail: &str, reached: usize, diagnostic: &mut dyn io::Write) {
        let (record, locus) = (self.record, self.locus);
        let _ = writeln!(
            diagnostic,
            "lane {} packet `{}` at record {}:{} event {} line {}:{} reached {reached} and \
             refused: {reason}: {detail}",
            record.lane,
            self.header,
            locus.member,
            locus.offset,
            locus.event,
            locus.line,
            locus.column,
        );
        if self.refusals == Refusals::Notify {
            crate::resident::supervisor::command::message_refused(
                self.paths,
                record.lane,
                MessageRefusalNotice {
                    occupant: record.author,
                    packet: self.packet(),
                    header: bounded(self.header),
                    line: locus.line,
                    column: locus.column,
                    reached,
                    reason: reason.to_owned(),
                    detail: bounded(detail),
                },
            );
        }
    }

    /// Record what the packet published beyond its deliveries and, under
    /// [`Refusals::Notify`], tell the author privately; no participant
    /// receipt is implied.
    fn published(&self, outcome: &Recorded<'_>, diagnostic: &mut dyn io::Write) {
        let (record, locus) = (self.record, self.locus);
        let Recorded {
            rooms,
            conversed,
            queued,
            reached,
            held_room,
        } = *outcome;
        let rooms = rooms.iter().map(ToString::to_string).collect::<Vec<_>>();
        let queued = queued.iter().map(ToString::to_string).collect::<Vec<_>>();
        let operator = conversed.is_some_and(|conversed| conversed.operator);
        let bare_operator = conversed.and_then(|conversed| conversed.bare_operator.clone());
        let (asked, answered) = match conversed {
            Some(Conversed {
                verb: Verb::Ask,
                result: Some(result),
                ..
            }) => (Some(result.to_string()), None),
            Some(Conversed {
                verb: Verb::Ans,
                result: Some(result),
                ..
            }) => (None, Some(result.to_string())),
            Some(_) | None => (None, None),
        };
        let mut outcome = String::new();
        if operator {
            outcome.push_str(" with an operator item");
        }
        if let Some(result) = &asked {
            let _ = write!(outcome, " opening result {result}");
        }
        if let Some(result) = &answered {
            let _ = write!(outcome, " answering result {result}");
        }
        if !queued.is_empty() {
            let _ = write!(outcome, " queued for [{}]", queued.join(", "));
        }
        let _ = writeln!(
            diagnostic,
            "lane {} packet `{}` at record {}:{} event {} line {}:{} recorded in [{}]{outcome}",
            record.lane,
            self.header,
            locus.member,
            locus.offset,
            locus.event,
            locus.line,
            locus.column,
            rooms.join(", "),
        );
        if self.refusals == Refusals::Notify {
            crate::resident::supervisor::command::message_recorded(
                self.paths,
                record.lane,
                MessageRecordedNotice {
                    occupant: record.author,
                    packet: self.packet(),
                    header: bounded(self.header),
                    line: locus.line,
                    column: locus.column,
                    rooms,
                    operator,
                    bare_operator,
                    asked,
                    answered,
                    queued,
                    reached,
                    held_room,
                },
            );
        }
    }
}

/// One private record of what a packet published beyond its deliveries: the
/// rooms it recorded in, the operator item it opened, and the question result
/// it opened or answered, scoped to the packet's authoring occupant.
#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::resident) struct MessageRecordedNotice {
    pub(in crate::resident) occupant: CanonicalUuid,
    /// Stable identity of the recorded packet's source locus.
    pub(in crate::resident) packet: String,
    /// The packet header as authored, bounded.
    pub(in crate::resident) header: String,
    pub(in crate::resident) line: u32,
    pub(in crate::resident) column: u32,
    /// Each room the packet recorded in, as its header spelled the task.
    pub(in crate::resident) rooms: Vec<String>,
    /// Historical addresses whose vacant logical lanes hold queued delivery.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(in crate::resident) queued: Vec<String>,
    /// Whether the packet opened one pending operator item.
    pub(in crate::resident) operator: bool,
    /// A bare `operator` route and what it could have named instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::resident) bare_operator: Option<BareOperator>,
    /// The question result an ASK opened.
    pub(in crate::resident) asked: Option<String>,
    /// The question result an ANS answered.
    pub(in crate::resident) answered: Option<String>,
    /// How many named recipients the packet reached.
    pub(in crate::resident) reached: usize,
    /// Some `#TASK` the packet named is the task its author's lane holds.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(in crate::resident) held_room: bool,
}

/// A bare `operator` route and the contextual selector its lane could have
/// named instead.
#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::resident) struct BareOperator {
    /// `#KEY` of the lane's latest operator direction, else `operator#TASK`
    /// of its active claim; absent when it has neither.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::resident) instead: Option<String>,
}

impl MessageRecordedNotice {
    /// The notice its author hears.
    pub(in crate::resident) fn text(&self) -> String {
        let mut clauses = Vec::new();
        if let Some(result) = &self.asked {
            clauses.push(format!(
                "opened pending question result `{result}`; its respondent owes one answer, \
                 which returns to you as its own delivery"
            ));
        }
        if let Some(result) = &self.answered {
            clauses.push(format!(
                "answered question result `{result}`; the answer returns to its requester"
            ));
        }
        if !self.queued.is_empty() {
            clauses.push(format!(
                "queued for {}; unheard until each lane's next occupant starts; nothing woke anyone",
                self.queued.join(", ")
            ));
        }
        if self.operator {
            clauses.push(
                "opened one pending operator item, which the operator reads and settles in the \
                 Operator inbox"
                    .to_owned(),
            );
        }
        if let Some(bare) = &self.bare_operator {
            clauses.push(match &bare.instead {
                Some(selector) => format!(
                    "named bare `operator`, the last resort, where `{selector}` would have told \
                     the operator what it concerns"
                ),
                None => "named bare `operator`, the last resort, where `#KEY` of a direction \
                         the operator sent you or `operator#TASK` tells the operator what it \
                         concerns"
                    .to_owned(),
            });
        }
        if !self.rooms.is_empty() {
            clauses.push(format!("was recorded in room {}", self.rooms.join(", ")));
        }
        match self.reached {
            0 => {}
            1 => clauses.push("reached 1 recipient".to_owned()),
            reached => clauses.push(format!("reached {reached} recipients")),
        }
        let recorded = if self.rooms.is_empty() {
            ""
        } else {
            " Recorded means the room holds it; no participant has received or acknowledged \
             it, and nothing woke anyone."
        };
        let held = if self.held_room {
            " It named your own held task: ordinary output needs no packet header, and \
             `MSG ADDRESS:` alone delivers; add `#TASK` only to record something \
             deliberately in that task's room."
        } else {
            ""
        };
        format!(
            "<system-notice>\nYour packet `{}` at line {}, column {} of its assistant text \
             block {}.{recorded}{held} This is a private notice; no acknowledgment key or \
             visible ACK/NAK is required.\n</system-notice>",
            self.header,
            self.line,
            self.column,
            clauses.join("; "),
        )
    }
}

/// One private packet refusal, scoped to the packet's authoring occupant.
#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::resident) struct MessageRefusalNotice {
    pub(in crate::resident) occupant: CanonicalUuid,
    /// Stable identity of the refused packet's source locus.
    pub(in crate::resident) packet: String,
    /// The packet header as authored, bounded.
    pub(in crate::resident) header: String,
    pub(in crate::resident) line: u32,
    pub(in crate::resident) column: u32,
    /// How many named recipients the packet did reach.
    pub(in crate::resident) reached: usize,
    pub(in crate::resident) reason: String,
    /// Which addresses the packet missed, or why it was refused, bounded.
    pub(in crate::resident) detail: String,
}

impl MessageRefusalNotice {
    /// The notice its author hears.
    pub(in crate::resident) fn text(&self) -> String {
        let (outcome, missed) = match self.reached {
            0 => ("published nothing".to_owned(), "No recipient received it"),
            reached => (
                format!(
                    "reached {reached} recipient{}, but not every address it named",
                    if reached == 1 { "" } else { "s" }
                ),
                "Only the addresses named here did not receive it",
            ),
        };
        format!(
            "<system-notice>\nYour packet `{}` at line {}, column {} of its assistant text \
             block {outcome}: {}: {}. {missed}. Name only exact addresses of other seated \
             occupants (`spice lane list` shows each lane's) or the operator, each once: \
             `#KEY` of a direction the operator sent you, else `operator#TASK`, else bare \
             `operator` as the last resort; give an ASK exactly one audience and an ANS \
             exactly its question's result handle; qualify a selector with `#TASK` only for \
             an existing task's handle, close the list with a colon, and author a packet to \
             the routes that still need it in new visible text. This is a private refusal \
             notice; no acknowledgment key or visible ACK/NAK is required.\n</system-notice>",
            self.header, self.line, self.column, self.reason, self.detail,
        )
    }
}

/// At most [`NOTICE_TEXT_BYTES`] of `text`, cut at a character boundary.
fn bounded(text: &str) -> String {
    if text.len() <= NOTICE_TEXT_BYTES {
        return text.to_owned();
    }
    let mut end = NOTICE_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}
