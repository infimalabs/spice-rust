//! Recognizing explicit acknowledgment outcomes in assistant prose.
//!
//! Acknowledgment is a narrow protocol at the assistant-output boundary, not an
//! interpretation of what a turn seems to have meant. A clause retires a
//! directive only when it begins a placed clause, spells a marker, names a
//! key-shaped token, and that key belongs to a directive still live. Everything else —
//! unknown, malformed, quoted, or historical — alters nothing and is reported
//! by name, because a silent non-recognition and a refusal look identical to an
//! operator waiting on an answer.
//!
//! The scanner performs no I/O. Standing arrives from the caller's reduced
//! facts, so the same prose read against the same standing always recognizes
//! the same outcomes, whether it runs at the door or during replay.

use std::fmt;
use std::ops::ControlFlow;

use wire::Disposition;

use crate::ack::keys::{Key, MARKERS, Marker};

/// The characters that quote a clause instead of speaking it.
///
/// Emphasis is not among them because it decorates a protocol header without
/// quoting it.
const QUOTING: [char; 4] = ['`', '"', '\'', '>'];

/// Where in the scanned prose a recognition sits.
///
/// Both parts are one-based, and the column is a byte offset within its line.
/// The pair is what the disposition fact carries as its transcript locus, so a
/// retirement can always be pointed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Locus {
    /// Line the recognition sits on.
    pub line: usize,
    /// Byte offset the recognized token begins at.
    pub column: usize,
}

impl fmt::Display for Locus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.line, self.column)
    }
}

/// Why a clause named a key without retiring anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Miss {
    /// The clause did not lead its line, so it is prose about acknowledgment
    /// rather than an acknowledgment.
    Buried,
    /// The clause sat inside quoting or a fenced block, where a key is shown
    /// rather than spoken.
    Quoted,
    /// The token is not spelled as a key.
    Malformed,
    /// No directive carries the key.
    Unknown,
    /// The directive carrying the key already retired.
    Historical,
}

impl Miss {
    /// The word this miss is reported under.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Buried => "buried",
            Self::Quoted => "quoted",
            Self::Malformed => "malformed",
            Self::Unknown => "unknown",
            Self::Historical => "historical",
        }
    }
}

impl fmt::Display for Miss {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What one token a clause named actually did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The key retired its directive into this disposition.
    Retired {
        /// Key the clause named.
        key: Key,
        /// Disposition the marker retires into.
        disposition: Disposition,
    },
    /// The token altered nothing, for this reason.
    Missed {
        /// Token exactly as it was written.
        token: String,
        /// Why it altered nothing.
        miss: Miss,
    },
}

/// One token a clause named, and where it was named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recognition {
    /// Where the token sits in the scanned prose.
    pub locus: Locus,
    /// What the token did.
    pub outcome: Outcome,
}

/// What the scanner must ask before it can retire anything.
///
/// Keeping this a question rather than a lookup is what holds the scanner to
/// zero I/O: the caller has already reduced the facts, and the scanner only
/// consults the result.
pub trait Standing {
    /// The disposition the directive carrying this key sits in, or `None` when
    /// no directive carries it.
    fn disposition(&self, key: &Key) -> Option<Disposition>;
}

/// One structured acknowledgment packet in assistant prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acknowledgement {
    /// Zero-based byte offset where the marker begins in the complete prose.
    pub start: usize,
    /// Zero-based byte offset immediately after the complete header.
    pub end: usize,
    /// Optional punctuation separating the key list from response prose.
    pub separator: Option<char>,
    /// Outcomes for every token named by this one header.
    pub recognitions: Vec<Recognition>,
}

/// Reads every acknowledgment outcome in one assistant turn.
pub fn scan(prose: &str, standing: &impl Standing) -> Vec<Recognition> {
    scan_acknowledgements(prose, standing)
        .into_iter()
        .flat_map(|acknowledgement| acknowledgement.recognitions)
        .collect()
}

/// Reads acknowledgment packets with their source header boundaries.
///
/// A packet may lead a line or follow sentence punctuation. The latter is the
/// compact form agents use when several independently keyed responses share
/// one physical line. Markers buried in ordinary narration remain misses.
pub fn scan_acknowledgements(prose: &str, standing: &impl Standing) -> Vec<Acknowledgement> {
    let mut acknowledgements = Vec::new();
    let _ = visit_acknowledgement_clauses(prose, |line, line_start, clause| {
        let mut recognitions = Vec::new();
        clause.judge(line, standing, &mut recognitions);
        acknowledgements.push(Acknowledgement {
            start: line_start + clause.start,
            end: line_start + clause.end,
            separator: clause.separator,
            recognitions,
        });
        ControlFlow::Continue(())
    });
    acknowledgements
}

