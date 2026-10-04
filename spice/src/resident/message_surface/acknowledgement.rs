//! Acknowledgement headers rewritten into quoted directive context.
//!
//! An assistant header such as `ACK KEY: applied` presents as the directive it
//! names, quoted verbatim above the response prose, whenever the journal
//! allocated that key: a bare key is useless to the operator. The response
//! then opens with one marker for the whole header, beside the speaking agent:
//! the plain ACK marker once the journal retired the header's ordinary keys for
//! this lane, including responses from later transcript sources, the pending
//! marker while one is still held, and NAK for a refusal. A key this response
//! did not answer -- another lane's, source-bound review feedback from another
//! source, or an answer the journal recorded otherwise -- quotes its directive
//! without earning a marker. Only a token no directive owns stays literal.
//! Presentation never grants a repeated response retirement, a badge, or
//! automatic-speech authority.

use std::ops::Range;

use super::*;

/// One directive context the resident inserted into a presented card.
#[derive(Debug)]
pub(super) struct PresentedDirectiveContext {
    pub(super) key: steer::Key,
    pub(super) range: Range<usize>,
}

/// One answer marker the resident itself wrote at the head of a response.
#[derive(Debug)]
pub(super) struct PresentedMarker {
    pub(super) start: usize,
    pub(super) kind: wire::MessageClassificationKind,
}

/// One presented message and the quote blocks the resident itself wrote into it.
#[derive(Debug)]
pub(super) struct Presentation {
    pub(super) text: String,
    /// Exact directive contexts in source order. Quotation nested inside them
    /// is operator-authored journal text; anything outside them is agent prose.
    pub(super) contexts: Vec<PresentedDirectiveContext>,
    /// Answer markers in source order, each opening the response after its
    /// header's contexts. The same words the agent wrote stay literal.
    pub(super) markers: Vec<PresentedMarker>,
}

pub(super) fn present_steering_acknowledgements_with_directives(
    text: &str,
    directives: &PresentationDirectives,
    event: PresentationEvent,
) -> Result<Presentation, String> {
    let standing = EventDirectives { directives, event };
    let mut rendered = String::new();
    let mut contexts = Vec::new();
    let mut markers = Vec::new();
    let mut cursor = 0;
    let mut changed = false;
    let mut capitalize_body = false;
    for acknowledgement in scan_presentations(text, &standing) {
        let separator = acknowledgement.separator;
        let packet = classify_packet(&standing, acknowledgement.recognitions);
        // Every allocated key a header names presents its directive; a header
        // whose tokens name no directive stays the agent's literal prose.
        if packet.recognized.is_empty() && packet.contexts.is_empty() {
            continue;
        }
        let start = presentation_framing_start(text, cursor, acknowledgement.start);
        let end = presentation_framing_end(text, acknowledgement.end);
        let framing_removed = start != acknowledgement.start || end != acknowledgement.end;
        let source = text
            .get(cursor..start)
            .ok_or_else(|| "acknowledgment header crossed a non-character boundary".to_owned())?;
        append_presented_source(&mut rendered, source, capitalize_body);
        capitalize_body = false;
        if !packet.contexts.is_empty() {
            start_quote_block(&mut rendered);
            for (index, (key, _)) in packet.contexts.iter().enumerate() {
                let directive = directives.0.get(key).ok_or_else(|| {
                    format!("canonical acknowledgment recognition lost directive {key}")
                })?;
                if index > 0 {
                    rendered.push_str("\n\n");
                }
                let start = rendered.len();
                append_acknowledgement_quote(&mut rendered, &directive.text);
                contexts.push(PresentedDirectiveContext {
                    key: key.clone(),
                    range: start..rendered.len(),
                });
            }
            rendered.push_str("\n\n");
        }
        let marker = packet_marker(&packet, directives);
        if let Some(kind) = marker {
            markers.push(PresentedMarker {
                start: rendered.len(),
                kind,
            });
            rendered.push_str(kind.marker());
        }
        cursor = end;
        if packet.missed.is_empty() {
            cursor = skip_horizontal_space(text, cursor);
            capitalize_body = true;
            if marker.is_some() {
                let first = text
                    .get(cursor..)
                    .and_then(|tail| tail.split('\n').next())
                    .unwrap_or_default();
                if opens_paragraph_text(first) {
                    rendered.push(' ');
                } else if !first.is_empty() {
                    rendered.push_str("\n\n");
                }
            }
        } else {
            // A marker the resident wrote already says ACK or NAK; otherwise
            // the header's literal word stays with the tokens it failed to own.
            if marker.is_none() {
                let literal = packet
                    .marker
                    .ok_or_else(|| "presented acknowledgment header lost its marker".to_owned())?;
                rendered.push_str(acknowledgement_marker(literal)?);
            }
            for token in packet.missed {
                rendered.push(' ');
                rendered.push_str(&token);
            }
            if let Some(separator) = separator {
                rendered.push(separator);
            } else if framing_removed
                && text
                    .get(cursor..)
                    .and_then(|tail| tail.chars().next())
                    .is_some_and(|character| !character.is_whitespace())
            {
                rendered.push(' ');
            }
        }
        changed = true;
    }

    if !changed {
        return Ok(Presentation {
            text: text.to_owned(),
            contexts,
            markers,
        });
    }
    let source = text
        .get(cursor..)
        .ok_or_else(|| "acknowledgment body crossed a non-character boundary".to_owned())?;
    append_presented_source(&mut rendered, source, capitalize_body);
    Ok(Presentation {
        text: rendered,
        contexts,
        markers,
    })
}

