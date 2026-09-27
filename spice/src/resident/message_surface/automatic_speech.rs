//! Reference-equivalent automatic Speak classification for Canvas messages.
//!
//! `spice-z` speaks a vendor-final answer or the bodies of positively parsed
//! ACK packets. It keeps commentary, NAK-only prose, tool activity, history,
//! and replay silent. Final answers use their first and last visible
//! paragraphs; ACK messages use every eligible packet body. This module makes
//! that selection once at the resident boundary so the browser never guesses
//! from presentation Markdown.

use super::{EventDirectives, PresentationDirectives, PresentationEvent};

use steer::ack::scan::{Acknowledgement, Miss, Outcome};
use wire::{AutomaticSpeech, AutomaticSpeechKind, Disposition};

pub(super) fn classify(
    prose: &listen::Prose,
    directives: Option<&PresentationDirectives>,
    event: PresentationEvent,
) -> Option<AutomaticSpeech> {
    let final_answer = prose.phase.as_deref() == Some("final_answer");
    let packets: Vec<(Acknowledgement, bool)> = directives.map_or_else(
        || {
            let standing = EmptyStanding;
            steer::ack::scan::scan_acknowledgements(&prose.text, &standing)
                .into_iter()
                .map(|packet| (packet, false))
                .collect()
        },
        |directives| {
            let standing = EventDirectives { directives, event };
            steer::ack::scan::scan_acknowledgements(&prose.text, &standing)
                .into_iter()
                .map(|packet| {
                    let acknowledged = packet.recognitions.iter().any(|recognition| {
                        matches!(
                            &recognition.outcome,
                            Outcome::Retired {
                                key,
                                disposition: Disposition::Acked,
                            } if standing.recognizes_authored(
                                key,
                                Disposition::Acked,
                                recognition.locus,
                            )
                        )
                    });
                    (packet, acknowledged)
                })
                .collect()
        },
    );

    if final_answer {
        let body = without_protocol_headers(&prose.text, &packets);
        return prepared_edges(&body).map(|text| AutomaticSpeech {
            kind: AutomaticSpeechKind::Final,
            text,
        });
    }

    let acknowledged = packet_bodies(&prose.text, &packets)
        .into_iter()
        .filter_map(|(body, eligible)| eligible.then_some(body))
        .filter_map(prepared_all)
        .collect::<Vec<_>>()
        .join(" ");
    (!acknowledged.is_empty()).then_some(AutomaticSpeech {
        kind: AutomaticSpeechKind::Acknowledgement,
        text: acknowledged,
    })
}

struct EmptyStanding;

impl steer::Standing for EmptyStanding {
    fn disposition(&self, _key: &steer::Key) -> Option<Disposition> {
        None
    }
}

fn packet_bodies<'a>(text: &'a str, packets: &[(Acknowledgement, bool)]) -> Vec<(&'a str, bool)> {
    packets
        .iter()
        .enumerate()
        .filter(|(_, (packet, _))| canonical_header(packet))
        .filter_map(|(index, (packet, eligible))| {
            let end = packets[index + 1..]
                .iter()
                .find(|(next, _)| canonical_header(next))
                .map_or(text.len(), |(next, _)| next.start);
            text.get(packet.end..end).map(|body| {
                (
                    body.trim_start_matches([' ', '\t', ':', '.', '-', ',']),
                    *eligible,
                )
            })
        })
        .collect()
}

fn without_protocol_headers(text: &str, packets: &[(Acknowledgement, bool)]) -> String {
    let mut rendered = String::new();
    let mut cursor = 0;
    for (packet, _) in packets
        .iter()
        .filter(|(packet, _)| canonical_header(packet))
    {
        if let Some(source) = text.get(cursor..packet.start) {
            rendered.push_str(source);
        }
        cursor = packet.end;
        while text
            .as_bytes()
            .get(cursor)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b':' | b'.' | b'-' | b','))
        {
            cursor += 1;
        }
    }
    if let Some(source) = text.get(cursor..) {
        rendered.push_str(source);
    }
    rendered
}

fn canonical_header(packet: &Acknowledgement) -> bool {
    packet
        .recognitions
        .iter()
        .any(|recognition| match recognition.outcome {
            Outcome::Retired { .. } => true,
            Outcome::Missed {
                miss: Miss::Unknown | Miss::Historical,
                ..
            } => true,
            Outcome::Missed { .. } => false,
        })
}

