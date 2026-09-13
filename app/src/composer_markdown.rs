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

mod taste_quotes;

pub(crate) struct RenderedMarkdown {
    pub(crate) html: String,
    pub(crate) image_only: bool,
}

/// Render received text through the one sanitized Markdown path used by cards.
#[cfg(test)]
pub(crate) fn render_received(markdown: &str) -> RenderedMarkdown {
    render_received_with_tastes(markdown, 0)
}

/// Prefix one composed document with the canonical broadcast marker.
pub(crate) fn mark_broadcast(markdown: &str) -> String {
    if has_marker_prefix(markdown, wire::stream::BROADCAST_MARKER) {
        markdown.to_owned()
    } else {
        format!("{} {markdown}", wire::stream::BROADCAST_MARKER)
    }
}

/// Render received text using resident-authenticated semantic classifications.
pub(crate) fn render_received_with_badges(
    markdown: &str,
    badges: wire::AgentMessageBadges,
) -> RenderedMarkdown {
    let mut rendered = render_received_with_classifications(
        markdown,
        badges.reviews,
        badges.tastes,
        badges.broadcasts,
    );
    if badges.acknowledgements > 0 {
        rendered.html = format!(
            "<span class=\"message-classifiers\">{}</span>{}",
            acknowledgement_pill(badges.acknowledgements),
            rendered.html,
        );
    }
    rendered
}

fn acknowledgement_pill(count: u32) -> String {
    format!(
        "<span class=\"message-ack-pill\" data-message-classification=\"ack\" \
         aria-label=\"Acknowledgement classification, {count} contexts\">ACK<span \
         class=\"message-badge-count\">{count}</span></span>"
    )
}

/// Render received text and replace authenticated quoted classification markers inline.
#[cfg(test)]
pub(crate) fn render_received_with_tastes(markdown: &str, tastes: u32) -> RenderedMarkdown {
    render_received_with_classifications(markdown, 0, tastes, 0)
}

fn render_received_with_classifications(
    markdown: &str,
    reviews: u32,
    tastes: u32,
    broadcasts: u32,
) -> RenderedMarkdown {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM;
    let markdown = taste_quotes::normalize(markdown, tastes, options);
    let events = Parser::new_ext(&markdown, options)
        .map(sanitize_markdown_event)
        .collect::<Vec<_>>();
    let events = coalesce_text_events(events);
    let image_only = markdown_is_image_only(&events);
    let mut code_block = false;
    let mut block_quote_depth = 0usize;
    let mut remaining_reviews = usize::try_from(reviews).unwrap_or(usize::MAX);
    let mut remaining_tastes = usize::try_from(tastes).unwrap_or(usize::MAX);
    let mut remaining_broadcasts = usize::try_from(broadcasts).unwrap_or(usize::MAX);
    let events = decorate_markdown_images(events, image_only)
        .into_iter()
        .flat_map(|event| {
            match &event {
                MarkdownEvent::Start(Tag::CodeBlock(_)) => code_block = true,
                MarkdownEvent::End(TagEnd::CodeBlock) => code_block = false,
                MarkdownEvent::Start(Tag::BlockQuote(_)) => block_quote_depth += 1,
                MarkdownEvent::End(TagEnd::BlockQuote(_)) => {
                    block_quote_depth = block_quote_depth.saturating_sub(1);
                }
                _ => {}
            }
            if code_block {
                vec![event]
            } else {
                let events = inline_classification_marker(
                    event,
                    (block_quote_depth > 0).then_some(&mut remaining_reviews),
                    (block_quote_depth > 0).then_some(&mut remaining_tastes),
                    (block_quote_depth > 0).then_some(&mut remaining_broadcasts),
                );
                events.into_iter().flat_map(inline_task_chips).collect()
            }
        });
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events);
    RenderedMarkdown { html, image_only }
}

