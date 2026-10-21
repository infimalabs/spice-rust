//! A Design room's typed contributions, in journal order or grouped by kind
//! exactly as `spice room history` groups them. Reading them moves no task,
//! claim, working group, candidate, decision, or authorization.

use wire::rooms::{RoomAuthorityStamp, RoomContributionKind, RoomQuestion, RoomSource};
use wire::{CanonicalUuid, Seq, TaskStamp};

use super::{RoomEventKind, RoomProjection};

/// One recorded contribution as its fact states it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contribution<'a> {
    /// Stable identity of the contribution.
    pub publication: &'a RoomAuthorityStamp,
    /// Journal position that recorded it.
    pub position: Seq,
    /// Logical lane that authored it.
    pub lane: CanonicalUuid,
    /// What it offers: a charge or a question.
    pub contribution: RoomContributionKind,
    /// The Design question item it weighs, when it names one.
    pub question: Option<RoomQuestion>,
    /// Its complete text.
    pub text: &'a str,
    /// Its cited sources, in the author's order.
    pub sources: &'a [RoomSource],
}

/// One kind's contributions, in journal order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContributionGroup<'a> {
    /// The kind every entry shares.
    pub kind: RoomContributionKind,
    /// The kind's contributions, oldest first.
    pub entries: Vec<Contribution<'a>>,
}

impl RoomProjection {
    /// `room`'s contributions, oldest first.
    #[must_use]
    pub fn contributions(&self, room: &TaskStamp) -> Vec<Contribution<'_>> {
        let mut contributions = self
            .rooms
            .get(room)
            .into_iter()
            .flat_map(|state| state.events.iter())
            .filter_map(|(publication, event)| match &event.kind {
                RoomEventKind::Contributed {
                    lane,
                    contribution,
                    question,
                    text,
                    sources,
                } => Some(Contribution {
                    publication,
                    position: event.position,
                    lane: *lane,
                    contribution: *contribution,
                    question: *question,
                    text,
                    sources,
                }),
                _ => None,
            })
            .collect::<Vec<_>>();
        contributions.sort_by_key(|contribution| contribution.position);
        contributions
    }

    /// `room`'s contributions grouped in [`RoomContributionKind::ALL`]
    /// order, each group oldest first; a kind with none is absent.
    #[must_use]
    pub fn contribution_groups(&self, room: &TaskStamp) -> Vec<ContributionGroup<'_>> {
        let contributions = self.contributions(room);
        RoomContributionKind::ALL
            .into_iter()
            .filter_map(|kind| {
                let entries = contributions
                    .iter()
                    .filter(|contribution| contribution.contribution == kind)
                    .copied()
                    .collect::<Vec<_>>();
                (!entries.is_empty()).then_some(ContributionGroup { kind, entries })
            })
            .collect()
    }
}
