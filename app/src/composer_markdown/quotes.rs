//! Synchronous HTML projection of resident-authenticated quote trees.

use std::borrow::Cow;
use std::fmt::Write as _;

use pulldown_cmark::{Event as MarkdownEvent, Tag, TagEnd};
use wire::{Disposition, QuoteAttribution, QuoteAuthor, QuotePresentation, QuoteTarget};

/// The name an agent wears when its lane is absent from the roster.
const UNAVAILABLE: &str = "unavailable";

pub(super) fn expand<'a>(
    events: Vec<MarkdownEvent<'a>>,
    presentations: &[QuotePresentation],
    agents: &super::QuoteAgents,
    sources: &super::QuoteSources,
    quote_depth: usize,
) -> Vec<MarkdownEvent<'a>> {
    if presentations.is_empty() {
        return events;
    }
    let mut expanded = Vec::with_capacity(events.len());
    let mut event = 0;
    let mut presentation = 0;
    let mut raw_quote_depth = 0usize;
    while event < events.len() {
        let node = structural_quote_target(&events[event..]).and_then(|target| {
            presentations
                .get(presentation)
                .filter(|node| presentation_target(node) == target)
        });
        let Some(node) = node else {
            match &events[event] {
                MarkdownEvent::Start(Tag::BlockQuote(_)) => raw_quote_depth += 1,
                MarkdownEvent::End(TagEnd::BlockQuote(_)) => {
                    raw_quote_depth = raw_quote_depth.saturating_sub(1);
                }
                _ => {}
            }
            expanded.push(events[event].clone());
            event += 1;
            continue;
        };
        expanded.push(MarkdownEvent::InlineHtml(
            render(node, agents, sources, quote_depth + raw_quote_depth + 1).into(),
        ));
        event += 3;
        presentation += 1;
    }
    expanded
}

/// The part of `agents` a card's quotes, directive contexts, and sent MSG
/// packets can render.
///
/// Authors resolve labels only among entries of the lane they name, so a card
/// rendered from this projection is identical to one rendered from the whole
/// roster, and a seat elsewhere changing occupancy leaves it untouched.
pub(crate) fn referenced_quote_agents(
    presentations: &[QuotePresentation],
    directives: &[wire::MessageDirective],
    sends: &[wire::MessageSend],
    agents: &super::QuoteAgents,
) -> super::QuoteAgents {
    let mut lanes = directives
        .iter()
        .flat_map(|directive| attribution_lanes(&directive.attribution))
        .chain(sends.iter().flat_map(|send| {
            std::iter::once(send.from).chain(send.recipients.iter().map(|recipient| recipient.lane))
        }))
        .collect::<std::collections::BTreeSet<_>>();
    let mut pending = presentations.iter().collect::<Vec<_>>();
    while let Some(presentation) = pending.pop() {
        let (attribution, quotes) = match presentation {
            QuotePresentation::Resolved {
                attribution,
                quotes,
                ..
            } => (attribution, quotes.as_slice()),
            QuotePresentation::Media { attribution, .. } => (attribution, &[][..]),
            QuotePresentation::Unavailable { .. } => continue,
        };
        lanes.extend(attribution_lanes(attribution));
        pending.extend(quotes);
    }
    super::QuoteAgents {
        members: agents
            .members
            .iter()
            .filter(|((lane, _), _)| lanes.contains(lane))
            .map(|(key, agent)| (*key, agent.clone()))
            .collect(),
        principals: agents.principals.clone(),
    }
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
        let (target, quotes) = match presentation {
            QuotePresentation::Resolved { target, quotes, .. } => (target, quotes.as_slice()),
            QuotePresentation::Media { target, .. } => (target, &[][..]),
            QuotePresentation::Unavailable { .. } => continue,
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
        let key = match target {
            QuoteTarget::Steering { key, .. } | QuoteTarget::SteeringMedia { key, .. } => key,
            _ => return,
        };
        if let Some(handles) = tasks.get(key) {
            referenced.insert(key.clone(), handles.clone());
        }
    };
    for directive in directives {
        take(&directive.quote_target);
    }
    let mut pending = presentations.iter().collect::<Vec<_>>();
    while let Some(presentation) = pending.pop() {
        let (target, quotes) = match presentation {
            QuotePresentation::Resolved { target, quotes, .. } => (target, quotes.as_slice()),
            QuotePresentation::Media { target, .. } => (target, &[][..]),
            QuotePresentation::Unavailable { .. } => continue,
        };
        if let Ok(target) = target.parse() {
            take(&target);
        }
        pending.extend(quotes);
    }
    referenced
}

