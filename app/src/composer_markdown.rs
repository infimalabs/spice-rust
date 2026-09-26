//! Target-neutral Markdown composition and rendering for browser messages.
//!
//! The module owning the forms and cards is wasm-only, so every Markdown
//! operation that does not need a DOM lives here and is reachable from an
//! ordinary native test.
//! Nothing here may be duplicated there: a second copy is what let the shipped
//! quote drift from the tested one until the copies disagreed on blank input.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Range;

use pulldown_cmark::{CowStr, Event as MarkdownEvent, Options, Parser, Tag, TagEnd};
use wire::{Media, MessageClassification, MessageClassificationKind};

use crate::watchband;

mod diagrams;
mod quotes;
mod taste_quotes;

pub(crate) struct RenderedMarkdown {
    pub(crate) html: String,
    pub(crate) image_only: bool,
    pub(crate) diagrams: Vec<String>,
}

/// Render received text through the one sanitized Markdown path used by cards.
#[cfg(test)]
pub(crate) fn render_received(markdown: &str) -> RenderedMarkdown {
    render_received_with_classifications(markdown, &[])
}

/// Prefix one composed document with the canonical broadcast marker.
pub(crate) fn mark_broadcast(markdown: &str) -> String {
    if has_marker_prefix(markdown, wire::stream::BROADCAST_MARKER) {
        markdown.to_owned()
    } else {
        format!("{} {markdown}", wire::stream::BROADCAST_MARKER)
    }
}
/// Expand display-only media payloads while preserving exact classification
/// offsets. A media reference containing an authenticated marker is left
/// untouched rather than making that marker ambiguous.
pub(crate) fn expand_media_payloads<'a>(
    markdown: &'a str,
    media: &[Media],
    classifications: &'a [MessageClassification],
) -> Result<(Cow<'a, str>, Cow<'a, [MessageClassification]>), wire::MediaError> {
    let mut projected = Cow::Borrowed(markdown);
    let mut remapped = Cow::Borrowed(classifications);
    if !markdown.contains("](attachment:") {
        return Ok((projected, remapped));
    }
    for item in media {
        let reference = item.reference_markdown()?;
        if !projected.contains(&reference) {
            continue;
        }
        let replacement = item.markdown()?;
        let reference_len = reference.len();
        let starts = projected
            .match_indices(&reference)
            .filter_map(|(start, _)| {
                let end = start + reference_len;
                let intersects_classification = remapped.iter().any(|classification| {
                    usize::try_from(classification.start)
                        .is_ok_and(|classification_start| classification_start < end)
                        && usize::try_from(classification.end)
                            .is_ok_and(|classification_end| start < classification_end)
                });
                (!intersects_classification).then_some(start)
            })
            .collect::<Vec<_>>();
        if starts.is_empty() {
            continue;
        }
        let source = projected.as_ref();
        let mut expanded = String::with_capacity(
            source
                .len()
                .saturating_add(replacement.len().saturating_mul(starts.len())),
        );
        let mut cursor = 0;
        for start in &starts {
            expanded.push_str(&source[cursor..*start]);
            expanded.push_str(&replacement);
            cursor = start + reference_len;
        }
        expanded.push_str(&source[cursor..]);

        if !remapped.is_empty() {
            let delta = i64::try_from(replacement.len()).expect("media Markdown length fits i64")
                - i64::try_from(reference_len).expect("media reference length fits i64");
            remapped = Cow::Owned(
                remapped
                    .iter()
                    .map(|classification| {
                        let start = usize::try_from(classification.start)
                            .expect("validated classification start fits usize");
                        let preceding = starts.partition_point(|replacement_start| {
                            replacement_start + reference_len <= start
                        });
                        let shift = delta
                            .checked_mul(
                                i64::try_from(preceding).expect("media replacement count fits i64"),
                            )
                            .expect("media classification offset shift fits i64");
                        let mut classification = *classification;
                        classification.start =
                            shift_classification_offset(classification.start, shift);
                        classification.end = shift_classification_offset(classification.end, shift);
                        classification
                    })
                    .collect(),
            );
        }
        projected = Cow::Owned(expanded);
    }
    Ok((projected, remapped))
}

