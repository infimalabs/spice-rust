use super::*;

#[cfg(test)]
pub(in crate::resident) fn publish_agent_messages(
    publisher: &impl AgentMessagePublisher,
    record: &lane::capture::CaptureRecord,
    state: &mut MessageState,
    steering: Option<&PresentationDirectives>,
) -> CommandResult<()> {
    publish_agent_messages_in(publisher, record, state, steering, None, None, None)
}

pub(super) fn publish_agent_messages_in(
    publisher: &impl AgentMessagePublisher,
    record: &lane::capture::CaptureRecord,
    state: &mut MessageState,
    steering: Option<&PresentationDirectives>,
    worktree: Option<&Path>,
    source: Option<CanonicalUuid>,
    observed: Option<wire::Instant>,
) -> CommandResult<()> {
    let Some(context) = record.decoded.context() else {
        return Ok(());
    };
    let source = source.or_else(|| state.observations.source(record.lane, record.locus));
    if state
        .presented
        .get(&(record.lane, record.occupant, source))
        .is_some_and(|next| (record.locus.member, record.locus.offset) < (next.member, next.offset))
    {
        return Ok(());
    }
    let mut events = Vec::new();
    for (event, decoded) in record.decoded.events().iter().enumerate() {
        let event = u32::try_from(event)
            .map_err(|error| store_fault("canvas::message-locus-invalid", &error.to_string()))?;
        events.push((event, decoded));
    }
    publish_agent_record(
        publisher,
        AgentRecord {
            lane: record.lane,
            author: record.occupant,
            source,
            worktree,
            locus: record.locus,
            context,
            events: &events,
            observed,
            unavailable_tool_terminals: &[],
        },
        state,
        steering,
    )
}

pub(super) fn publish_agent_record(
    publisher: &impl AgentMessagePublisher,
    record: AgentRecord<'_>,
    state: &mut MessageState,
    steering: Option<&PresentationDirectives>,
) -> CommandResult<()> {
    begin_agent_record(publisher, &record, state)?;
    let successful_terminal = record.context.turn == Some(listen::TurnMoment::Completed)
        && record
            .events
            .iter()
            .any(|(_, event)| matches!(event, listen::Event::Usage(_)));
    let mut explicit_final = false;
    for &(ordinal, decoded) in record.events {
        let Some(projected) = project_agent_event(
            AgentEvent {
                lane: record.lane,
                author: record.author,
                source: record.source,
                worktree: record.worktree,
                ordinal,
                decoded,
                authored: record.context.timestamp.as_deref(),
                observed: record.observed,
                observations: &state.observations,
                unavailable_tool_terminals: record.unavailable_tool_terminals,
            },
            &mut state.previews,
            steering,
        )?
        else {
            continue;
        };
        let ProjectedMessage {
            published,
            terminal,
            pending,
        } = projected;
        if published.activity != AgentActivityKind::Prose {
            publish_message(publisher, published)?;
            continue;
        }
        if published.final_answer {
            explicit_final = true;
            if revise_repeated_final(publisher, record.lane, &published, state)? {
                continue;
            }
        }
        publish_message(publisher, published.clone())?;
        if let Some(pending) = pending {
            state.pending.remember(pending);
        }
        let terminal = terminal.unwrap_or_else(|| published.clone());
        state.prose.insert(
            record.lane,
            ProseState {
                published,
                terminal,
            },
        );
    }
    if successful_terminal
        && !explicit_final
        && let Some(previous) = state.prose.get(&record.lane)
        && previous.published.author == record.author
        && !previous.published.final_answer
    {
        revise_final_message(publisher, previous.terminal.clone())?;
    }
    if record.context.turn == Some(listen::TurnMoment::Completed) {
        state.prose.remove(&record.lane);
    }
    let source = record
        .source
        .or_else(|| state.observations.source(record.lane, record.locus));
    let next = listen::Position {
        member: record.locus.member,
        offset: record
            .locus
            .offset
            .checked_add(record.locus.length)
            .ok_or_else(|| {
                store_fault(
                    "canvas::message-locus-invalid",
                    "record end exceeds the tape position range",
                )
            })?,
    };
    state
        .presented
        .entry((record.lane, record.author, source))
        .and_modify(|known| *known = (*known).max(next))
        .or_insert(next);
    Ok(())
}

