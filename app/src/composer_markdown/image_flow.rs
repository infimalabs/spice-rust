//! Quote layers that end in images flow their prose around those images.
//!
//! A quote layer whose trailing blocks are image stacks, after at least one
//! other block, renders inside one flow wrapper: a zero-width anchor float,
//! the layer's images merged into one stack floated beneath it, then the prose
//! and the attribution. Floats only push content that follows them, so the
//! images lead in source; the browser then lifts the anchor until the images'
//! bottom meets the end of the prose (`browser::quote_image_flow`), leaving
//! the images at the bottom with prose beside and above them and the
//! attribution as the layer's last line.

use std::ops::Range;

use pulldown_cmark::{Event as MarkdownEvent, Tag};

pub(super) const STACK_OPEN: &str = "<p class=\"message-image-stack\">";
const STACK_CLOSE: &str = "</p>";
const FLOW_OPEN: &str = "<div class=\"message-image-flow\">\
                         <span class=\"message-image-flow-anchor\" aria-hidden=\"true\"></span>";
const FLOW_CLOSE: &str = "</div>";
pub(super) const SIGNOFF_LINE_OPEN: &str = "<p class=\"message-quote-signoff-line\">";

/// What a quote card adds to its body's own top-level layer.
pub(super) struct CardTail<'t> {
    /// Disposition pill leading the first paragraph, empty when none.
    pub(super) disposition: &'t str,
    /// The card's attribution, empty when it has none.
    pub(super) signoff: &'t str,
}

/// The layer a run of events forms.
pub(super) enum Layer<'t> {
    /// A message body: only its quotes flow.
    Body,
    /// A quote card body, which flows like a quote and carries its tail.
    Card(CardTail<'t>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    Paragraph,
    Stack,
    SignoffLine,
    Quote,
    Other,
}

/// Flow every quote layer in `events` that ends in images. For a card body,
/// the result says whether its attribution moved inside the body.
pub(super) fn flow<'a>(
    events: &[MarkdownEvent<'a>],
    layer: &Layer<'_>,
) -> (Vec<MarkdownEvent<'a>>, bool) {
    let mut blocks = blocks(events)
        .into_iter()
        .map(|(kind, range)| {
            let block = &events[range];
            let block = if kind == Block::Quote {
                let inner = &block[1..block.len() - 1];
                let (body, _) = flow_quote(inner, None);
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
        Layer::Body => (
            blocks.into_iter().flat_map(|(_, block)| block).collect(),
            false,
        ),
        Layer::Card(tail) => {
            let lead = (!tail.disposition.is_empty()).then(|| match blocks.first_mut() {
                Some((Block::Paragraph, block)) => {
                    block.insert(
                        1,
                        MarkdownEvent::InlineHtml(format!("{} ", tail.disposition).into()),
                    );
                    None
                }
                _ => Some(MarkdownEvent::InlineHtml(
                    tail.disposition.to_owned().into(),
                )),
            });
            let (body, signed) = hoist(blocks, Some(tail.signoff));
            (lead.flatten().into_iter().chain(body).collect(), signed)
        }
    }
}

fn flow_quote<'a>(
    events: &[MarkdownEvent<'a>],
    signoff: Option<&str>,
) -> (Vec<MarkdownEvent<'a>>, bool) {
    let (blocks, _) = flow(events, &Layer::Body);
    let blocks = self::blocks(&blocks)
        .into_iter()
        .map(|(kind, range)| (kind, blocks[range].to_vec()))
        .collect();
    hoist(blocks, signoff)
}

/// Lead with the trailing image stacks inside one flow wrapper when the layer
/// has other blocks before them; otherwise keep the layer as it is.
fn hoist<'a>(
    mut blocks: Vec<(Block, Vec<MarkdownEvent<'a>>)>,
    signoff: Option<&str>,
) -> (Vec<MarkdownEvent<'a>>, bool) {
    let line = blocks
        .last()
        .is_some_and(|(kind, _)| *kind == Block::SignoffLine)
        .then(|| blocks.pop().map(|(_, block)| block))
        .flatten();
    let first_stack = blocks
        .iter()
        .rposition(|(kind, _)| *kind != Block::Stack)
        .map_or(0, |index| index + 1);
    if first_stack == 0 || first_stack == blocks.len() {
        let events = blocks
            .into_iter()
            .flat_map(|(_, block)| block)
            .chain(line.into_iter().flatten())
            .collect();
        return (events, false);
    }
    let stacks = blocks.split_off(first_stack);
    let mut events = vec![
        MarkdownEvent::Html(FLOW_OPEN.into()),
        MarkdownEvent::InlineHtml(STACK_OPEN.into()),
    ];
    for (_, stack) in stacks {
        events.extend(stack[1..stack.len() - 1].iter().cloned());
    }
    events.push(MarkdownEvent::InlineHtml(STACK_CLOSE.into()));
    let ends_in_paragraph = blocks
        .last()
        .is_some_and(|(kind, _)| *kind == Block::Paragraph);
    events.extend(blocks.into_iter().flat_map(|(_, block)| block));
    match signoff.filter(|signoff| !signoff.is_empty()) {
        Some(signoff) if ends_in_paragraph => {
            let end = events.len() - 1;
            events.insert(end, MarkdownEvent::InlineHtml(signoff.to_owned().into()));
        }
        Some(signoff) => events.push(MarkdownEvent::Html(
            format!("{SIGNOFF_LINE_OPEN}{signoff}</p>").into(),
        )),
        None => {}
    }
    events.extend(line.into_iter().flatten());
    events.push(MarkdownEvent::Html(FLOW_CLOSE.into()));
    (events, signoff.is_some())
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
            _ => (Block::Other, index),
        };
        blocks.push((kind, index..end + 1));
        index = end + 1;
    }
    blocks
}