fn shift_classification_offset(offset: u32, shift: i64) -> u32 {
    let shifted = i64::from(offset)
        .checked_add(shift)
        .expect("media classification offset shift does not overflow");
    u32::try_from(shifted).expect("admitted media projection fits classification offsets")
}

/// Render received text using exact resident-authenticated semantic ranges.
#[cfg(test)]
pub(crate) fn render_received_with_classifications(
    markdown: &str,
    classifications: &[MessageClassification],
) -> RenderedMarkdown {
    render_received_with_quotes(markdown, classifications, &[], &BTreeMap::new())
}

/// Render received text and its resident-resolved structural quote links.
pub(crate) fn render_received_with_quotes(
    markdown: &str,
    classifications: &[MessageClassification],
    quotes: &[wire::QuotePresentation],
    quote_tasks: &BTreeMap<String, wire::TaskHandle>,
) -> RenderedMarkdown {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM;
    let normalized = taste_quotes::normalize(markdown, classifications, options);
    let markdown = normalized.markdown;
    let mut remaining = normalized
        .classifications
        .into_iter()
        .map(|classification| (classification.start, classification))
        .collect::<BTreeMap<_, _>>();
    let mut skipped_marker = None;
    let mut code_block = false;
    let events = Parser::new_ext(&markdown, options)
        .into_offset_iter()
        .flat_map(|(event, source)| {
            match &event {
                MarkdownEvent::Start(Tag::CodeBlock(_)) => code_block = true,
                MarkdownEvent::End(TagEnd::CodeBlock) => code_block = false,
                _ => {}
            }
            let event = sanitize_markdown_event(event);
            if code_block {
                vec![event]
            } else {
                inline_classification_marker(event, source, &mut remaining, &mut skipped_marker)
            }
        })
        .collect::<Vec<_>>();
    debug_assert!(
        remaining.is_empty(),
        "every resident classification maps to one Markdown text event"
    );
    let events = coalesce_text_events(events);
    let events = quotes::expand(events, quotes, quote_tasks);
    let (events, diagrams) = diagrams::extract(events);
    let has_diagrams = !diagrams.is_empty();
    let image_only = markdown_is_image_only(&events, has_diagrams);
    // Diagram controls and nested block structure must not collapse into an image row.
    let mut task_code_block = false;
    let events = decorate_markdown_images(events, image_only && !has_diagrams)
        .into_iter()
        .flat_map(|event| match &event {
            MarkdownEvent::Start(Tag::CodeBlock(_)) => {
                task_code_block = true;
                vec![event]
            }
            MarkdownEvent::End(TagEnd::CodeBlock) => {
                task_code_block = false;
                vec![event]
            }
            _ if task_code_block => vec![event],
            _ => inline_task_chips(event),
        });
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events);
    RenderedMarkdown {
        html,
        image_only,
        diagrams,
    }
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