/// Raise the previous card to final when the final answer repeats its prose,
/// answering whether that card stood in for the repeated one.
fn revise_repeated_final(
    publisher: &impl AgentMessagePublisher,
    lane: CanonicalUuid,
    published: &wire::AgentMessage,
    state: &mut MessageState,
) -> CommandResult<bool> {
    let Some(previous) = state.prose.get(&lane) else {
        return Ok(false);
    };
    if !same_prose(&previous.published, published) {
        return Ok(false);
    }
    if !previous.published.final_answer {
        let terminal = previous.terminal.clone();
        revise_final_message(publisher, terminal.clone())?;
        state.prose.insert(
            lane,
            ProseState {
                published: terminal.clone(),
                terminal,
            },
        );
    }
    Ok(true)
}

fn begin_agent_record(
    publisher: &impl AgentMessagePublisher,
    record: &AgentRecord<'_>,
    state: &mut MessageState,
) -> CommandResult<()> {
    if record.context.turn == Some(listen::TurnMoment::Started) {
        state.prose.remove(&record.lane);
        publish_turn_boundary(publisher, record, &state.observations)?;
    } else if record
        .events
        .iter()
        .any(|(_, event)| matches!(event, listen::Event::Session(_)))
    {
        state.prose.remove(&record.lane);
    }
    Ok(())
}

fn publish_turn_boundary(
    publisher: &impl AgentMessagePublisher,
    record: &AgentRecord<'_>,
    observations: &MessageObservations,
) -> CommandResult<()> {
    let (authored, authored_synthetic) =
        parse_agent_message_authored(record.context.timestamp.as_deref(), record.locus, 0)?;
    let observed = authored_synthetic
        .then(|| {
            record
                .observed
                .or_else(|| observations.at(record.lane, record.locus))
        })
        .flatten();
    publish_message(
        publisher,
        wire::AgentMessage {
            lane: record.lane,
            author: record.author,
            locus: wire::MessageLocus {
                member: record.locus.member,
                offset: record.locus.offset,
                length: record.locus.length,
                event: TURN_BOUNDARY_EVENT,
            },
            authored,
            authored_synthetic,
            observed,
            activity: AgentActivityKind::Turn,
            final_answer: false,
            text: "New turn".to_owned(),
            badges: wire::AgentMessageBadges::default(),
            automatic_speech: None,
            unavailable_tool_terminals: Vec::new(),
            media: Vec::new(),
        },
    )
}

fn same_prose(left: &wire::AgentMessage, right: &wire::AgentMessage) -> bool {
    left.author == right.author && left.text == right.text && left.media == right.media
}

fn publish_message(
    publisher: &impl AgentMessagePublisher,
    message: wire::AgentMessage,
) -> CommandResult<()> {
    publisher
        .publish(message)
        .map_err(|error| store_fault("canvas::message-publish-failed", &error.to_string()))?;
    Ok(())
}

fn revise_final_message(
    publisher: &impl AgentMessagePublisher,
    message: wire::AgentMessage,
) -> CommandResult<()> {
    publisher
        .revise_final(message)
        .map_err(|error| store_fault("canvas::message-publish-failed", &error.to_string()))?;
    Ok(())
}

/// Project one typed tape event through the sole live/restored message policy.
pub(super) fn publish_agent_event(
    publisher: &impl AgentMessagePublisher,
    event: AgentEvent<'_>,
    previews: &mut CallPreviews,
    steering: Option<&PresentationDirectives>,
) -> CommandResult<()> {
    if let Some(projected) = project_agent_event(event, previews, steering)? {
        publish_message(publisher, projected.published)?;
    }
    Ok(())
}

