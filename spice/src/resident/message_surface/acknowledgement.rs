//! Acknowledgement headers rewritten into quoted directive context.
//!
//! An assistant header such as `ACK KEY: applied` presents the directive it
//! names, quoted verbatim above the response prose, whenever the journal
//! allocated that key: a bare key is useless to the operator. One authenticated
//! message disposition records what the whole answer earned: acknowledged once
//! ordinary keys retire, pending while one remains held, and declined when any
//! owned header refuses its directive. The disposition is independent of the
//! presented prose and directive ranges. A key this response did not answer --
//! another lane's, source-bound review feedback from another source, or an
//! answer the journal recorded otherwise -- quotes its directive without
//! earning a disposition. Only a token no directive owns stays literal.
//! Presentation never grants repeated response retirement, a disposition, or
//! automatic-speech authority.

use std::ops::Range;

use super::*;

/// One directive context the resident inserted into a presented card.
#[derive(Debug)]
pub(super) struct PresentedDirectiveContext {
    pub(super) key: steer::Key,
    pub(super) range: Range<usize>,
}

/// One presented message and the quote blocks the resident itself wrote into it.
#[derive(Debug)]
pub(super) struct Presentation {
    pub(super) text: String,
    /// Exact directive contexts in source order. Quotation nested inside them
    /// is operator-authored journal text; anything outside them is agent prose.
    pub(super) contexts: Vec<PresentedDirectiveContext>,
    /// Authenticated disposition of the whole answer. Mixed headers use
    /// declined, then pending, then acknowledged precedence.
    pub(super) acknowledgement: Option<wire::MessageAcknowledgement>,
}

pub(super) fn present_steering_acknowledgements_with_directives(
    text: &str,
    directives: &PresentationDirectives,
    event: PresentationEvent,
) -> Result<Presentation, String> {
    let standing = EventDirectives { directives, event };
    let mut rendered = String::new();
    let mut contexts = Vec::new();
    let mut answer_disposition: Option<wire::MessageAcknowledgement> = None;
    let mut cursor = 0;
    let mut changed = false;
    let mut capitalize_body = false;
    for acknowledgement in scan_presentations(text, &standing) {
        let separator = acknowledgement.separator;
        let packet = classify_packet(&standing, acknowledgement.recognitions);
        // Every allocated key a header names presents its directive; a header
        // whose tokens name no directive stays the agent's literal prose.
        if packet.states.is_empty() && packet.contexts.is_empty() {
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
        let disposition = packet_acknowledgement(&packet);
        if let Some(disposition) = disposition
            && answer_disposition
                .is_none_or(|current| disposition.precedence() > current.precedence())
        {
            answer_disposition = Some(disposition);
        }
        cursor = end;
        if packet.missed.is_empty() {
            cursor = skip_horizontal_space(text, cursor);
            if !packet.contexts.is_empty() {
                cursor = skip_line_breaks(text, cursor);
            }
            capitalize_body = true;
        } else {
            // An authenticated disposition removes the authored ACK or NAK;
            // otherwise the header's literal word stays with tokens it failed
            // to own.
            if disposition.is_none() {
                let literal = packet
                    .marker
                    .ok_or_else(|| "presented acknowledgment header lost its marker".to_owned())?;
                rendered.push_str(acknowledgement_marker(literal)?);
            }
            for token in packet.missed {
                if rendered
                    .chars()
                    .next_back()
                    .is_some_and(|character| !character.is_whitespace())
                {
                    rendered.push(' ');
                }
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
            acknowledgement: answer_disposition,
        });
    }
    let source = text
        .get(cursor..)
        .ok_or_else(|| "acknowledgment body crossed a non-character boundary".to_owned())?;
    append_presented_source(&mut rendered, source, capitalize_body);
    Ok(Presentation {
        text: rendered,
        contexts,
        acknowledgement: answer_disposition,
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
    /// States authenticated by this event, including non-authored system keys
    /// and the lane seal, which deliberately have no quoted context.
    states: Vec<ContextState>,
    contexts: Vec<(steer::Key, ContextState)>,
    missed: Vec<String>,
}

fn classify_packet(
    standing: &EventDirectives<'_>,
    recognitions: Vec<steer::ack::scan::Recognition>,
) -> ClassifiedPacket {
    let mut packet = ClassifiedPacket {
        marker: None,
        states: Vec::new(),
        contexts: Vec::new(),
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
            packet
                .states
                .push(standing.context_state(&key, disposition));
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

fn skip_line_breaks(text: &str, mut cursor: usize) -> usize {
    while let Some(tail) = text.get(cursor..) {
        if tail.starts_with("\r\n") {
            cursor += 2;
        } else if tail.starts_with('\n') || tail.starts_with('\r') {
            cursor += 1;
        } else {
            break;
        }
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

/// The one disposition a header contributes to its containing message.
///
/// Every quoted directive decides it, regardless of flavor: a refusal is an
/// answer the operator must see at a glance, so declined wins; a key the
/// journal still holds reads pending, so spoken acknowledgement never reads as
/// credit before steer::acked lands; otherwise retired keys read acknowledged.
/// A bare acknowledgement of the lane seal quotes nothing and still answers.
fn packet_acknowledgement(packet: &ClassifiedPacket) -> Option<wire::MessageAcknowledgement> {
    use wire::MessageAcknowledgement as Acknowledgement;
    if packet.states.contains(&ContextState::Declined) {
        Some(Acknowledgement::Declined)
    } else if packet.states.contains(&ContextState::Pending) {
        Some(Acknowledgement::Pending)
    } else if packet.states.contains(&ContextState::Acknowledged) {
        Some(Acknowledgement::Acknowledged)
    } else {
        None
    }
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
