//! Authored `MSG SELECTOR [SELECTOR ...]: BODY` publication packets.
//!
//! MSG shares the authored-context rules of acknowledgment headers: the same
//! quotation, code, rendered-source, negation, hypothetical and narration
//! exclusions decide whether the marker speaks at all. It never names a
//! steering key and never retires anything; its body is the source hunk it
//! owns.
//!
//! Each selector is one route, `audience[#context]` or `#context`: an
//! [`OccupantAddress`] directs attention to that occupant, and a `#` qualifier
//! names the [`TaskHandle`] whose room records the packet. A selector owns its
//! qualifier; an unqualified route borrows no context from its neighbors.
//!
//! A body begins after the header's colon and runs to the next placed ACK,
//! NAK, or MSG header or to the end of the scanned prose, with surrounding
//! whitespace trimmed. When that next header opens its line behind list or
//! dash framing, the body ends at that line's start, so a later packet's
//! framing never leaks into an earlier send. A quoted, coded, or narrated
//! marker is neither a packet nor a boundary. Text before the first header
//! belongs to no packet.

use std::ops::Range;

use wire::{OccupantAddress, OccupantAddressError, TaskHandle, TaskHandleError};

use super::{skip_emphasis, skip_header_space};

/// The word an authored publication packet leads with.
pub const MARKER: &str = "MSG";

/// The byte that opens a selector's task context.
const CONTEXT: u8 = b'#';

/// One placed MSG packet: its header and what it publishes, or why it
/// publishes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Header bytes from the marker, including touching emphasis, through the
    /// colon and any emphasis closing it; through the last selector when the
    /// colon is missing.
    pub header: Range<usize>,
    /// The publication the packet requests, refused whole on any defect.
    pub publication: Result<Publication, MessageRefusal>,
}

/// A well-formed packet: distinct routes and the body hunk each publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    /// Every route in source order.
    pub routes: Vec<Route>,
    /// The nonempty trimmed body hunk, protocol headers excluded.
    pub body: Range<usize>,
}

impl Publication {
    /// Every occupant the packet directs attention to, in source order.
    pub fn addresses(&self) -> impl Iterator<Item = OccupantAddress> + '_ {
        self.routes.iter().filter_map(|route| route.address)
    }
}

/// One selector: an audience, a task context, or an audience in a context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// Occupant the route directs attention to; absent on a room-only route.
    pub address: Option<OccupantAddress>,
    /// Task whose room the route records in; absent on a direct route.
    pub context: Option<TaskHandle>,
    /// Byte range of the whole selector.
    pub span: Range<usize>,
}

/// Why one placed MSG packet publishes nothing.
///
/// Refusal covers the whole packet: no route is guessed, deduplicated, or
/// dropped, and no neighboring packet lends its body. The first defect in
/// source order decides: the header's colon, then each selector, then the
/// body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRefusal {
    /// The selector list is not closed by a colon.
    Unterminated,
    /// A selector's audience does not spell an occupant address.
    Recipient {
        /// Byte range of the audience.
        span: Range<usize>,
        /// Why the audience is not an address.
        error: OccupantAddressError,
    },
    /// A selector's `#` qualifier does not spell a task handle.
    Context {
        /// Byte range of the qualifier after its `#`.
        span: Range<usize>,
        /// Why the qualifier is not a task handle.
        error: TaskHandleError,
    },
    /// An address is named more than once.
    Repeated {
        /// The repeated address.
        address: OccupantAddress,
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
            Self::Context { .. } => "msg::context-malformed",
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
/// `#` and an ASCII-alphanumeric-or-hyphen context run; selectors are separated
/// by header space or a comma. A colon-closed list is an attempt when any
/// selector spells a valid route or carries the hyphen required by a task
/// handle, so a malformed room-only selector is refused rather than ignored.
/// An unclosed list is one only when its first selector is already valid.
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
        let (audience, context) = split(prose, start, end);
        prose[audience].parse::<OccupantAddress>().is_ok()
            || context.is_some_and(|context| prose[context].parse::<TaskHandle>().is_ok())
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
        end: if terminated {
            skip_emphasis(prose, colon + 1)
        } else {
            last
        },
        tokens,
        terminated,
    })
}

/// One selector's audience range and, when qualified, its context range.
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
    let mut routes: Vec<Route> = Vec::with_capacity(header.tokens.len());
    for &(start, end) in &header.tokens {
        let route = route(prose, start, end)?;
        if let Some(address) = route.address
            && routes.iter().any(|known| known.address == Some(address))
        {
            return Err(MessageRefusal::Repeated {
                address,
                span: split(prose, start, end).0,
            });
        }
        if route.address.is_none()
            && let Some(context) = &route.context
            && routes
                .iter()
                .any(|known| known.address.is_none() && same_task(known.context.as_ref(), context))
        {
            return Err(MessageRefusal::RepeatedContext {
                context: context.clone(),
                span: route.span,
            });
        }
        routes.push(route);
    }
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

/// Judge one selector: its audience, if written, then its qualifier.
fn route(prose: &str, start: usize, end: usize) -> Result<Route, MessageRefusal> {
    let (audience, context) = split(prose, start, end);
    let address = if audience.is_empty() {
        None
    } else {
        Some(
            prose[audience.clone()]
                .parse::<OccupantAddress>()
                .map_err(|error| MessageRefusal::Recipient {
                    span: audience,
                    error,
                })?,
        )
    };
    let context = context
        .map(|span| {
            prose[span.clone()]
                .parse::<TaskHandle>()
                .map_err(|error| MessageRefusal::Context { span, error })
        })
        .transpose()?;
    Ok(Route {
        address,
        context,
        span: start..end,
    })
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
