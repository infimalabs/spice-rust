//! Authored `MSG`, `ASK`, and `ANS` publication packets.
//!
//! ```text
//! selector = audience[#context] | #context | #KEY
//! audience = agent-code | operator
//!
//! MSG selector+: body
//! ASK audience[#context] | #KEY: body
//! ANS KEY[#context]: body
//! ```
//!
//! Every verb shares the authored-context rules of acknowledgment headers: the
//! same quotation, code, rendered-source, negation, hypothetical and narration
//! exclusions decide whether the marker speaks at all. None retires anything;
//! each body is the source hunk its packet owns.
//!
//! An audience is an [`OccupantAddress`] or the literal `operator`, which no
//! vowel-free address can spell. A `#` qualifier with a hyphen names the
//! [`TaskHandle`] whose room context the selector carries; one without names
//! the steering [`Key`] of the operator direction the packet concerns, so
//! `#KEY` and `operator#KEY` both route to the operator about that direction.
//! A selector owns its qualifier; an unqualified route borrows no context from
//! its neighbors. ASK names exactly one audience. ANS names the public result
//! handle of the question it answers; its optional qualifier asserts that
//! question's stored context and never retargets it.
//!
//! A body begins after the header's colon and runs to the next placed ACK,
//! NAK, MSG, ASK, or ANS header or to the end of the scanned prose, with
//! surrounding whitespace trimmed. When that next header opens its line behind
//! list or dash framing, the body ends at that line's start, so a later
//! packet's framing never leaks into an earlier one. A quoted, coded, or
//! narrated marker is neither a packet nor a boundary. Text before the first
//! header belongs to no packet.

use std::ops::Range;

use wire::rooms::{QuestionResultRef, RoomAuthorityStamp, RoomAuthorityStampError};
use wire::{OccupantAddress, OccupantAddressError, TaskHandle, TaskHandleError};

use super::{skip_emphasis, skip_header_space};
use crate::Key;

/// Every word an authored publication packet leads with.
pub const MARKERS: [&str; 3] = [Verb::Msg.marker(), Verb::Ask.marker(), Verb::Ans.marker()];

/// The audience spelling that names the operator.
pub const OPERATOR: &str = "operator";

/// The byte that opens a selector's task context.
const CONTEXT: u8 = b'#';

/// The publication verb a packet's marker spells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verb {
    /// An ordinary message to one or more routes.
    Msg,
    /// A question to one audience that opens a correlated result.
    Ask,
    /// The answer to one stored question.
    Ans,
}

impl Verb {
    /// The marker word as written.
    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Msg => "MSG",
            Self::Ask => "ASK",
            Self::Ans => "ANS",
        }
    }

    /// The verb whose marker begins `text`.
    pub(super) fn leading(text: &str) -> Option<Self> {
        [Self::Msg, Self::Ask, Self::Ans]
            .into_iter()
            .find(|verb| text.starts_with(verb.marker()))
    }
}

/// One placed packet: its verb, its header, and what it publishes, or why it
/// publishes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The verb its marker spells.
    pub verb: Verb,
    /// Header bytes from the marker, including touching emphasis, through the
    /// colon and any emphasis closing it; through the last selector when the
    /// colon is missing.
    pub header: Range<usize>,
    /// The publication the packet requests, refused whole on any defect.
    pub publication: Result<Publication, MessageRefusal>,
}

/// A well-formed packet: what it requests and the body hunk it publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    /// The verb-specific routing the header spelled.
    pub request: Request,
    /// The nonempty trimmed body hunk, protocol headers excluded.
    pub body: Range<usize>,
}

/// What one well-formed header requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// MSG: distinct routes in source order.
    Msg { routes: Vec<Route> },
    /// ASK: its one audience route.
    Ask { route: Route },
    /// ANS: the question it answers and the context it asserts.
    Ans {
        /// Public result handle of the answered question.
        result: QuestionResultRef,
        /// Asserted stored context, checked for equality only.
        context: Option<TaskHandle>,
        /// Byte range of the whole selector.
        span: Range<usize>,
    },
}

impl Publication {
    /// Every route in source order; ANS has none, since it inherits its
    /// question's parties.
    #[must_use]
    pub fn routes(&self) -> &[Route] {
        match &self.request {
            Request::Msg { routes } => routes,
            Request::Ask { route } => std::slice::from_ref(route),
            Request::Ans { .. } => &[],
        }
    }