/// Every lane one attribution names: its author's, then each recipient's.
fn attribution_lanes(
    attribution: &QuoteAttribution,
) -> impl Iterator<Item = wire::CanonicalUuid> + '_ {
    let author = match attribution.author {
        QuoteAuthor::Agent { lane, .. }
        | QuoteAuthor::Directive {
            actor: wire::Actor::Lane(lane),
        } => Some(lane),
        QuoteAuthor::Directive { .. } => None,
    };
    author.into_iter().chain(
        attribution
            .recipients
            .iter()
            .map(|recipient| recipient.lane),
    )
}

/// The lead of each hunk of quoted words, the same wherever the source
/// appears: source age, author name, then every recipient its record proves.
pub(super) fn signoff_html(
    attribution: &QuoteAttribution,
    target: &str,
    agents: &super::QuoteAgents,
    sources: &super::QuoteSources,
) -> String {
    let Some(source) = sources.get(target) else {
        return String::new();
    };
    let age = quote_age_slot_html(&attribution.author, agents, source);
    let name = quote_author_name_html(&attribution.author, agents, source);
    let mut destination = String::new();
    for recipient in &attribution.recipients {
        push_recipient_separator(&mut destination);
        destination.push_str(&recipient_html(recipient, agents));
    }
    packet_signoff_html(&age, &name, &destination, "incoming")
}

/// One recipient: the exact occupant its record proves, else its lane.
fn recipient_html(recipient: &wire::QuoteRecipient, agents: &super::QuoteAgents) -> String {
    recipient.occupant.map_or_else(
        || lane_participant_html(recipient.lane, agents),
        |occupant| participant_html(recipient.lane, occupant, None, agents),
    )
}

/// The accent slot a delivered direction's quote wears: its first
/// recipient's, as that member's own cards wear it.
pub(super) fn recipient_accent(
    attribution: &QuoteAttribution,
    agents: &super::QuoteAgents,
) -> Option<usize> {
    let recipient = attribution.recipients.first()?;
    match recipient.occupant {
        Some(occupant) => {
            occupant_agent(agents, recipient.lane, occupant).map(|(agent, _)| agent.accent)
        }
        None => lane_member_agent(agents, recipient.lane).map(|agent| agent.accent),
    }
}

fn packet_signoff_html(age: &str, sender: &str, recipients: &str, direction: &str) -> String {
    if recipients.is_empty() {
        return format!("<span class=\"message-quote-signoff\">{age}{sender}</span>");
    }
    format!(
        "<span class=\"message-quote-signoff\" data-quote-direction=\"{direction}\">{age}{sender}\
         <span class=\"message-quote-direction\" role=\"img\" aria-label=\"sent to\">→</span>\
         <span class=\"message-quote-recipients\">{recipients}</span></span>"
    )
}