fn prepared_edges(raw: &str) -> Option<String> {
    let paragraphs = speech_paragraphs(raw);
    let selected = match paragraphs.as_slice() {
        [] => return None,
        [only] => only.clone(),
        [first, .., last] => format!("{first} {last}"),
    };
    prepared_all(&selected)
}

fn prepared_all(raw: &str) -> Option<String> {
    let paragraphs = speech_paragraphs(raw);
    let joined = paragraphs.join(" ");
    let markdown = replace_markdown(&joined);
    let dates = speak_utc_datetimes(&markdown);
    let hashes = speak_hashes(&dates);
    let collapsed = hashes.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then_some(collapsed)
}

fn speech_paragraphs(raw: &str) -> Vec<String> {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut paragraphs = Vec::new();
    let mut lines = Vec::new();
    for line in normalized.split('\n') {
        if line.trim_matches([' ', '\t']).is_empty() {
            push_speech_paragraph(&mut paragraphs, &mut lines);
        } else {
            lines.push(line);
        }
    }
    push_speech_paragraph(&mut paragraphs, &mut lines);
    paragraphs
}

fn push_speech_paragraph(paragraphs: &mut Vec<String>, raw_lines: &mut Vec<&str>) {
    let lines = raw_lines
        .drain(..)
        .filter(|line| {
            let trimmed = line.trim();
            !(trimmed.starts_with("::") && trimmed.ends_with('}')) && !trimmed.starts_with("TASK ")
        })
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if !lines.is_empty() {
        paragraphs.push(lines.join("\n"));
    }
}

fn replace_markdown(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut rendered = String::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let image = bytes.get(cursor) == Some(&b'!') && bytes.get(cursor + 1) == Some(&b'[');
        let link = bytes.get(cursor) == Some(&b'[');
        if image || link {
            let label_start = cursor + usize::from(image) + 1;
            if let Some(label_end) = raw[label_start..].find(']').map(|at| label_start + at)
                && bytes.get(label_end + 1) == Some(&b'(')
                && let Some(target_end) =
                    raw[label_end + 2..].find(')').map(|at| label_end + 2 + at)
            {
                let label = raw[label_start..label_end].trim();
                if image {
                    if label.is_empty() {
                        rendered.push_str("An image.");
                    } else {
                        rendered.push_str("Image: ");
                        rendered.push_str(label);
                        rendered.push('.');
                    }
                } else {
                    rendered.push_str(label);
                }
                cursor = target_end + 1;
                continue;
            }
        }
        let character = raw[cursor..].chars().next().expect("cursor is in text");
        rendered.push(character);
        cursor += character.len_utf8();
    }
    rendered
}

fn speak_hashes(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut rendered = String::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor].is_ascii_hexdigit() && (cursor == 0 || !is_ascii_word(bytes[cursor - 1])) {
            let end = (cursor..bytes.len())
                .find(|index| !bytes[*index].is_ascii_hexdigit())
                .unwrap_or(bytes.len());
            if end - cursor >= 7 && (end == bytes.len() || !is_ascii_word(bytes[end])) {
                if !hash_has_label(&raw[..cursor]) {
                    rendered.push_str("commit ");
                }
                for (index, byte) in bytes[cursor..cursor + 7].iter().enumerate() {
                    if index > 0 {
                        rendered.push(' ');
                    }
                    rendered.push((*byte as char).to_ascii_lowercase());
                }
                cursor = end;
                continue;
            }
        }
        let character = raw[cursor..].chars().next().expect("cursor is in text");
        rendered.push(character);
        cursor += character.len_utf8();
    }
    rendered
}

fn hash_has_label(prefix: &str) -> bool {
    let context = prefix
        .chars()
        .rev()
        .take(16)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>()
        .to_ascii_lowercase();
    let trimmed = context.trim_end();
    ["commit", "sha"].into_iter().any(|label| {
        trimmed.strip_suffix(label).is_some_and(|before| {
            before
                .as_bytes()
                .last()
                .is_none_or(|byte| !is_ascii_word(*byte))
        })
    })
}