    /// Every occupant the packet directs attention to, in source order.
    pub fn addresses(&self) -> impl Iterator<Item = OccupantAddress> + '_ {
        self.routes()
            .iter()
            .filter_map(|route| match route.audience {
                Some(Audience::Occupant(address)) => Some(address),
                Some(Audience::Operator) | None => None,
            })
    }
}

/// Who one selector directs attention to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Audience {
    /// One exact occupant, by its address.
    Occupant(OccupantAddress),
    /// The operator.
    Operator,
}

impl std::fmt::Display for Audience {
    /// The audience as a header spells it.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Occupant(address) => address.fmt(formatter),
            Self::Operator => formatter.write_str(OPERATOR),
        }
    }
}

/// One selector: an audience, a task context, or an audience in a context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// Audience the route directs attention to; absent on a room-only route.
    pub audience: Option<Audience>,
    /// Task whose room context the route carries; absent on a direct route.
    pub context: Option<TaskHandle>,
    /// Operator direction an operator route concerns.
    pub direction: Option<Key>,
    /// Byte range of the whole selector.
    pub span: Range<usize>,
}

/// Why one placed packet publishes nothing.
///
/// Refusal covers the whole packet: no route is guessed, deduplicated, or
/// dropped, and no neighboring packet lends its body. The first defect in
/// source order decides: the header's colon, then the selector count, then
/// each selector, then the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRefusal {
    /// The selector list is not closed by a colon.
    Unterminated,
    /// ASK or ANS names other than exactly one selector.
    Selectors {
        /// The packet's verb.
        verb: Verb,
        /// Selectors the header named.
        count: usize,
    },
    /// A selector's audience spells neither an occupant address nor the
    /// operator.
    Recipient {
        /// Byte range of the audience.
        span: Range<usize>,
        /// Why the audience is not an address.
        error: OccupantAddressError,
    },
    /// An ASK selector names no audience.
    Audience {
        /// Byte range of the selector.
        span: Range<usize>,
    },
    /// An ANS selector does not spell a result handle.
    Result {
        /// Byte range of the handle.
        span: Range<usize>,
        /// Why it is not a result handle.
        error: RoomAuthorityStampError,
    },
    /// A selector's `#` qualifier does not spell a task handle.
    Context {
        /// Byte range of the qualifier after its `#`.
        span: Range<usize>,
        /// Why the qualifier is not a task handle.
        error: TaskHandleError,
    },
    /// An occupant selector names a direction, whose audience is the operator.
    DirectionAudience {
        /// Byte range of the selector.
        span: Range<usize>,
    },
    /// An audience is named more than once.
    Repeated {
        /// The repeated audience.
        audience: Audience,
        /// Byte range of its later naming.
        span: Range<usize>,
    },
    /// A room-only route names a task another room-only route already names.
    RepeatedContext {
        /// The later spelling of the repeated task.
        context: TaskHandle,
        /// Byte range of the later selector.
        span: Range<usize>,
    },
    /// Nothing but whitespace lies between the colon and the next boundary.
    EmptyBody,
}

impl MessageRefusal {
    /// Stable refusal name.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::Unterminated => "msg::unterminated",
            Self::Selectors {
                verb: Verb::Ans, ..
            } => "ans::selector-count",
            Self::Selectors { .. } => "ask::selector-count",
            Self::Recipient { .. } => "msg::recipient-malformed",
            Self::Audience { .. } => "ask::audience-missing",
            Self::Result { .. } => "ans::result-malformed",
            Self::Context { .. } => "msg::context-malformed",
            Self::DirectionAudience { .. } => "msg::direction-audience",
            Self::Repeated { .. } => "msg::recipient-repeated",
            Self::RepeatedContext { .. } => "msg::context-repeated",
            Self::EmptyBody => "msg::body-empty",
        }
    }
}

/// The selector list after one marker, before its body is bounded.
pub(super) struct Header {
    pub(super) verb: Verb,
    /// Each selector as a half-open byte range.
    pub(super) tokens: Vec<(usize, usize)>,
    /// Header end: past the colon and closing emphasis, or past the last token.
    pub(super) end: usize,
    pub(super) terminated: bool,
}

