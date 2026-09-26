//! Synchronous HTML projection of resident-authenticated quote trees.

use std::collections::BTreeMap;

use pulldown_cmark::{Event as MarkdownEvent, Tag, TagEnd};
use wire::{CanonicalUuid, Disposition, QuoteAuthor, QuotePresentation, QuoteTarget, TaskHandle};

pub(super) fn expand<'a>(
    events: Vec<MarkdownEvent<'a>>,
    presentations: &[QuotePresentation],
    agents: &BTreeMap<CanonicalUuid, String>,
    tasks: &BTreeMap<String, TaskHandle>,
) -> Vec<MarkdownEvent<'a>> {
    if presentations.is_empty() {
        return events;
    }
    let mut expanded = Vec::with_capacity(events.len());
    let mut event = 0;
    let mut presentation = 0;
    while event < events.len() {
        let Some(target) = structural_quote_target(&events[event..]) else {
            expanded.push(events[event].clone());
            event += 1;
            continue;
        };
        let Some(node) = presentations
            .get(presentation)
            .filter(|node| presentation_target(node) == target)
        else {
            expanded.push(events[event].clone());
            event += 1;
            continue;
        };
        expanded.push(MarkdownEvent::InlineHtml(
            render(node, agents, tasks).into(),
        ));
        event += 3;
        presentation += 1;
    }
    expanded
}

fn structural_quote_target<'event>(events: &'event [MarkdownEvent<'_>]) -> Option<&'event str> {
    match events {
        [
            MarkdownEvent::Start(Tag::Link { dest_url, .. }),
            MarkdownEvent::Text(label),
            MarkdownEvent::End(TagEnd::Link),
            ..,
        ] if label.as_ref() == "quote" && dest_url.starts_with("spice://quote/v1/") => {
            Some(dest_url.as_ref())
        }
        _ => None,
    }
}

fn presentation_target(presentation: &QuotePresentation) -> &str {
    match presentation {
        QuotePresentation::Resolved { target, .. }
        | QuotePresentation::Unavailable { target, .. } => target,
    }
}

fn render(
    presentation: &QuotePresentation,
    agents: &BTreeMap<CanonicalUuid, String>,
    tasks: &BTreeMap<String, TaskHandle>,
) -> String {
    match presentation {
        QuotePresentation::Resolved {
            target,
            text,
            author,
            disposition,
            quotes,
        } => {
            let body = super::render_received_with_quotes(text, &[], quotes, agents, tasks);
            format!(
                "<div class=\"message-quote-card\" data-quote-state=\"resolved\" \
                 data-quote-target=\"{}\"><span class=\"message-quote-header\">{}{}{}</span>\
                 <div class=\"message-quote-body\">{}</div></div>",
                super::escape_html_attribute(target),
                author_html(author, agents),
                disposition.map(disposition_html).unwrap_or_default(),
                task_html(target, tasks),
                body.html,
            )
        }
        QuotePresentation::Unavailable { target, reason } => {
            let reason = unavailable_key(*reason);
            format!(
                "<div class=\"message-quote-card message-quote-card--unavailable\" \
                 data-quote-state=\"unavailable\" data-quote-reason=\"{reason}\" \
                 data-quote-target=\"{}\"><span class=\"message-quote-header\">\
                 <span class=\"message-quote-author\">Quote unavailable: {reason}</span></span>\
                 <div class=\"message-quote-body\"><code>{}</code></div></div>",
                super::escape_html_attribute(target),
                escape_html_text(target),
            )
        }
    }
}

fn author_html(author: &QuoteAuthor, agents: &BTreeMap<CanonicalUuid, String>) -> String {
    match author {
        QuoteAuthor::Agent { lane, .. } => {
            let label = agents
                .get(lane)
                .map(String::as_str)
                .unwrap_or(crate::watchband::UNKNOWN_AGENT_LABEL);
            format!(
                "<span class=\"message-quote-author\" data-quote-author=\"agent\">\
                 Agent: {}</span>",
                escape_html_text(label),
            )
        }
        QuoteAuthor::Directive { actor } => {
            let actor = actor.to_string();
            format!(
                "<span class=\"message-quote-author\" data-quote-author=\"directive\">\
                 Directive by {}</span>",
                escape_html_text(&actor),
            )
        }
    }
}

fn disposition_html(disposition: Disposition) -> String {
    let (key, class, label) = match disposition {
        Disposition::Live => ("live", " active", "LIVE"),
        Disposition::Acked => ("acked", " active", "ACK"),
        Disposition::Declined => ("nacked", " warning", "NACK"),
        Disposition::Lapsed => ("lapsed", " warning", "LAPSED"),
        Disposition::DeadLettered => ("dead-letter", " warning", "DEAD LETTER"),
        Disposition::Canceled => ("canceled", "", "CANCELED"),
    };
    format!(
        "<span class=\"message-quote-disposition tag{class}\" \
         data-quote-disposition=\"{key}\">{label}</span>"
    )
}

fn task_html(target: &str, tasks: &BTreeMap<String, TaskHandle>) -> String {
    let Ok(QuoteTarget::Steering { key, .. }) = target.parse() else {
        return String::new();
    };
    let Some(handle) = tasks.get(&key) else {
        return String::new();
    };
    let handle = handle.to_string();
    format!(
        "<span class=\"message-quote-task\" role=\"button\" tabindex=\"0\" \
         data-task-handle=\"{}\" aria-label=\"Open task {} in Work\">Work</span>",
        super::escape_html_attribute(&handle),
        super::escape_html_attribute(&handle),
    )
}

const fn unavailable_key(reason: wire::QuoteUnavailable) -> &'static str {
    match reason {
        wire::QuoteUnavailable::Malformed => "malformed",
        wire::QuoteUnavailable::Unknown => "unknown",
        wire::QuoteUnavailable::Unreadable => "unreadable",
        wire::QuoteUnavailable::DigestMismatch => "digest-mismatch",
        wire::QuoteUnavailable::DepthExceeded => "depth-exceeded",
    }
}

fn escape_html_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            _ => escaped.push(character),
        }
    }
    escaped
}