/// Reuse the protocol scanner with display standing, not retirement authority.
pub(super) fn scan_presentations(
    text: &str,
    standing: &EventDirectives<'_>,
) -> Vec<steer::ack::scan::Acknowledgement> {
    steer::ack::scan::scan_acknowledgements(text, &DisplayStanding(standing))
}

/// Display standing: every allocated key and the lane seal read as live, so the
/// scanner reports each with the header's disposition and presentation decides
/// what it earned.
struct DisplayStanding<'a>(&'a EventDirectives<'a>);

impl steer::Standing for DisplayStanding<'_> {
    fn disposition(&self, key: &steer::Key) -> Option<wire::Disposition> {
        (self.0.directives.0.contains_key(key) || self.0.is_lane_seal(key))
            .then_some(wire::Disposition::Live)
    }
}

/// How one authored directive presents inside an acknowledgement quote.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ContextState {
    /// Positively acknowledged and retired by the journal for this lane.
    Acknowledged,
    /// Positively acknowledged in prose while the journal still holds the key.
    Pending,
    /// Declined here: the lane answered and refused.
    Declined,
    /// Referenced context this response did not answer.
    Unmarked,
}

/// One acknowledgement header sorted into what it answered, quotes, and missed.
struct ClassifiedPacket {
    /// Disposition the header's marker spells.
    marker: Option<wire::Disposition>,
    recognized: Vec<(steer::Key, wire::Disposition)>,
    contexts: Vec<(steer::Key, ContextState)>,
    bare_acknowledgement: bool,
    missed: Vec<String>,
}

fn classify_packet(
    standing: &EventDirectives<'_>,
    recognitions: Vec<steer::ack::scan::Recognition>,
) -> ClassifiedPacket {
    let mut packet = ClassifiedPacket {
        marker: None,
        recognized: Vec::new(),
        contexts: Vec::new(),
        bare_acknowledgement: false,
        missed: Vec::new(),
    };
    for recognition in recognitions {
        let (key, disposition) = match recognition.outcome {
            steer::ack::scan::Outcome::Retired { key, disposition } => (key, disposition),
            steer::ack::scan::Outcome::Missed { token, .. } => {
                packet.missed.push(token);
                continue;
            }
        };
        packet.marker.get_or_insert(disposition);
        let presents = standing.presents(&key, disposition, recognition.locus);
        if presents {
            packet.bare_acknowledgement |=
                disposition == wire::Disposition::Acked && standing.is_lane_seal(&key);
            if !packet
                .recognized
                .iter()
                .any(|known| known == &(key.clone(), disposition))
            {
                packet.recognized.push((key.clone(), disposition));
            }
        }
        if standing.authored(&key) {
            if !packet.contexts.iter().any(|(known, _)| known == &key) {
                let state = if presents {
                    standing.context_state(&key, disposition)
                } else {
                    ContextState::Unmarked
                };
                packet.contexts.push((key, state));
            }
        } else if !presents {
            packet.missed.push(key.to_string());
        }
    }
    packet
}

fn append_presented_source(rendered: &mut String, source: &str, capitalize: bool) {
    if !capitalize {
        rendered.push_str(source);
        return;
    }
    let terminal = source.trim_end_matches(char::is_whitespace).len();
    let (body, trailing) = source.split_at(terminal);
    let (body, normalize_colon) = body
        .strip_suffix(':')
        .map_or((body, false), |body| (body, true));
    let mut characters = body.chars();
    if let Some(first) = characters.next() {
        rendered.extend(first.to_uppercase());
        rendered.push_str(characters.as_str());
    }
    if normalize_colon {
        rendered.push('.');
    }
    rendered.push_str(trailing);
}

