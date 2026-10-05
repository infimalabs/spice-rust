//! Recursive attribution-hunk layout for message and quote Markdown.
//!
//! A speaker owns each direct run of content between nested quote blocks. Every
//! non-empty run leads with that speaker's controls and attribution, followed in
//! DOM order by all image stacks in the run and then its remaining content.

use std::ops::Range;

use pulldown_cmark::{Event as MarkdownEvent, Tag};

pub(super) const STACK_OPEN: &str = "<p class=\"message-image-stack\">";
pub(super) const STACK_CLOSE: &str = "</p>";
const HUNK_OPEN: &str = "<div class=\"message-hunk\">";
const HUNK_CONTENT_OPEN: &str = "<div class=\"message-hunk-content\">";
const HUNK_CLOSE: &str = "</div>";

/// Attribution repeated at the start of every hunk one speaker owns.
#[derive(Clone)]
pub(super) struct Attribution {
    /// Stable browser control model key. Empty names the containing message.
    pub(super) controls_target: String,
    /// Source age and name, or the containing card's Yew signoff slot.
    pub(super) signoff: String,
    /// Source disposition following its name.
    pub(super) disposition: String,
    /// Board provenance retained once at the complete attributed layer's tail.
    pub(super) task_tail: String,
}

impl Attribution {
    fn lead_html(&self) -> String {
        format!(
            "<span class=\"message-controls-slot\" data-message-controls=\"{}\"></span>\
             <p class=\"message-attribution\">{}{}</p>",
            super::escape_html_attribute(&self.controls_target),
            self.signoff,
            self.disposition,
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    Paragraph,
    Stack,
    Quote,
    QuoteCard,
    Other,
}

/// Lay out one Markdown layer and every raw block quote it directly contains.
///
/// `quote_attributions` follows the layer's direct raw block quotes in document
/// order. Resolved quote cards render recursively before they become opaque HTML.
pub(super) fn flow<'a>(
    events: &[MarkdownEvent<'a>],
    attribution: Option<&Attribution>,
    quote_attributions: &[Option<Attribution>],
) -> Vec<MarkdownEvent<'a>> {
    let mut quote_attributions = quote_attributions.iter();
    let blocks = prepare_blocks(events, &mut quote_attributions);
    lay_out_hunks(blocks, attribution)
}

fn prepare_blocks<'a, 'q>(
    events: &[MarkdownEvent<'a>],
    quote_attributions: &mut impl Iterator<Item = &'q Option<Attribution>>,
) -> Vec<(Block, Vec<MarkdownEvent<'a>>)> {
    let mut prepared = Vec::new();
    for (kind, range) in blocks(events) {
        let block = &events[range];
        if kind == Block::Paragraph {
            let mut start = 1usize;
            for (index, event) in block[1..block.len() - 1].iter().enumerate() {
                let index = index + 1;
                if !matches!(event, MarkdownEvent::InlineHtml(html)
                    if html.starts_with("<div class=\"message-quote-card"))
                {
                    continue;
                }
                push_paragraph_segment(&mut prepared, block, start, index);
                prepared.push((Block::QuoteCard, vec![event.clone()]));
                start = index + 1;
            }
            push_paragraph_segment(&mut prepared, block, start, block.len() - 1);
        } else if kind == Block::Quote {
            let inner = &block[1..block.len() - 1];
            let attribution = quote_attributions.next().and_then(Option::as_ref);
            let body = lay_out_hunks(prepare_blocks(inner, &mut std::iter::empty()), attribution);
            let mut quote = Vec::with_capacity(body.len() + 2);
            quote.push(block[0].clone());
            quote.extend(body);
            quote.push(block[block.len() - 1].clone());
            prepared.push((kind, quote));
        } else {
            prepared.push((kind, block.to_vec()));
        }
    }
    prepared
}

fn push_paragraph_segment<'a>(
    blocks: &mut Vec<(Block, Vec<MarkdownEvent<'a>>)>,
    paragraph: &[MarkdownEvent<'a>],
    start: usize,
    end: usize,
) {
    if start == end {
        return;
    }
    let mut segment = Vec::with_capacity(end - start + 2);
    segment.push(paragraph[0].clone());
    segment.extend(paragraph[start..end].iter().cloned());
    segment.push(paragraph[paragraph.len() - 1].clone());
    blocks.push((Block::Paragraph, segment));
}

fn lay_out_hunks<'a>(
    blocks: Vec<(Block, Vec<MarkdownEvent<'a>>)>,
    attribution: Option<&Attribution>,
) -> Vec<MarkdownEvent<'a>> {
    let Some(attribution) = attribution else {
        return blocks.into_iter().flat_map(|(_, block)| block).collect();
    };
    let mut events = Vec::new();
    let mut hunk = Vec::new();
    for (kind, block) in blocks {
        if matches!(kind, Block::Quote | Block::QuoteCard) {
            push_hunk(&mut events, &mut hunk, attribution);
            events.extend(block);
        } else {
            hunk.push((kind, block));
        }
    }
    push_hunk(&mut events, &mut hunk, attribution);
    if !attribution.task_tail.is_empty() {
        events.push(MarkdownEvent::Html(
            format!(
                "<p class=\"message-quote-task-line\">{}</p>",
                attribution.task_tail
            )
            .into(),
        ));
    }
    events
}

fn push_hunk<'a>(
    events: &mut Vec<MarkdownEvent<'a>>,
    hunk: &mut Vec<(Block, Vec<MarkdownEvent<'a>>)>,
    attribution: &Attribution,
) {
    if hunk.is_empty() {
        return;
    }
    events.push(MarkdownEvent::Html(HUNK_OPEN.into()));
    events.push(MarkdownEvent::Html(attribution.lead_html().into()));
    let mut stacks = Vec::new();
    let mut content = Vec::new();
    for (kind, block) in hunk.drain(..) {
        if kind == Block::Stack {
            stacks.push(block);
        } else {
            content.extend(block);
        }
    }
    if !stacks.is_empty() {
        events.push(MarkdownEvent::InlineHtml(STACK_OPEN.into()));
        for stack in stacks {
            events.extend(stack[1..stack.len() - 1].iter().cloned());
        }
        events.push(MarkdownEvent::InlineHtml(STACK_CLOSE.into()));
    }
    if !content.is_empty() {
        events.push(MarkdownEvent::Html(HUNK_CONTENT_OPEN.into()));
        events.extend(content);
        events.push(MarkdownEvent::Html(HUNK_CLOSE.into()));
    }
    events.push(MarkdownEvent::Html(HUNK_CLOSE.into()));
}

/// The layer's direct child blocks, in order.
fn blocks(events: &[MarkdownEvent<'_>]) -> Vec<(Block, Range<usize>)> {
    let mut blocks = Vec::new();
    let mut index = 0usize;
    while index < events.len() {
        let (kind, end) = match &events[index] {
            MarkdownEvent::Start(tag) => {
                let mut depth = 0usize;
                let mut end = index;
                for (offset, event) in events[index..].iter().enumerate() {
                    match event {
                        MarkdownEvent::Start(_) => depth += 1,
                        MarkdownEvent::End(_) => depth -= 1,
                        _ => {}
                    }
                    if depth == 0 {
                        end = index + offset;
                        break;
                    }
                }
                let kind = match tag {
                    Tag::BlockQuote(_) => Block::Quote,
                    Tag::Paragraph => Block::Paragraph,
                    _ => Block::Other,
                };
                (kind, end)
            }
            MarkdownEvent::InlineHtml(html) if html.as_ref() == STACK_OPEN => {
                let end = events[index + 1..]
                    .iter()
                    .position(|event| {
                        matches!(event, MarkdownEvent::InlineHtml(html) if html.as_ref() == STACK_CLOSE)
                    })
                    .map_or(events.len() - 1, |offset| index + 1 + offset);
                (Block::Stack, end)
            }
            _ => (Block::Other, index),
        };
        blocks.push((kind, index..end + 1));
        index = end + 1;
    }
    blocks
}
