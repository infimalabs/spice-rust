//! Synchronous HTML projection of resident-authenticated quote trees.

use std::borrow::Cow;
use std::collections::BTreeMap;

use pulldown_cmark::{Event as MarkdownEvent, Tag, TagEnd};
use wire::{Disposition, QuoteAuthor, QuotePresentation, QuoteTarget, TaskHandle};

pub(super) fn expand<'a>(
    events: Vec<MarkdownEvent<'a>>,
    presentations: &[QuotePresentation],
    agents: &super::QuoteAgents,
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
    agents: &super::QuoteAgents,
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
            let disposition = disposition.map(disposition_html).unwrap_or_default();
            let body = body_with_disposition(&body.html, &disposition);
            format!(
                "<div class=\"message-quote-card\" data-quote-state=\"resolved\" \
                 data-quote-target=\"{}\"><div class=\"message-quote-body\">{body}</div>\
                 <span class=\"message-quote-signoff\">— {}{}</span></div>",
                super::escape_html_attribute(target),
                author_html(author, agents),
                task_html(target, tasks),
            )
        }
        QuotePresentation::Unavailable { target, reason } => {
            let reason = unavailable_key(*reason);
            format!(
                "<div class=\"message-quote-card message-quote-card--unavailable\" \
                 data-quote-state=\"unavailable\" data-quote-reason=\"{reason}\" \
                 data-quote-target=\"{}\"><span class=\"message-quote-unavailable\">\
                 Quote unavailable: {reason} · authorship unavailable</span></div>",
                super::escape_html_attribute(target),
            )
        }
    }
}

fn body_with_disposition<'a>(body: &'a str, disposition: &str) -> Cow<'a, str> {
    if disposition.is_empty() {
        return Cow::Borrowed(body);
    }
    body.strip_prefix("<p>").map_or_else(
        || Cow::Owned(format!("{disposition}{body}")),
        |prose| Cow::Owned(format!("<p>{disposition} {prose}")),
    )
}

fn author_html(author: &QuoteAuthor, agents: &super::QuoteAgents) -> String {
    match author {
        QuoteAuthor::Agent { lane, occupant } => {
            if let Some(label) = agents.get(&(*lane, *occupant)) {
                let label = format!("Agent {label}");
                return format!(
                    "<a class=\"message-quote-author\" data-quote-author=\"agent\" \
                     data-quote-agent-lane=\"{lane}\" data-quote-agent-occupant=\"{occupant}\" \
                     data-quote-history=\"available\" href=\"#/info\" \
                     aria-label=\"Open information for {}\">{}</a>",
                    super::escape_html_attribute(&label),
                    escape_html_text(&label),
                );
            }
            let label = format!(
                "Agent {}/{}",
                crate::watchband::short(*lane),
                crate::watchband::short(*occupant),
            );
            format!(
                "<span class=\"message-quote-author\" data-quote-author=\"agent\" \
                 data-quote-agent-lane=\"{lane}\" data-quote-agent-occupant=\"{occupant}\" \
                 data-quote-history=\"unavailable\" \
                 aria-label=\"{}; identity history unavailable\">{} \
                 <span class=\"message-quote-history-unavailable\">· history unavailable</span></span>",
                super::escape_html_attribute(&label),
                escape_html_text(&label),
            )
        }
        QuoteAuthor::Directive { actor } => match actor {
            wire::Actor::Operator => available_principal_html("Operator", "operator"),
            wire::Actor::Spice => available_principal_html("Spice", "spice"),
            wire::Actor::Lane(lane) => {
                let label = format!("Agent channel {}", crate::watchband::short(*lane));
                format!(
                    "<span class=\"message-quote-author\" data-quote-author=\"directive\" \
                     data-quote-actor=\"lane:{lane}\" data-quote-history=\"unavailable\" \
                     aria-label=\"{}; identity history unavailable\">{} \
                     <span class=\"message-quote-history-unavailable\">· history unavailable</span></span>",
                    super::escape_html_attribute(&label),
                    escape_html_text(&label),
                )
            }
        },
    }
}

fn available_principal_html(label: &str, actor: &str) -> String {
    format!(
        "<a class=\"message-quote-author\" data-quote-author=\"directive\" \
         data-quote-actor=\"{actor}\" data-quote-history=\"available\" href=\"#/info\" \
         aria-label=\"Open information for {label}\">{label}</a>"
    )
}

fn disposition_html(disposition: Disposition) -> String {
    let (key, label) = match disposition {
        Disposition::Live => ("live", "LIVE"),
        Disposition::Acked => {
            return super::acknowledgement_pill_html_with_attributes(
                "ack",
                "acked",
                "Acknowledgement classification",
                "ACK",
                " data-quote-disposition=\"acked\"",
            );
        }
        Disposition::Declined => {
            return super::acknowledgement_pill_html_with_attributes(
                "nack",
                "declined",
                "Declined classification",
                "NACK",
                " data-quote-disposition=\"nacked\"",
            );
        }
        Disposition::Lapsed => ("lapsed", "LAPSED"),
        Disposition::DeadLettered => ("dead-letter", "DEAD LETTER"),
        Disposition::Canceled => ("canceled", "CANCELED"),
    };
    format!(
        "<span class=\"message-quote-disposition\" \
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