/// The selector list after one `verb` marker, if the marker opens an attempt.
///
/// A selector is an ASCII-alphanumeric audience or result run, optionally
/// followed by `#` and an ASCII-alphanumeric-or-hyphen context run; selectors
/// are separated by header space or a comma. A colon-closed list is an attempt
/// when any selector spells a valid route or carries the hyphen required by a
/// task handle, so a malformed room-only selector is refused rather than
/// ignored. An unclosed list is one only when its first selector is already
/// valid.
pub(super) fn header_after(prose: &str, verb: Verb, marker_end: usize) -> Option<Header> {
    let marker_boundary = skip_emphasis(prose, marker_end);
    let mut cursor = skip_header_space(prose, marker_boundary);
    if cursor == marker_boundary {
        return None;
    }
    let bytes = prose.as_bytes();
    let run = |from: usize, admits: fn(&u8) -> bool| {
        from + bytes[from..].iter().take_while(|byte| admits(byte)).count()
    };
    let mut tokens = Vec::new();
    let mut last = cursor;
    loop {
        let start = skip_emphasis(prose, cursor);
        let mut end = run(start, u8::is_ascii_alphanumeric);
        if bytes.get(end) == Some(&CONTEXT) {
            end = run(end + 1, |byte| {
                byte.is_ascii_alphanumeric() || *byte == b'-'
            });
        }
        if end == start {
            break;
        }
        tokens.push((start, end));
        last = skip_emphasis(prose, end);
        cursor = skip_header_space(prose, last);
        if bytes.get(cursor) == Some(&b',') {
            cursor = skip_header_space(prose, cursor + 1);
        }
    }
    let first = *tokens.first()?;
    let colon = skip_header_space(prose, last);
    let terminated = bytes.get(colon) == Some(&b':');
    let spells = |&(start, end): &(usize, usize)| {
        let (lead, context) = split(prose, start, end);
        let lead = &prose[lead];
        let lead_spells = match verb {
            Verb::Ans => lead.parse::<RoomAuthorityStamp>().is_ok(),
            Verb::Msg | Verb::Ask => lead == OPERATOR || lead.parse::<OccupantAddress>().is_ok(),
        };
        lead_spells
            || context.is_some_and(|context| {
                let context = &prose[context];
                context.parse::<TaskHandle>().is_ok() || (verb != Verb::Ans && Key::shaped(context))
            })
    };
    let room_like = |&(start, end): &(usize, usize)| {
        let (_, context) = split(prose, start, end);
        context.is_some_and(|context| prose.as_bytes()[context].contains(&b'-'))
    };
    let attempt = if terminated {
        tokens.iter().any(|token| spells(token) || room_like(token))
    } else {
        spells(&first)
    };
    attempt.then(|| Header {
        verb,
        end: if terminated {
            skip_emphasis(prose, colon + 1)
        } else {
            last
        },
        tokens,
        terminated,
    })
}

/// One selector's lead range and, when qualified, its context range.
fn split(prose: &str, start: usize, end: usize) -> (Range<usize>, Option<Range<usize>>) {
    match prose[start..end].find(char::from(CONTEXT)) {
        Some(at) => (start..start + at, Some(start + at + 1..end)),
        None => (start..end, None),
    }
}

/// Judge one placed header against the boundary its body runs to.
pub(super) fn judge(prose: &str, start: usize, header: Header, boundary: usize) -> Message {
    Message {
        verb: header.verb,
        header: start..header.end,
        publication: publication(prose, &header, boundary),
    }
}

