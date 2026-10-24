//! Authored `MSG` publication packets.
//!
//! ```text
//! selector = occupant[#task] | #task | #KEY
//! occupant = agent-code
//!
//! MSG selector+: body
//! ```
//!
//! The packet shares the authored-context rules of acknowledgment headers: the
//! same quotation, code, rendered-source, negation, hypothetical and narration
//! exclusions decide whether the marker speaks at all. It retires nothing; its
//! body is the source hunk the packet owns.
//!
//! An explicit audience is one [`OccupantAddress`]. A pointer-only selector
//! reaches the operator through the concern it names: a `#` qualifier with a
//! hyphen names the [`TaskHandle`] whose room receives the packet, while one
//! without names the steering [`Key`] of an operator direction to this lane.
//! The context-free legacy spelling `operator` is recognized only so it can
//! refuse visibly with pointer guidance. A selector owns its qualifier; an
//! unqualified route borrows no context from its neighbors.
//!
//! A body begins after the header's colon and runs through the rest of that
//! paragraph, ending earlier at the next placed ACK, NAK, or MSG header, with
//! surrounding whitespace trimmed. A blank line always returns following prose
//! to the author. A header in a Markdown list item or fenced block is quoted
//! prose: it opens no packet and bounds no body. Text before the first header
//! belongs to no packet.

use std::ops::Range;

use wire::{OccupantAddress, OccupantAddressError, TaskHandle, TaskHandleError};

use super::{skip_emphasis, skip_header_space};
use crate::Key;

/// The word an authored publication packet leads with.
pub const MARKER: &str = "MSG";

/// Context-free legacy spelling recognized only to produce a visible refusal.
const UNSCOPED_OPERATOR: &str = "operator";

/// The byte that opens a selector's task context.
const CONTEXT: u8 = b'#';

/// One placed packet: its header and what it publishes, or why it publishes
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Header bytes from the marker, including touching emphasis, through the
    /// colon and any emphasis closing it; through the last selector when the
    /// colon is missing.
    pub header: Range<usize>,
    /// The publication the packet requests, refused whole on any defect.
    pub publication: Result<Publication, MessageRefusal>,
}

/// A well-formed packet: its routes and the body hunk it publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    /// Distinct routes in source order.
    pub routes: Vec<Route>,
    /// The nonempty trimmed body hunk, protocol headers excluded.
    pub body: Range<usize>,
}

impl Publication {
    /// Every explicitly addressed occupant, in source order.
    pub fn addresses(&self) -> impl Iterator<Item = OccupantAddress> + '_ {
        self.routes.iter().filter_map(|route| match route.audience {
            Some(Audience::Occupant(address)) => Some(address),
            Some(Audience::Operator) | None => None,
        })
    }
}

/// Where one selector directs attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Audience {
    /// One exact occupant, by its address.
    Occupant(OccupantAddress),
    /// The operator, reached only through the operator direction this route's
    /// `#KEY` names, so everything it hears traces back to what it said.
    Operator,
}

impl std::fmt::Display for Audience {
    /// Human-readable destination used by duplicate-route refusals.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Occupant(address) => address.fmt(formatter),
            Self::Operator => formatter.write_str("the operator"),
        }
    }
}

/// One selector: an occupant, an operator direction, a room record, or an
/// occupant in a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// Explicit occupant, or the operator a `#KEY` points at; absent on a
    /// room-only `#TASK` record, which reaches nobody.
    pub audience: Option<Audience>,
    /// Task whose room context the route carries; absent on a direct route.
    pub context: Option<TaskHandle>,
    /// Operator direction an operator route names.
    pub direction: Option<Key>,
    /// Byte range of the whole selector.
    pub span: Range<usize>,
}