/// Replace one exact resident-authenticated marker with its final inline pill.
///
/// `pulldown-cmark` can split bracketed markers into several adjacent text
/// events. The authenticated source range, rather than event shape, controls
/// which bytes disappear.
fn inline_classification_marker<'a>(
    event: MarkdownEvent<'a>,
    source: Range<usize>,
    remaining: &mut BTreeMap<u32, MessageClassification>,
    skipped_marker: &mut Option<(usize, bool)>,
) -> Vec<MarkdownEvent<'a>> {
    let MarkdownEvent::Text(mut text) = event else {
        return vec![event];
    };
    if let Some((end, taste)) = *skipped_marker {
        if source.end <= end {
            return Vec::new();
        }
        if source.start < end {
            let relative = end - source.start;
            text = text
                .get(relative..)
                .expect("canonical marker source and rendered text align")
                .to_owned()
                .into();
        }
        *skipped_marker = None;
        if taste {
            text = taste_guidance_suffix(&text).into();
        }
        return vec![MarkdownEvent::Text(text)];
    }

    let source_start = u32::try_from(source.start).expect("Markdown source offset fits u32");
    let source_end = u32::try_from(source.end).expect("Markdown source offset fits u32");
    let Some((&start, _)) = remaining.range(source_start..source_end).next() else {
        return vec![MarkdownEvent::Text(text)];
    };
    let classification = remaining
        .remove(&start)
        .expect("the selected resident classification remains");
    let start =
        usize::try_from(classification.start).expect("validated classification start fits usize");
    let end = usize::try_from(classification.end).expect("validated classification end fits usize");
    let mut events = Vec::with_capacity(3);
    if source.start < start {
        events.push(MarkdownEvent::Text(
            text.get(..start - source.start)
                .expect("classification prefix aligns with rendered text")
                .to_owned()
                .into(),
        ));
    }
    events.push(MarkdownEvent::InlineHtml(
        classification_pill_html(classification.kind).into(),
    ));
    let taste = classification.kind == MessageClassificationKind::Taste;
    if source.end <= end {
        *skipped_marker = Some((end, taste));
    } else {
        let mut suffix = text
            .get(end - source.start..)
            .expect("classification suffix aligns with rendered text")
            .to_owned();
        if taste {
            suffix = taste_guidance_suffix(&suffix);
        }
        events.push(MarkdownEvent::Text(suffix.into()));
    }
    events
}