fn publication(
    prose: &str,
    header: &Header,
    boundary: usize,
) -> Result<Publication, MessageRefusal> {
    if !header.terminated {
        return Err(MessageRefusal::Unterminated);
    }
    let request = match header.verb {
        Verb::Msg => Request::Msg {
            routes: routes(prose, &header.tokens)?,
        },
        Verb::Ask => {
            let &[(start, end)] = header.tokens.as_slice() else {
                return Err(selectors(header));
            };
            let route = route(prose, start, end)?;
            if route.audience.is_none() {
                return Err(MessageRefusal::Audience { span: start..end });
            }
            Request::Ask { route }
        }
        Verb::Ans => {
            let &[(start, end)] = header.tokens.as_slice() else {
                return Err(selectors(header));
            };
            let (lead, context) = split(prose, start, end);
            let result = prose[lead.clone()]
                .parse::<RoomAuthorityStamp>()
                .map_err(|error| MessageRefusal::Result { span: lead, error })?;
            Request::Ans {
                result: QuestionResultRef::new(result),
                context: context.map(|span| task_context(prose, span)).transpose()?,
                span: start..end,
            }
        }
    };
    let hunk = &prose[header.end..boundary.max(header.end)];
    let leading = hunk.len() - hunk.trim_start().len();
    let body = hunk.trim();
    if body.is_empty() {
        return Err(MessageRefusal::EmptyBody);
    }
    let start = header.end + leading;
    Ok(Publication {
        request,
        body: start..start + body.len(),
    })
}

fn selectors(header: &Header) -> MessageRefusal {
    MessageRefusal::Selectors {
        verb: header.verb,
        count: header.tokens.len(),
    }
}

/// Judge every MSG selector in source order, refusing the first defect.
fn routes(prose: &str, tokens: &[(usize, usize)]) -> Result<Vec<Route>, MessageRefusal> {
    let mut routes: Vec<Route> = Vec::with_capacity(tokens.len());
    for &(start, end) in tokens {
        let route = route(prose, start, end)?;
        if let Some(audience) = route.audience
            && routes.iter().any(|known| known.audience == Some(audience))
        {
            return Err(MessageRefusal::Repeated {
                audience,
                span: split(prose, start, end).0,
            });
        }
        if route.audience.is_none()
            && let Some(context) = &route.context
            && routes
                .iter()
                .any(|known| known.audience.is_none() && same_task(known.context.as_ref(), context))
        {
            return Err(MessageRefusal::RepeatedContext {
                context: context.clone(),
                span: route.span,
            });
        }
        routes.push(route);
    }
    Ok(routes)
}

/// Judge one selector: its audience, if written, then its qualifier.
fn route(prose: &str, start: usize, end: usize) -> Result<Route, MessageRefusal> {
    let (audience, context) = split(prose, start, end);
    let mut audience = match &prose[audience.clone()] {
        "" => None,
        OPERATOR => Some(Audience::Operator),
        written => Some(Audience::Occupant(written.parse().map_err(|error| {
            MessageRefusal::Recipient {
                span: audience,
                error,
            }
        })?)),
    };
    let direction = context
        .as_ref()
        .filter(|span| !prose[(*span).clone()].contains('-'))
        .and_then(|span| prose[span.clone()].parse::<Key>().ok());
    if direction.is_some() {
        if matches!(audience, Some(Audience::Occupant(_))) {
            return Err(MessageRefusal::DirectionAudience { span: start..end });
        }
        audience = Some(Audience::Operator);
    }
    Ok(Route {
        audience,
        context: match direction {
            Some(_) => None,
            None => context.map(|span| task_context(prose, span)).transpose()?,
        },
        direction,
        span: start..end,
    })
}

fn task_context(prose: &str, span: Range<usize>) -> Result<TaskHandle, MessageRefusal> {
    prose[span.clone()]
        .parse::<TaskHandle>()
        .map_err(|error| MessageRefusal::Context { span, error })
}

/// Whether two spellings name one task: a renamed handle keeps its birth stamp.
fn same_task(known: Option<&TaskHandle>, context: &TaskHandle) -> bool {
    known.is_some_and(|known| known.born().ok() == context.born().ok())
}

/// Where a body preceding the header at `start` ends.
///
/// List bullets, dashes, brackets, and item numbers that alone precede a header
/// on its line frame that header, so the body ends at the line start.
pub(super) fn boundary_before(prose: &str, floor: usize, start: usize) -> usize {
    let line_start = prose[..start]
        .rfind('\n')
        .map_or(0, |newline| newline + 1)
        .max(floor);
    let framed = prose[line_start..start].chars().all(|character| {
        character.is_ascii_digit()
            || matches!(
                character,
                ' ' | '\t' | '-' | '+' | '*' | '[' | '.' | ')' | '\u{2013}' | '\u{2014}'
            )
    });
    if framed { line_start } else { start }
}
