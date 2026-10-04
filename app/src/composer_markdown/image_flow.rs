//! Quote layers that end in images flow their prose around those images.
//!
//! A quote layer whose trailing blocks are image stacks, after at least one
//! other block, renders inside one flow wrapper: a zero-width anchor float,
//! the layer's images merged into one stack floated beneath it, then the prose.
//! Floats only push content that follows them, so the images lead in source;
//! the browser then lifts the anchor until the images' bottom meets the end of
//! the prose (`browser::quote_image_flow`), leaving the images at the bottom
//! with prose beside and above them. A layer's attribution leads it: inside
//! its first prose paragraph, or on a line of its own above the flow.

use std::ops::Range;

use pulldown_cmark::{Event as MarkdownEvent, Tag};

pub(super) const STACK_OPEN: &str = "<p class=\"message-image-stack\">";
pub(super) const STACK_CLOSE: &str = "</p>";
const FLOW_OPEN: &str = "<div class=\"message-image-flow\">\
                         <span class=\"message-image-flow-anchor\" aria-hidden=\"true\"></span>";
const FLOW_CLOSE: &str = "</div>";
pub(super) const SIGNOFF_LINE_OPEN: &str = "<p class=\"message-quote-signoff-line\">";
pub(super) const TASK_LINE_OPEN: &str = "<p class=\"message-quote-task-line\">";

/// What a quote card places around its own top-level content.
pub(super) struct CardHead<'t> {
    /// The card's leading attribution, empty when it has none.
    pub(super) signoff: &'t str,
    /// Disposition pill following the attribution, empty when none.
    pub(super) disposition: &'t str,
    /// Empty board-provenance portal target placed at the content's tail.
    pub(super) task_tail: &'t str,
}

/// The layer a run of events forms.
pub(super) enum Layer<'t> {
    /// A message body: only its quotes flow.
    Body,
    /// A quote card body, which flows like a quote and leads with its head.
    Card(CardHead<'t>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    Paragraph,
    Stack,
    SignoffLine,
    TaskLine,
    Quote,
    Other,
}

/// Flow every quote layer in `events` that ends in images.
pub(super) fn flow<'a>(events: &[MarkdownEvent<'a>], layer: &Layer<'_>) -> Vec<MarkdownEvent<'a>> {
    let mut blocks = blocks(events)
        .into_iter()
        .map(|(kind, range)| {
            let block = &events[range];
            let block = if kind == Block::Quote {
                let inner = &block[1..block.len() - 1];
                let body = flow_quote(inner);
                let mut quote = Vec::with_capacity(body.len() + 2);
                quote.push(block[0].clone());
                quote.extend(body);
                quote.push(block[block.len() - 1].clone());
                quote
            } else {
                block.to_vec()
            };
            (kind, block)
        })
        .collect::<Vec<_>>();
    match layer {
        Layer::Body => blocks.into_iter().flat_map(|(_, block)| block).collect(),
        Layer::Card(head) => {
            let task_tail = head.task_tail;
            let head = format!(
                "{}{}",
                head.signoff,
                if head.disposition.is_empty() {
                    String::new()
                } else {
                    format!("{} ", head.disposition)
                }
            );
            let line = (!head.is_empty()).then(|| match blocks.first_mut() {
                Some((Block::Paragraph, block)) => {
                    block.insert(1, MarkdownEvent::InlineHtml(head.into()));
                    None
                }
                _ => Some(MarkdownEvent::Html(
                    format!("{SIGNOFF_LINE_OPEN}{head}</p>").into(),
                )),
            });
            trail(&mut blocks, task_tail);
            line.flatten().into_iter().chain(hoist(blocks)).collect()
        }
    }
}

/// Keep a prose tail inline so provenance can only add content after its final
/// word. A non-prose tail gets its own line, which `hoist` keeps behind any
/// trailing image flow.
fn trail<'a>(blocks: &mut Vec<(Block, Vec<MarkdownEvent<'a>>)>, task_tail: &str) {
    if task_tail.is_empty() {
        return;
    }
    if let Some((Block::Paragraph, block)) = blocks.last_mut() {
        block.insert(
            block.len() - 1,
            MarkdownEvent::InlineHtml(task_tail.to_owned().into()),
        );
    } else {
        blocks.push((
            Block::TaskLine,
            vec![MarkdownEvent::Html(
                format!("{TASK_LINE_OPEN}{task_tail}</p>").into(),
            )],
        ));
    }
}

fn flow_quote<'a>(events: &[MarkdownEvent<'a>]) -> Vec<MarkdownEvent<'a>> {
    let blocks = flow(events, &Layer::Body);
    let blocks = self::blocks(&blocks)
        .into_iter()
        .map(|(kind, range)| (kind, blocks[range].to_vec()))
        .collect();
    hoist(blocks)
}

/// Lead with the trailing image stacks inside one flow wrapper when the layer
/// has other blocks before them; otherwise keep the layer as it is. A leading
/// attribution line stays above the flow, and a task line stays behind it.
fn hoist<'a>(mut blocks: Vec<(Block, Vec<MarkdownEvent<'a>>)>) -> Vec<MarkdownEvent<'a>> {
    let mut events = Vec::new();
    if blocks
        .first()
        .is_some_and(|(kind, _)| *kind == Block::SignoffLine)
    {
        events.extend(blocks.remove(0).1);
    }
    let task_line = if blocks
        .last()
        .is_some_and(|(kind, _)| *kind == Block::TaskLine)
    {
        blocks.pop().map(|(_, block)| block)
    } else {
        None
    };
    let first_stack = blocks
        .iter()
        .rposition(|(kind, _)| *kind != Block::Stack)
        .map_or(0, |index| index + 1);
    if first_stack == 0 || first_stack == blocks.len() {
        events.extend(blocks.into_iter().flat_map(|(_, block)| block));
        events.extend(task_line.into_iter().flatten());
        return events;
    }
    let stacks = blocks.split_off(first_stack);
    events.push(MarkdownEvent::Html(FLOW_OPEN.into()));
    events.push(MarkdownEvent::InlineHtml(STACK_OPEN.into()));
    for (_, stack) in stacks {
        events.extend(stack[1..stack.len() - 1].iter().cloned());
    }
    events.push(MarkdownEvent::InlineHtml(STACK_CLOSE.into()));
    events.extend(blocks.into_iter().flat_map(|(_, block)| block));
    events.push(MarkdownEvent::Html(FLOW_CLOSE.into()));
    events.extend(task_line.into_iter().flatten());
    events
}

/// The layer's direct child blocks, in order.
fn blocks(events: &[MarkdownEvent<'_>]) -> Vec<(Block, Range<usize>)> {
    let mut blocks = Vec::new();
    let mut index = 0usize;
    while index < events.len() {
        let (kind, end) = match &events[index] {
            MarkdownEvent::Start(tag) => {
                let kind = match tag {
                    Tag::Paragraph => Block::Paragraph,
                    Tag::BlockQuote(_) => Block::Quote,
                    _ => Block::Other,
                };
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
            MarkdownEvent::Html(html) if html.starts_with(SIGNOFF_LINE_OPEN) => {
                (Block::SignoffLine, index)
            }
            MarkdownEvent::Html(html) if html.starts_with(TASK_LINE_OPEN) => {
                (Block::TaskLine, index)
            }
            _ => (Block::Other, index),
        };
        blocks.push((kind, index..end + 1));
        index = end + 1;
    }
    blocks
}