fn participant_html(
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    address: Option<wire::OccupantAddress>,
    agents: &super::QuoteAgents,
) -> String {
    let (agent, state) = occupant_agent(agents, lane, occupant)
        .map_or((None, "unresolved"), |(agent, state)| (Some(agent), state));
    let label = agent.map_or_else(
        || {
            Cow::Owned(
                address.map_or_else(|| UNAVAILABLE.to_owned(), |address| address.to_string()),
            )
        },
        |agent| Cow::Borrowed(agent.label.as_str()),
    );
    let address = address.map_or_else(String::new, |address| {
        format!(" data-quote-recipient-address=\"{address}\" title=\"{address}\"")
    });
    quote_name_html(
        &label,
        &format!(
            " data-quote-agent-lane=\"{lane}\" data-quote-agent-occupant=\"{occupant}\" \
             data-quote-author-state=\"{state}\"{address}"
        ),
        agent.map(|agent| agent.accent),
    )
}

/// A lane addressed as a whole, named by its team member, seated or vacant.
/// A lane gone from the roster reads as unavailable, never as raw identifiers.
fn lane_participant_html(lane: wire::CanonicalUuid, agents: &super::QuoteAgents) -> String {
    let member = lane_member_agent(agents, lane);
    let label = member.map_or(UNAVAILABLE, |agent| agent.label.as_str());
    quote_name_html(
        label,
        &format!(" data-quote-recipient-lane=\"{lane}\""),
        member.map(|agent| agent.accent),
    )
}

/// The accent slot of the speaker a quote's attribution names: the slot that
/// speaker's own top-level cards wear, carried on the quote's shell so its rail
/// wears it too. Operators and Spice speak in the neutral accent and have no
/// slot, as does a quote whose attribution is not shown.
pub(super) fn speaker_accent(
    author: &QuoteAuthor,
    target: &str,
    agents: &super::QuoteAgents,
    sources: &super::QuoteSources,
) -> Option<usize> {
    author_accent(author, agents, sources.get(target)?)
}

fn author_accent(
    author: &QuoteAuthor,
    agents: &super::QuoteAgents,
    source: &super::QuoteSource,
) -> Option<usize> {
    match author {
        QuoteAuthor::Agent { lane, occupant } => {
            agent_author(agents, source, *lane, *occupant).map(|(agent, _)| agent.accent)
        }
        QuoteAuthor::Directive {
            actor: wire::Actor::Lane(lane),
        } => lane_member_agent(agents, *lane).map(|agent| agent.accent),
        QuoteAuthor::Directive { .. } => None,
    }
}

/// The roster entry that names one occupant's prose, and whether it is that
/// occupant's current seat or a historical attribution: its own seat first,
/// then the agent its source card recorded, then its lane. A lane's name and
/// accent belong to the lane, so an occupant that has since left still reads
/// under them.
fn agent_author<'a>(
    agents: &'a super::QuoteAgents,
    source: &'a super::QuoteSource,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
) -> Option<(&'a super::QuoteAgent, &'static str)> {
    agents
        .members
        .get(&(lane, Some(occupant)))
        .map(|agent| (agent, "current"))
        .or_else(|| historical_agent(agents, source, lane).map(|agent| (agent, "historical")))
}

fn historical_agent<'a>(
    agents: &'a super::QuoteAgents,
    source: &'a super::QuoteSource,
    lane: wire::CanonicalUuid,
) -> Option<&'a super::QuoteAgent> {
    source
        .agent
        .as_ref()
        .or_else(|| lane_member_agent(agents, lane))
}

