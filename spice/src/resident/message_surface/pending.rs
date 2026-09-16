//! Cards spoken as acknowledged before the journal retired their keys.
//!
//! An agent speaks `ACK KEY` in prose; the key retires only when `spice ack`
//! or exit disposal commits steer::acked. The card publishes awaiting that
//! retirement, and once the fact lands it is presented again from the journal
//! disposition, so the card and the Agents tile pending count never disagree.

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
        let keys = positively_acknowledged_keys(text, &standing)
            .into_iter()
            .filter(|key| standing.pending(key))
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

/// Present again every retained card whose spoken acknowledgement the journal
/// has since retired, so the card follows the steer::acked fact rather than
/// the prose that anticipated it.
pub(in crate::resident) fn settle_pending_presentations(
    bus: &Bus,
    publisher: &canvas::stream::MessagePublisher,
    state: &mut MessageState,
) -> CommandResult<()> {
    if state.pending.is_empty() {
        return Ok(());
    }
    let rows = load_steering_presentations(&bus.reader(), &state.pending.keys())?;
    let directives = PresentationDirectives::from_presentations(&rows)
        .map_err(|error| store_fault("canvas::message-steering-invalid", &error))?;
    settle_with_directives(publisher, state, &directives)
}

fn settle_with_directives(
    publisher: &canvas::stream::MessagePublisher,
    state: &mut MessageState,
    directives: &PresentationDirectives,
) -> CommandResult<()> {
    let still_pending = |key: &steer::Key| {
        directives
            .0
            .get(key)
            .is_some_and(|directive| directive.disposition == wire::Disposition::Live)
    };
    let mut remaining = Vec::new();
    for mut card in std::mem::take(&mut state.pending.0) {
        if card.keys.iter().all(still_pending) {
            remaining.push(card);
            continue;
        }
        present_again(publisher, state, &card, directives)?;
        card.keys.retain(still_pending);
        if !card.keys.is_empty() {
            remaining.push(card);
        }
    }
    state.pending.0 = remaining;
    Ok(())
}

/// Re-present one card from the current journal disposition of its keys.
///
/// The prose state the turn end revises to final follows the same text and
/// badges, so the terminal revision still names the card retention holds.
fn present_again(
    publisher: &canvas::stream::MessagePublisher,
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
