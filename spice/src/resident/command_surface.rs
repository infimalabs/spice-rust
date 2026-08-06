//! The resident's single exhaustive command dispatcher.
//! Every admitted command reaches exactly one domain handler from this table.

use bus::Bus;
use lane::occupant::Seats;
use spice::placeholder::PlaceholderReason;
use spice::repository::Repository;
use wire::intent::{CommandAnswer, Intent};
use wire::{CanonicalUuid, CommandKind, Outcome, Refusal};

use super::{
    RuntimePaths, SurfaceConfig, dispatch_release_publish, dispatch_up_change, lane_recovery,
    lane_surface, policy_surface, steering,
};

pub(super) struct Surface<'a> {
    bus: &'a Bus,
    repository: &'a Repository,
    paths: &'a RuntimePaths,
    epoch: CanonicalUuid,
    lane: lane_surface::Runtime<'a>,
    recovery: lane_recovery::Runtime,
    config: &'a SurfaceConfig,
}

impl<'a> Surface<'a> {
    pub(super) const fn new(
        bus: &'a Bus,
        repository: &'a Repository,
        paths: &'a RuntimePaths,
        epoch: CanonicalUuid,
        lane: lane_surface::Runtime<'a>,
        recovery: lane_recovery::Runtime,
        config: &'a SurfaceConfig,
    ) -> Self {
        Self {
            bus,
            repository,
            paths,
            epoch,
            lane,
            recovery,
            config,
        }
    }

    pub(super) fn dispatch(
        &self,
        seats: &mut Seats,
        command: &wire::AdmittedCommand,
    ) -> Outcome<CommandAnswer> {
        if let Some(outcome) = crate::board_command::dispatch_with_repository(
            self.bus,
            command,
            Some(self.repository),
            &self.config.projects,
        ) {
            return outcome;
        }
        match (command.kind(), command.body()) {
            (CommandKind::UpChange, Intent::UpChange(change)) => {
                dispatch_up_change(self.bus, self.paths, self.epoch, seats, command, change)
            }
            (CommandKind::ReleasePublish, Intent::ReleasePublish(publish)) => {
                dispatch_release_publish(self.bus, self.paths, self.epoch, command, publish)
            }
            (CommandKind::LaneRead, Intent::LaneRead(_)) => {
                lane_surface::read(self.bus, self.paths, self.epoch, seats, command)
            }
            (CommandKind::LaneSeat, Intent::LaneSeat(seat)) => lane_surface::seat(
                self.bus, self.paths, self.epoch, seats, &self.lane, command, seat,
            ),
            (CommandKind::LaneState, Intent::LaneState(state)) if state.worktree.is_some() => {
                lane_recovery::import(
                    self.bus,
                    self.paths,
                    self.epoch,
                    seats,
                    &self.recovery,
                    command,
                    state,
                )
            }
            (CommandKind::LaneState, Intent::LaneState(state)) => lane_surface::state(
                self.bus, self.paths, self.epoch, seats, &self.lane, command, state,
            ),
            (CommandKind::LineageConfigure, Intent::LineageConfigure(configure)) => {
                lane_surface::configure(self.bus, self.epoch, seats, command, configure)
            }
            (CommandKind::LaneRecall, Intent::LaneRecall(recall)) => {
                lane_surface::recall(self.bus, self.paths, self.epoch, seats, command, recall)
            }
            (CommandKind::SteerInbox, Intent::SteerInbox(inbox)) => {
                steering::inbox(self.bus, self.epoch, command, inbox)
            }
            (CommandKind::SteerSend, Intent::SteerSend(send)) => steering::send(
                self.bus,
                self.paths,
                self.epoch,
                &self.config.steering,
                command,
                send,
            ),
            (CommandKind::SteerCancel, Intent::DirectiveRef(cancel)) => {
                steering::cancel(self.bus, self.paths, self.epoch, command, cancel)
            }
            (CommandKind::SteerResend, Intent::DirectiveRef(recover)) => steering::recover(
                self.bus,
                self.paths,
                self.epoch,
                &self.config.steering,
                command,
                recover,
            ),
            (CommandKind::SteerDispose, Intent::SteerDispose(dispose)) => {
                steering::dispose(self.bus, self.paths, self.epoch, command, dispose)
            }
            (CommandKind::ConsentRead, Intent::ConsentRead(_)) => {
                policy_surface::read(self.bus, &self.config.consent)
            }
            (CommandKind::ConsentPolicy, Intent::ConsentPolicy(_)) => {
                policy_surface::policy(self.bus, &self.config.consent)
            }
            (CommandKind::ConsentDecide, Intent::ConsentDecide(decide)) => {
                policy_surface::decide(self.bus, self.epoch, &self.config.consent, command, decide)
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
                reason: PlaceholderReason::HandlerNotLanded.to_reason(),
                witness: None,
                expected: Some(command.kind().to_string()),
                observed: None,
                remedy: "land the registered resident handler before exposing this CLI leaf"
                    .to_owned(),
            }),
        }
    }
}
