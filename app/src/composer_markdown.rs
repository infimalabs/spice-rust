//! Target-neutral Markdown composition and rendering for browser messages.
//!
//! The module owning the forms and cards is wasm-only, so every Markdown
//! operation that does not need a DOM lives here and is reachable from an
//! ordinary native test.
//! Nothing here may be duplicated there: a second copy is what let the shipped
//! quote drift from the tested one until the copies disagreed on blank input.

use pulldown_cmark::{CowStr, Event as MarkdownEvent, Options, Parser, Tag, TagEnd};
use wire::Media;

use crate::watchband;

pub(crate) struct RenderedMarkdown {
    pub(crate) html: String,
    pub(crate) image_only: bool,
}

/// Render received text through the one sanitized Markdown path used by cards.
pub(crate) fn render_received(markdown: &str) -> RenderedMarkdown {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM;
    let events = Parser::new_ext(markdown, options)
        .map(sanitize_markdown_event)
        .collect::<Vec<_>>();
    let image_only = markdown_is_image_only(&events);
    let mut rendered_tasks = Vec::new();
    let mut code_block = false;
    let events = decorate_markdown_images(events, image_only)
        .into_iter()
        .flat_map(|event| {
            match &event {
                MarkdownEvent::Start(Tag::CodeBlock(_)) => code_block = true,
                MarkdownEvent::End(TagEnd::CodeBlock) => code_block = false,
                _ => {}
            }
            if code_block {
                vec![event]
            } else {
                inline_task_chips(event, &mut rendered_tasks)
            }
        });
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events);
    RenderedMarkdown { html, image_only }
}

fn markdown_is_image_only(events: &[MarkdownEvent<'_>]) -> bool {
    let mut image_depth = 0usize;
    let mut images = 0usize;
    for event in events {
        match event {
            MarkdownEvent::Start(Tag::Image { .. }) => {
                image_depth += 1;
                images += 1;
            }
            MarkdownEvent::End(TagEnd::Image) => image_depth = image_depth.saturating_sub(1),
            _ if image_depth > 0 => {}
            MarkdownEvent::Start(Tag::Paragraph)
            | MarkdownEvent::End(TagEnd::Paragraph)
            | MarkdownEvent::Start(Tag::Link { .. })
            | MarkdownEvent::End(TagEnd::Link)
            | MarkdownEvent::SoftBreak
            | MarkdownEvent::HardBreak => {}
            MarkdownEvent::Text(text) if text.trim().is_empty() => {}
            _ => return false,
        }
    }
    images > 0
}

fn decorate_markdown_images(
    events: Vec<MarkdownEvent<'_>>,
    image_only: bool,
) -> Vec<MarkdownEvent<'_>> {
    if image_only {
        return decorate_image_only_markdown(events);
    }
    let mut decorated = Vec::with_capacity(events.len());
    let mut index = 0usize;
    let mut image_stack = false;
    while index < events.len() {
        match &events[index] {
            MarkdownEvent::Start(Tag::Paragraph) => {
                image_stack = paragraph_is_image_stack(&events[index + 1..]);
                if image_stack {
                    decorated.push(MarkdownEvent::InlineHtml(
                        "<p class=\"message-image-stack\">".into(),
                    ));
                } else {
                    decorated.push(events[index].clone());
                }
            }
            MarkdownEvent::End(TagEnd::Paragraph) if image_stack => {
                decorated.push(MarkdownEvent::InlineHtml("</p>".into()));
                image_stack = false;
            }
            MarkdownEvent::Start(Tag::Link { .. })
                if linked_image_end(&events, index).is_some() =>
            {
                let image_index = index + 1;
                let image_end = push_decorated_image(&events, image_index, &mut decorated);
                index = image_end + 1;
            }
            MarkdownEvent::Start(Tag::Image { .. }) => {
                index = push_decorated_image(&events, index, &mut decorated);
            }
            _ => decorated.push(events[index].clone()),
        }
        index += 1;
    }
    decorated
}

