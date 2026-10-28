//! What a packet's author and the resident diagnostic hear about one
//! publication or refusal.

use super::*;

pub(super) fn refusal_detail(text: &str, refusal: &steer::ack::scan::MessageRefusal) -> String {
    use steer::ack::scan::MessageRefusal;
    match refusal {
        MessageRefusal::Unterminated => "the selector list is not closed by a colon".to_owned(),
        MessageRefusal::Recipient { span, error } => {
            format!(
                "`{}` is not an occupant address: {error}",
                &text[span.clone()],
            )
        }
        MessageRefusal::Concern { span } => format!(
            "`{}` traces to nothing the operator said; reach the operator through `#KEY` of \
             a direction it sent you, or write `#TASK` to record the task's room without \
             reaching anyone",
            &text[span.clone()]
        ),
        MessageRefusal::Context { span, .. } => {
            format!("`#{}` is not a task handle", &text[span.clone()])
        }
        MessageRefusal::DirectionAudience { span } => format!(
            "`{}` combines an occupant with an operator-direction pointer; use `#KEY` alone",
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
            redirected,
            unreached,
            rooms,
            operator,
            held_room,
        }) => {
            for lane in lanes {
                notify_inbox(paths, *lane);
            }
            let beyond =
                !rooms.is_empty() || *operator || !queued.is_empty() || !redirected.is_empty();
            if beyond || !lanes.is_empty() {
                teller.published(
                    &Recorded {
                        rooms,
                        operator: *operator,
                        queued,
                        redirected,
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
        Err(Unpublished::AuthorDirected { reason, detail }) => {
            teller.wake(reason, detail, diagnostic);
        }
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
    operator: bool,
    queued: &'a [OccupantAddress],
    redirected: &'a [String],
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

    /// Record the refusal and wake the author through the durable directive.
    fn wake(&self, reason: &str, detail: &str, diagnostic: &mut dyn io::Write) {
        self.record_refusal(reason, detail, 0, diagnostic);
        notify_inbox(self.paths, self.record.lane);
    }

    /// Record the refusal and, under [`Refusals::Notify`], tell the author
    /// privately, with how many recipients the packet did reach.
    fn tell(&self, reason: &str, detail: &str, reached: usize, diagnostic: &mut dyn io::Write) {
        self.record_refusal(reason, detail, reached, diagnostic);
        if self.refusals == Refusals::Notify {
            crate::resident::supervisor::command::message_refused(
                self.paths,
                self.record.lane,
                MessageRefusalNotice {
                    occupant: self.record.author,
                    packet: self.packet(),
                    header: bounded(self.header),
                    line: self.locus.line,
                    column: self.locus.column,
                    reached,
                    reason: reason.to_owned(),
                    detail: bounded(detail),
                },
            );
        }
    }

    fn record_refusal(
        &self,
        reason: &str,
        detail: &str,
        reached: usize,
        diagnostic: &mut dyn io::Write,
    ) {
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
    }

    /// Tell the author privately, under [`Refusals::Notify`], how many
    /// recipients the packet reached and what it published beyond those
    /// deliveries, so a packet that sent always earns a receipt; no participant
    /// acknowledgment is implied. Only what goes beyond plain delivery reaches
    /// the diagnostic.
    fn published(&self, outcome: &Recorded<'_>, diagnostic: &mut dyn io::Write) {
        let (record, locus) = (self.record, self.locus);
        let Recorded {
            rooms,
            operator,
            queued,
            redirected,
            reached,
            held_room,
        } = *outcome;
        let rooms = rooms.iter().map(ToString::to_string).collect::<Vec<_>>();
        let queued = queued.iter().map(ToString::to_string).collect::<Vec<_>>();
        let mut outcome = String::new();
        if operator {
            outcome.push_str(" with an operator item");
        }
        if !queued.is_empty() {
            let _ = write!(outcome, " queued for [{}]", queued.join(", "));
        }
        if !redirected.is_empty() {
            let _ = write!(outcome, " redirected [{}]", redirected.join("; "));
        }
        if !rooms.is_empty() || !outcome.is_empty() {
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
        }
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
                    queued,
                    redirected: redirected.to_vec(),
                    reached,
                    held_room,
                },
            );
        }
    }
}

/// One private receipt for a packet that sent: how many recipients it reached
/// and what it published beyond those deliveries, such as the rooms it
/// recorded in and the operator item it opened, scoped to the packet's
/// authoring occupant.
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
    /// Each route that named an earlier occupant and reached its successor:
    /// the named address, the address that heard it, and the interval between
    /// their births.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(in crate::resident) redirected: Vec<String>,
    /// Whether the packet opened one pending operator item.
    pub(in crate::resident) operator: bool,
    /// How many named recipients the packet reached.
    pub(in crate::resident) reached: usize,
    /// Some `#TASK` the packet named is the task its author's lane holds.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(in crate::resident) held_room: bool,
}

impl MessageRecordedNotice {
    /// The notice its author hears.
    pub(in crate::resident) fn text(&self) -> String {
        let mut clauses = Vec::new();
        if !self.redirected.is_empty() {
            clauses.push(format!(
                "reached a later occupant of a lane you addressed by an earlier code: {}; its \
                 delivery header tells the receiver the same",
                self.redirected.join("; ")
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
                "opened one pending operator item, which the operator reads and settles in \
                 their inbox"
                    .to_owned(),
            );
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
            " It pointed at your own held task. Ordinary output needs no packet header. \
             `MSG ADDRESS:` delivers to a peer, while `#TASK` only records that room. To \
             raise a concern with the principal who assigned the work, use \
             `MSG #DIRECTION: BODY`, replacing `DIRECTION` with that direction's exact key."
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
             block {outcome}: {}: {}. {missed}. Name exact addresses of other seated \
             occupants (`spice lane list` shows each lane's), or point at one concern the \
             assigning principal already sees: `#DIRECTION` with the exact key of an operator \
             direction sent to this lane. `#TASK` only records the task or room and reaches \
             nobody by itself. Qualify an occupant with `#TASK` only for an existing \
             task's handle, close the list with a colon, and author a packet to the routes \
             that still need it in new visible text. This is a private refusal notice; no \
             acknowledgment key or visible ACK/NAK is required.\n</system-notice>",
            self.header, self.line, self.column, self.reason, self.detail,
        )
    }
}

/// At most [`NOTICE_TEXT_BYTES`] of `text`, cut at a character boundary.
pub(super) fn bounded(text: &str) -> String {
    if text.len() <= NOTICE_TEXT_BYTES {
        return text.to_owned();
    }
    let mut end = NOTICE_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}