/// The lead of a packet the card's author sent: the card's live age, its kind
/// when not MSG, exact authoring occupant, an outgoing arrow, then every
/// addressed occupant and the operator. A quoted peer context uses the same
/// direction lead with its own recorded destination; badges never change
/// either side's attribution.
fn send_signoff_html(
    send: &wire::MessageSend,
    agents: &super::QuoteAgents,
    card: Option<&super::QuoteSource>,
) -> String {
    let (sender, state) = occupant_agent(agents, send.from, send.sender)
        .map_or((None, "unresolved"), |(agent, state)| (Some(agent), state));
    let label = sender.map_or(UNAVAILABLE, |agent| agent.label.as_str());
    let sender = quote_name_html(
        label,
        &format!(
            " data-quote-author=\"agent\" data-quote-agent-lane=\"{}\" \
             data-quote-agent-occupant=\"{}\" data-quote-author-state=\"{state}\"",
            send.from, send.sender
        ),
        sender.map(|agent| agent.accent),
    );
    let mut recipients = String::new();
    for recipient in &send.recipients {
        push_recipient_separator(&mut recipients);
        recipients.push_str(&participant_html(
            recipient.lane,
            recipient.occupant,
            Some(recipient.address),
            agents,
        ));
    }
    if send.operator {
        push_recipient_separator(&mut recipients);
        recipients.push_str(&quote_name_html(
            "Operator",
            " data-quote-recipient=\"operator\"",
            None,
        ));
    }
    let author = QuoteAuthor::Agent {
        lane: send.from,
        occupant: send.sender,
    };
    let mut lead = card.map_or_else(String::new, |card| {
        quote_age_slot_html(&author, agents, card)
    });
    lead.push_str(&packet_kind_html(send.kind, send.result.as_ref()));
    packet_signoff_html(&lead, &sender, &recipients, "outgoing")
}

/// The pill a packet's kind wears when not MSG, with the result an ANS names.
fn packet_kind_html(
    kind: wire::MessageSendKind,
    result: Option<&wire::rooms::QuestionResultRef>,
) -> String {
    match kind {
        wire::MessageSendKind::Msg => String::new(),
        wire::MessageSendKind::Ask => super::acknowledgement_pill_html_with_attributes(
            "ask",
            "question",
            "Question",
            "ASK",
            " data-message-packet=\"ask\"",
        ),
        wire::MessageSendKind::Ans => {
            let result = result.map(wire::rooms::QuestionResultRef::as_str);
            let text = result.map_or_else(|| "ANS".to_owned(), |result| format!("ANS {result}"));
            let attribute = result.map_or_else(
                || " data-message-packet=\"ans\"".to_owned(),
                |result| {
                    format!(
                        " data-message-packet=\"ans\" data-message-result=\"{}\"",
                        super::escape_html_attribute(result),
                    )
                },
            );
            super::acknowledgement_pill_html_with_attributes(
                "ans", "answer", "Answer", &text, &attribute,
            )
        }
    }
}

fn push_recipient_separator(recipients: &mut String) {
    if !recipients.is_empty() {
        recipients.push_str("<span class=\"message-quote-recipient-separator\">, </span>");
    }
}

/// Task/room contexts the packet recorded, mounted after the quoted body's
/// final word as the same task chips ordinary authored prose uses.
fn send_contexts_html(send: &wire::MessageSend) -> String {
    send.contexts
        .iter()
        .map(|context| super::task_chip_html(context, None, ""))
        .collect()
}

/// The accent slot of the occupant that sent one packet, which its quote's
/// rail wears as any quote wears its speaker's.
pub(super) fn send_accent(send: &wire::MessageSend, agents: &super::QuoteAgents) -> Option<usize> {
    occupant_agent(agents, send.from, send.sender).map(|(agent, _)| agent.accent)
}

/// A sent MSG's body reads as the card author's own words, so its hunks keep
/// the containing message's controls under an outgoing sender-to-recipient
/// lead.
pub(super) fn send_attribution(
    send: &wire::MessageSend,
    agents: &super::QuoteAgents,
    card: Option<&super::QuoteSource>,
) -> super::image_flow::Attribution {
    super::image_flow::Attribution {
        controls_target: String::new(),
        signoff: send_signoff_html(send, agents, card),
        disposition: String::new(),
        badge: String::new(),
        task_tail: send_contexts_html(send),
    }
}

pub(super) fn directive_attribution(
    context: &wire::MessageDirective,
    agents: &super::QuoteAgents,
    sources: &super::QuoteSources,
) -> super::image_flow::Attribution {
    let target = context.quote_target.to_string();
    super::image_flow::Attribution {
        controls_target: target.clone(),
        signoff: signoff_html(&context.attribution, &target, agents, sources),
        disposition: String::new(),
        badge: directive_badges_html(context),
        task_tail: task_tail_html(&target),
    }
}