/// Whether prose contains a placed acknowledgement attempt.
///
/// This is the standing-independent preflight for callers that need to load
/// standing before they can run [`scan`]. It deliberately shares the canonical
/// clause parser, including horizontal whitespace, quoting, and fence rules.
#[must_use]
pub fn has_acknowledgement(prose: &str) -> bool {
    visit_acknowledgement_clauses(prose, |_, _, clause| {
        if clause.position == Position::Placed {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .is_break()
}

fn visit_acknowledgement_clauses(
    prose: &str,
    mut visit: impl FnMut(usize, usize, Clause) -> ControlFlow<()>,
) -> ControlFlow<()> {
    let mut fence = None;
    let mut line_start = 0;
    for (index, terminated) in prose.split_inclusive('\n').enumerate() {
        let line = terminated.strip_suffix('\n').unwrap_or(terminated);
        if fence.is_some_and(|open: Fence| open.closes(line)) {
            fence = None;
            line_start += terminated.len();
            continue;
        }
        if fence.is_none()
            && let Some(open) = Fence::opens(line)
        {
            fence = Some(open);
            line_start += terminated.len();
            continue;
        }
        for clause in Clause::read_all(line, fence.is_some()) {
            visit(index + 1, line_start, clause)?;
        }
        line_start += terminated.len();
    }
    ControlFlow::Continue(())
}

/// One Markdown fenced block, closed only by its own character and width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fence {
    marker: u8,
    length: usize,
}

impl Fence {
    fn opens(line: &str) -> Option<Self> {
        let (marker, length, _) = fence_run(line)?;
        Some(Self { marker, length })
    }

    fn closes(self, line: &str) -> bool {
        fence_run(line).is_some_and(|(marker, length, rest)| {
            marker == self.marker
                && length >= self.length
                && line.get(rest..).is_some_and(|tail| tail.trim().is_empty())
        })
    }
}

/// A backtick or tilde run of at least three characters after at most three spaces.
fn fence_run(line: &str) -> Option<(u8, usize, usize)> {
    let bytes = line.as_bytes();
    let indentation = bytes.iter().take_while(|byte| **byte == b' ').count();
    if indentation > 3 {
        return None;
    }
    let marker = *bytes.get(indentation)?;
    if !matches!(marker, b'`' | b'~') {
        return None;
    }
    let length = bytes[indentation..]
        .iter()
        .take_while(|byte| **byte == marker)
        .count();
    (length >= 3).then_some((marker, length, indentation + length))
}

/// Where a clause sat, which decides whether it speaks at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Position {
    /// Leading its line, so it speaks.
    Placed,
    /// Inside quoting or a fenced block.
    Quoted,
    /// Behind other prose on its line.
    Buried,
}

/// One well-formed `MARKER key[ key...][separator]` clause found on a line.
struct Clause {
    marker: Marker,
    position: Position,
    start: usize,
    end: usize,
    separator: Option<char>,
    /// Each named token with the byte offset it begins at.
    tokens: Vec<(usize, String)>,
}

impl Clause {
    /// Reads every clause on a line that is an acknowledgment attempt.
    ///
    /// A clause is an attempt when it leads its line or names something
    /// key-shaped. Leading is how an acknowledgment is written; a key-shaped
    /// token is how a misplaced or quoted one still gets diagnosed. Prose that
    /// merely uses the word does neither, so it stays silent instead of
    /// reporting every colon-terminated sentence as a missed acknowledgment.
    fn read_all(line: &str, fenced: bool) -> Vec<Self> {
        let mut clauses = Vec::new();
        for start in 0..line.len() {
            let Some(marker) = marker_at(line, start) else {
                continue;
            };
            let decorated_start = emphasis_start(line, start);
            let Some(header) = tokens_after(line, start + marker.as_str().len()) else {
                continue;
            };
            let position = position(line, decorated_start, fenced);
            let keylike = header.tokens.iter().any(|(_, token)| keylike(token));
            if position == Position::Placed || keylike {
                clauses.push(Self {
                    marker,
                    position,
                    start: decorated_start,
                    end: header.end,
                    separator: header.separator,
                    tokens: header.tokens,
                });
            }
        }
        clauses
    }

    /// Judges every token this clause named.
    fn judge(&self, line: usize, standing: &impl Standing, into: &mut Vec<Recognition>) {
        for (offset, token) in &self.tokens {
            let locus = Locus {
                line,
                column: offset + 1,
            };
            into.push(Recognition {
                locus,
                outcome: self.outcome(token, standing),
            });
        }
    }

    /// What one token did, given where its clause sat.
    fn outcome(&self, token: &str, standing: &impl Standing) -> Outcome {
        let missed = |miss| Outcome::Missed {
            token: token.to_owned(),
            miss,
        };
        match self.position {
            Position::Quoted => return missed(Miss::Quoted),
            Position::Buried => return missed(Miss::Buried),
            Position::Placed => {}
        }
        let Ok(key) = token.parse::<Key>() else {
            return missed(Miss::Malformed);
        };
        match standing.disposition(&key) {
            None => missed(Miss::Unknown),
            Some(Disposition::Live) => Outcome::Retired {
                key,
                disposition: self.marker.disposition(),
            },
            Some(_) => missed(Miss::Historical),
        }
    }
}

/// The marker beginning at this offset, if a whole word begins there.
fn marker_at(line: &str, start: usize) -> Option<Marker> {
    let leading = line.get(..start)?;
    if leading
        .bytes()
        .next_back()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
    {
        return None;
    }
    let rest = line.get(start..)?;
    MARKERS
        .into_iter()
        .find(|marker| rest.starts_with(marker.as_str()))
}

struct Header {
    tokens: Vec<(usize, String)>,
    end: usize,
    separator: Option<char>,
}

/// The leading key run and optional separator after one marker.
fn tokens_after(line: &str, marker_end: usize) -> Option<Header> {
    let marker_boundary = skip_emphasis(line, marker_end);
    let mut cursor = skip_horizontal(line, marker_boundary);
    if cursor == marker_boundary {
        return None;
    }

    let mut tokens = Vec::new();
    let mut header_end = cursor;
    loop {
        let token_start = skip_emphasis(line, cursor);
        let Some(end) = key_token_end(line, token_start, !tokens.is_empty()) else {
            break;
        };
        tokens.push((token_start, line[token_start..end].to_owned()));
        header_end = skip_emphasis(line, end);
        cursor = skip_horizontal(line, header_end);
    }
    if tokens.is_empty() {
        return None;
    }

    let mut probe = skip_emphasis(line, header_end);
    probe = skip_horizontal(line, probe);
    let separator = line
        .get(probe..)?
        .chars()
        .next()
        .filter(|character| matches!(character, ':' | '.' | ',' | ';' | '!' | '?'));
    if let Some(separator) = separator {
        probe += separator.len_utf8();
        probe = skip_emphasis(line, probe);
        header_end = probe;
    } else if probe != cursor {
        header_end = probe;
    }

    Some(Header {
        tokens,
        end: header_end,
        separator,
    })
}

fn key_token_end(line: &str, start: usize, current_shape: bool) -> Option<usize> {
    for width in [Key::WIDTH, Key::LEGACY_WIDTH] {
        let end = start.checked_add(width)?;
        let Some(token) = line.get(start..end) else {
            continue;
        };
        let accepted = if current_shape {
            Key::shaped(token)
                || (token.len() == Key::LEGACY_WIDTH
                    && Key::admitted(token)
                    && token
                        .bytes()
                        .any(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase()))
        } else {
            Key::admitted(token)
        };
        if accepted
            && !line
                .as_bytes()
                .get(end)
                .is_some_and(u8::is_ascii_alphanumeric)
        {
            return Some(end);
        }
    }
    None
}

fn skip_horizontal(line: &str, mut cursor: usize) -> usize {
    while line
        .as_bytes()
        .get(cursor)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        cursor += 1;
    }
    cursor
}

