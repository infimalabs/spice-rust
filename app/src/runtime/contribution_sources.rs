//! The resolved sources of typed Design contributions. A contribution's text
//! arrives with its fact, but whether each cited transcript, journal fact, or
//! retained artifact still yields its bytes only the resident can say, so a
//! shown Design reads each of its contributions' sources once; the
//! connection's bounded pump takes owed reads as mailbox capacity allows.

use wire::intent::RoomContributionSourcesAnswer;
use wire::rooms::{RoomAnchor, RoomAuthorityStamp, RoomSourceView};
use wire::{Outcome, Response};

use super::AppRuntime;
use crate::socket::Admitted;

/// One contribution's sources as the resident resolved them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContributionSources {
    /// A read is in flight.
    Loading,
    /// Each cited source, in the author's order, as a link it can still
    /// follow or the typed reason it cannot.
    Read(Vec<RoomSourceView>),
    /// The resident refused or failed the read, with its stable reason.
    Failed(String),
}

impl AppRuntime {
    /// The resolved sources of one contribution, absent until a read starts.
    #[must_use]
    pub fn contribution_sources(
        &self,
        publication: &RoomAuthorityStamp,
    ) -> Option<&ContributionSources> {
        self.contribution_sources.get(publication)
    }

    /// Take up to `limit` owed reads, oldest contribution first, each now
    /// marked loading. A contribution owes one when a surface shows its
    /// Design, it cites any source, and it has no read.
    pub fn take_contribution_source_reads(
        &mut self,
        limit: usize,
    ) -> Vec<(RoomAnchor, RoomAuthorityStamp)> {
        let mut owed = Vec::new();
        for room in self.task_details.shown() {
            for contribution in self.rooms.contributions(room) {
                if owed.len() == limit {
                    break;
                }
                if !contribution.sources.is_empty()
                    && !self
                        .contribution_sources
                        .contains_key(contribution.publication)
                {
                    owed.push((room.clone(), contribution.publication.clone()));
                }
            }
        }
        for (_, publication) in &owed {
            self.contribution_sources
                .insert(publication.clone(), ContributionSources::Loading);
        }
        owed
    }

    /// Owe a taken read again because the connection could not send it.
    pub fn restore_contribution_source_read(&mut self, publication: &RoomAuthorityStamp) {
        self.contribution_sources.remove(publication);
    }

    /// Apply one correlated sources read without crossing capability scope
    /// or contribution identity; only a read still in flight takes it.
    pub fn apply_contribution_sources(
        &mut self,
        publication: &RoomAuthorityStamp,
        admitted: &Admitted<Response<RoomContributionSourcesAnswer>>,
    ) {
        if admitted.scope() != self.accepted.scope()
            || self.contribution_sources.get(publication) != Some(&ContributionSources::Loading)
        {
            return;
        }
        let sources = match &admitted.get().outcome {
            Outcome::Reply(reply) if reply.body.publication == *publication => {
                ContributionSources::Read(reply.body.sources.clone())
            }
            Outcome::Reply(_) => ContributionSources::Failed(
                "room::contribution-sources-identity-mismatch".to_owned(),
            ),
            Outcome::Refusal(refusal) => ContributionSources::Failed(
                refusal
                    .diagnostic()
                    .map_or(
                        "room::contribution-sources-refused",
                        wire::RefusalDiagnostic::reason,
                    )
                    .to_owned(),
            ),
            Outcome::Fault(fault) => ContributionSources::Failed(fault.reason.clone()),
        };
        self.contribution_sources
            .insert(publication.clone(), sources);
    }

    /// A reconnect re-reads every source: a read in flight never answers, and
    /// a settled one may have lost its owning bytes since.
    pub(super) fn forget_contribution_sources(&mut self) {
        self.contribution_sources.clear();
    }
}
