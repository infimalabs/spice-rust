//! Synchronous HTML projection of resident-authenticated quote trees.

use std::borrow::Cow;
use std::fmt::Write as _;

use pulldown_cmark::{Event as MarkdownEvent, Tag, TagEnd};
use wire::{Disposition, QuoteAuthor, QuotePresentation, QuoteTarget};

pub(super) fn expand<'a>(
    events: Vec<MarkdownEvent<'a>>,
    presentations: &[QuotePresentation],
    agents: &super::QuoteAgents,
    sources: &super::QuoteSources,
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
            render(node, agents, sources).into(),
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

/// The task badges caused by the directions one card's quotes and directive
/// contexts name, so a card holds only its own slice of the board's origin index.
pub(crate) fn referenced_quote_tasks(
    presentations: &[QuotePresentation],
    directives: &[wire::MessageDirective],
    tasks: &crate::runtime::QuoteTasks,
) -> crate::runtime::QuoteTasks {
    let mut referenced = crate::runtime::QuoteTasks::new();
    let mut take = |target: &QuoteTarget| {
        if let QuoteTarget::Steering { key, .. } = target
            && let Some(handles) = tasks.get(key)
        {
            referenced.insert(key.clone(), handles.clone());
        }
    };
    for directive in directives {
        take(&directive.quote_target);
    }
    let mut pending = presentations.iter().collect::<Vec<_>>();
    while let Some(presentation) = pending.pop() {
        let QuotePresentation::Resolved { target, quotes, .. } = presentation else {
            continue;
        };
        if let Ok(target) = target.parse() {
            take(&target);
        }
        pending.extend(quotes);
    }
    referenced
}

/// The head of quoted words: the source message's live relative age, then an
/// explicit non-Operator author name.
pub(super) fn signoff_html(
    author: &QuoteAuthor,
    target: &str,
    agents: &super::QuoteAgents,
    sources: &super::QuoteSources,
) -> String {
    let Some(source) = sources.get(target) else {
        return String::new();
    };
    let age = quote_age_slot_html(author, agents, source);
    let name = quote_author_name_html(author, agents, source);
    format!("<span class=\"message-quote-signoff\">{age}{name}</span>")
}

