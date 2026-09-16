//! Acknowledgement headers rewritten into quoted directive context.
//!
//! An assistant header such as `ACK KEY: applied` presents as the directive it
//! answers, quoted above the response prose. The quote carries the plain ACK
//! marker only once the journal retired the key at this event; while the key
//! is still pending the quote says so, so the Canvas card and the Agents tile
//! count read the same state.

use std::borrow::Cow;

use super::*;

pub(super) fn present_steering_acknowledgements_with_directives(
    text: &str,
    directives: &PresentationDirectives,
    event: PresentationEvent,
) -> Result<String, String> {
    let standing = EventDirectives { directives, event };
    let mut rendered = String::new();
    let mut cursor = 0;
    let mut changed = false;
    let mut capitalize_body = false;
    for acknowledgement in steer::ack::scan::scan_acknowledgements(text, &standing) {
        let separator = acknowledgement.separator;
        let packet = classify_packet(&standing, acknowledgement.recognitions);
        // Historical keys add display context only when this packet also
        // carries a retirement authenticated to the current event.
        if packet.recognized.is_empty() {
            continue;
        }
        let source = text
            .get(cursor..acknowledgement.start)
            .ok_or_else(|| "acknowledgment header crossed a non-character boundary".to_owned())?;
        append_presented_source(&mut rendered, source, capitalize_body);
        capitalize_body = false;
        if !packet.contexts.is_empty() {
            start_quote_block(&mut rendered);
            for (index, (key, state)) in packet.contexts.iter().enumerate() {
                let directive = directives.0.get(key).ok_or_else(|| {
                    format!("canonical acknowledgment recognition lost directive {key}")
                })?;
                if index > 0 {
                    rendered.push_str("\n\n");
                }
                append_acknowledgement_quote(
                    &mut rendered,
                    &acknowledged_context_text(directive, *state),
                );
            }
            rendered.push_str("\n\n");
        }
        cursor = acknowledgement.end;
        if packet.missed.is_empty() {
            cursor = skip_horizontal_space(text, cursor);
            capitalize_body = true;
        } else {
            rendered.push_str(acknowledgement_marker(packet.recognized[0].1)?);
            for token in packet.missed {
                rendered.push(' ');
                rendered.push_str(&token);
            }
            if let Some(separator) = separator {
                rendered.push(separator);
            }
        }
        changed = true;
    }

    if !changed {
        return Ok(text.to_owned());
    }
    let source = text
        .get(cursor..)
        .ok_or_else(|| "acknowledgment body crossed a non-character boundary".to_owned())?;
    append_presented_source(&mut rendered, source, capitalize_body);
    Ok(rendered)
}

/// How one authored directive presents inside an acknowledgement quote.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ContextState {
    /// Positively acknowledged and retired by the journal at this event.
    Acknowledged,
    /// Positively acknowledged in prose while the journal still holds the key.
    Pending,
    /// Declined here, or historical context for a key retired elsewhere.
    Unmarked,
}

/// One acknowledgement header sorted into what it retired, quotes, and missed.
struct ClassifiedPacket {
    recognized: Vec<(steer::Key, wire::Disposition)>,
    contexts: Vec<(steer::Key, ContextState)>,
    missed: Vec<String>,
}

fn classify_packet(
    standing: &EventDirectives<'_>,
    recognitions: Vec<steer::ack::scan::Recognition>,
) -> ClassifiedPacket {
    let mut packet = ClassifiedPacket {
        recognized: Vec::new(),
        contexts: Vec::new(),
        missed: Vec::new(),
    };
    for recognition in recognitions {
        match recognition.outcome {
            steer::ack::scan::Outcome::Retired { key, disposition }
                if standing.recognizes(&key, disposition, recognition.locus) =>
            {
                if !packet
                    .recognized
                    .iter()
                    .any(|known| known == &(key.clone(), disposition))
                {
                    packet.recognized.push((key.clone(), disposition));
                }
                if standing.authored(&key)
                    && !packet.contexts.iter().any(|(known, _)| known == &key)
                {
                    let state = standing.context_state(&key, disposition);
                    packet.contexts.push((key, state));
                }
            }
            steer::ack::scan::Outcome::Retired { key, .. } => {
                packet.missed.push(key.to_string());
            }
            steer::ack::scan::Outcome::Missed { token, miss } => {
                if let Some(key) = standing.historical_context(&token, miss) {
                    if !packet.contexts.iter().any(|(known, _)| known == &key) {
                        packet.contexts.push((key, ContextState::Unmarked));
                    }
                } else {
                    packet.missed.push(token);
                }
            }
        }
    }
    packet
}

fn append_presented_source(rendered: &mut String, source: &str, capitalize: bool) {
    if !capitalize {
        rendered.push_str(source);
        return;
    }
    let mut characters = source.chars();
    let Some(first) = characters.next() else {
        return;
    };
    rendered.extend(first.to_uppercase());
    rendered.push_str(characters.as_str());
}

fn start_quote_block(rendered: &mut String) {
    while rendered.ends_with([' ', '\t']) {
        rendered.pop();
    }
    if !rendered.is_empty() && !rendered.ends_with('\n') {
        rendered.push('\n');
    }
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
        wire::Disposition::Declined => Ok("NACK"),
        wire::Disposition::Live
        | wire::Disposition::Lapsed
        | wire::Disposition::DeadLettered
        | wire::Disposition::Canceled => Err(format!(
            "acknowledgment scanner returned nonterminal disposition {disposition:?}"
        )),
    }
}

/// The quoted context text for one acknowledged directive.
///
/// An ordinary directive retired by a positive acknowledgement carries the
/// canonical ACK marker so Canvas can badge it inline; while the journal still
/// holds its key pending it carries the pending marker instead, so a spoken
/// acknowledgment never reads as credit before steer::acked lands. A taste,
/// broadcast, or review directive already opens with its own flavor marker,
/// and a declined or merely historical context earns no badge.
fn acknowledged_context_text(
    directive: &PresentationDirective,
    state: ContextState,
) -> Cow<'_, str> {
    let marker = match state {
        ContextState::Acknowledged => wire::stream::ACK_MARKER,
        ContextState::Pending => wire::stream::ACK_PENDING_MARKER,
        ContextState::Unmarked => return Cow::Borrowed(&directive.text),
    };
    if !is_ordinary(directive) {
        return Cow::Borrowed(&directive.text);
    }
    Cow::Owned(mark_acknowledged_context(marker, &directive.text))
}

/// Prefix one ordinary acknowledged directive with its acknowledgement marker.
///
/// The marker shares the first line when that line reads as paragraph text so
/// the Canvas pill sits inline with the directive. A first line that opens
/// another block (a nested operator quote, list, heading, table, fence, rule,
/// or indented code) keeps its structure beneath a marker paragraph of its own.
fn mark_acknowledged_context(marker: &str, text: &str) -> String {
    if text.is_empty() {
        return marker.to_owned();
    }
    let first = text.split('\n').next().unwrap_or_default();
    if opens_paragraph_text(first) {
        format!("{marker} {text}")
    } else {
        format!("{marker}\n\n{text}")
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