fn decorate_image_only_markdown(events: Vec<MarkdownEvent<'_>>) -> Vec<MarkdownEvent<'_>> {
    let mut decorated = Vec::with_capacity(events.len());
    decorated.push(MarkdownEvent::InlineHtml(
        "<p class=\"message-image-stack\">".into(),
    ));
    let mut index = 0usize;
    while index < events.len() {
        match &events[index] {
            MarkdownEvent::Start(Tag::Image { .. }) => {
                index = push_decorated_image(&events, index, &mut decorated);
            }
            MarkdownEvent::Start(Tag::Paragraph)
            | MarkdownEvent::End(TagEnd::Paragraph)
            | MarkdownEvent::Start(Tag::Link { .. })
            | MarkdownEvent::End(TagEnd::Link)
            | MarkdownEvent::SoftBreak
            | MarkdownEvent::HardBreak => {}
            MarkdownEvent::Text(text) if text.trim().is_empty() => {}
            _ => debug_assert!(
                false,
                "image-only classification admitted a non-image event"
            ),
        }
        index += 1;
    }
    decorated.push(MarkdownEvent::InlineHtml("</p>".into()));
    decorated
}

fn linked_image_end(events: &[MarkdownEvent<'_>], index: usize) -> Option<usize> {
    if !matches!(
        events.get(index),
        Some(MarkdownEvent::Start(Tag::Link { .. }))
    ) || !matches!(
        events.get(index + 1),
        Some(MarkdownEvent::Start(Tag::Image { .. }))
    ) {
        return None;
    }
    let image_end = events[index + 2..]
        .iter()
        .position(|event| matches!(event, MarkdownEvent::End(TagEnd::Image)))?
        + index
        + 2;
    matches!(
        events.get(image_end + 1),
        Some(MarkdownEvent::End(TagEnd::Link))
    )
    .then_some(image_end + 1)
}

fn push_decorated_image<'a>(
    events: &[MarkdownEvent<'a>],
    index: usize,
    decorated: &mut Vec<MarkdownEvent<'a>>,
) -> usize {
    let MarkdownEvent::Start(Tag::Image {
        dest_url, title, ..
    }) = &events[index]
    else {
        return index;
    };
    let end = events[index + 1..]
        .iter()
        .position(|event| matches!(event, MarkdownEvent::End(TagEnd::Image)))
        .map_or(events.len(), |offset| index + offset + 1);
    let alt = image_alt_text(&events[index + 1..end]);
    decorated.push(MarkdownEvent::InlineHtml(
        image_preview_html(dest_url, title, &alt).into(),
    ));
    end
}

fn paragraph_is_image_stack(events: &[MarkdownEvent<'_>]) -> bool {
    let mut image_depth = 0usize;
    let mut images = 0usize;
    for event in events {
        match event {
            MarkdownEvent::End(TagEnd::Paragraph) if image_depth == 0 => return images > 0,
            MarkdownEvent::Start(Tag::Image { .. }) => {
                image_depth += 1;
                images += 1;
            }
            MarkdownEvent::End(TagEnd::Image) => image_depth = image_depth.saturating_sub(1),
            _ if image_depth > 0 => {}
            MarkdownEvent::SoftBreak | MarkdownEvent::HardBreak => {}
            MarkdownEvent::Text(text) if text.trim().is_empty() => {}
            _ => return false,
        }
    }
    false
}

fn image_alt_text(events: &[MarkdownEvent<'_>]) -> String {
    let mut alt = String::new();
    for event in events {
        match event {
            MarkdownEvent::Text(text) | MarkdownEvent::Code(text) => alt.push_str(text),
            MarkdownEvent::SoftBreak | MarkdownEvent::HardBreak => alt.push(' '),
            _ => {}
        }
    }
    alt
}

fn image_preview_html(source: &str, authored_title: &str, alt: &str) -> String {
    let source = escape_html_attribute(source);
    let alt = escape_html_attribute(alt);
    let label = if alt.is_empty() { "image" } else { &alt };
    let action = escape_html_attribute(&format!("Open {label} full size"));
    let image_title = if authored_title.is_empty() {
        String::new()
    } else {
        format!(" title=\"{}\"", escape_html_attribute(authored_title))
    };
    let openable_data_image = data_image_is_openable(&source);
    let malformed_data_image = data_image_has_malformed_base64(&source);
    let image = if source.is_empty() || malformed_data_image {
        format!("<img alt=\"{alt}\"{image_title}>")
    } else {
        format!("<img src=\"{source}\" alt=\"{alt}\"{image_title}>")
    };
    if openable_data_image {
        format!(
            "<a class=\"message-image message-image--openable\" href=\"about:blank\" \
             target=\"_blank\" rel=\"noopener\" title=\"{action}\" aria-label=\"{action}\" \
             data-full-size-data-image=\"true\">{image}</a>"
        )
    } else if !source.is_empty() && !is_data_image_source(&source) {
        format!(
            "<a class=\"message-image message-image--openable\" href=\"{source}\" \
             target=\"_blank\" rel=\"noopener\" title=\"{action}\" \
             aria-label=\"{action}\">{image}</a>"
        )
    } else {
        format!("<span class=\"message-image\" title=\"Image preview\">{image}</span>")
    }
}

fn data_image_has_malformed_base64(source: &str) -> bool {
    source.split_once(',').is_some_and(|(metadata, payload)| {
        let metadata = metadata.to_ascii_lowercase();
        metadata.starts_with("data:image/")
            && metadata.ends_with(";base64")
            && !canonical_base64_payload(payload)
    })
}

fn data_image_is_openable(source: &str) -> bool {
    source.split_once(',').is_some_and(|(metadata, payload)| {
        matches!(
            metadata.to_ascii_lowercase().as_str(),
            "data:image/png;base64"
                | "data:image/jpeg;base64"
                | "data:image/gif;base64"
                | "data:image/webp;base64"
        ) && canonical_base64_payload(payload)
    })
}

fn is_data_image_source(source: &str) -> bool {
    source
        .split_once(',')
        .map_or(source, |(metadata, _)| metadata)
        .to_ascii_lowercase()
        .starts_with("data:image/")
}

fn canonical_base64_payload(payload: &str) -> bool {
    if payload.is_empty() || !payload.len().is_multiple_of(4) {
        return false;
    }
    let body = payload.trim_end_matches('=');
    let padding = payload.len() - body.len();
    padding <= 2
        && body
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
}

fn escape_html_attribute(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn inline_task_chips<'a>(
    event: MarkdownEvent<'a>,
    rendered: &mut Vec<wire::TaskHandle>,
) -> Vec<MarkdownEvent<'a>> {
    match event {
        MarkdownEvent::Text(text) => inline_task_text(text, rendered),
        MarkdownEvent::Code(code) => {
            let Some(task) = watchband::canonical_task_handle(&code) else {
                return vec![MarkdownEvent::Code(code)];
            };
            if rendered.contains(&task) {
                vec![MarkdownEvent::Code(code)]
            } else {
                rendered.push(task.clone());
                vec![MarkdownEvent::InlineHtml(task_chip_html(&task).into())]
            }
        }
        other => vec![other],
    }
}

fn inline_task_text<'a>(
    text: CowStr<'a>,
    rendered: &mut Vec<wire::TaskHandle>,
) -> Vec<MarkdownEvent<'a>> {
    let mentions = watchband::task_mentions(&text);
    if mentions.is_empty() {
        return vec![MarkdownEvent::Text(text)];
    }
    let mut events = Vec::new();
    let mut cursor = 0;
    for (range, task) in mentions {
        if rendered.contains(&task) {
            continue;
        }
        if cursor < range.start {
            events.push(MarkdownEvent::Text(
                text[cursor..range.start].to_owned().into(),
            ));
        }
        events.push(MarkdownEvent::InlineHtml(task_chip_html(&task).into()));
        rendered.push(task);
        cursor = range.end;
    }
    if cursor < text.len() {
        events.push(MarkdownEvent::Text(text[cursor..].to_owned().into()));
    }
    events
}

fn task_chip_html(task: &wire::TaskHandle) -> String {
    // TaskHandle parsing admits only the canonical ASCII handle grammar, so
    // these fixed attributes cannot acquire markup from authored prose.
    let handle = task.to_string();
    format!(
        "<span class=\"message-task-chip\" data-task-handle=\"{handle}\" \
         aria-label=\"Reported task {handle}\" title=\"{handle}\">{handle}</span>"
    )
}

fn sanitize_markdown_event(event: MarkdownEvent<'_>) -> MarkdownEvent<'_> {
    match event {
        MarkdownEvent::Start(tag) => MarkdownEvent::Start(sanitize_markdown_tag(tag)),
        MarkdownEvent::Html(raw) | MarkdownEvent::InlineHtml(raw) => MarkdownEvent::Text(raw),
        other => other,
    }
}

fn sanitize_markdown_tag(tag: Tag<'_>) -> Tag<'_> {
    match tag {
        Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        } => Tag::Link {
            link_type,
            dest_url: sanitize_markdown_url(dest_url, false),
            title,
            id,
        },
        Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        } => Tag::Image {
            link_type,
            dest_url: sanitize_markdown_url(dest_url, true),
            title,
            id,
        },
        other => other,
    }
}

fn sanitize_markdown_url(url: CowStr<'_>, image: bool) -> CowStr<'_> {
    let candidate = url.trim().to_ascii_lowercase();
    let safe_scheme = candidate.starts_with("https://")
        || candidate.starts_with("http://")
        || (!image && candidate.starts_with("mailto:"))
        || (image && candidate.starts_with("data:image/"));
    let relative = !candidate.starts_with("//")
        && !candidate
            .split(['/', '#', '?'])
            .next()
            .is_some_and(|head| head.contains(':'));
    if safe_scheme || relative {
        url
    } else {
        CowStr::Borrowed("")
    }
}

/// One quoted message and the reply the operator typed beneath it.
pub(crate) struct QuotedReply<'a> {
    /// Quoted text, prefixed line by line as one Markdown block quote.
    pub(crate) quote: &'a str,
    /// Reply body shown under the quote, blank when the operator wrote none.
    pub(crate) body: &'a str,
}

/// Join every visible quote shard, the primary body, and every attachment.
///
/// The quote shards precede the primary composer in the visible vertical stack,
/// so their document parts precede the primary body here as well. Attachments
/// belong to the assembled document and follow its text. Parts are separated by
/// the blank line Markdown reads as a block boundary, and a part that would
/// contribute nothing is dropped rather than emitted as an empty block.
pub(crate) fn compose(body: &str, quotes: &[QuotedReply<'_>], attachments: &[Media]) -> String {
    let mut parts = Vec::new();
    parts.extend(quotes.iter().filter_map(|quoted| {
        let quote = block_quote(quoted.quote);
        let body = quoted.body.trim();
        let part = [quote.as_str(), body]
            .into_iter()
            .filter(|part| !part.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        (!part.is_empty()).then_some(part)
    }));
    let body = body.trim();
    if !body.is_empty() {
        parts.push(body.to_owned());
    }
    parts.extend(attachments.iter().map(|attachment| {
        attachment
            .markdown()
            .expect("composer attachments passed admission before storage")
    }));
    parts.join("\n\n")
}

/// Normalize line endings, then prefix every line as one Markdown block quote.
///
/// The trim happens to the source and never to the joined result. Trimming the
/// result takes the space off the marker on a final blank line, leaving a bare
/// `>` on the last line, and it collapses whitespace-only text to a lone `>`
/// instead of to nothing, which is a directive the caller then has no reason to
/// suppress. Trimming the source empties that case, so callers can drop it.
pub(crate) fn block_quote(raw: &str) -> String {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed
        .lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Display the producer's preview or the explicit absent-preview label.
///
/// This is the module's runtime boundary: agent transcript cards deliberately
/// pass absence instead of reparsing authored prose in the browser.
pub(crate) fn quote_label(preview: Option<&str>) -> &str {
    preview.unwrap_or("quoted message")
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPENABLE_PNG: &str = "data:image/png;base64,iVBORw0KGgo=";

    fn quoted<'a>(values: &'a [(&'a str, &'a str)]) -> Vec<QuotedReply<'a>> {
        values
            .iter()
            .map(|(quote, body)| QuotedReply { quote, body })
            .collect()
    }

    #[test]
    fn composed_markdown_follows_visible_shard_order_and_drops_blank_parts() {
        assert_eq!(compose("  body only  ", &[], &[]), "body only");
        assert_eq!(
            compose(
                "",
                &quoted(&[("first quote", ""), ("  ", ""), ("second quote", "")]),
                &[]
            ),
            "> first quote\n\n> second quote"
        );
        assert_eq!(
            compose("body", &quoted(&[("quoted line", "")]), &[]),
            "> quoted line\n\nbody"
        );
    }

    #[test]
    fn a_reply_follows_the_quote_it_answers_inside_one_part() {
        assert_eq!(
            compose("", &quoted(&[("quoted line", "  my reply  ")]), &[]),
            "> quoted line\n\nmy reply"
        );
    }

    #[test]
    fn attachment_names_round_trip_through_canonical_media_markdown() {
        let attachment = Media::from_base64(r"one](two)\\雪.png", "image/png", "iVBORw0KGgo=")
            .expect("fixture attachment admits");
        assert_eq!(
            compose("", &[], std::slice::from_ref(&attachment)),
            attachment.markdown().expect("fixture Markdown renders"),
        );
    }

    #[test]
    fn block_quotes_normalize_every_line_ending_before_prefixing() {
        assert_eq!(block_quote("first\r\nsecond"), "> first\n> second");
        assert_eq!(block_quote("first\rsecond"), "> first\n> second");
        assert_eq!(block_quote("first\n\nthird"), "> first\n> \n> third");
    }

    /// Quoted message text routinely ends on a blank line, and the blank line is
    /// the one the marker can lose. The copy that trimmed the joined result left
    /// `>` alone on the last line: a bare marker, not the `> ` every line of a
    /// block quote carries. Asserting the whole string and the per-line marker
    /// together states both the exact output and the invariant behind it.
    #[test]
    fn a_quote_that_ends_on_a_blank_line_keeps_the_marker_on_every_line() {
        let quote = block_quote("first\n\n");

        assert_eq!(quote, "> first");
        assert!(
            quote.lines().all(|line| line.starts_with("> ")),
            "every line of a block quote carries the marker, got {quote:?}"
        );
    }

    /// The composer drops a quote that would contribute nothing, and it can only
    /// drop what arrives empty. The copy that trimmed the joined result returned
    /// a lone `>` here, which is not empty, so the drag handler sent a directive
    /// whose entire body was a marker.
    #[test]
    fn whitespace_only_quoted_text_composes_to_nothing_at_all() {
        assert_eq!(block_quote("   \n\t\n  "), "");
        assert_eq!(block_quote(""), "");
        assert_eq!(compose("", &quoted(&[("   \n  ", "")]), &[]), "");
    }

    #[test]
    fn absent_preview_has_one_documented_label() {
        assert_eq!(quote_label(Some("author words")), "author words");
        assert_eq!(quote_label(None), "quoted message");
    }

    #[test]
    fn every_image_only_markdown_shape_renders_one_card_wide_stack() {
        let image = |name: &str| format!("![{name}]({OPENABLE_PNG})");
        let shapes = [
            [image("one"), image("two"), image("three")].join(" "),
            [image("one"), image("two"), image("three")].join("\n"),
            [image("one"), image("two"), image("three")].join("  \n"),
            [image("one"), image("two"), image("three")].join("\n\n"),
        ];

        for markdown in shapes {
            let rendered = render_received(&markdown);
            assert!(
                rendered.image_only,
                "shape must receive the large tier: {markdown:?}"
            );
            assert_eq!(
                rendered
                    .html
                    .matches("class=\"message-image-stack\"")
                    .count(),
                1,
                "all image blocks must share one stack: {}",
                rendered.html,
            );
            assert_eq!(
                rendered
                    .html
                    .matches("class=\"message-image message-image--openable\"")
                    .count(),
                3,
                "every preview must remain present: {}",
                rendered.html,
            );
            assert_eq!(rendered.html.matches("target=\"_blank\"").count(), 3);
            assert_eq!(rendered.html.matches("rel=\"noopener\"").count(), 3);
        }
    }

    #[test]
    fn unsupported_image_only_sources_share_the_bounded_stack_without_an_action() {
        let rendered = render_received(
            "![remote](https://example.test/image.png)\n\n\
             ![unsupported](data:image/svg+xml;base64,PHN2Zz4=)\n\n\
             ![refused](javascript:alert(1))",
        );

        assert!(rendered.image_only);
        assert_eq!(
            rendered
                .html
                .matches("class=\"message-image-stack\"")
                .count(),
            1
        );
        assert_eq!(rendered.html.matches("<a class=\"message-image").count(), 1);
        assert_eq!(
            rendered
                .html
                .matches("<span class=\"message-image\"")
                .count(),
            2
        );
        assert!(!rendered.html.contains("javascript:"));
    }

    #[test]
    fn linked_images_keep_one_full_size_action_without_nested_anchors() {
        let image_only = render_received(
            "[![one](https://images.example.test/one.png)](https://links.example.test/one)\n\n\
             [![two](https://images.example.test/two.png)](https://links.example.test/two)",
        );

        assert!(image_only.image_only);
        assert_eq!(
            image_only
                .html
                .matches("class=\"message-image-stack\"")
                .count(),
            1
        );
        assert_eq!(image_only.html.matches("<a ").count(), 2);
        assert!(
            image_only
                .html
                .contains("href=\"https://images.example.test/one.png\"")
        );
        assert!(
            image_only
                .html
                .contains("href=\"https://images.example.test/two.png\"")
        );
        assert!(!image_only.html.contains("links.example.test"));

        let mixed = render_received(
            "See [![preview](https://images.example.test/mixed.png)](https://links.example.test/mixed) now.",
        );
        assert!(!mixed.image_only);
        assert_eq!(mixed.html.matches("<a ").count(), 1);
        assert!(
            mixed
                .html
                .contains("href=\"https://images.example.test/mixed.png\"")
        );
        assert!(!mixed.html.contains("links.example.test"));
    }

    #[test]
    fn prose_and_images_keep_the_ordinary_rendering_path() {
        let rendered =
            render_received("ordinary prose\n\n![preview](https://example.test/image.png)");

        assert!(!rendered.image_only);
        assert!(rendered.html.starts_with("<p>ordinary prose</p>\n"));
        assert_eq!(
            rendered
                .html
                .matches("class=\"message-image-stack\"")
                .count(),
            1
        );
        assert_eq!(rendered.html.matches("target=\"_blank\"").count(), 1);
        assert_eq!(rendered.html.matches("rel=\"noopener\"").count(), 1);
    }
}