fn is_ascii_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn speak_utc_datetimes(raw: &str) -> String {
    let mut rendered = String::new();
    let mut cursor = 0;
    while cursor < raw.len() {
        if let Some((end, spoken)) = utc_datetime_at(raw, cursor) {
            rendered.push_str(&spoken);
            cursor = end;
            continue;
        }
        let character = raw[cursor..].chars().next().expect("cursor is in text");
        rendered.push(character);
        cursor += character.len_utf8();
    }
    rendered
}

fn utc_datetime_at(raw: &str, start: usize) -> Option<(usize, String)> {
    let bytes = raw.as_bytes();
    if start > 0 && is_ascii_word(*bytes.get(start - 1)?) {
        return None;
    }
    if !has_utc_datetime_prefix(bytes, start) {
        return None;
    }
    let (end, second) = utc_datetime_suffix(raw, start + 16)?;
    if bytes.get(end).is_some_and(|byte| is_ascii_word(*byte)) {
        return None;
    }

    let year = raw.get(start..start + 4)?;
    let month = raw.get(start + 5..start + 7)?.parse::<usize>().ok()?;
    let day = raw.get(start + 8..start + 10)?.parse::<u8>().ok()?;
    let hour = raw.get(start + 11..start + 13)?.parse::<u8>().ok()?;
    let minute = raw.get(start + 14..start + 16)?;
    let month = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ]
    .get(month.checked_sub(1)?)?;
    let hour12 = hour % 12;
    let hour12 = if hour12 == 0 { 12 } else { hour12 };
    let seconds = second.map_or_else(String::new, |value| format!(":{value}"));
    let meridiem = if hour < 12 { "AM" } else { "PM" };
    Some((
        end,
        format!("{month} {day}, {year} at {hour12}:{minute}{seconds} {meridiem} UTC"),
    ))
}

fn has_utc_datetime_prefix(bytes: &[u8], start: usize) -> bool {
    let digits = |range: std::ops::Range<usize>| {
        bytes
            .get(range)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
    };
    digits(start..start + 4)
        && bytes.get(start + 4) == Some(&b'-')
        && digits(start + 5..start + 7)
        && bytes.get(start + 7) == Some(&b'-')
        && digits(start + 8..start + 10)
        && matches!(bytes.get(start + 10), Some(b' ' | b'T'))
        && digits(start + 11..start + 13)
        && bytes.get(start + 13) == Some(&b':')
        && digits(start + 14..start + 16)
}