fn coalesce_text_events(events: Vec<MarkdownEvent<'_>>) -> Vec<MarkdownEvent<'_>> {
    let mut coalesced = Vec::with_capacity(events.len());
    for event in events {
        if let (Some(MarkdownEvent::Text(previous)), MarkdownEvent::Text(next)) =
            (coalesced.last_mut(), &event)
        {
            let mut joined = previous.to_string();
            joined.push_str(next);
            *previous = joined.into();
        } else {
            coalesced.push(event);
        }
    }
    coalesced
}

fn inline_classification_marker<'a>(
    event: MarkdownEvent<'a>,
    remaining_reviews: Option<&mut usize>,
    remaining_tastes: Option<&mut usize>,
    remaining_broadcasts: Option<&mut usize>,
) -> Vec<MarkdownEvent<'a>> {
    let MarkdownEvent::Text(text) = event else {
        return vec![event];
    };
    if let Some(remaining) = remaining_reviews.filter(|remaining| **remaining > 0)
        && let Some(suffix) = marker_suffix(&text, "[REVIEW]")
    {
        *remaining -= 1;
        return vec![
            MarkdownEvent::InlineHtml(review_pill_html().into()),
            MarkdownEvent::Text(suffix.into()),
        ];
    }
    if let Some(remaining) = remaining_broadcasts.filter(|remaining| **remaining > 0)
        && let Some(suffix) = marker_suffix(&text, wire::stream::BROADCAST_MARKER)
    {
        *remaining -= 1;
        return vec![
            MarkdownEvent::InlineHtml(broadcast_pill_html().into()),
            MarkdownEvent::Text(suffix.into()),
        ];
    }
    let Some(remaining) = remaining_tastes.filter(|remaining| **remaining > 0) else {
        return vec![MarkdownEvent::Text(text)];
    };
    let Some(suffix) = marker_suffix(&text, "[TASTE]") else {
        return vec![MarkdownEvent::Text(text)];
    };
    *remaining -= 1;
    vec![
        MarkdownEvent::InlineHtml(taste_pill_html().into()),
        MarkdownEvent::Text(taste_guidance_suffix(&suffix).into()),
    ]
}

fn has_marker_prefix(text: &str, marker: &str) -> bool {
    text.strip_prefix(marker)
        .is_some_and(|suffix| suffix.starts_with(char::is_whitespace))
}

fn marker_suffix(text: &str, marker: &str) -> Option<String> {
    let suffix = text
        .strip_prefix(marker)
        .filter(|suffix| suffix.starts_with(char::is_whitespace))?;
    Some(suffix.to_owned())
}

fn taste_guidance_suffix(suffix: &str) -> String {
    let trimmed = suffix.trim_start();
    let Some((name, guidance)) = trimmed.split_once(':') else {
        return suffix.to_owned();
    };
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        return suffix.to_owned();
    }
    format!(" {}", guidance.trim_start())
}

fn taste_pill_html() -> &'static str {
    "<span class=\"message-taste-pill\" data-message-classification=\"taste\" \
     aria-label=\"Taste classification\">TASTE</span>"
}

fn broadcast_pill_html() -> &'static str {
    "<span class=\"message-broadcast-pill\" data-message-classification=\"broadcast\" \
     aria-label=\"Broadcast classification\">BROADCAST</span>"
}

