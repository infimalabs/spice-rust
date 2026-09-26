//! Structural normalization for authenticated taste paragraphs inside quotes.

use std::borrow::Cow;
use std::ops::Range;

use pulldown_cmark::{Event as MarkdownEvent, Options, Parser, Tag, TagEnd};
use wire::{MessageClassification, MessageClassificationKind};

/// Markdown plus exact classifications after structural quote normalization.
pub(super) struct Normalized<'a> {
    pub(super) markdown: Cow<'a, str>,
    pub(super) classifications: Vec<MessageClassification>,
}

/// Promote each authenticated taste paragraph to one quote level.
///
/// A quoted reply can carry an earlier taste block several levels deep. The
/// taste remains one semantic notice, while the reply still belongs to its
/// surrounding conversation. Rewriting only source prefixes and remapping the
/// resident's exact annotation offsets lets the Markdown parser close and
/// reopen those surrounding quotes without rediscovering classification from
/// prose.
pub(super) fn normalize<'a>(
    markdown: &'a str,
    classifications: &[MessageClassification],
    options: Options,
) -> Normalized<'a> {
    if !classifications
        .iter()
        .any(|classification| classification.kind == MessageClassificationKind::Taste)
    {
        return Normalized {
            markdown: Cow::Borrowed(markdown),
            classifications: classifications.to_vec(),
        };
    }
    let taste_offsets = classifications
        .iter()
        .filter(|classification| classification.kind == MessageClassificationKind::Taste)
        .map(|classification| classification.start as usize)
        .collect::<Vec<_>>();
    let mut quote_depth = 0usize;
    let mut code_block = false;
    let mut prefixes = Vec::new();

    let events = Parser::new_ext(markdown, options)
        .into_offset_iter()
        .collect::<Vec<_>>();
    let mut index = 0usize;
    while let Some((event, source)) = events.get(index) {
        match &event {
            MarkdownEvent::Start(Tag::CodeBlock(_)) => code_block = true,
            MarkdownEvent::End(TagEnd::CodeBlock) => code_block = false,
            MarkdownEvent::Start(Tag::BlockQuote(_)) => quote_depth += 1,
            MarkdownEvent::End(TagEnd::BlockQuote(_)) => {
                quote_depth = quote_depth.saturating_sub(1);
            }
            MarkdownEvent::Text(_) if !code_block && quote_depth > 1 => {
                let mut source = source.clone();
                while let Some((MarkdownEvent::Text(_), next_source)) = events.get(index + 1) {
                    source.end = next_source.end;
                    index += 1;
                }
                if taste_offsets
                    .iter()
                    .any(|offset| source.start <= *offset && *offset < source.end)
                    && let Some(prefix) = explicit_quote_prefix(markdown, source, quote_depth)
                {
                    prefixes.push(prefix);
                }
            }
            _ => {}
        }
        index += 1;
    }

    if prefixes.is_empty() {
        return Normalized {
            markdown: Cow::Borrowed(markdown),
            classifications: classifications.to_vec(),
        };
    }
    let mut normalized = String::with_capacity(markdown.len());
    let mut cursor = 0usize;
    for prefix in &prefixes {
        normalized.push_str(&markdown[cursor..prefix.start]);
        normalized.push_str("> ");
        cursor = prefix.end;
    }
    normalized.push_str(&markdown[cursor..]);
    let classifications = classifications
        .iter()
        .map(|classification| MessageClassification {
            start: remap_offset(classification.start, &prefixes),
            end: remap_offset(classification.end, &prefixes),
            kind: classification.kind,
        })
        .collect();
    Normalized {
        markdown: Cow::Owned(normalized),
        classifications,
    }
}

fn remap_offset(offset: u32, prefixes: &[Range<usize>]) -> u32 {
    let offset = offset as usize;
    let removed = prefixes
        .iter()
        .take_while(|prefix| prefix.end <= offset)
        .map(|prefix| prefix.len().saturating_sub(2))
        .sum::<usize>();
    u32::try_from(offset.saturating_sub(removed))
        .expect("a normalized classification offset began in the wire range")
}

fn explicit_quote_prefix(
    markdown: &str,
    source: Range<usize>,
    depth: usize,
) -> Option<Range<usize>> {
    let text_start = source.start + markdown[source].find("[TASTE]")?;
    let line_start = markdown[..text_start]
        .rfind('\n')
        .map_or(0, |newline| newline + 1);
    let prefix = &markdown[line_start..text_start];
    (quote_depth(prefix) == Some(depth)).then_some(line_start..text_start)
}

fn quote_depth(mut prefix: &str) -> Option<usize> {
    let mut depth = 0usize;
    loop {
        prefix = prefix.trim_start_matches([' ', '\t']);
        let Some(suffix) = prefix.strip_prefix('>') else {
            break;
        };
        depth += 1;
        prefix = suffix;
    }
    (depth > 0 && prefix.trim().is_empty()).then_some(depth)
}

#[cfg(test)]
mod tests {
    use super::super::render_received_with_tastes;

    fn quote_depth_at(html: &str, needle: &str) -> usize {
        let prefix = html
            .split_once(needle)
            .unwrap_or_else(|| panic!("rendered HTML omitted {needle:?}: {html}"))
            .0;
        prefix.matches("<blockquote>").count() - prefix.matches("</blockquote>").count()
    }

    #[test]
    fn nested_taste_is_one_card_while_quoted_reply_keeps_its_depth() {
        let rendered = render_received_with_tastes(
            "> > > [TASTE] compatibility: Keep one tested path.\n\
             > >\n\
             > > Understood inside the quoted reply.",
            1,
        );

        assert_eq!(quote_depth_at(&rendered.html, "message-taste-pill"), 1);
        assert_eq!(
            quote_depth_at(&rendered.html, "Understood inside the quoted reply."),
            2,
        );
        assert_eq!(
            rendered
                .html
                .matches("class=\"message-taste-pill\"")
                .count(),
            1,
        );
        assert!(rendered.html.contains("Keep one tested path."));
    }

    #[test]
    fn unnested_taste_keeps_the_canonical_card_shape() {
        let rendered = render_received_with_tastes(
            "> [TASTE] compatibility: Keep one tested path.\n\nUnderstood outside the notice.",
            1,
        );

        assert_eq!(quote_depth_at(&rendered.html, "message-taste-pill"), 1);
        assert_eq!(
            quote_depth_at(&rendered.html, "Understood outside the notice."),
            0,
        );
        assert!(rendered.html.starts_with("<blockquote>"));
        assert!(rendered.html.contains("Keep one tested path."));
    }
}