fn project_agent_event(
    event: AgentEvent<'_>,
    previews: &mut CallPreviews,
    steering: Option<&PresentationDirectives>,
) -> CommandResult<Option<ProjectedMessage>> {
    let AgentEvent {
        lane,
        author,
        source,
        worktree,
        ordinal,
        decoded,
        authored,
        observed,
        observations,
        unavailable_tool_terminals,
    } = event;
    let presentation_event = |locus| PresentationEvent {
        lane,
        source: source.or_else(|| observations.source(lane, locus)),
        locus,
        ordinal,
    };
    let (activity, final_answer, locus, text, badges, automatic_speech, media) = match decoded {
        listen::Event::Prose(prose) if prose.role == listen::ProseRole::Assistant => {
            let event = presentation_event(prose.locus);
            (
                AgentActivityKind::Prose,
                prose.phase.as_deref() == Some("final_answer"),
                prose.locus,
                present_steering_acknowledgements(&prose.text, steering, event)?,
                message_badges(&prose.text, steering, event),
                automatic_speech::classify(prose, steering, event),
                prose.media.clone(),
            )
        }
        listen::Event::Tool(tool) => {
            let activity = tool_preview::activity_in(lane, tool, previews, worktree);
            let media = match &tool.moment {
                listen::ToolMoment::Result(outcome) => outcome.media.clone(),
                listen::ToolMoment::Call { .. } | listen::ToolMoment::Update { .. } => Vec::new(),
            };
            let text = tool_preview::message_text(tool, activity, &media);
            (
                AgentActivityKind::Tool,
                false,
                tool.locus,
                text,
                wire::AgentMessageBadges::default(),
                None,
                media,
            )
        }
        listen::Event::Reasoning(listen::Reasoning {
            content: listen::ReasoningContent::Plain(parts),
            locus,
        }) => (
            AgentActivityKind::Reasoning,
            false,
            *locus,
            reasoning_preview(parts),
            wire::AgentMessageBadges::default(),
            None,
            Vec::new(),
        ),
        listen::Event::Compaction(compaction) => (
            AgentActivityKind::Compaction,
            false,
            compaction.locus,
            compaction
                .summary
                .clone()
                .filter(|summary| !summary.trim().is_empty())
                .unwrap_or_else(|| "Context compacted".to_owned()),
            wire::AgentMessageBadges::default(),
            None,
            Vec::new(),
        ),
        _ => return Ok(None),
    };
    let (authored, authored_synthetic) = parse_agent_message_authored(authored, locus, ordinal)?;
    let observed = authored_synthetic
        .then(|| observed.or_else(|| observations.at(lane, locus)))
        .flatten();
    wire::media::validate_media(&media)
        .map_err(|error| store_fault("canvas::message-media-invalid", &error.to_string()))?;
    let published = wire::AgentMessage {
        lane,
        author,
        locus: wire::MessageLocus {
            member: locus.member,
            offset: locus.offset,
            length: locus.length,
            event: ordinal,
        },
        authored,
        authored_synthetic,
        observed,
        activity,
        final_answer,
        text,
        badges,
        automatic_speech,
        unavailable_tool_terminals: unavailable_tool_terminals.to_vec(),
        media,
    };
    let pending = pending_presentation(decoded, steering, presentation_event, published.locus);
    let terminal = terminal_prose_card(&published, decoded, steering, presentation_event);
    Ok(Some(ProjectedMessage {
        published,
        terminal,
        pending,
    }))
}

/// The keys an assistant prose card acknowledged ahead of the journal.
fn pending_presentation(
    decoded: &listen::Event,
    steering: Option<&PresentationDirectives>,
    presentation_event: impl Fn(listen::Locus) -> PresentationEvent,
    locus: wire::MessageLocus,
) -> Option<pending::PendingPresentation> {
    let listen::Event::Prose(prose) = decoded else {
        return None;
    };
    if prose.role != listen::ProseRole::Assistant {
        return None;
    }
    pending::PendingPresentation::of(
        &prose.text,
        steering,
        presentation_event(prose.locus),
        locus,
    )
}