/// Empty portal target that board provenance fills at the quoted content's
/// trailing edge after the card has rendered.
pub(super) fn task_tail_html(target: &str) -> String {
    let Ok(QuoteTarget::Steering { key, .. }) = target.parse() else {
        return String::new();
    };
    format!(
        "<span class=\"message-quote-task-tail\" data-quote-tasks=\"{}\"></span>",
        super::escape_html_attribute(&key)
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
) -> String {
    match presentation {
        QuotePresentation::Resolved {
            target,
            text,
            author,
            disposition,
            quotes,
        } => {
            let disposition = disposition.map(disposition_html).unwrap_or_default();
            let signoff = signoff_html(author, target, agents, sources);
            let task_tail = task_tail_html(target);
            let body = super::render_quote_card_body(
                text,
                quotes,
                agents,
                sources,
                super::image_flow::CardHead {
                    signoff: &signoff,
                    disposition: &disposition,
                    task_tail: &task_tail,
                },
            );
            format!(
                "<div class=\"message-quote-card\" data-quote-state=\"resolved\" \
                 data-quote-target=\"{}\"><div class=\"message-quote-body\">{}</div></div>",
                super::escape_html_attribute(target),
                body.html,
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

fn quote_author_name_html(
    author: &QuoteAuthor,
    agents: &super::QuoteAgents,
    source: &super::QuoteSource,
) -> String {
    match author {
        QuoteAuthor::Agent { lane, occupant } => {
            let (agent, state) = agents
                .get(&(*lane, Some(*occupant)))
                .map(|agent| (Some(agent), "current"))
                .or_else(|| {
                    source
                        .agent
                        .as_ref()
                        .map(|agent| (Some(agent), "historical"))
                })
                .unwrap_or((None, "unresolved"));
            let label = agent.map_or_else(
                || {
                    Cow::Owned(format!(
                        "{}/{}",
                        crate::watchband::short(*lane),
                        crate::watchband::short(*occupant),
                    ))
                },
                |agent| Cow::Borrowed(agent.label.as_str()),
            );
            quote_name_html(
                &label,
                &format!(
                    " data-quote-agent-lane=\"{lane}\" data-quote-agent-occupant=\"{occupant}\" \
                     data-quote-author-state=\"{state}\""
                ),
                agent.map(|agent| agent.accent),
            )
        }
        QuoteAuthor::Directive {
            actor: wire::Actor::Operator,
        } => quote_name_html(
            "",
            " data-quote-author=\"directive\" data-quote-actor=\"operator\" role=\"img\" \
             aria-label=\"Operator\"",
            None,
        ),
        QuoteAuthor::Directive {
            actor: wire::Actor::Spice,
        } => quote_name_html(
            "🌶️",
            " data-quote-author=\"directive\" data-quote-actor=\"spice\" aria-label=\"Spice\"",
            None,
        ),
        QuoteAuthor::Directive {
            actor: wire::Actor::Lane(lane),
        } => {
            // A peer message names its sending lane, not an occupant: the
            // lane's team member speaks for it, seated or vacant.
            let member = lane_member_agent(agents, *lane);
            let (label, state) = member.map_or_else(
                || {
                    (
                        Cow::Owned(format!("lane {}", crate::watchband::short(*lane))),
                        "lane",
                    )
                },
                |agent| (Cow::Borrowed(agent.label.as_str()), "current"),
            );
            quote_name_html(
                &label,
                &format!(
                    " data-quote-author=\"directive\" data-quote-actor=\"lane:{lane}\" \
                     data-quote-author-state=\"{state}\""
                ),
                member.map(|agent| agent.accent),
            )
        }
    }
}

fn quote_name_html(label: &str, data: &str, accent: Option<usize>) -> String {
    let accent = accent.map_or_else(String::new, |accent| format!(" data-accent=\"{accent}\""));
    format!(
        "<span class=\"message-quote-name\"{data}{accent}>{}</span>",
        escape_html_text(label),
    )
}

fn quote_age_slot_html(
    author: &QuoteAuthor,
    agents: &super::QuoteAgents,
    source: &super::QuoteSource,
) -> String {
    let mut data = match author {
        QuoteAuthor::Agent { lane, occupant } => {
            let state = if agents.contains_key(&(*lane, Some(*occupant))) {
                "current"
            } else {
                "historical"
            };
            format!(
                " data-quote-author=\"agent\" data-quote-agent-lane=\"{lane}\" \
                 data-quote-agent-occupant=\"{occupant}\" data-quote-author-state=\"{state}\""
            )
        }
        QuoteAuthor::Directive { actor } => {
            let actor = match actor {
                wire::Actor::Operator => "operator".to_owned(),
                wire::Actor::Spice => "spice".to_owned(),
                wire::Actor::Lane(lane) => format!("lane:{lane}"),
            };
            format!(
                " data-quote-author=\"directive\" data-quote-actor=\"{}\"",
                super::escape_html_attribute(&actor),
            )
        }
    };
    let accent = source
        .agent
        .as_ref()
        .map(|agent| agent.accent)
        .or_else(|| match author {
            QuoteAuthor::Directive {
                actor: wire::Actor::Lane(lane),
            } => lane_member_agent(agents, *lane).map(|agent| agent.accent),
            QuoteAuthor::Agent { .. } | QuoteAuthor::Directive { .. } => None,
        });
    if let Some(accent) = accent {
        write!(data, " data-accent=\"{accent}\"").expect("writing HTML to a String cannot fail");
    }
    let source_data = source.card_id.as_ref().map_or_else(String::new, |card_id| {
        format!(
            " data-quote-source=\"{}\"",
            super::escape_html_attribute(card_id)
        )
    });
    format!(
        "<span class=\"message-quote-age-slot\" data-quote-age-authored=\"{}\"{source_data}{data}></span>",
        source.display_millis,
    )
}

/// The team member for one lane, seated or vacant, which speaks for a peer
/// message that lane told: a lane's directive names the lane, never an
/// occupant.
fn lane_member_agent(
    agents: &super::QuoteAgents,
    lane: wire::CanonicalUuid,
) -> Option<&super::QuoteAgent> {
    agents
        .iter()
        .find_map(|((agent_lane, _), agent)| (*agent_lane == lane).then_some(agent))
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
                "nak",
                "declined",
                "Declined classification",
                "NAK",
                " data-quote-disposition=\"declined\"",
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