fn review_pill_html() -> &'static str {
    "<span class=\"message-review-pill\" data-message-classification=\"review\" \
     aria-label=\"Review classification\">REVIEW</span>"
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
    let (state, status) = if source.is_empty() || malformed_data_image {
        ("error", "Image unavailable")
    } else {
        ("pending", "Loading image")
    };
    let image = if source.is_empty() || malformed_data_image {
        format!("<img alt=\"{alt}\"{image_title} data-mosaic-image-state=\"{state}\">")
    } else {
        format!(
            "<img src=\"{source}\" alt=\"{alt}\"{image_title} \
             data-mosaic-image-state=\"{state}\">"
        )
    };
    let preview = format!(
        "{image}<span class=\"message-image-status\" role=\"status\" \
         data-mosaic-image-status=\"{state}\">{status}</span>"
    );
    if openable_data_image {
        format!(
            "<a class=\"message-image message-image--openable\" href=\"{source}\" \
             target=\"_blank\" rel=\"noopener\" title=\"{action}\" aria-label=\"{action}\" \
             data-full-size-data-image=\"true\">{preview}</a>"
        )
    } else if !source.is_empty() && !is_data_image_source(&source) {
        format!(
            "<a class=\"message-image message-image--openable\" href=\"{source}\" \
             target=\"_blank\" rel=\"noopener\" title=\"{action}\" \
             aria-label=\"{action}\">{preview}</a>"
        )
    } else {
        format!("<span class=\"message-image\" title=\"Image preview\">{preview}</span>")
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

fn inline_task_chips(event: MarkdownEvent<'_>) -> Vec<MarkdownEvent<'_>> {
    match event {
        MarkdownEvent::Text(text) => inline_task_text(text),
        MarkdownEvent::Code(code) => {
            let Some(task) = watchband::canonical_task_handle(&code) else {
                return vec![MarkdownEvent::Code(code)];
            };
            vec![MarkdownEvent::InlineHtml(task_chip_html(&task).into())]
        }
        other => vec![other],
    }
}

fn inline_task_text(text: CowStr<'_>) -> Vec<MarkdownEvent<'_>> {
    let mentions = watchband::task_mentions(&text);
    if mentions.is_empty() {
        return vec![MarkdownEvent::Text(text)];
    }
    let mut events = Vec::new();
    let mut cursor = 0;
    for (range, task) in mentions {
        if cursor < range.start {
            events.push(MarkdownEvent::Text(
                text[cursor..range.start].to_owned().into(),
            ));
        }
        events.push(MarkdownEvent::InlineHtml(task_chip_html(&task).into()));
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
         aria-label=\"Reported task {handle}\" title=\"{handle}\">\
         <span class=\"message-task-chip-label\">{handle}</span></span>"
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
    /// Attachments pasted into this quoted reply composer.
    pub(crate) attachments: &'a [Media],
}

/// Join every visible quote shard, the primary body, and their attachments.
///
/// The quote shards precede the primary composer in the visible vertical stack,
/// so their document parts precede the primary body here as well. Each
/// attachment follows the text block whose composer received it. Parts are
/// separated by the blank line Markdown reads as a block boundary, and a part
/// that would contribute nothing is dropped rather than emitted empty.
pub(crate) fn compose(body: &str, quotes: &[QuotedReply<'_>], attachments: &[Media]) -> String {
    let mut parts = Vec::new();
    for quoted in quotes {
        let quote = block_quote(quoted.quote);
        let body = quoted.body.trim();
        let part = [quote.as_str(), body]
            .into_iter()
            .filter(|part| !part.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        if !part.is_empty() {
            parts.push(part);
        }
        parts.extend(quoted.attachments.iter().map(attachment_markdown));
    }
    let body = body.trim();
    if !body.is_empty() {
        parts.push(body.to_owned());
    }
    parts.extend(attachments.iter().map(attachment_markdown));
    parts.join("\n\n")
}

fn attachment_markdown(attachment: &Media) -> String {
    attachment
        .markdown()
        .expect("composer attachments passed admission before storage")
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
            .map(|(quote, body)| QuotedReply {
                quote,
                body,
                attachments: &[],
            })
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
    fn every_task_handle_occurrence_renders_in_source_order() {
        let control = "CONTROL-1kKbC3j4"
            .parse::<wire::TaskHandle>()
            .expect("fixture handle parses");
        let mosaic = "MOSAIC-1kRYqQxy"
            .parse::<wire::TaskHandle>()
            .expect("fixture handle parses");
        let identity = "IDENTIT-dRyDYZqJJ"
            .parse::<wire::TaskHandle>()
            .expect("current fixture handle parses");
        let control_chip = task_chip_html(&control);
        let mosaic_chip = task_chip_html(&mosaic);
        let identity_chip = task_chip_html(&identity);

        let rendered = render_received(
            "Plain CONTROL-1kKbC3j4, CONTROL-1kKbC3j4, and `CONTROL-1kKbC3j4`.\n\n\
             Then MOSAIC-1kRYqQxy, IDENTIT-dRyDYZqJJ, and CONTROL-1kKbC3j4!",
        );

        assert_eq!(
            rendered.html,
            format!(
                "<p>Plain {control_chip}, {control_chip}, and {control_chip}.</p>\n\
                 <p>Then {mosaic_chip}, {identity_chip}, and {control_chip}!</p>\n"
            ),
        );
        assert_eq!(
            rendered.html.matches("class=\"message-task-chip\"").count(),
            6
        );
    }

    #[test]
    fn quoted_first_and_nested_mentions_do_not_consume_later_prose_occurrences() {
        let rendered = render_received(
            "> Quoted CONTROL-1kKbC3j4.\n\
             > > Nested `CONTROL-1kKbC3j4`.\n\n\
             Authored CONTROL-1kKbC3j4 and CONTROL-1kKbC3j4.",
        );
        let marker = "data-task-handle=\"CONTROL-1kKbC3j4\"";

        assert_eq!(
            rendered.html.matches(marker).count(),
            4,
            "{}",
            rendered.html
        );
        assert_eq!(rendered.html.matches("<blockquote>").count(), 2);
        assert!(rendered.html.contains("Quoted "));
        assert!(rendered.html.contains("Nested "));
        assert!(rendered.html.contains("Authored "));
        assert!(
            rendered.html.rfind(marker).expect("authored chip renders")
                > rendered
                    .html
                    .rfind("</blockquote>")
                    .expect("quoted context closes"),
            "the authored occurrence follows the quoted context: {}",
            rendered.html,
        );
    }

    #[test]
    fn authenticated_classification_markers_become_inline_pills() {
        let rendered = render_received_with_badges(
            "> [REVIEW] Peer feedback for CANVAS-1kKRRsDR:\n>\n\
             > > missing current coverage\n>\n\
             > Keep working the current task.\n\n\
             [REVIEW] Ordinary prose remains unchanged.\n\n\
             > [REVIEW] A second quoted marker remains unchanged.\n\n\
             > `[REVIEW]` remains inline code.\n\n\
             > ```text\n> [REVIEW] remains fenced code\n> ```\n\n\
             > [TASTE] compatibility: Keep one tested path.\n\n\
             > > [TASTE] fallbacks: Nested guidance keeps its own accent.\n\n\
             > [TASTE] polling: Wait for completion.\n\n\
             > [BROADCAST] Send this to every occupied agent.\n\n\
             ACK: ordinary [TASTE] prose stays text.",
            wire::AgentMessageBadges {
                acknowledgements: 0,
                tastes: 3,
                broadcasts: 1,
                reviews: 1,
            },
        );
        assert!(!rendered.html.contains("compatibility:"));
        assert!(!rendered.html.contains("fallbacks:"));
        assert!(!rendered.html.contains("polling:"));
        assert!(rendered.html.contains("Keep one tested path."));
        assert!(
            rendered
                .html
                .contains("Nested guidance keeps its own accent.")
        );
        assert!(rendered.html.contains("Wait for completion."));
        assert!(rendered.html.contains("Peer feedback for "));
        assert!(
            rendered
                .html
                .contains("data-task-handle=\"CANVAS-1kKRRsDR\"")
        );
        assert!(rendered.html.contains("missing current coverage"));
        assert!(rendered.html.contains("Keep working the current task."));
        assert!(!rendered.html.contains("[REVIEW] Peer feedback"));
        assert!(
            rendered
                .html
                .contains("[REVIEW] Ordinary prose remains unchanged.")
        );
        assert!(
            rendered
                .html
                .contains("[REVIEW] A second quoted marker remains unchanged.")
        );
        assert!(rendered.html.contains("<code>[REVIEW]</code>"));
        assert!(rendered.html.contains("[REVIEW] remains fenced code"));
        assert!(rendered.html.contains("class=\"message-review-pill\""));
        assert!(
            rendered
                .html
                .contains("aria-label=\"Review classification\">REVIEW</span>")
        );
        assert_eq!(rendered.html.matches("message-review-pill").count(), 1);

        assert_eq!(
            rendered
                .html
                .matches("class=\"message-broadcast-pill\"")
                .count(),
            1,
        );

        assert_eq!(
            rendered
                .html
                .matches("class=\"message-taste-pill\"")
                .count(),
            3,
            "{}",
            rendered.html,
        );
        assert_eq!(
            rendered
                .html
                .matches("aria-label=\"Taste classification\">TASTE</span>")
                .count(),
            3,
        );
        assert!(
            rendered
                .html
                .contains("ACK: ordinary [TASTE] prose stays text.")
        );

        let unauthenticated = render_received("> [REVIEW] Quoted prose is not metadata.");
        assert!(!unauthenticated.html.contains("message-review-pill"));
        assert!(
            unauthenticated
                .html
                .contains("[REVIEW] Quoted prose is not metadata.")
        );
    }

    #[test]
    fn broadcast_marker_is_idempotent_and_requires_authenticated_quote_metadata() {
        let marked = mark_broadcast("Send this to every occupied agent.");
        assert_eq!(marked, "[BROADCAST] Send this to every occupied agent.");
        assert_eq!(mark_broadcast(&marked), marked);

        let classified = render_received_with_badges(
            "> [BROADCAST] Send this to every occupied agent.\n\n\
             > [BROADCAST] A second marker remains literal.\n\n\
             `[BROADCAST]` remains code.",
            wire::AgentMessageBadges {
                broadcasts: 1,
                ..wire::AgentMessageBadges::default()
            },
        );
        assert_eq!(
            classified
                .html
                .matches("class=\"message-broadcast-pill\"")
                .count(),
            1,
        );
        assert!(
            classified
                .html
                .contains("Send this to every occupied agent.")
        );
        assert!(
            classified
                .html
                .contains("[BROADCAST] A second marker remains literal.")
        );
        assert!(classified.html.contains("<code>[BROADCAST]</code>"));

        let unclassified = render_received("> [BROADCAST] Remains ordinary quoted prose.");
        assert!(!unclassified.html.contains("message-broadcast-pill"));
        assert!(
            unclassified
                .html
                .contains("[BROADCAST] Remains ordinary quoted prose.")
        );
    }

    #[test]
    fn taste_metadata_neither_overreaches_nor_decorates_code() {
        let one = render_received_with_tastes(
            "> `[TASTE]` remains code.\n\n\
             > [TASTE] first.\n\n\
             > [TASTE] second remains literal.",
            1,
        );
        assert_eq!(one.html.matches("class=\"message-taste-pill\"").count(), 1);
        assert!(one.html.contains("<code>[TASTE]</code>"));
        assert!(one.html.contains("[TASTE] second remains literal."));

        let none = render_received_with_tastes("> [TASTE] untouched.", 0);
        assert!(!none.html.contains("message-taste-pill"));
        assert!(none.html.contains("[TASTE] untouched."));

        let token = render_received_with_tastes("> [TASTE]ful is not a marker.", 1);
        assert!(!token.html.contains("message-taste-pill"));
        assert!(token.html.contains("[TASTE]ful is not a marker."));

        let prefixed =
            render_received_with_tastes("> prefix[TASTE] stays text.\n\n> [TASTE] actual.", 1);
        assert_eq!(
            prefixed
                .html
                .matches("class=\"message-taste-pill\"")
                .count(),
            1,
        );
        assert!(prefixed.html.contains("prefix[TASTE] stays text."));
    }

    #[test]
    fn noncanonical_and_block_code_handles_remain_ordinary_markdown() {
        let rendered = render_received("`APP-a1` and APP-a1.\n\n```text\nCONTROL-1kKbC3j4\n```");

        assert!(!rendered.html.contains("message-task-chip"));
        assert!(rendered.html.contains("<code>APP-a1</code> and APP-a1."));
        assert!(rendered.html.contains("CONTROL-1kKbC3j4"));
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

    #[test]
    fn every_image_preview_owns_an_explicit_initial_lifecycle() {
        let remote = render_received("![remote](https://example.test/image.png)");
        assert!(remote.html.contains("data-mosaic-image-state=\"pending\""));
        assert!(
            remote
                .html
                .contains("data-mosaic-image-status=\"pending\">Loading image</span>")
        );

        let cached = render_received(&format!("![cached]({OPENABLE_PNG})"));
        assert!(cached.html.contains("data-mosaic-image-state=\"pending\""));
        assert_eq!(cached.html.matches("data-mosaic-image-status=").count(), 1);
        assert!(cached.html.contains(&format!("href=\"{OPENABLE_PNG}\"")));

        let malformed = render_received("![broken](data:image/png;base64,%%%)");
        assert!(malformed.html.contains("data-mosaic-image-state=\"error\""));
        assert!(
            malformed
                .html
                .contains("data-mosaic-image-status=\"error\">Image unavailable</span>")
        );
    }
}
