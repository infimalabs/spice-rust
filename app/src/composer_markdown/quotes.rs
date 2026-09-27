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
    tasks: &crate::runtime::QuoteTasks,
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
///
/// When the quoted message's card is known, the author's name is the link to it.
pub(super) fn signoff_html(
    author: &QuoteAuthor,
    target: &str,
    agents: &super::QuoteAgents,
    sources: &super::QuoteSources,
    tasks: &crate::runtime::QuoteTasks,
) -> String {
    let author = author_byline(author, target, agents, sources);
    let author = sources.get(target).map_or_else(
        || author.html(),
        |source| author.linked_html(&source.card_id),
    );
    format!(
        "<span class=\"message-quote-signoff\">{author}{}</span>",
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
    tasks: &crate::runtime::QuoteTasks,
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

/// One byline author: its data attributes, visible label, and accessible name.
struct Byline {
    /// Data attributes identifying the author, each with a leading space.
    data: String,
    /// Escaped visible label.
    label: String,
    /// Plain accessible name.
    name: String,
    /// How the author renders without a source link.
    form: BylineForm,
}

enum BylineForm {
    /// Opens the current agent's information.
    Button,
    /// Named by its visible label.
    Text,
    /// Named by its accessible name, which qualifies a terse label.
    Labeled,
    /// Named only by its accessible name, such as an emoji mark.
    Image,
}

impl Byline {
    fn html(&self) -> String {
        let name = super::escape_html_attribute(&self.name);
        let Self { data, label, .. } = self;
        match self.form {
            BylineForm::Button => format!(
                "<button type=\"button\" class=\"message-quote-author\"{data} \
                 aria-label=\"Open information for {name}\">{label}</button>"
            ),
            BylineForm::Text => {
                format!("<span class=\"message-quote-author\"{data}>{label}</span>")
            }
            BylineForm::Labeled => format!(
                "<span class=\"message-quote-author\"{data} \
                 aria-label=\"{name}\">{label}</span>"
            ),
            BylineForm::Image => format!(
                "<span class=\"message-quote-author\"{data} role=\"img\" \
                 aria-label=\"{name}\">{label}</span>"
            ),
        }
    }

    fn linked_html(&self, card_id: &str) -> String {
        format!(
            "<a class=\"message-quote-author\" href=\"{}\" data-quote-source=\"{}\"{} \
             aria-label=\"Open message from {}\">{}</a>",
            super::escape_html_attribute(&crate::view::card_href(card_id)),
            super::escape_html_attribute(card_id),
            self.data,
            super::escape_html_attribute(&self.name),
            self.label,
        )
    }
}

fn author_byline(
    author: &QuoteAuthor,
    target: &str,
    agents: &super::QuoteAgents,
    sources: &super::QuoteSources,
) -> Byline {
    match author {
        QuoteAuthor::Agent { lane, occupant } => {
            if let Some(agent) = agents.get(&(*lane, *occupant)) {
                return Byline {
                    data: format!(
                        " data-quote-author=\"agent\" data-quote-agent-team=\"{}\" \
                         data-quote-agent-lane=\"{lane}\" data-quote-agent-occupant=\"{occupant}\" \
                         data-quote-author-state=\"current\" data-accent=\"{}\"",
                        agent.team, agent.accent,
                    ),
                    label: escape_html_text(&agent.label),
                    name: agent.label.clone(),
                    form: BylineForm::Button,
                };
            }
            if let Some(agent) = sources.get(target).and_then(|source| source.agent.as_ref()) {
                return agent_byline(*lane, *occupant, agent, "source");
            }
            if let Some(agent) = agents
                .iter()
                .find_map(|((candidate, _), agent)| (*candidate == *lane).then_some(agent))
            {
                return agent_byline(*lane, *occupant, agent, "lane");
            }
            let label = format!(
                "{}/{}",
                crate::watchband::short(*lane),
                crate::watchband::short(*occupant),
            );
            Byline {
                data: format!(
                    " data-quote-author=\"agent\" data-quote-agent-lane=\"{lane}\" \
                     data-quote-agent-occupant=\"{occupant}\" \
                     data-quote-author-state=\"unresolved\""
                ),
                name: format!("Agent {label}"),
                label,
                form: BylineForm::Labeled,
            }
        }
        QuoteAuthor::Directive { actor } => match actor {
            wire::Actor::Operator => Byline {
                data: " data-quote-author=\"directive\" data-quote-actor=\"operator\"".to_owned(),
                label: "Operator".to_owned(),
                name: "Operator".to_owned(),
                form: BylineForm::Text,
            },
            wire::Actor::Spice => Byline {
                data: " data-quote-author=\"directive\" data-quote-actor=\"spice\"".to_owned(),
                label: "🌶️".to_owned(),
                name: "Spice".to_owned(),
                form: BylineForm::Image,
            },
            wire::Actor::Lane(lane) => {
                if let Some(agent) = agents
                    .iter()
                    .find_map(|((candidate, _), agent)| (*candidate == *lane).then_some(agent))
                {
                    return Byline {
                        data: format!(
                            " data-quote-author=\"directive\" data-quote-actor=\"lane:{lane}\" \
                             data-quote-author-state=\"lane\" data-accent=\"{}\"",
                            agent.accent,
                        ),
                        label: escape_html_text(&agent.label),
                        name: agent.label.clone(),
                        form: BylineForm::Text,
                    };
                }
                let label = crate::watchband::short(*lane);
                Byline {
                    data: format!(
                        " data-quote-author=\"directive\" data-quote-actor=\"lane:{lane}\" \
                         data-quote-author-state=\"unresolved\""
                    ),
                    name: format!("Lane {label}"),
                    label,
                    form: BylineForm::Labeled,
                }
            }
        },
    }
}

fn agent_byline(
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    agent: &super::QuoteAgent,
    state: &str,
) -> Byline {
    Byline {
        data: format!(
            " data-quote-author=\"agent\" data-quote-agent-lane=\"{lane}\" \
             data-quote-agent-occupant=\"{occupant}\" data-quote-author-state=\"{state}\" \
             data-accent=\"{}\"",
            agent.accent,
        ),
        label: escape_html_text(&agent.label),
        name: agent.label.clone(),
        form: BylineForm::Labeled,
    }
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

fn task_html(target: &str, tasks: &crate::runtime::QuoteTasks) -> String {
    let Ok(QuoteTarget::Steering { key, .. }) = target.parse() else {
        return String::new();
    };
    tasks.get(&key).map_or_else(String::new, |handles| {
        let mut html = String::new();
        for handle in handles {
            let handle = super::escape_html_attribute(handle.as_str());
            write!(
                html,
                "<span class=\"message-quote-task\" role=\"button\" tabindex=\"0\" \
                 data-task-handle=\"{handle}\" aria-label=\"Open task {handle} in Work\">\
                 Work</span>"
            )
            .expect("writing HTML to a String cannot fail");
        }
        html
    })
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
