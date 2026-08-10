//! One structural classification for transcript prose consumers.

use serde::Serialize;

/// The one shape assigned to a transcript message from its text alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MessageShape {
    /// Prose authored as part of the conversation.
    AuthoredAsk,
    /// Harness or skill bootstrap material rather than an authored ask.
    Scaffolding,
    /// A post-compaction summary used to reconstruct earlier intent.
    CompactionRehydration,
    /// A machine envelope, patch, diff, or tool result.
    ToolGeneratedEnvelope,
}

impl MessageShape {
    pub(crate) const fn is_authored(self) -> bool {
        matches!(self, Self::AuthoredAsk)
    }
}

/// One classification plus the exact authored prefix consumers may inspect.
///
/// Mixed assistant text is clipped at the first structural tool-output
/// boundary. Operator-role machine messages begin at that boundary and thus
/// carry no authored prefix at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MessageClassification<'a> {
    pub(crate) shape: MessageShape,
    pub(crate) authored: Option<&'a str>,
}

/// Classify one message using only its own text.
pub(crate) fn classify(text: &str) -> MessageClassification<'_> {
    let trimmed = text.trim();
    if complete_machine_envelope(trimmed) {
        return generated();
    }
    if is_rehydration(trimmed) {
        return MessageClassification {
            shape: MessageShape::CompactionRehydration,
            authored: None,
        };
    }
    if is_scaffolding(trimmed) {
        return MessageClassification {
            shape: MessageShape::Scaffolding,
            authored: None,
        };
    }
    if let Some(boundary) = generated_boundary(text) {
        let authored = text[..boundary].trim_end();
        if authored.is_empty() {
            return generated();
        }
        return MessageClassification {
            shape: MessageShape::AuthoredAsk,
            authored: Some(authored),
        };
    }
    MessageClassification {
        shape: MessageShape::AuthoredAsk,
        authored: Some(text),
    }
}

fn generated() -> MessageClassification<'static> {
    MessageClassification {
        shape: MessageShape::ToolGeneratedEnvelope,
        authored: None,
    }
}

fn complete_machine_envelope(text: &str) -> bool {
    let Some(open_end) = text.find('>') else {
        return false;
    };
    if !text.starts_with('<') || text.starts_with("</") {
        return false;
    }
    let opening = &text[1..open_end];
    if opening.trim_end().ends_with('/') {
        return open_end + 1 == text.len() && machine_tag(opening.trim_end_matches('/').trim());
    }
    let Some(tag) = opening.split_ascii_whitespace().next() else {
        return false;
    };
    machine_tag(tag) && text.ends_with(&format!("</{tag}>"))
}

