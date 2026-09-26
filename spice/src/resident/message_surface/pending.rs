//! Cards spoken as acknowledged before the journal retired their keys.
//!
//! An agent speaks `ACK KEY` in prose; the card follows the journaled
//! steer::acked disposition, whether committed by the ack command, live
//! interjection handling, startup recovery, or exit disposal. Once that fact
//! lands, the card is presented again from the journal disposition.

use super::*;

/// Every retained prose card still waiting on a steering retirement.
#[derive(Debug, Default)]
pub(super) struct PendingPresentations(Vec<PendingPresentation>);

/// One prose card and the keys it acknowledged while they were pending.
#[derive(Clone, Debug)]
pub(super) struct PendingPresentation {
    lane: CanonicalUuid,
    locus: wire::MessageLocus,
    keys: BTreeSet<steer::Key>,
    text: String,
    event: PresentationEvent,
}

impl PendingPresentation {
    /// The card `text` publishes at `locus` when it positively acknowledges
    /// keys the journal still holds pending.
    pub(super) fn of(
        text: &str,
        directives: Option<&PresentationDirectives>,
        event: PresentationEvent,
        locus: wire::MessageLocus,
    ) -> Option<Self> {
        let standing = EventDirectives {
            directives: directives?,
            event,
        };
        let keys = recognized_answers(text, &standing)
            .filter_map(|(key, disposition)| {
                (disposition == wire::Disposition::Acked && standing.pending(&key)).then_some(key)
            })
            .collect::<BTreeSet<_>>();
        (!keys.is_empty()).then(|| Self {
            lane: event.lane,
            locus,
            keys,
            text: text.to_owned(),
            event,
        })
    }

    #[cfg(test)]
    pub(super) fn keys(&self) -> &BTreeSet<steer::Key> {
        &self.keys
    }
}

impl PendingPresentations {
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }

    /// Track one published card; a card published again replaces its entry.
    pub(super) fn remember(&mut self, presentation: PendingPresentation) {
        self.0
            .retain(|known| known.lane != presentation.lane || known.locus != presentation.locus);
        self.0.push(presentation);
    }

    /// Every key the retained prose names, pending or not, so a card presents
    /// again with the same context the live publication had.
    fn keys(&self) -> BTreeSet<steer::Key> {
        steering_keys(self.0.iter().map(|card| card.text.as_str()))
    }
}

pub(in crate::resident) trait PresentationPublisher {
    fn revise_presentation(
        &self,
        lane: CanonicalUuid,
        locus: wire::MessageLocus,
        text: String,
        badges: wire::AgentMessageBadges,
    ) -> Result<bool, canvas::stream::MessagePublishError>;
}

impl PresentationPublisher for canvas::stream::MessagePublisher {
    fn revise_presentation(
        &self,
        lane: CanonicalUuid,
        locus: wire::MessageLocus,
        text: String,
        badges: wire::AgentMessageBadges,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        Self::revise_presentation(self, lane, locus, text, badges)
    }
}

impl PresentationPublisher for canvas::stream::MessageBatch {
    fn revise_presentation(
        &self,
        lane: CanonicalUuid,
        locus: wire::MessageLocus,
        text: String,
        badges: wire::AgentMessageBadges,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        Self::revise_presentation(self, lane, locus, text, badges)
    }
}

/// Present again every retained card whose spoken acknowledgement the journal
/// has since retired, so the card follows the steer::acked fact rather than
/// the prose that anticipated it.
pub(in crate::resident) fn settle_pending_presentations(
    bus: &Bus,
    publisher: &impl PresentationPublisher,
    state: &Mutex<MessageState>,
) -> CommandResult<()> {
    // Detach exactly this batch before crossing the writer queue. New cards
    // may publish meanwhile and must not settle against this older query.
    let pending = std::mem::take(&mut lock_state(state)?.pending);
    if pending.is_empty() {
        return Ok(());
    }
    let directives = load_steering_presentations(&bus.reader(), &pending.keys()).and_then(|rows| {
        PresentationDirectives::from_presentations(&rows)
            .map_err(|error| store_fault("canvas::message-steering-invalid", &error))
    });
    let mut state = lock_state(state)?;
    let incoming = std::mem::replace(&mut state.pending, pending);
    let settled = directives
        .and_then(|directives| settle_with_directives(publisher, &mut state, &directives));
    state.pending.0.extend(incoming.0);
    settled
}

fn settle_with_directives(
    publisher: &impl PresentationPublisher,
    state: &mut MessageState,
    directives: &PresentationDirectives,
) -> CommandResult<()> {
    let still_pending = |key: &steer::Key| {
        directives
            .0
            .get(key)
            .is_some_and(|directive| directive.disposition == wire::Disposition::Live)
    };
    let mut pending = std::mem::take(&mut state.pending.0);
    let settled = pending.iter_mut().try_for_each(|card| {
        if card.keys.iter().all(still_pending) {
            return Ok(());
        }
        present_again(publisher, state, card, directives)?;
        card.keys.retain(still_pending);
        Ok(())
    });
    pending.retain(|card| !card.keys.is_empty());
    state.pending.0 = pending;
    settled
}

/// Re-present one card from the current journal disposition of its keys.
///
/// The prose state the turn end revises to final follows the same text and
/// badges, so the terminal revision still names the card retention holds.
fn present_again(
    publisher: &impl PresentationPublisher,
    state: &mut MessageState,
    card: &PendingPresentation,
    directives: &PresentationDirectives,
) -> CommandResult<()> {
    let mut event = card.event;
    if event.source.is_none() {
        event.source = state.observations.source(card.lane, event.locus);
    }
    let text = present_steering_acknowledgements_with_directives(&card.text, directives, event)
        .map_err(|detail| store_fault("canvas::message-steering-presentation-invalid", &detail))?;
    let badges = message_badges(&card.text, Some(directives), event);
    publisher
        .revise_presentation(card.lane, card.locus, text.clone(), badges)
        .map_err(|error| store_fault("canvas::message-publish-failed", &error.to_string()))?;
    if let Some(prose) = state.prose.get_mut(&card.lane)
        && prose.published.locus == card.locus
    {
        prose.published.text.clone_from(&text);
        prose.published.badges = badges;
        prose.terminal.text = text;
        prose.terminal.badges = badges;
    }
    Ok(())
}
