//! Recognizing explicit acknowledgment outcomes in assistant prose.
//!
//! Acknowledgment is a narrow protocol at the assistant-output boundary, not an
//! interpretation of what a turn seems to have meant. A clause retires a
//! directive only when it leads its line, spells a marker, names a key-shaped
//! token, and that key belongs to a directive still live. Everything else —
//! unknown, malformed, quoted, or historical — alters nothing and is reported
//! by name, because a silent non-recognition and a refusal look identical to an
//! operator waiting on an answer.
//!
//! The scanner performs no I/O. Standing arrives from the caller's reduced
//! facts, so the same prose read against the same standing always recognizes
//! the same outcomes, whether it runs at the door or during replay.

use std::fmt;

use wire::Disposition;

use crate::ack::keys::{Key, MARKERS, Marker};

/// The characters that quote a clause instead of speaking it.
///
/// Emphasis is not among them: the protocol asks for a plain-text header, so
/// decorated prose is misplaced rather than quoted.
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
    /// The key retired its lineage into this disposition.
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

/// Reads every acknowledgment outcome in one assistant turn.
///
/// At most one clause is read per line: an acknowledgment header leads its
/// line, so a second marker further along the same line is prose about the
/// first.
pub fn scan(prose: &str, standing: &impl Standing) -> Vec<Recognition> {
    let mut recognitions = Vec::new();
    let mut fence = None;
    for (index, line) in prose.lines().enumerate() {
        if fence.is_some_and(|open: Fence| open.closes(line)) {
            fence = None;
            continue;
        }
        if fence.is_none()
            && let Some(open) = Fence::opens(line)
        {
            fence = Some(open);
            continue;
        }
        if let Some(clause) = Clause::read(line, fence.is_some()) {
            clause.judge(index + 1, standing, &mut recognitions);
        }
    }
    recognitions
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

/// One well-formed `MARKER token[ token...]:` clause found on a line.
struct Clause {
    marker: Marker,
    position: Position,
    /// Each named token with the byte offset it begins at.
    tokens: Vec<(usize, String)>,
}

impl Clause {
    /// Reads the first clause on a line that is an acknowledgment attempt.
    ///
    /// A clause is an attempt when it leads its line or names something
    /// key-shaped. Leading is how an acknowledgment is written; a key-shaped
    /// token is how a misplaced or quoted one still gets diagnosed. Prose that
    /// merely uses the word does neither, so it stays silent instead of
    /// reporting every colon-terminated sentence as a missed acknowledgment.
    fn read(line: &str, fenced: bool) -> Option<Self> {
        for start in 0..line.len() {
            let Some(marker) = marker_at(line, start) else {
                continue;
            };
            let Some(tokens) = tokens_after(line, start + marker.as_str().len()) else {
                continue;
            };
            let position = position(line, start, fenced);
            let keylike = tokens.iter().any(|(_, token)| keylike(token));
            if position == Position::Placed || keylike {
                return Some(Self {
                    marker,
                    position,
                    tokens,
                });
            }
        }
        None
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

/// The tokens a clause names, or `None` when no clause is formed here.
///
/// The marker must be followed by horizontal space, at least one token, and the
/// colon that closes the clause. A tab separates as well as a space does;
/// refusing one would drop a real acknowledgment over whitespace.
fn tokens_after(line: &str, marker_end: usize) -> Option<Vec<(usize, String)>> {
    let rest = line.get(marker_end..)?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let colon = rest.find(':')?;
    let segment = rest.get(..colon)?;

    let mut tokens = Vec::new();
    let mut begun: Option<usize> = None;
    for (offset, character) in segment.char_indices() {
        if character.is_whitespace() {
            if let Some(begin) = begun.take() {
                tokens.push((marker_end + begin, segment[begin..offset].to_owned()));
            }
        } else if begun.is_none() {
            begun = Some(offset);
        }
    }
    if let Some(begin) = begun {
        tokens.push((marker_end + begin, segment[begin..].to_owned()));
    }

    (!tokens.is_empty()).then_some(tokens)
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
    let trimmed = leading.trim();
    if trimmed.is_empty() {
        if indentation_width(leading) >= 4 {
            Position::Quoted
        } else {
            Position::Placed
        }
    } else if trimmed
        .chars()
        .all(|character| QUOTING.contains(&character))
    {
        Position::Quoted
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