fn utc_datetime_suffix(raw: &str, mut end: usize) -> Option<(usize, Option<&str>)> {
    let bytes = raw.as_bytes();
    let mut second = None;
    if bytes.get(end) == Some(&b':') {
        let candidate = bytes.get(end + 1..end + 3)?;
        if !candidate.iter().all(u8::is_ascii_digit) {
            return None;
        }
        second = raw.get(end + 1..end + 3);
        end += 3;
        if bytes.get(end) == Some(&b'.') {
            let fraction_start = end + 1;
            end = fraction_start;
            while end < bytes.len() && bytes[end].is_ascii_digit() && end - fraction_start < 6 {
                end += 1;
            }
            if end == fraction_start || bytes.get(end).is_some_and(u8::is_ascii_digit) {
                return None;
            }
        }
    }
    if bytes.get(end) == Some(&b'Z') {
        Some((end + 1, second))
    } else if bytes.get(end..end + 6) == Some(b"+00:00") {
        Some((end + 6, second))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;

    use crate::resident::message_surface::{PresentationDirective, PresentationDirectives};

    fn lane() -> wire::CanonicalUuid {
        "018f82d4-6f59-7d06-92e3-bce9131163e5"
            .parse()
            .expect("fixture lane")
    }

    fn published() -> wire::Instant {
        "2030-04-12T18:44:09.001Z"
            .parse()
            .expect("fixture publication instant")
    }

    fn event() -> PresentationEvent {
        PresentationEvent {
            lane: lane(),
            source: None,
            locus: listen::Locus {
                member: 1,
                offset: 2,
                length: 3,
            },
            ordinal: 0,
        }
    }

    fn prose(text: &str, phase: Option<&str>) -> listen::Prose {
        listen::Prose {
            role: listen::ProseRole::Assistant,
            text: text.to_owned(),
            phase: phase.map(str::to_owned),
            media: Vec::new(),
            locus: event().locus,
        }
    }

    fn directives(keys: &[&str]) -> PresentationDirectives {
        PresentationDirectives(BTreeMap::from_iter(keys.iter().map(|key| {
            let text = format!("direction for {key}");
            let quote_target = wire::QuoteTarget::Steering {
                key: (*key).to_owned(),
                digest: wire::QuoteTarget::text_digest(&text),
            };
            (
                key.parse().expect("fixture steering key"),
                PresentationDirective {
                    actor: wire::Actor::Operator,
                    published: published(),
                    lane: lane(),
                    text,
                    authored: true,
                    taste: false,
                    review: false,
                    disposition: Disposition::Live,
                    quote_target,
                    retirement: None,
                },
            )
        })))
    }

    #[test]
    fn system_acknowledgment_is_not_speech_evidence() {
        let key = "dRyzkGX8b";
        let mut directives = directives(&[key]);
        directives
            .0
            .get_mut(&key.parse().expect("current system key parses"))
            .expect("fixture directive exists")
            .authored = false;

        assert_eq!(
            classify(
                &prose("ACK dRyzkGX8b: Accepted revision 44.", None),
                Some(&directives),
                event(),
            ),
            None,
        );
    }

    #[test]
    fn final_edges_drop_middle_paragraphs_and_clean_markdown() {
        assert_eq!(
            prepared_edges(concat!(
                "Opened [the change](https://example.test).\n\n",
                "middle detail\n\n",
                "Landed 7A12bc99 and ![proof](proof.png).",
            )),
            Some("Opened the change. Landed commit 7 a 1 2 b c 9 and Image: proof..".to_owned()),
        );
    }

    #[test]
    fn task_directives_and_empty_image_alt_follow_reference_cleanup() {
        assert_eq!(
            prepared_all("TASK title=hidden | project=app.test\nVisible ![](proof.png)"),
            Some("Visible An image.".to_owned()),
        );
    }

    #[test]
    fn utc_dates_whitespace_boundaries_and_hash_context_follow_reference_cleanup() {
        assert_eq!(
            prepared_edges(concat!(
                "Started 2026-09-02T00:04:05.123456Z.\n \t\n",
                "middle paragraph\n\n",
                "Finished 2026-09-02 13:14+00:00 at sha 7A12bc99; keep x_7a12bc99 literal.",
            )),
            Some(
                concat!(
                    "Started September 2, 2026 at 12:04:05 AM UTC. ",
                    "Finished September 2, 2026 at 1:14 PM UTC at sha 7 a 1 2 b c 9; ",
                    "keep x_7a12bc99 literal.",
                )
                .to_owned()
            ),
        );
    }

    #[test]
    fn final_answer_speaks_edges_after_removing_an_ack_header() {
        let speech = classify(
            &prose(
                "ACK 1kJK6N3R: first paragraph\n\nmiddle paragraph\n\nlast paragraph",
                Some("final_answer"),
            ),
            None,
            event(),
        )
        .expect("final answer is eligible");
        assert_eq!(speech.kind, AutomaticSpeechKind::Final);
        assert_eq!(speech.text, "first paragraph last paragraph");
    }

    #[test]
    fn speak_selects_only_positive_canonical_acknowledgement_bodies() {
        let speech = classify(
            &prose(
                concat!(
                    "ordinary preamble\n\n",
                    "ACK 1kJK6N3R: applied [first](https://example.test)\n\n",
                    "NAK 1kJK7Sqy: unsafe\n\n",
                    "ACK 1kJK8Abc: applied last",
                ),
                None,
            ),
            Some(&directives(&["1kJK6N3R", "1kJK7Sqy", "1kJK8Abc"])),
            event(),
        )
        .expect("positive acknowledgements are eligible");
        assert_eq!(speech.kind, AutomaticSpeechKind::Acknowledgement);
        assert_eq!(speech.text, "applied first applied last");
    }

    #[test]
    fn commentary_malformed_ack_and_nak_only_prose_stay_silent() {
        let known = directives(&["1kJK6N3R"]);
        for text in [
            "ordinary progress",
            "ACK not-a-key: discussed",
            "NAK 1kJK6N3R: unsafe",
        ] {
            assert_eq!(classify(&prose(text, None), Some(&known), event()), None);
        }
    }
}
