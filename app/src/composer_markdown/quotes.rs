//! Synchronous HTML projection of resident-authenticated quote trees.

use std::borrow::Cow;
use std::collections::BTreeMap;

use pulldown_cmark::{Event as MarkdownEvent, Tag, TagEnd};
use wire::{Disposition, QuoteAuthor, QuotePresentation, QuoteTarget, TaskHandle};

pub(super) fn expand<'a>(
    events: Vec<MarkdownEvent<'a>>,
    presentations: &[QuotePresentation],
    agents: &super::QuoteAgents,
    sources: &super::QuoteSources,
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
            render(node, agents, sources, tasks).into(),
        ));
        event += 3;
        presentation += 1;
    }
    expanded
}

/// The part of `agents` a card's quotes and directive contexts can render.
///
/// Authors resolve labels only among entries of the lane they name, so a card
/// rendered from this projection is identical to one rendered from the whole
/// roster, and a seat elsewhere changing occupancy leaves it untouched.
pub(crate) fn referenced_quote_agents(
    presentations: &[QuotePresentation],
    directives: &[wire::MessageDirective],
    agents: &super::QuoteAgents,
) -> super::QuoteAgents {
    let mut lanes = directives
        .iter()
        .filter_map(|directive| match directive.actor {
            wire::Actor::Lane(lane) => Some(lane),
            wire::Actor::Operator | wire::Actor::Spice => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    let mut pending = presentations.iter().collect::<Vec<_>>();
    while let Some(presentation) = pending.pop() {
        let QuotePresentation::Resolved { author, quotes, .. } = presentation else {
            continue;
        };
        match author {
            QuoteAuthor::Agent { lane, .. }
            | QuoteAuthor::Directive {
                actor: wire::Actor::Lane(lane),
            } => {
                lanes.insert(*lane);
            }
            QuoteAuthor::Directive { .. } => {}
        }
        pending.extend(quotes);
    }
    agents
        .iter()
        .filter(|((lane, _), _)| lanes.contains(lane))
        .map(|(key, agent)| (*key, agent.clone()))
        .collect()
}

/// The source cards referenced by one card's quotes and directive contexts.
pub(crate) fn referenced_quote_sources(
    presentations: &[QuotePresentation],
    directives: &[wire::MessageDirective],
    sources: &super::QuoteSources,
) -> super::QuoteSources {
    let mut referenced = super::QuoteSources::new();
    for directive in directives {
        let target = directive.quote_target.to_string();
        if let Some(source) = sources.get(&target) {
            referenced.insert(target, source.clone());
        }
    }
    let mut pending = presentations.iter().collect::<Vec<_>>();
    while let Some(presentation) = pending.pop() {
        let QuotePresentation::Resolved { target, quotes, .. } = presentation else {
            continue;
        };
        if let Some(source) = sources.get(target) {
            referenced.insert(target.clone(), source.clone());
        }
        pending.extend(quotes);
    }
    referenced
}

/// The byline closing quoted words: who authored `target`, and its task.
pub(super) fn signoff_html(
    author: &QuoteAuthor,
    target: &str,
    agents: &super::QuoteAgents,
    sources: &super::QuoteSources,
    tasks: &BTreeMap<String, TaskHandle>,
) -> String {
    format!(
        "<span class=\"message-quote-signoff\">{}{}{}</span>",
        author_html(author, target, agents, sources),
        source_html(target, sources),
        task_html(target, tasks),
    )
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
    sources: &super::QuoteSources,
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
            let body =
                super::render_received_with_quotes(text, &[], &[], quotes, agents, sources, tasks);
            let disposition = disposition.map(disposition_html).unwrap_or_default();
            let body = body_with_disposition(&body.html, &disposition);
            format!(
                "<div class=\"message-quote-card\" data-quote-state=\"resolved\" \
                 data-quote-target=\"{}\"><div class=\"message-quote-body\">{body}</div> \
                 {}</div>",
                super::escape_html_attribute(target),
                signoff_html(author, target, agents, sources, tasks),
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

fn author_html(
    author: &QuoteAuthor,
    target: &str,
    agents: &super::QuoteAgents,
    sources: &super::QuoteSources,
) -> String {
    match author {
        QuoteAuthor::Agent { lane, occupant } => {
            if let Some(agent) = agents.get(&(*lane, *occupant)) {
                let label = &agent.label;
                return format!(
                    "<button type=\"button\" class=\"message-quote-author\" \
                     data-quote-author=\"agent\" data-quote-agent-team=\"{}\" \
                     data-quote-agent-lane=\"{lane}\" data-quote-agent-occupant=\"{occupant}\" \
                     data-quote-author-state=\"current\" data-accent=\"{}\" \
                     aria-label=\"Open information for {}\">{}</button>",
                    agent.team,
                    agent.accent,
                    super::escape_html_attribute(label),
                    escape_html_text(label),
                );
            }
            if let Some(agent) = sources.get(target).and_then(|source| source.agent.as_ref()) {
                return agent_label_html(*lane, *occupant, agent, "source");
            }
            if let Some(agent) = agents
                .iter()
                .find_map(|((candidate, _), agent)| (*candidate == *lane).then_some(agent))
            {
                return agent_label_html(*lane, *occupant, agent, "lane");
            }
            let label = format!(
                "{}/{}",
                crate::watchband::short(*lane),
                crate::watchband::short(*occupant),
            );
            format!(
                "<span class=\"message-quote-author\" data-quote-author=\"agent\" \
                 data-quote-agent-lane=\"{lane}\" data-quote-agent-occupant=\"{occupant}\" \
                 data-quote-author-state=\"unresolved\" \
                 aria-label=\"Agent {label}\">{label}</span>"
            )
        }
        QuoteAuthor::Directive { actor } => match actor {
            wire::Actor::Operator => principal_html("Operator", "operator"),
            wire::Actor::Spice => "<span class=\"message-quote-author\" \
                data-quote-author=\"directive\" data-quote-actor=\"spice\" \
                role=\"img\" aria-label=\"Spice\">🌶️</span>"
                .to_owned(),
            wire::Actor::Lane(lane) => {
                if let Some(agent) = agents
                    .iter()
                    .find_map(|((candidate, _), agent)| (*candidate == *lane).then_some(agent))
                {
                    return format!(
                        "<span class=\"message-quote-author\" data-quote-author=\"directive\" \
                         data-quote-actor=\"lane:{lane}\" data-quote-author-state=\"lane\" \
                         data-accent=\"{}\">{}</span>",
                        agent.accent,
                        escape_html_text(&agent.label),
                    );
                }
                let label = crate::watchband::short(*lane);
                format!(
                    "<span class=\"message-quote-author\" data-quote-author=\"directive\" \
                     data-quote-actor=\"lane:{lane}\" data-quote-author-state=\"unresolved\" \
                     aria-label=\"Lane {label}\">{label}</span>"
                )
            }
        },
    }
}

fn agent_label_html(
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    agent: &super::QuoteAgent,
    state: &str,
) -> String {
    format!(
        "<span class=\"message-quote-author\" data-quote-author=\"agent\" \
         data-quote-agent-lane=\"{lane}\" data-quote-agent-occupant=\"{occupant}\" \
         data-quote-author-state=\"{state}\" data-accent=\"{}\" \
         aria-label=\"{}\">{}</span>",
        agent.accent,
        super::escape_html_attribute(&agent.label),
        escape_html_text(&agent.label),
    )
}

fn principal_html(label: &str, actor: &str) -> String {
    format!(
        "<span class=\"message-quote-author\" data-quote-author=\"directive\" \
         data-quote-actor=\"{actor}\">{label}</span>"
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

fn source_html(target: &str, sources: &super::QuoteSources) -> String {
    let Some(source) = sources.get(target) else {
        return String::new();
    };
    format!(
        "<a class=\"message-quote-source\" href=\"{}\" \
         data-quote-source=\"{}\" aria-label=\"Open original message\">Original</a>",
        super::escape_html_attribute(&crate::view::card_href(&source.card_id)),
        super::escape_html_attribute(&source.card_id),
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