/// The final-answer twin a mid-turn prose card carries for the turn end.
///
/// Only assistant prose short of its final answer has one; the twin repeats
/// the card with the final-answer phase so its automatic speech is classified
/// the way the terminal record will be.
fn terminal_prose_card(
    published: &wire::AgentMessage,
    decoded: &listen::Event,
    steering: Option<&PresentationDirectives>,
    presentation_event: impl FnOnce(listen::Locus) -> PresentationEvent,
) -> Option<wire::AgentMessage> {
    if published.activity != AgentActivityKind::Prose || published.final_answer {
        return None;
    }
    let listen::Event::Prose(prose) = decoded else {
        unreachable!("only assistant prose projects a prose card")
    };
    let mut terminal_prose = prose.clone();
    terminal_prose.phase = Some("final_answer".to_owned());
    let mut terminal = published.clone();
    terminal.final_answer = true;
    terminal.automatic_speech =
        automatic_speech::classify(&terminal_prose, steering, presentation_event(prose.locus));
    Some(terminal)
}

/// Strip preview for one reasoning event.
///
/// The narration is flattened to one bounded line the way a tool preview is.
/// A thinking block the vendor redacted to its signature previews as empty
/// text, so the strip still shows the lane thinking without inventing words;
/// opaque reasoning never reaches here because it carries no text at all.
fn reasoning_preview(parts: &[String]) -> String {
    tool_preview::flatten(&parts.join(" "))
}

fn parse_agent_message_authored(
    timestamp: Option<&str>,
    locus: listen::Locus,
    event: u32,
) -> CommandResult<(wire::Instant, bool)> {
    match timestamp {
        Some(timestamp) => timestamp
            .parse::<wire::Instant>()
            .map(|authored| (authored, false))
            .map_err(|error| store_fault("canvas::message-authored-invalid", &error.to_string())),
        None => synthetic_agent_message_authored(locus, event).map(|authored| (authored, true)),
    }
}

fn synthetic_agent_message_authored(
    locus: listen::Locus,
    event: u32,
) -> CommandResult<wire::Instant> {
    // Some current Codex records omit vendor time. The wire identity still
    // needs a stable ordering key, so derive one from durable tape coordinates;
    // presentation must use `observed` and never expose this surrogate as time.
    const BASE_MILLIS: i64 = 946_684_800_000;
    const MEMBER_MILLIS: i64 = 86_400_000;

    // A preserved session is one file, so its namespaced member identifies the
    // source rather than advancing tape chronology. Its byte offset still gives
    // every timestamp-less event a stable in-file order, while the unchanged
    // locus carries the full session identity through publication and dedupe.
    let member = if locus.member & SESSION_MESSAGE_MEMBER_NAMESPACE == 0 {
        locus.member.saturating_sub(1)
    } else {
        0
    };
    let member = i64::try_from(member).map_err(|error| {
        store_fault(
            "canvas::message-authored-unrepresentable",
            &error.to_string(),
        )
    })?;
    let offset = i64::try_from(locus.offset).map_err(|error| {
        store_fault(
            "canvas::message-authored-unrepresentable",
            &error.to_string(),
        )
    })?;
    let delta = member
        .checked_mul(MEMBER_MILLIS)
        .and_then(|millis| millis.checked_add(offset))
        .and_then(|millis| millis.checked_add(i64::from(event)))
        .ok_or_else(|| {
            store_fault(
                "canvas::message-authored-unrepresentable",
                "transcript locus cannot fit a synthetic authored instant",
            )
        })?;
    let millis = BASE_MILLIS.checked_add(delta).ok_or_else(|| {
        store_fault(
            "canvas::message-authored-unrepresentable",
            "transcript locus cannot fit a synthetic authored instant",
        )
    })?;
    let authored = jiff::Timestamp::from_millisecond(millis).map_err(|error| {
        store_fault(
            "canvas::message-authored-unrepresentable",
            &error.to_string(),
        )
    })?;
    wire::Instant::new(authored).map_err(|error| {
        store_fault(
            "canvas::message-authored-unrepresentable",
            &error.to_string(),
        )
    })
}