/// Why one placed packet publishes nothing.
///
/// Refusal covers the whole packet: no route is guessed, deduplicated, or
/// dropped, and no neighboring packet lends its body. The first defect in
/// source order decides: the header's colon, then each selector, then the
/// body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRefusal {
    /// The selector list is not closed by a colon.
    Unterminated,
    /// A selector's explicit recipient is not an occupant address.
    Recipient {
        /// Byte range of the recipient.
        span: Range<usize>,
        /// Why the recipient is not an address.
        error: OccupantAddressError,
    },
    /// A legacy context-free operator spelling names no reachable concern.
    Concern {
        /// Byte range of the spelling.
        span: Range<usize>,
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
    /// A destination is named more than once.
    Repeated {
        /// The repeated destination.
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
            Self::Recipient { .. } => "msg::recipient-malformed",
            Self::Concern { .. } => "msg::concern-missing",
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
    /// Each selector as a half-open byte range.
    pub(super) tokens: Vec<(usize, usize)>,
    /// Header end: past the colon and closing emphasis, or past the last token.
    pub(super) end: usize,
    pub(super) terminated: bool,
}

/// The selector list after one `MSG` marker, if the marker opens an attempt.
///
/// A selector is an ASCII-alphanumeric audience run, optionally followed by
/// `#` and an ASCII-alphanumeric-or-hyphen context run; selectors are
/// separated by header space or a comma. A colon-closed list whose first
/// selector is route-shaped is an attempt anywhere authored prose can place a
/// packet. At a line packet boundary, a later route-shaped selector also makes
/// the list an attempt so an explicitly malformed first selector is refused
/// rather than ignored. This boundary keeps ordinary mid-clause narration from
/// borrowing a later address-shaped word before its colon. An unclosed list is
/// one only when its first selector is already valid.
pub(super) fn header_after(prose: &str, marker_end: usize) -> Option<Header> {
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
        lead == UNSCOPED_OPERATOR
            || lead.parse::<OccupantAddress>().is_ok()
            || context.is_some_and(|context| {
                let context = &prose[context];
                context.parse::<TaskHandle>().is_ok() || Key::shaped(context)
            })
    };
    let room_like = |&(start, end): &(usize, usize)| {
        let (_, context) = split(prose, start, end);
        context.is_some_and(|context| prose.as_bytes()[context].contains(&b'-'))
    };
    let first_spells = spells(&first) || room_like(&first);
    let attempt = if terminated {
        (first_spells || marker_starts_line(prose, marker_end - MARKER.len()))
            && tokens.iter().any(|token| spells(token) || room_like(token))
    } else {
        spells(&first)
    };
    attempt.then(|| Header {
        end: if terminated {
            skip_emphasis(prose, colon + 1)
        } else {
            last
        },
        tokens,
        terminated,
    })
}

/// Whether a marker begins an explicit line packet, ignoring Markdown framing:
/// indentation, bullet or emphasis characters, and one ordered-list number
/// closed by `.` or `)`.
fn marker_starts_line(prose: &str, marker_start: usize) -> bool {
    let line_start = prose[..marker_start]
        .rfind(['\r', '\n'])
        .map_or(0, |index| index + 1);
    let framing = |text: &str| {
        text.bytes()
            .all(|byte| matches!(byte, b' ' | b'\t' | b'-' | b'+' | b'*' | b'_'))
    };
    let prefix = &prose[line_start..marker_start];
    let lead = prefix.trim_start_matches([' ', '\t']);
    let rest = lead.trim_start_matches(|character: char| character.is_ascii_digit());
    framing(prefix)
        || (rest.len() < lead.len() && rest.strip_prefix(['.', ')']).is_some_and(framing))
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
    let routes = routes(prose, &header.tokens)?;
    let hunk = &prose[header.end..boundary.max(header.end)];
    let leading = hunk.len() - hunk.trim_start().len();
    let body = hunk.trim();
    if body.is_empty() {
        return Err(MessageRefusal::EmptyBody);
    }
    let start = header.end + leading;
    Ok(Publication {
        routes,
        body: start..start + body.len(),
    })
}

/// Judge every MSG selector in source order, refusing the first defect.
fn routes(prose: &str, tokens: &[(usize, usize)]) -> Result<Vec<Route>, MessageRefusal> {
    let mut routes: Vec<Route> = Vec::with_capacity(tokens.len());
    for &(start, end) in tokens {
        let route = route(prose, start, end)?;
        if let Some(audience) = route.audience
            && routes.iter().any(|known| known.audience == Some(audience))
        {
            let span = match audience {
                Audience::Occupant(_) => split(prose, start, end).0,
                Audience::Operator => route.span.clone(),
            };
            return Err(MessageRefusal::Repeated { audience, span });
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

/// Judge one selector: its occupant, if written, then its concern or context.
fn route(prose: &str, start: usize, end: usize) -> Result<Route, MessageRefusal> {
    let (recipient, qualifier) = split(prose, start, end);
    let recipient = match &prose[recipient.clone()] {
        "" => None,
        UNSCOPED_OPERATOR => {
            return Err(MessageRefusal::Concern { span: recipient });
        }
        written => Some(Audience::Occupant(written.parse().map_err(|error| {
            MessageRefusal::Recipient {
                span: recipient,
                error,
            }
        })?)),
    };
    let direction = qualifier
        .as_ref()
        .filter(|span| !prose[(*span).clone()].contains('-'))
        .and_then(|span| prose[span.clone()].parse::<Key>().ok());
    if direction.is_some() && recipient.is_some() {
        return Err(MessageRefusal::DirectionAudience { span: start..end });
    }
    let context = match direction {
        Some(_) => None,
        None => qualifier
            .map(|span| task_context(prose, span))
            .transpose()?,
    };
    Ok(Route {
        audience: recipient.or(direction.is_some().then_some(Audience::Operator)),
        context,
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
/// Presentation framing that alone precedes a placed header on its line belongs
/// to that header, so the earlier body ends at the line start.
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
