use super::*;

#[cfg(test)]
pub(in crate::resident) fn publish_agent_messages(
    publisher: &impl AgentMessagePublisher,
    record: &lane::capture::CaptureRecord,
    state: &mut MessageState,
    steering: Option<&PresentationDirectives>,
) -> CommandResult<()> {
    publish_agent_messages_in(publisher, record, state, steering, None)
}

pub(super) fn publish_agent_messages_in(
    publisher: &impl AgentMessagePublisher,
    record: &lane::capture::CaptureRecord,
    state: &mut MessageState,
    steering: Option<&PresentationDirectives>,
    worktree: Option<&Path>,
) -> CommandResult<()> {
    let Some(context) = record.decoded.context() else {
        return Ok(());
    };
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
            source: None,
            worktree,
            context,
            events: &events,
            observed: None,
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
    if record.context.turn == Some(listen::TurnMoment::Started)
        || record
            .events
            .iter()
            .any(|(_, event)| matches!(event, listen::Event::Session(_)))
    {
        state.prose.remove(&record.lane);
    }
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
        if projected.published.activity != AgentActivityKind::Prose {
            publish_message(publisher, projected.published)?;
            continue;
        }
        if projected.published.final_answer {
            explicit_final = true;
            if let Some(previous) = state.prose.get(&record.lane)
                && same_prose(&previous.published, &projected.published)
            {
                if !previous.published.final_answer {
                    let terminal = previous.terminal.clone();
                    revise_final_message(publisher, terminal.clone())?;
                    state.prose.insert(
                        record.lane,
                        ProseState {
                            published: terminal.clone(),
                            terminal,
                        },
                    );
                }
                continue;
            }
        }
        publish_message(publisher, projected.published.clone())?;
        let terminal = projected
            .terminal
            .unwrap_or_else(|| projected.published.clone());
        state.prose.insert(
            record.lane,
            ProseState {
                published: projected.published,
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
    Ok(())
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
    wire::media::admit_media(&media)
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
    let terminal = if published.activity == AgentActivityKind::Prose && !published.final_answer {
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
    } else {
        None
    };
    Ok(Some(ProjectedMessage {
        published,
        terminal,
    }))
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
