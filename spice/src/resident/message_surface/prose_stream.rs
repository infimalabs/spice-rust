//! Provider message identity shared by live capture and bounded tape replay.

use super::*;

#[cfg(test)]
mod tests;

pub(super) fn observe(record: &AgentRecord<'_>, state: &mut MessageState) {
    let Some(message) = &record.context.message else {
        return;
    };
    let Some(timestamp) = message.timestamp else {
        return;
    };
    if message.settled {
        return;
    }
    state.messages.insert(
        record.lane,
        OpenMessage {
            author: record.author,
            source: record.source,
            id: message.id.clone(),
            timestamp,
            answers: BTreeMap::new(),
        },
    );
}

pub(super) fn origin(
    record: &AgentRecord<'_>,
    state: &MessageState,
    ordinal: u32,
    decoded: &listen::Event,
) -> Option<wire::MessageProseOrigin> {
    let message = record.context.message.as_ref()?;
    let streamed = matches!(decoded, listen::Event::StreamedProse(_));
    let text = decoded.assistant_text()?;
    let presented = match decoded {
        listen::Event::StreamedProse(_) => {
            steer::has_acknowledgement(text) || steer::ack::scan::has_message(text)
        }
        listen::Event::Tool(_) => steer::has_acknowledgement(text),
        _ => true,
    };
    if !presented {
        return None;
    }
    let timestamp = message.timestamp.or_else(|| {
        state.messages.get(&record.lane).and_then(|open| {
            (open.author == record.author && open.source == record.source && open.id == message.id)
                .then_some(open.timestamp)
        })
    })?;
    Some(wire::MessageProseOrigin {
        source: record.source?,
        message: message.id.clone(),
        timestamp,
        block: if streamed {
            message.content_index?
        } else {
            ordinal
        },
        streamed,
        intent: matches!(decoded, listen::Event::Tool(_)),
        answers: steering_keys(std::iter::once(text))
            .into_iter()
            .map(|key| key.to_string())
            .collect(),
    })
}

fn open<'a>(record: &AgentRecord<'_>, state: &'a MessageState) -> Option<&'a OpenMessage> {
    let message = record.context.message.as_ref()?;
    let open = state.messages.get(&record.lane)?;
    (message.settled
        && open.author == record.author
        && open.source == record.source
        && open.id == message.id
        && message.timestamp == Some(open.timestamp))
    .then_some(open)
}

pub(super) fn settled_block(
    record: &AgentRecord<'_>,
    state: &MessageState,
    ordinal: u32,
    decoded: &listen::Event,
) -> bool {
    matches!(decoded, listen::Event::Prose(_))
        && open(record, state).is_some_and(|open| open.answers.contains_key(&ordinal))
}

pub(super) fn intent_represented(
    record: &AgentRecord<'_>,
    state: &MessageState,
    decoded: &listen::Event,
) -> bool {
    let listen::Event::Tool(listen::Tool {
        moment:
            listen::ToolMoment::Call {
                intent: Some(intent),
                ..
            },
        ..
    }) = decoded
    else {
        return false;
    };
    let keys = steering_keys(std::iter::once(intent.as_str()));
    if keys.is_empty() {
        return false;
    }
    let settled = steering_keys(record.events.iter().filter_map(|(_, event)| match event {
        listen::Event::Prose(prose) if prose.role == listen::ProseRole::Assistant => {
            Some(prose.text.as_str())
        }
        _ => None,
    }));
    let streamed = open(record, state);
    keys.iter().all(|key| {
        settled.contains(key)
            || streamed
                .is_some_and(|open| open.answers.values().any(|answers| answers.contains(key)))
    })
}

pub(super) fn remember(
    record: &AgentRecord<'_>,
    state: &mut MessageState,
    decoded: &listen::Event,
) {
    let listen::Event::StreamedProse(prose) = decoded else {
        return;
    };
    let Some(index) = record
        .context
        .message
        .as_ref()
        .and_then(|message| message.content_index)
    else {
        return;
    };
    if let Some(open) = state.messages.get_mut(&record.lane) {
        open.answers
            .insert(index, steering_keys(std::iter::once(prose.text.as_str())));
    }
}