/// The quoted directive's classification, then the kind of the MSG, ASK, or
/// ANS packet that delivered it, in the card head's own pill group.
fn directive_badges_html(context: &wire::MessageDirective) -> String {
    let mut badges = context.badge.map_or_else(String::new, directive_badge_html);
    badges.push_str(&packet_group_html(&context.attribution));
    badges
}

/// The pill group naming the MSG, ASK, or ANS packet that delivered a quoted
/// directive; empty when no packet delivered it or an MSG wears none.
fn packet_group_html(attribution: &QuoteAttribution) -> String {
    let pill = attribution.packet.map_or_else(String::new, |kind| {
        packet_kind_html(kind, attribution.result.as_ref())
    });
    if pill.is_empty() {
        return pill;
    }
    format!(
        "<span class=\"message-classification-group\" role=\"group\" \
         aria-label=\"Packet kind\">{pill}</span>"
    )
}

/// The quoted directive's own kind, where its source marker once stood.
fn directive_badge_html(badge: wire::DirectiveBadge) -> String {
    let (class, classification, label, text) = match badge {
        wire::DirectiveBadge::Review => (
            "message-review-pill",
            "review",
            "Review classification",
            "REVIEW",
        ),
        wire::DirectiveBadge::Taste => (
            "message-taste-pill",
            "taste",
            "Taste classification",
            "TASTE",
        ),
    };
    format!(
        "<span class=\"message-classification-group\" role=\"group\" \
         aria-label=\"Directive classification\"><span class=\"{class}\" \
         data-message-classification=\"{classification}\" aria-label=\"{label}\">{text}</span></span>"
    )
}

/// The roster entry naming one exact occupant with no source card to consult:
/// its own seat, else its lane, whose name and accent outlast any one seat.
fn occupant_agent(
    agents: &super::QuoteAgents,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
) -> Option<(&super::QuoteAgent, &'static str)> {
    agents
        .members
        .get(&(lane, Some(occupant)))
        .map(|agent| (agent, "current"))
        .or_else(|| lane_member_agent(agents, lane).map(|agent| (agent, "historical")))
}

/// Empty portal target that board provenance fills at the quoted content's
/// trailing edge after the card has rendered.
fn task_tail_html(target: &str) -> String {
    let key = match target.parse() {
        Ok(QuoteTarget::Steering { key, .. } | QuoteTarget::SteeringMedia { key, .. }) => key,
        _ => return String::new(),
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
        | QuotePresentation::Media { target, .. }
        | QuotePresentation::Unavailable { target, .. } => target,
    }
}

