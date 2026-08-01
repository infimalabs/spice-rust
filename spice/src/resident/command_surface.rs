//! The resident's single exhaustive command dispatcher.

use bus::Bus;
use lane::occupant::Seats;
use spice::repository::Repository;
use wire::intent::{CommandAnswer, Intent};
use wire::{CanonicalUuid, CommandKind, Outcome, Refusal};

use super::{
    RuntimePaths, dispatch_release_publish, dispatch_up_change, lane_surface, policy_surface,
};

pub(super) struct Surface<'a> {
    bus: &'a Bus,
    repository: &'a Repository,
    paths: &'a RuntimePaths,
    epoch: CanonicalUuid,
    capabilities: &'a canvas::axum::Capabilities,
    consent_policy: &'a consent::Policy,
}

impl<'a> Surface<'a> {
    pub(super) const fn new(
        bus: &'a Bus,
        repository: &'a Repository,
        paths: &'a RuntimePaths,
        epoch: CanonicalUuid,
        capabilities: &'a canvas::axum::Capabilities,
        consent_policy: &'a consent::Policy,
    ) -> Self {
        Self {
            bus,
            repository,
            paths,
            epoch,
            capabilities,
            consent_policy,
        }
    }

    pub(super) fn dispatch(
        &self,
        seats: &mut Seats,
        command: &wire::AdmittedCommand,
    ) -> Outcome<CommandAnswer> {
        match (command.kind(), command.body()) {
            (CommandKind::UpChange, Intent::UpChange(change)) => {
                dispatch_up_change(self.bus, self.paths, self.epoch, seats, command, change)
            }
            (CommandKind::ReleasePublish, Intent::ReleasePublish(publish)) => {
                dispatch_release_publish(self.bus, self.paths, self.epoch, command, publish)
            }
            (CommandKind::LaneRead, Intent::LaneRead(_)) => {
                lane_surface::read(self.bus, self.paths, self.epoch, command)
            }
            (CommandKind::LaneSeat, Intent::LaneSeat(seat)) => lane_surface::seat(
                self.bus,
                self.paths,
                self.epoch,
                seats,
                self.capabilities,
                command,
                seat,
            ),
            (CommandKind::LaneState, Intent::LaneState(state)) => {
                lane_surface::state(self.bus, self.paths, self.epoch, seats, command, state)
            }
            (CommandKind::LaneRecall, Intent::LaneRecall(recall)) => {
                lane_surface::recall(self.bus, self.paths, self.epoch, command, recall)
            }
            (CommandKind::ConsentRead, Intent::ConsentRead(_)) => {
                policy_surface::read(self.bus, self.consent_policy)
            }
            (CommandKind::ConsentPolicy, Intent::ConsentPolicy(_)) => {
                policy_surface::policy(self.bus, self.consent_policy)
            }
            (CommandKind::ConsentDecide, Intent::ConsentDecide(decide)) => {
                policy_surface::decide(self.bus, self.epoch, self.consent_policy, command, decide)
            }
            (CommandKind::TrustGrant | CommandKind::TrustRevoke, Intent::TrustRef(reference)) => {
                policy_surface::trust_rule(
                    self.bus,
                    self.repository,
                    self.epoch,
                    command,
                    reference,
                )
            }
            _ => Outcome::Refusal(Refusal {
                reason: "command::handler-not-landed".to_owned(),
                witness: None,
                expected: Some(command.kind().to_string()),
                observed: None,
                remedy: "land the registered resident handler before exposing this CLI leaf"
                    .to_owned(),
            }),
        }
    }
}
