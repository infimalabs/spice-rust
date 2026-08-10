//! The resident's single exhaustive command dispatcher.
//! Every admitted command reaches exactly one domain handler from this table.

use bus::Bus;
use lane::occupant::Seats;
use spice::placeholder::PlaceholderReason;
use spice::repository::Repository;
use wire::intent::{CommandAnswer, Intent};
use wire::{Actor, CanonicalUuid, CommandKind, Outcome, Refusal};

use super::{
    RuntimePaths, SurfaceConfig, dispatch_release_publish, dispatch_up_change, effort_surface,
    lane_recovery, lane_surface, lifecycle, policy_surface, steering,
};

pub(super) struct Surface<'a> {
    bus: &'a Bus,
    repository: &'a Repository,
    pub(super) paths: &'a RuntimePaths,
    epoch: CanonicalUuid,
    lane: lane_surface::Runtime<'a>,
    recovery: lane_recovery::Runtime,
    pub(super) config: &'a SurfaceConfig,
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
            &self.config.allocation,
        ) {
            return outcome;
        }
        if let Some(outcome) = self.dispatch_lane(seats, command) {
            return outcome;
        }
        if let Some(outcome) = self.dispatch_controls(command) {
            return outcome;
        }
        Outcome::Refusal(Refusal {
            reason: PlaceholderReason::HandlerNotLanded.to_reason(),
            witness: None,
            expected: Some(command.kind().to_string()),
            observed: None,
            remedy: "land the registered resident handler before exposing this CLI leaf".to_owned(),
        })
    }

    fn dispatch_lane(
        &self,
        seats: &mut Seats,
        command: &wire::AdmittedCommand,
    ) -> Option<Outcome<CommandAnswer>> {
        match (command.kind(), command.body()) {
            (CommandKind::UpChange, Intent::UpChange(change)) => Some(dispatch_up_change(
                self.bus, self.paths, self.epoch, seats, command, change,
            )),
            (CommandKind::ReleasePublish, Intent::ReleasePublish(publish)) => Some(
                dispatch_release_publish(self.bus, self.paths, self.epoch, command, publish),
            ),
            (CommandKind::LaneRead, Intent::LaneRead(_)) => Some(lane_surface::read(
                self.bus, self.paths, self.epoch, seats, command,
            )),
            (CommandKind::LaneSeat, Intent::LaneSeat(seat)) => Some(lane_surface::seat(
                self.bus, self.paths, self.epoch, seats, &self.lane, command, seat,
            )),
            (CommandKind::LaneArrange, Intent::LaneArrange(arrange)) => Some(
                lane_surface::arrange(self.bus, self.epoch, seats, command, arrange),
            ),
            (CommandKind::LaneState, Intent::LaneState(state)) if state.worktree.is_some() => {
                Some(lane_recovery::import(
                    self.bus,
                    self.paths,
                    self.epoch,
                    seats,
                    &self.recovery,
                    command,
                    state,
                ))
            }
            (CommandKind::LaneState, Intent::LaneState(state)) => Some(lane_surface::state(
                self.bus, self.paths, self.epoch, seats, &self.lane, command, state,
            )),
            (CommandKind::LineageArrange, Intent::LineageArrange(arrange)) => Some(
                lane_surface::arrange_lineages(self.bus, self.epoch, seats, command, arrange),
            ),
            (CommandKind::LineageConfigure, Intent::LineageConfigure(configure)) => {
                Some(lane_surface::configure(
                    self.bus,
                    self.epoch,
                    seats,
                    &self.config.projects,
                    command,
                    configure,
                ))
            }
            (CommandKind::LaneRecall, Intent::LaneRecall(recall)) => Some(lane_surface::recall(
                self.bus, self.paths, self.epoch, seats, command, recall,
            )),
            _ => None,
        }
    }

    fn dispatch_controls(&self, command: &wire::AdmittedCommand) -> Option<Outcome<CommandAnswer>> {
        match (command.kind(), command.body()) {
            (CommandKind::SteerInbox, Intent::SteerInbox(inbox)) => Some(steering::inbox(
                self.bus, self.paths, self.epoch, command, inbox,
            )),
            (CommandKind::SteerRepair, Intent::SteerRepair(repair)) => Some(steering::repair(
                self.bus, self.paths, self.epoch, command, repair,
            )),
            (CommandKind::SteerSend, Intent::SteerSend(send)) => Some(steering::send(
                self.bus,
                self.paths,
                self.epoch,
                &self.config.steering,
                command,
                send,
            )),
            (CommandKind::SteerCancel, Intent::DirectiveRef(cancel)) => Some(steering::cancel(
                self.bus, self.paths, self.epoch, command, cancel,
            )),
            (CommandKind::SteerResend, Intent::DirectiveRef(resend)) => Some(steering::resend(
                self.bus,
                self.paths,
                self.epoch,
                &self.config.steering,
                command,
                resend,
            )),
            (CommandKind::SteerRecover, Intent::DirectiveRef(recover)) => Some(steering::recover(
                self.bus,
                self.paths,
                self.epoch,
                &self.config.steering,
                command,
                recover,
            )),
            (CommandKind::SteerDispose, Intent::SteerDispose(dispose)) => Some(steering::dispose(
                self.bus, self.paths, self.epoch, command, dispose,
            )),
            (CommandKind::EffortRead, Intent::EffortRead(read)) => {
                Some(effort_surface::read(self.bus, self.paths, read))
            }
            (CommandKind::ConsentRead, Intent::ConsentRead(_)) => {
                Some(policy_surface::read(self.bus, &self.config.consent))
            }
            (CommandKind::ConsentPolicy, Intent::ConsentPolicy(_)) => {
                Some(policy_surface::policy(self.bus, &self.config.consent))
            }
            (CommandKind::ConsentDecide, Intent::ConsentDecide(decide)) => Some(
                policy_surface::decide(self.bus, self.epoch, &self.config.consent, command, decide),
            ),
            (CommandKind::TrustGrant | CommandKind::TrustRevoke, Intent::TrustRef(reference)) => {
                Some(policy_surface::trust_rule(
                    self.bus,
                    self.repository,
                    self.epoch,
                    command,
                    reference,
                ))
            }
            _ => None,
        }
    }

    pub(super) fn reconcile_lifecycle(
        &self,
        lifecycle: &mut lifecycle::Runtime,
        seats: &mut Seats,
        diagnostic: &mut dyn std::io::Write,
    ) -> super::CommandResult<Option<lifecycle::WakeDeadline>> {
        lifecycle.reconcile(
            self.bus,
            self.repository,
            self.paths,
            self.epoch,
            seats,
            &self.lane,
            self.config,
            diagnostic,
        )
    }

    pub(super) fn observe_lifecycle_death(
        &self,
        lifecycle: &mut lifecycle::Runtime,
        seats: &Seats,
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
    ) -> super::CommandResult<()> {
        lifecycle.observe_death(
            self.bus,
            self.epoch,
            seats,
            lane,
            occupant,
            &self.config.lane,
        )
    }

    pub(super) fn expire_lifecycle_hold(
        &self,
        lifecycle: &mut lifecycle::Runtime,
        lane: CanonicalUuid,
    ) -> super::CommandResult<()> {
        lifecycle.expire_hold(self.bus, self.epoch, lane)
    }

    pub(super) fn prepare_explicit_lifecycle(
        &self,
        lifecycle: &mut lifecycle::Runtime,
        command: &wire::AdmittedCommand,
    ) -> super::CommandResult<()> {
        if let Intent::LaneState(state) = command.body()
            && command.actor() == &Actor::Operator
            && state.action == wire::intent::LaneAction::Wake
            && state.worktree.is_none()
        {
            lifecycle.operator_wake(self.bus, self.epoch, state.lane)?;
        }
        Ok(())
    }
}
