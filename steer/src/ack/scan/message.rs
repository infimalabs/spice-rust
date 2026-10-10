//! Authored `MSG ADDRESS [ADDRESS ...]: BODY` peer-publication packets.
//!
//! MSG shares the authored-context rules of acknowledgment headers: the same
//! quotation, code, rendered-source, negation, hypothetical and narration
//! exclusions decide whether the marker speaks at all. It never names a
//! steering key and never retires anything; its header names recipients by
//! [`OccupantAddress`] and its body is the source hunk it owns.
//!
//! A body begins after the header's colon and runs to the next placed ACK,
//! NAK, or MSG header or to the end of the scanned prose, with surrounding
//! whitespace trimmed. When that next header opens its line behind list or
//! dash framing, the body ends at that line's start, so a later packet's
//! framing never leaks into an earlier send. A quoted, coded, or narrated
//! marker is neither a packet nor a boundary. Text before the first header
//! belongs to no packet.

use std::ops::Range;

use wire::{OccupantAddress, OccupantAddressError};

use super::{skip_emphasis, skip_header_space};

/// The word an authored peer-publication packet leads with.
pub const MARKER: &str = "MSG";

/// One placed MSG packet: its header and what it publishes, or why it
/// publishes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Header bytes from the marker, including touching emphasis, through the
    /// colon and any emphasis closing it; through the last recipient token
    /// when the colon is missing.
    pub header: Range<usize>,
    /// The publication the packet requests, refused whole on any defect.
    pub publication: Result<Publication, MessageRefusal>,
}

/// A well-formed packet: distinct recipients and the body hunk sent to each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    /// Every named recipient in source order.
    pub recipients: Vec<Recipient>,
    /// The nonempty trimmed body hunk, protocol headers excluded.
    pub body: Range<usize>,
}

/// One recipient token and the address it spells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recipient {
    /// Address the token spells.
    pub address: OccupantAddress,
    /// Half-open byte range of the token.
    pub start: usize,
    /// End of the token.
    pub end: usize,
}

/// Why one placed MSG packet publishes nothing.
///
/// Refusal covers the whole packet: no recipient is guessed, deduplicated, or
/// dropped, and no neighboring packet lends its body. The first defect in
/// source order decides: the header's colon, then each recipient token, then
/// the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRefusal {
    /// The recipient list is not closed by a colon.
    Unterminated,
    /// A recipient token does not spell an occupant address.
    Recipient {
        /// Byte range of the token.
        span: Range<usize>,
        /// Why the token is not an address.
        error: OccupantAddressError,
    },
    /// An address is named more than once.
    Repeated {
        /// The repeated address.
        address: OccupantAddress,
        /// Byte range of its later naming.
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
            Self::Repeated { .. } => "msg::recipient-repeated",
            Self::EmptyBody => "msg::body-empty",
        }
    }
}

/// The recipient list after one marker, before its body is bounded.
pub(super) struct Header {
    /// Each recipient token as a half-open byte range.
    pub(super) tokens: Vec<(usize, usize)>,
    /// Header end: past the colon and closing emphasis, or past the last token.
    pub(super) end: usize,
    pub(super) terminated: bool,
}

/// The recipient list after one `MSG` marker, if the marker opens an attempt.
///
/// Recipient tokens are ASCII-alphanumeric runs separated by header space or a
/// comma. A colon-closed list is an attempt when any token spells an address;
/// an unclosed list is one only when its first token does. Anything else is
/// prose that happens to use the word.
pub(super) fn header_after(prose: &str, marker_end: usize) -> Option<Header> {
    let marker_boundary = skip_emphasis(prose, marker_end);
    let mut cursor = skip_header_space(prose, marker_boundary);
    if cursor == marker_boundary {
        return None;
    }
    let mut tokens = Vec::new();
    let mut last = cursor;
    loop {
        let start = skip_emphasis(prose, cursor);
        let end = start
            + prose.as_bytes()[start..]
                .iter()
                .take_while(|byte| byte.is_ascii_alphanumeric())
                .count();
        if end == start {
            break;
        }
        tokens.push((start, end));
        last = skip_emphasis(prose, end);
        cursor = skip_header_space(prose, last);
        if prose.as_bytes().get(cursor) == Some(&b',') {
            cursor = skip_header_space(prose, cursor + 1);
        }
    }
    let &(first_start, first_end) = tokens.first()?;
    let colon = skip_header_space(prose, last);
    let terminated = prose.as_bytes().get(colon) == Some(&b':');
    let addresses = |(start, end): &(usize, usize)| prose[*start..*end].parse::<OccupantAddress>();
    let attempt = if terminated {
        tokens.iter().any(|token| addresses(token).is_ok())
    } else {
        addresses(&(first_start, first_end)).is_ok()
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
    let mut recipients: Vec<Recipient> = Vec::with_capacity(header.tokens.len());
    for &(start, end) in &header.tokens {
        let address = prose[start..end]
            .parse::<OccupantAddress>()
            .map_err(|error| MessageRefusal::Recipient {
                span: start..end,
                error,
            })?;
        if recipients
            .iter()
            .any(|recipient| recipient.address == address)
        {
            return Err(MessageRefusal::Repeated {
                address,
                span: start..end,
            });
        }
        recipients.push(Recipient {
            address,
            start,
            end,
        });
    }
    let hunk = &prose[header.end..boundary.max(header.end)];
    let leading = hunk.len() - hunk.trim_start().len();
    let body = hunk.trim();
    if body.is_empty() {
        return Err(MessageRefusal::EmptyBody);
    }
    let start = header.end + leading;
    Ok(Publication {
        recipients,
        body: start..start + body.len(),
    })
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