fn classification_pill_html(kind: MessageClassificationKind) -> String {
    match kind {
        MessageClassificationKind::Review => review_pill_html(),
        MessageClassificationKind::Broadcast => broadcast_pill_html(),
        MessageClassificationKind::Taste => taste_pill_html(),
        MessageClassificationKind::Acknowledgement => {
            acknowledgement_pill_html("ack", "acked", "Acknowledgement classification", "ACK")
        }
        MessageClassificationKind::AcknowledgementPending => acknowledgement_pill_html(
            "ack-pending",
            "pending",
            "Acknowledgement awaiting retirement",
            "ACK",
        ),
        MessageClassificationKind::Decline => {
            acknowledgement_pill_html("nack", "declined", "Declined classification", "NACK")
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
struct TestClassifications {
    reviews: u32,
    tastes: u32,
    broadcasts: u32,
    acknowledgements: u32,
    pending: u32,
    declines: u32,
}

#[cfg(test)]
fn render_received_with_fixture_classifications(
    markdown: &str,
    requested: TestClassifications,
) -> RenderedMarkdown {
    let ordinary = requested
        .acknowledgements
        .saturating_sub(requested.tastes)
        .saturating_sub(requested.broadcasts);
    let requested = [
        (MessageClassificationKind::Review, requested.reviews),
        (MessageClassificationKind::Taste, requested.tastes),
        (MessageClassificationKind::Broadcast, requested.broadcasts),
        (
            MessageClassificationKind::AcknowledgementPending,
            requested.pending,
        ),
        (MessageClassificationKind::Decline, requested.declines),
        (MessageClassificationKind::Acknowledgement, ordinary),
    ];
    let mut classifications = requested
        .into_iter()
        .flat_map(|(kind, count)| test_classifications(markdown, kind, count))
        .collect::<Vec<_>>();
    classifications.sort_by_key(|classification| classification.start);
    render_received_with_classifications(markdown, &classifications)
}
#[cfg(test)]
fn test_classifications(
    markdown: &str,
    kind: MessageClassificationKind,
    count: u32,
) -> Vec<MessageClassification> {
    if count == 0 {
        return Vec::new();
    }
    let mut classifications = Vec::new();
    let mut line_start = 0usize;
    for line in markdown.split_inclusive('\n') {
        let mut marker_start = line_start;
        let mut content = line.trim_end_matches(['\r', '\n']);
        let mut quote_depth = 0usize;
        while let Some(rest) = content.strip_prefix("> ") {
            quote_depth += 1;
            marker_start += 2;
            content = rest;
        }
        let eligible_depth = if kind == MessageClassificationKind::Taste {
            quote_depth > 0
        } else {
            quote_depth == 1
        };
        let marker = kind.marker();
        if eligible_depth
            && content
                .strip_prefix(marker)
                .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(char::is_whitespace))
        {
            classifications.push(MessageClassification {
                start: u32::try_from(marker_start).expect("fixture marker offset fits u32"),
                end: u32::try_from(marker_start + marker.len())
                    .expect("fixture marker offset fits u32"),
                kind,
            });
            if classifications.len() == usize::try_from(count).unwrap_or(usize::MAX) {
                break;
            }
        }
        line_start += line.len();
    }
    classifications
}

/// Render received text with authenticated taste locations in native tests.
#[cfg(test)]
pub(crate) fn render_received_with_tastes(markdown: &str, tastes: u32) -> RenderedMarkdown {
    render_received_with_fixture_classifications(
        markdown,
        TestClassifications {
            tastes,
            ..TestClassifications::default()
        },
    )
}

fn has_marker_prefix(text: &str, marker: &str) -> bool {
    text.strip_prefix(marker)
        .is_some_and(|suffix| suffix.starts_with(char::is_whitespace))
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

fn taste_pill_html() -> String {
    "<span class=\"message-taste-pill\" data-message-classification=\"taste\" \
     aria-label=\"Taste classification\">TASTE</span>"
        .to_owned()
}

fn broadcast_pill_html() -> String {
    "<span class=\"message-broadcast-pill\" data-message-classification=\"broadcast\" \
     aria-label=\"Broadcast classification\">BROADCAST</span>"
        .to_owned()
}

fn review_pill_html() -> String {
    "<span class=\"message-review-pill\" data-message-classification=\"review\" \
     aria-label=\"Review classification\">REVIEW</span>"
        .to_owned()
}

fn acknowledgement_pill_html(
    classification: &str,
    acknowledgement: &str,
    label: &str,
    text: &str,
) -> String {
    format!(
        "<span class=\"message-ack-pill\" data-message-classification=\"{classification}\" \
         data-acknowledgement=\"{acknowledgement}\" aria-label=\"{label}\">\
         <span class=\"message-ack-label\">{text}</span></span>"
    )
}
/// Whether two rendered messages have identical measured content.
///
/// Pending-to-acknowledged settlement changes only attributes on the same
/// fixed-width ACK pill. Normalize that state transition while retaining every
/// geometry-bearing distinction, including a declined NACK label.
pub(crate) fn same_measured_shape(left: &RenderedMarkdown, right: &RenderedMarkdown) -> bool {
    measured_html(&left.html) == measured_html(&right.html)
        && left.image_only == right.image_only
        && left.diagrams == right.diagrams
}

fn measured_html(html: &str) -> std::borrow::Cow<'_, str> {
    let pending = acknowledgement_pill_html(
        "ack-pending",
        "pending",
        "Acknowledgement awaiting retirement",
        "ACK",
    );
    if !html.contains(&pending) {
        return std::borrow::Cow::Borrowed(html);
    }
    let acknowledged =
        acknowledgement_pill_html("ack", "acked", "Acknowledgement classification", "ACK");
    std::borrow::Cow::Owned(html.replace(&pending, &acknowledged))
}

fn markdown_is_image_only(events: &[MarkdownEvent<'_>], has_diagrams: bool) -> bool {
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
            // Raw HTML was converted to text before extraction; these are owned slots.
            MarkdownEvent::Html(_) => images += 1,
            MarkdownEvent::Start(Tag::BlockQuote(_) | Tag::List(_) | Tag::Item)
            | MarkdownEvent::End(TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Item)
                if has_diagrams => {}
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
    if !source.is_empty() && !malformed_data_image {
        format!(
            "<button type=\"button\" class=\"message-image message-image--openable\" \
             data-image-preview=\"\" title=\"{action}\" aria-label=\"{action}\">{preview}</button>"
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
        "<span class=\"message-task-chip\" data-task-handle=\"{handle}\" role=\"button\" \
         tabindex=\"0\" aria-label=\"Inspect task {handle}\" title=\"Inspect {handle}\">\
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
        || (!image
            && (candidate.starts_with("mailto:") || candidate.starts_with("spice://quote/v1/")))
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
        .reference_markdown()
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

#[cfg(test)]
mod tests;