fn machine_tag(opening: &str) -> bool {
    let Some(tag) = opening.split_ascii_whitespace().next() else {
        return false;
    };
    let mut bytes = tag.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn is_rehydration(text: &str) -> bool {
    intent_section(text).is_some() || {
        let lower = text.to_ascii_lowercase();
        lower.contains("session is being continued from a previous conversation")
            && (lower.contains("summary") || lower.contains("context"))
    }
}

fn is_scaffolding(text: &str) -> bool {
    text.lines().any(is_skill_link) || bracketed_notice(text) || harness_retry(text)
}

fn is_skill_link(line: &str) -> bool {
    let line = line.trim();
    let Some(rest) = line.strip_prefix("[$") else {
        return false;
    };
    let Some((label, target)) = rest.split_once("](") else {
        return false;
    };
    let Some(target) = target.strip_suffix(')') else {
        return false;
    };
    !label.is_empty()
        && !target.is_empty()
        && !target.chars().any(char::is_whitespace)
        && target.ends_with("SKILL.md")
}

fn bracketed_notice(text: &str) -> bool {
    text.starts_with('[') && text.ends_with(']') && !text.contains('\n')
}

fn harness_retry(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let words = lower
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    words.contains(&"tool")
        && words.contains(&"call")
        && words
            .iter()
            .any(|word| matches!(*word, "malformed" | "parse" | "parsed"))
        && words.iter().any(|word| matches!(*word, "retry" | "again"))
}

fn generated_boundary(text: &str) -> Option<usize> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let stripped = line.trim();
        if generated_line(stripped) || complete_machine_envelope(text[offset..].trim()) {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

fn generated_line(line: &str) -> bool {
    line.eq_ignore_ascii_case("apply patch")
        || line.starts_with("*** Begin Patch")
        || line.starts_with("patch:")
        || line.starts_with("diff --git ")
        || line.starts_with("index ")
        || line.starts_with("--- a/")
        || line.starts_with("+++ b/")
        || line.starts_with("@@ ")
        || line.starts_with("tool output:")
        || line.starts_with("tool_result:")
        || line.starts_with("Process exited with code ")
        || line.starts_with("Command exited with code ")
}

/// Recover the primary intent section or return a visible unrecoverable marker.
pub(crate) fn rehydration_intent(text: &str) -> String {
    let Some((heading, inline)) = intent_section(text) else {
        return UNRECOVERABLE_REHYDRATION_INTENT.to_owned();
    };
    let body = text[heading..]
        .lines()
        .take_while(|line| numbered_heading(line).is_none())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let intent = [inline, body.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    if intent.is_empty() {
        UNRECOVERABLE_REHYDRATION_INTENT.to_owned()
    } else {
        intent
    }
}

pub(crate) const UNRECOVERABLE_REHYDRATION_INTENT: &str = "[unparseable compaction-summary intent]";

fn intent_section(text: &str) -> Option<(usize, &str)> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if let Some((heading, inline)) = numbered_heading(line)
            && heading.eq_ignore_ascii_case("Primary Request and Intent")
        {
            return Some((offset + line.len(), inline));
        }
        offset += line.len();
    }
    None
}

fn numbered_heading(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || !matches!(line.as_bytes().get(digits), Some(b'.' | b')')) {
        return None;
    }
    let body = line[digits + 1..].trim().trim_matches('*');
    let (heading, inline) = body.split_once(':').unwrap_or((body, ""));
    Some((heading.trim().trim_matches('*'), inline.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_structural_shapes_are_exclusive_and_unknown_envelopes_are_generated() {
        let rows = [
            ("Please finish the task.", MessageShape::AuthoredAsk),
            (
                "[$spice](.agents/skills/spice/SKILL.md)",
                MessageShape::Scaffolding,
            ),
            (
                "1. Primary Request and Intent\nKeep going.\n2. Work Completed\nNone.",
                MessageShape::CompactionRehydration,
            ),
            (
                "<future-harness-block>generated</future-harness-block>",
                MessageShape::ToolGeneratedEnvelope,
            ),
        ];
        for (text, expected) in rows {
            assert_eq!(classify(text).shape, expected);
        }
    }

    #[test]
    fn malformed_envelopes_remain_authored_but_complete_unknown_tags_do_not() {
        assert_eq!(
            classify("<future-harness-block>generated").shape,
            MessageShape::AuthoredAsk
        );
        assert_eq!(
            classify("<future-harness-block>x</different-block>").shape,
            MessageShape::AuthoredAsk
        );
    }

    #[test]
    fn generated_diff_body_is_clipped_from_an_authored_prefix() {
        let text = "I made the focused repair.\ndiff --git a/a.rs b/a.rs\n@@ -1 +1 @@\n-old\n+new";
        let classified = classify(text);
        assert_eq!(classified.shape, MessageShape::AuthoredAsk);
        assert_eq!(classified.authored, Some("I made the focused repair."));
        assert_eq!(
            classify("*** Begin Patch\n*** End Patch").shape,
            MessageShape::ToolGeneratedEnvelope
        );
    }

    #[test]
    fn missing_rehydration_intent_is_an_explicit_marker() {
        assert_eq!(
            rehydration_intent(
                "This session is being continued from a previous conversation summary"
            ),
            UNRECOVERABLE_REHYDRATION_INTENT
        );
        assert_eq!(
            rehydration_intent(
                "1. Primary Request and Intent\n- preserve the ask\n2. Work Completed\n- none"
            ),
            "- preserve the ask"
        );
    }
}
