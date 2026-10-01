//! Authored task text: descriptions, acceptance criteria, and observations.
//!
//! Task text is Markdown written by operators and agents, so it keeps the
//! same sanitization as message cards: raw HTML stays literal text and unsafe
//! URL schemes lose their destination. A line break is authored structure and
//! stays one. Only handles that resolve on the admitted board become task
//! chips; any other handle stays plain text, so no chip leads nowhere. Code
//! and link text stay literal, so a handle, path, or command quoted as code or
//! used as a link label never gains a second, nested control.

use pulldown_cmark::{CowStr, Event as MarkdownEvent, HeadingLevel, Options, Parser, Tag, TagEnd};
use wire::TaskStamp;

use super::{coalesce_text_events, sanitize_markdown_event, tables, task_chip_html};
use crate::watchband;

/// Headings nest under the detail's own section and subsection headings.
const TASK_TEXT_HEADING: HeadingLevel = HeadingLevel::H5;

/// Render one authored task text field as sanitized HTML.
///
/// `resolves` answers whether a task stamp is present on the admitted board.
pub(crate) fn render_task_text(markdown: &str, resolves: impl Fn(&TaskStamp) -> bool) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let events = Parser::new_ext(markdown, options)
        .map(sanitize_markdown_event)
        .map(|event| match event {
            MarkdownEvent::Start(Tag::Heading {
                id, classes, attrs, ..
            }) => MarkdownEvent::Start(Tag::Heading {
                level: TASK_TEXT_HEADING,
                id,
                classes,
                attrs,
            }),
            MarkdownEvent::End(TagEnd::Heading(_)) => {
                MarkdownEvent::End(TagEnd::Heading(TASK_TEXT_HEADING))
            }
            MarkdownEvent::SoftBreak => MarkdownEvent::HardBreak,
            other => other,
        })
        .collect();
    let mut literal = 0usize;
    let mut linked = Vec::new();
    for event in coalesce_text_events(events) {
        match event {
            MarkdownEvent::Start(Tag::CodeBlock(_) | Tag::Link { .. } | Tag::Image { .. }) => {
                literal += 1;
                linked.push(event);
            }
            MarkdownEvent::End(TagEnd::CodeBlock | TagEnd::Link | TagEnd::Image) => {
                literal = literal.saturating_sub(1);
                linked.push(event);
            }
            MarkdownEvent::Text(text) if literal == 0 => {
                push_task_chips(text, &resolves, &mut linked)
            }
            other => linked.push(other),
        }
    }
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, tables::lay_out_tables(linked));
    html
}

fn push_task_chips<'a>(
    text: CowStr<'a>,
    resolves: &impl Fn(&TaskStamp) -> bool,
    events: &mut Vec<MarkdownEvent<'a>>,
) {
    let mentions = watchband::task_mentions(&text)
        .into_iter()
        .filter(|(_, task)| task.born().is_ok_and(|stamp| resolves(&stamp)))
        .collect::<Vec<_>>();
    if mentions.is_empty() {
        events.push(MarkdownEvent::Text(text));
        return;
    }
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
}

#[cfg(test)]
mod tests;