fn start_quote_block(rendered: &mut String) {
    while rendered.ends_with([' ', '\t']) {
        rendered.pop();
    }
    if !rendered.is_empty() && !rendered.ends_with('\n') {
        rendered.push('\n');
    }
}

/// Decorative punctuation that models sometimes wrap around a protocol header
/// is not authored response prose. Consume it only when the whole current-line
/// prefix is framing, so brackets and dashes after real words remain content.
fn presentation_framing_start(text: &str, floor: usize, start: usize) -> usize {
    let Some(prefix) = text.get(..start) else {
        return start;
    };
    let line_start = prefix
        .rfind('\n')
        .map_or(0, |newline| newline + 1)
        .max(floor);
    let Some(framing) = text.get(line_start..start) else {
        return start;
    };
    let mut decorated = false;
    for character in framing.chars() {
        match character {
            ' ' | '\t' => {}
            '[' | '\u{2013}' | '\u{2014}' => decorated = true,
            _ => return start,
        }
    }
    if decorated { line_start } else { start }
}

/// A closing square bracket can hide the ordinary separator from the protocol
/// scanner. Remove that close and any adjacent Unicode dash before the body is
/// capitalized and before Markdown turns inserted directive context into a
/// structural quote card.
fn presentation_framing_end(text: &str, end: usize) -> usize {
    let mut cursor = skip_horizontal_space(text, end);
    let mut bracketed = false;
    while text.get(cursor..).is_some_and(|tail| tail.starts_with(']')) {
        cursor += ']'.len_utf8();
        cursor = skip_horizontal_space(text, cursor);
        bracketed = true;
    }
    if !bracketed {
        return end;
    }
    while let Some(character) = text.get(cursor..).and_then(|tail| tail.chars().next()) {
        if !matches!(character, '\u{2013}' | '\u{2014}') {
            break;
        }
        cursor += character.len_utf8();
        cursor = skip_horizontal_space(text, cursor);
    }
    cursor
}

fn skip_horizontal_space(text: &str, mut cursor: usize) -> usize {
    while text
        .as_bytes()
        .get(cursor)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        cursor += 1;
    }
    cursor
}

fn acknowledgement_marker(disposition: wire::Disposition) -> Result<&'static str, String> {
    match disposition {
        wire::Disposition::Acked => Ok("ACK"),
        wire::Disposition::Declined => Ok("NAK"),
        wire::Disposition::Live
        | wire::Disposition::Lapsed
        | wire::Disposition::DeadLettered
        | wire::Disposition::Canceled => Err(format!(
            "acknowledgment scanner returned nonterminal disposition {disposition:?}"
        )),
    }
}

/// The one marker a header's response opens with.
///
/// Ordinary directives the header answered decide it: a refusal is an answer
/// the operator must see at a glance, so NAK wins; a key the journal still
/// holds reads pending, so a spoken acknowledgement never reads as credit
/// before steer::acked lands; otherwise retired keys read ACK. A taste,
/// broadcast, or review directive already opens with its own flavor marker,
/// and merely historical context earns no marker. A bare acknowledgement of
/// the lane seal quotes nothing and still answers.
fn packet_marker(
    packet: &ClassifiedPacket,
    directives: &PresentationDirectives,
) -> Option<wire::MessageClassificationKind> {
    use wire::MessageClassificationKind as Kind;
    let states = packet
        .contexts
        .iter()
        .filter(|(key, _)| directives.0.get(key).is_some_and(is_ordinary))
        .map(|(_, state)| *state)
        .collect::<Vec<_>>();
    if states.contains(&ContextState::Declined) {
        Some(Kind::Decline)
    } else if states.contains(&ContextState::Pending) {
        Some(Kind::AcknowledgementPending)
    } else if states.contains(&ContextState::Acknowledged) || packet.bare_acknowledgement {
        Some(Kind::Acknowledgement)
    } else {
        None
    }
}

fn opens_paragraph_text(line: &str) -> bool {
    if line.starts_with("    ") || line.starts_with('\t') {
        return false;
    }
    let line = line.trim_start();
    let Some(first) = line.chars().next() else {
        return false;
    };
    if first.is_ascii_digit() {
        let after_digits = line.trim_start_matches(|character: char| character.is_ascii_digit());
        return !after_digits.starts_with(['.', ')']);
    }
    !matches!(
        first,
        '>' | '#' | '-' | '+' | '*' | '`' | '~' | '|' | '<' | '_'
    )
}

fn append_acknowledgement_quote(rendered: &mut String, directive: &str) {
    for (index, line) in directive.split('\n').enumerate() {
        if index > 0 {
            rendered.push('\n');
        }
        rendered.push('>');
        if !line.is_empty() {
            rendered.push(' ');
        }
        rendered.push_str(line);
    }
}
