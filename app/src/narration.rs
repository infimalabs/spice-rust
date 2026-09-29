//! What one rendered Canvas card may hand the voice backend.
//!
//! The backend speaks whatever it is given, so deciding what is speakable
//! belongs on this side of it. A card the operator can read is the only
//! source: a compaction boundary renders as a rule and a label, and a tool
//! card renders as its image, so neither has words to say even though both
//! carry text on the wire. Speaking that text reads retained context nobody
//! can see back to the room.
//!
//! Inside a card the agent's own sentences are speakable and quoted lines are
//! not, because a reply that quotes an operator back would otherwise voice the
//! operator as the agent. Fenced code is shown to be read rather than heard,
//! and a quote or a fence ends the segment around it instead of joining the
//! sentences on either side.
//!
//! Automatic narration speaks only a card's edge segments, the same selection
//! the resident makes for Speak-mode utterances, so one long message cannot
//! hold the single global voice for minutes. Playback the operator asked for
//! keeps every visible segment.

use wire::AgentActivityKind;

/// The concise automatic-narration utterance for one rendered card.
///
/// The first segment says what the message is about and the last says where it
/// landed; everything between them is detail the operator can read on the card
/// itself. A card with one segment speaks it once rather than twice.
#[must_use]
pub fn narration_utterance(activity: AgentActivityKind, text: &str) -> Option<String> {
    match card_segments(activity, text).as_slice() {
        [] => None,
        [only] => Some(only.clone()),
        [first, .., last] => Some(format!("{first} {last}")),
    }
}

/// Every visible word of one rendered card, for playback the operator asked for.
#[must_use]
pub fn card_utterance(activity: AgentActivityKind, text: &str) -> Option<String> {
    joined(card_segments(activity, text))
}

/// A resident-classified utterance held to the same visible-words contract.
///
/// The resident selects Speak-mode text from the transcript rather than from
/// the card, so an acknowledgement that restates the operator's directive can
/// still arrive carrying quotation. The card contract governs the submission
/// either way; the selection the resident already made is left intact.
#[must_use]
pub fn prepared_utterance(prepared: &str) -> Option<String> {
    joined(segments(prepared))
}

fn joined(segments: Vec<String>) -> Option<String> {
    (!segments.is_empty()).then(|| segments.join(" "))
}

/// Visible agent-authored segments of one card, in reading order.
fn card_segments(activity: AgentActivityKind, text: &str) -> Vec<String> {
    match activity {
        AgentActivityKind::Prose => segments(text),
        AgentActivityKind::Tool
        | AgentActivityKind::Reasoning
        | AgentActivityKind::Turn
        | AgentActivityKind::Compaction
        | AgentActivityKind::SignOut => Vec::new(),
    }
}

fn segments(text: &str) -> Vec<String> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut segments = Vec::new();
    let mut lines = Vec::new();
    let mut fenced = false;
    for line in normalized.split('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            push_segment(&mut segments, &mut lines);
            continue;
        }
        if fenced || trimmed.is_empty() || trimmed.starts_with('>') {
            if !fenced {
                push_segment(&mut segments, &mut lines);
            }
            continue;
        }
        lines.push(spoken_line(trimmed));
    }
    push_segment(&mut segments, &mut lines);
    segments
}

fn push_segment(segments: &mut Vec<String>, lines: &mut Vec<String>) {
    let spoken = std::mem::take(lines)
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if !spoken.is_empty() {
        segments.push(spoken);
    }
}

/// One line with its Markdown targets removed.
///
/// A link is its label; the destination is for clicking. An image is not words
/// at all: the card shows it, and reading a file name aloud says nothing the
/// picture does not already say.
fn spoken_line(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut rendered = String::new();
    let mut cursor = 0;
    while cursor < line.len() {
        let image = bytes[cursor] == b'!' && bytes.get(cursor + 1) == Some(&b'[');
        let link = bytes[cursor] == b'[';
        if (image || link)
            && let Some((label, end)) = markdown_target(line, cursor + usize::from(image))
        {
            if !image {
                rendered.push_str(label);
            }
            cursor = end;
            continue;
        }
        let character = line[cursor..].chars().next().expect("cursor is in text");
        rendered.push(character);
        cursor += character.len_utf8();
    }
    rendered
}

fn markdown_target(line: &str, open: usize) -> Option<(&str, usize)> {
    let label_start = open + 1;
    let label_end = line[label_start..].find(']')? + label_start;
    if line.as_bytes().get(label_end + 1) != Some(&b'(') {
        return None;
    }
    let target_end = line[label_end + 2..].find(')')? + label_end + 2;
    Some((line[label_start..label_end].trim(), target_end + 1))
}