/// Markdown emphasis touching the front of a protocol header.
fn emphasis_start(line: &str, marker_start: usize) -> usize {
    let mut start = marker_start;
    while line
        .as_bytes()
        .get(start.wrapping_sub(1))
        .is_some_and(|byte| matches!(byte, b'*' | b'_'))
    {
        start -= 1;
    }
    start
}

fn skip_emphasis(line: &str, mut cursor: usize) -> usize {
    while line
        .as_bytes()
        .get(cursor)
        .is_some_and(|byte| matches!(byte, b'*' | b'_'))
    {
        cursor += 1;
    }
    cursor
}

/// Whether a token is worth diagnosing as an intended key.
///
/// This is deliberately narrower than the grammar in [`Key`]. It decides only
/// whether a misplaced clause is an acknowledgment attempt worth reporting, and
/// an eight-letter English word is far more often prose than a key: `protocol`
/// spells a key perfectly well by grammar. Requiring a digit or a capital keeps
/// ordinary sentences from each producing a diagnostic.
///
/// Nothing is lost by the narrowing. A clause that leads its line is an attempt
/// on position alone, which is where acknowledgments are actually written, so
/// this never decides whether a key retires — only whether a key that did not
/// retire gets named.
fn keylike(token: &str) -> bool {
    Key::shaped(token)
        && token
            .bytes()
            .any(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
}

/// Where a clause beginning at this offset sits on its line.
fn position(line: &str, start: usize, fenced: bool) -> Position {
    if fenced {
        return Position::Quoted;
    }
    let leading = line.get(..start).unwrap_or(line);
    let line_leading = line.trim_start();
    let indentation = line
        .get(..line.len() - line_leading.len())
        .unwrap_or_default();
    if line_leading.starts_with(QUOTING) || indentation_width(indentation) >= 4 {
        return Position::Quoted;
    }
    let trimmed = leading.trim();
    if trimmed.is_empty() {
        Position::Placed
    } else if trimmed
        .chars()
        .all(|character| QUOTING.contains(&character))
    {
        Position::Quoted
    } else if trimmed.ends_with(['.', '!', '?']) {
        Position::Placed
    } else {
        Position::Buried
    }
}

/// Markdown indentation columns, with a tab advancing to the next four-column stop.
fn indentation_width(leading: &str) -> usize {
    leading.chars().fold(0, |width, character| match character {
        ' ' => width + 1,
        '\t' => width + (4 - width % 4),
        _ if character.is_whitespace() => width + 1,
        _ => width,
    })
}
