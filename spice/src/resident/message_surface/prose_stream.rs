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

fn origin(
    record: &AgentRecord<'_>,
    state: &MessageState,
    ordinal: u32,
    decoded: &listen::Event,
    answers: &BTreeSet<steer::Key>,
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
        answers: answers.iter().map(ToString::to_string).collect(),
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

#[derive(Debug, Default)]
pub(super) struct AnswerSelection {
    winners: BTreeSet<steer::Key>,
    superseded: Vec<steer::Key>,
}

impl AnswerSelection {
    pub(super) fn prose_origin(
        &self,
        record: &AgentRecord<'_>,
        state: &MessageState,
        ordinal: u32,
        decoded: &listen::Event,
    ) -> Option<wire::MessageProseOrigin> {
        (!self.suppresses_event())
            .then(|| origin(record, state, ordinal, decoded, &self.winners))
            .flatten()
    }

    pub(super) fn superseded(&self) -> &[steer::Key] {
        &self.superseded
    }

    pub(super) fn suppresses_event(&self) -> bool {
        self.winners.is_empty() && !self.superseded.is_empty()
    }
}

/// Selects the steering keys whose answer this event owns.
///
/// Transcript retirement reads complete events newest-first, so presentation
/// uses the same event order instead of preferring visible text over a later
/// tool intent. A completed streamed answer remains authoritative over its
/// settled intent fallback because it was recognized when it landed.
pub(super) fn answer_selection(
    record: &AgentRecord<'_>,
    state: &MessageState,
    position: usize,
    decoded: &listen::Event,
) -> AnswerSelection {
    let Some(text) = decoded.assistant_text() else {
        return AnswerSelection::default();
    };
    let keys = steering_keys(std::iter::once(text));
    if keys.is_empty() {
        return AnswerSelection::default();
    }
    let later = steering_keys(
        record
            .events
            .iter()
            .skip(position + 1)
            .filter_map(|(_, event)| event.assistant_text()),
    );
    let streamed = open(record, state);
    let streamed_wins = |key: &steer::Key| {
        matches!(decoded, listen::Event::Tool(_))
            && streamed
                .is_some_and(|open| open.answers.values().any(|answers| answers.contains(key)))
    };
    let mut selection = AnswerSelection::default();
    for key in keys {
        if later.contains(&key) || streamed_wins(&key) {
            selection.superseded.push(key);
        } else {
            selection.winners.insert(key);
        }
    }
    selection
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
