//! Acknowledgement headers rewritten into quoted directive context.
//!
//! An assistant header such as `ACK KEY: applied` presents as the directive it
//! answers, quoted above the response prose. The quote carries the plain ACK
//! marker once the journal retired the key for this lane, including responses
//! from later transcript sources; while the key is still pending the quote says
//! so. Presentation never grants a repeated response retirement or
//! automatic-speech authority.

use std::borrow::Cow;
use std::ops::Range;

use super::*;

/// One presented message and the quote blocks the resident itself wrote into it.
#[derive(Debug)]
pub(super) struct Presentation {
    pub(super) text: String,
    /// Byte ranges of the directive-context quotes presentation inserted, in
    /// source order. Quotation nested inside them is operator-authored journal
    /// text; anything outside them is the agent's own prose.
    pub(super) contexts: Vec<Range<usize>>,
}

pub(super) fn present_steering_acknowledgements_with_directives(
    text: &str,
    directives: &PresentationDirectives,
    event: PresentationEvent,
) -> Result<Presentation, String> {
    let standing = EventDirectives { directives, event };
    let mut rendered = String::new();
    let mut contexts = Vec::new();
    let mut cursor = 0;
    let mut changed = false;
    let mut capitalize_body = false;
    for acknowledgement in scan_presentations(text, &standing) {
        let separator = acknowledgement.separator;
        let packet = classify_packet(&standing, acknowledgement.recognitions);
        // Other historical dispositions add context only alongside a recognized
        // response; an acknowledged key can present again anywhere on its lane.
        if packet.recognized.is_empty() {
            continue;
        }
        let source = text
            .get(cursor..acknowledgement.start)
            .ok_or_else(|| "acknowledgment header crossed a non-character boundary".to_owned())?;
        append_presented_source(&mut rendered, source, capitalize_body);
        capitalize_body = false;
        if packet.bare_acknowledgement || !packet.contexts.is_empty() {
            start_quote_block(&mut rendered);
            let context = rendered.len();
            let mut separated = false;
            if packet.bare_acknowledgement {
                append_acknowledgement_quote(&mut rendered, wire::stream::ACK_MARKER);
                separated = true;
            }
            for (key, state) in &packet.contexts {
                let directive = directives.0.get(key).ok_or_else(|| {
                    format!("canonical acknowledgment recognition lost directive {key}")
                })?;
                if separated {
                    rendered.push_str("\n\n");
                }
                append_acknowledgement_quote(
                    &mut rendered,
                    &acknowledged_context_text(directive, *state),
                );
                separated = true;
            }
            contexts.push(context..rendered.len());
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
        return Ok(Presentation {
            text: text.to_owned(),
            contexts,
        });
    }
    let source = text
        .get(cursor..)
        .ok_or_else(|| "acknowledgment body crossed a non-character boundary".to_owned())?;
    append_presented_source(&mut rendered, source, capitalize_body);
    Ok(Presentation {
        text: rendered,
        contexts,
    })
}

/// Reuse the protocol scanner with display standing, not retirement authority.
pub(super) fn scan_presentations(
    text: &str,
    standing: &EventDirectives<'_>,
) -> Vec<steer::ack::scan::Acknowledgement> {
    steer::ack::scan::scan_acknowledgements(text, &DisplayStanding(standing))
}

struct DisplayStanding<'a>(&'a EventDirectives<'a>);

impl steer::Standing for DisplayStanding<'_> {
    fn disposition(&self, key: &steer::Key) -> Option<wire::Disposition> {
        if self.0.acknowledged_for_lane(key) || self.0.is_lane_seal(key) {
            Some(wire::Disposition::Live)
        } else {
            steer::Standing::disposition(self.0, key)
        }
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
    /// Historical context for a key retired elsewhere.
    Unmarked,
}

/// One acknowledgement header sorted into what it retired, quotes, and missed.
struct ClassifiedPacket {
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
        recognized: Vec::new(),
        contexts: Vec::new(),
        bare_acknowledgement: false,
        missed: Vec::new(),
    };
    for recognition in recognitions {
        match recognition.outcome {
            steer::ack::scan::Outcome::Retired { key, disposition }
                if standing.presents(&key, disposition, recognition.locus) =>
            {
                let lane_seal =
                    disposition == wire::Disposition::Acked && standing.is_lane_seal(&key);
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
                packet.bare_acknowledgement |= lane_seal;
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

/// The quoted context text for one answered directive.
///
/// An ordinary directive retired by a positive acknowledgement carries the
/// canonical ACK marker so Canvas can badge it inline; while the journal still
/// holds its key pending it carries the pending marker instead, so a spoken
/// acknowledgment never reads as credit before steer::acked lands. A declined
/// directive carries the NACK marker, because a refusal is an answer the
/// operator must be able to see at a glance. A taste, broadcast, or review
/// directive already opens with its own flavor marker, and merely historical
/// context earns no badge.
fn acknowledged_context_text(
    directive: &PresentationDirective,
    state: ContextState,
) -> Cow<'_, str> {
    let marker = match state {
        ContextState::Acknowledged => wire::stream::ACK_MARKER,
        ContextState::Pending => wire::stream::ACK_PENDING_MARKER,
        ContextState::Declined => wire::stream::NACK_MARKER,
        ContextState::Unmarked => return Cow::Borrowed(&directive.text),
    };
    if !is_ordinary(directive) {
        return Cow::Borrowed(&directive.text);
    }
    mark_acknowledged_context(marker, &directive.text)
}

/// Mark one ordinary acknowledged directive where the operator's own words start.
///
/// A directive commonly carries quotation it is answering -- an agent card, an
/// earlier exchange -- and the direction itself is the prose left outside those
/// quotes. The marker therefore anchors the last run of unquoted lines:
/// scanning back from the end, quoted and blank lines are passed over, and the
/// run holding the first line found reaches back to the quote above it or to
/// the start of the directive. A directive that quotes nothing anchors at its
/// first line, which is the head the marker has always taken.
///
/// The marker shares that line when it reads as paragraph text, so the Canvas
/// pill sits inline with the words the operator wrote. A line that opens
/// another block (list, heading, table, fence, rule, or indented code) keeps
/// its structure beneath a marker paragraph of its own. Either way the marker
/// stands clear of the quote above it: a line directly below a quote would
/// lazily continue that quote's paragraph and carry the marker one quote level
/// deeper than the directive it answers.
fn mark_acknowledged_context<'a>(marker: &str, text: &'a str) -> Cow<'a, str> {
    if text.is_empty() {
        return Cow::Owned(marker.to_owned());
    }
    let Some(anchor) = own_words_offset(text) else {
        return Cow::Owned(format!("{marker}\n\n{text}"));
    };
    let (quoted, own) = text.split_at(anchor);
    let clear = quoted.strip_suffix('\n').is_none_or(|above| {
        above
            .rsplit('\n')
            .next()
            .is_some_and(|line| line.trim().is_empty())
    });
    let separation = if clear { "" } else { "\n" };
    let first = own.split('\n').next().unwrap_or_default();
    if opens_paragraph_text(first) {
        Cow::Owned(format!("{quoted}{separation}{marker} {own}"))
    } else {
        Cow::Owned(format!("{quoted}{separation}{marker}\n\n{own}"))
    }
}

/// Where the directive's last unquoted run begins.
///
/// `None` answers a directive that is quotation and blank lines all the way
/// down: nothing of the operator's own is left for the marker to sit beside,
/// so the caller gives it a paragraph above the whole quotation instead.
fn own_words_offset(text: &str) -> Option<usize> {
    let mut offset = 0;
    let mut run = None;
    let mut last = None;
    for line in text.split('\n') {
        if line.trim_start().starts_with('>') {
            run = None;
        } else if !line.trim().is_empty() {
            last = Some(*run.get_or_insert(offset));
        }
        offset += line.len() + 1;
    }
    last
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