fn render(
    presentation: &QuotePresentation,
    agents: &super::QuoteAgents,
    sources: &super::QuoteSources,
    quote_depth: usize,
) -> String {
    match presentation {
        QuotePresentation::Resolved {
            target,
            text,
            attribution,
            disposition,
            quotes,
            media_quotes,
        } => {
            let disposition = disposition.map(disposition_html).unwrap_or_default();
            let signoff = signoff_html(attribution, target, agents, sources);
            let task_tail = task_tail_html(target);
            let body = super::render_quote_card_body(
                text,
                quotes,
                agents,
                sources,
                media_quotes,
                super::image_flow::Attribution {
                    controls_target: target.clone(),
                    signoff,
                    disposition,
                    badge: packet_group_html(attribution),
                    task_tail,
                },
                quote_depth,
            );
            let accent = speaker_accent(&attribution.author, target, agents, sources)
                .map_or_else(String::new, |accent| format!(" data-accent=\"{accent}\""));
            format!(
                "<div class=\"message-quote-card\" data-quote-state=\"resolved\" \
                 data-quote-target=\"{}\"{accent}><div class=\"message-quote-body\">{}</div></div>",
                super::escape_html_attribute(target),
                body.html,
            )
        }
        QuotePresentation::Media {
            target,
            media,
            attribution,
            disposition,
        } => {
            let disposition = disposition.map(disposition_html).unwrap_or_default();
            let signoff = signoff_html(attribution, target, agents, sources);
            let task_tail = task_tail_html(target);
            let own_quote = target.parse().ok().map(|target| wire::MediaQuote {
                target,
                name: media.name.clone(),
            });
            let markdown = media
                .markdown()
                .expect("resident-resolved media passed admission");
            let body = super::render_quote_card_body(
                &markdown,
                &[],
                agents,
                sources,
                own_quote.as_slice(),
                super::image_flow::Attribution {
                    controls_target: target.clone(),
                    signoff,
                    disposition,
                    badge: packet_group_html(attribution),
                    task_tail,
                },
                quote_depth,
            );
            let accent = speaker_accent(&attribution.author, target, agents, sources)
                .map_or_else(String::new, |accent| format!(" data-accent=\"{accent}\""));
            format!(
                "<div class=\"message-quote-card message-quote-card--media\" \
                 data-quote-state=\"resolved\" data-quote-target=\"{}\"{accent}>\
                 <div class=\"message-quote-body\">{}</div></div>",
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
            let (agent, state) = agent_author(agents, source, *lane, *occupant)
                .map_or((None, "unresolved"), |(agent, state)| (Some(agent), state));
            let label = agent.map_or(UNAVAILABLE, |agent| agent.label.as_str());
            quote_name_html(
                label,
                &format!(
                    " data-quote-agent-lane=\"{lane}\" data-quote-agent-occupant=\"{occupant}\" \
                     data-quote-author-state=\"{state}\""
                ),
                author_accent(author, agents, source),
            )
        }
        // A human directive names its author by the initials of the
        // principal profile it was authored as, and by nothing before that
        // profile is known or when it records no name.
        QuoteAuthor::Directive {
            actor: actor @ (wire::Actor::Operator | wire::Actor::Human(_)),
        } => agents
            .principals
            .identity(actor)
            .and_then(|identity| {
                let initials = identity.initials.as_deref()?;
                let name = identity.name.as_deref().unwrap_or(initials);
                Some(quote_name_html(
                    initials,
                    &format!(
                        " data-quote-author=\"directive\" data-quote-actor=\"human\" \
                         role=\"img\" aria-label=\"{}\"",
                        super::escape_html_attribute(name),
                    ),
                    None,
                ))
            })
            .unwrap_or_default(),
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
            // A lane directive with no recorded authoring occupant names its
            // sending lane: the lane's team member speaks for it, seated or
            // vacant. An MSG names its exact occupant as an `Agent` instead.
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
                author_accent(author, agents, source),
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
            let state = if agents.members.contains_key(&(*lane, Some(*occupant))) {
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
            let actor = actor.to_string();
            format!(
                " data-quote-author=\"directive\" data-quote-actor=\"{}\"",
                super::escape_html_attribute(&actor),
            )
        }
    };
    let accent = match author {
        QuoteAuthor::Agent { lane, .. } => historical_agent(agents, source, *lane),
        QuoteAuthor::Directive {
            actor: wire::Actor::Lane(lane),
        } => source
            .agent
            .as_ref()
            .or_else(|| lane_member_agent(agents, *lane)),
        QuoteAuthor::Directive { .. } => source.agent.as_ref(),
    }
    .map(|agent| agent.accent);
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

/// The team member for one lane, seated or vacant, which speaks for a
/// directive that lane sent with no recorded occupant: such a directive names
/// the lane, never an occupant.
fn lane_member_agent(
    agents: &super::QuoteAgents,
    lane: wire::CanonicalUuid,
) -> Option<&super::QuoteAgent> {
    agents
        .members
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
