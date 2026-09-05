//! The resident's single exhaustive command dispatcher.
//! Every admitted command reaches exactly one domain handler from this table.

use bus::Bus;
use lane::occupant::Seats;
use spice::placeholder::PlaceholderReason;
use spice::repository::Repository;
use wire::intent::{CommandAnswer, Intent};
use wire::{Actor, CanonicalUuid, CommandKind, Outcome, Refusal, Reply};

use super::{
    RuntimePaths, SurfaceConfig, dispatch_release_publish, dispatch_up_change, effort_surface,
    lane_recovery, lane_surface, lifecycle, policy_surface, read_materializations::CurrentReads,
    steering,
};

pub(super) struct Surface<'a> {
    bus: &'a Bus,
    repository: &'a Repository,
    pub(super) paths: &'a RuntimePaths,
    epoch: CanonicalUuid,
    lane: lane_surface::Runtime<'a>,
    recovery: lane_recovery::Runtime,
    pub(super) config: &'a SurfaceConfig,
    reads: &'a CurrentReads,
}

pub(super) struct Runtimes<'a> {
    lane: lane_surface::Runtime<'a>,
    recovery: lane_recovery::Runtime,
}

impl<'a> Runtimes<'a> {
    pub(super) fn new(lane: lane_surface::Runtime<'a>, recovery: lane_recovery::Runtime) -> Self {
        Self { lane, recovery }
    }
}

impl<'a> Surface<'a> {
    pub(super) fn new(
        bus: &'a Bus,
        repository: &'a Repository,
        paths: &'a RuntimePaths,
        epoch: CanonicalUuid,
        runtimes: Runtimes<'a>,
        config: &'a SurfaceConfig,
        reads: &'a CurrentReads,
    ) -> Self {
        let Runtimes { lane, recovery } = runtimes;
        Self {
            bus,
            repository,
            paths,
            epoch,
            lane,
            recovery,
            config,
            reads,
        }
    }

    pub(super) fn refresh_reads(&self) -> super::CommandResult<()> {
        self.reads
            .refresh()
            .map_err(|error| super::store_fault("resident::read-materializer-unavailable", &error))
    }

    pub(super) fn synchronize_reads_before_publish(&self) -> super::CommandResult<()> {
        self.reads
            .synchronize_before_publish()
            .map_err(|error| super::store_fault("resident::read-materialization-failed", &error))
    }

    pub(super) fn replay(
        &self,
        seats: &Seats,
        command: &wire::AdmittedCommand,
    ) -> Option<Outcome<CommandAnswer>> {
        let outcome = replay_command(self.bus, command)?;
        let Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Lane(_),
        }) = outcome
        else {
            return Some(outcome);
        };
        let current = lane_surface::read(self.bus, self.paths, self.epoch, seats, command);
        let Outcome::Reply(Reply {
            body: CommandAnswer::Lane(answer),
            ..
        }) = current
        else {
            return Some(current);
        };
        Some(Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Lane(answer),
        }))
    }

    pub(super) fn dispatch_fresh(
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
        Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
            PlaceholderReason::HandlerNotLanded.to_reason(),
            None,
            Some(command.kind().to_string()),
            None,
        )))
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
                Some(effort_surface::read(self.reads, read))
            }
            (CommandKind::ConsentRead, Intent::ConsentRead(_)) => {
                Some(policy_surface::read(self.reads, &self.config.consent))
            }
            (CommandKind::ConsentPolicy, Intent::ConsentPolicy(_)) => {
                Some(policy_surface::policy(self.reads, &self.config.consent))
            }
            (CommandKind::ConsentDecide, Intent::ConsentDecide(decide)) => {
                Some(policy_surface::decide(
                    self.bus,
                    self.reads,
                    self.epoch,
                    &self.config.consent,
                    command,
                    decide,
                ))
            }
            (CommandKind::TrustPend, Intent::TrustRef(reference)) => {
                Some(policy_surface::trust_pend(
                    self.bus,
                    self.repository,
                    self.epoch,
                    command,
                    reference,
                ))
            }
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
    ) -> super::CommandResult<bool> {
        let mut released = lifecycle.observe_topology(command);
        if let Intent::LaneState(state) = command.body()
            && command.actor() == &Actor::Operator
            && state.action == wire::intent::LaneAction::Wake
            && state.worktree.is_none()
        {
            lifecycle.operator_wake(self.bus, self.epoch, state.lane)?;
            released = true;
        }
        Ok(released)
    }
}

pub(in crate::resident) fn replay_command(
    bus: &Bus,
    command: &wire::AdmittedCommand,
) -> Option<Outcome<CommandAnswer>> {
    let replayed = command.clone();
    match bus.read(move |read| read.command_replay(&replayed)) {
        Ok(Some(journal::CommandReplay::Exact(outcome))) => Some(outcome),
        Ok(Some(journal::CommandReplay::Conflict)) => Some(Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "command::identity-reused",
                Some("id".to_owned()),
                Some("an unused identity or the exact original request".to_owned()),
                Some(command.id().to_string()),
            ),
        ))),
        Ok(None) => None,
        Err(error) => Some(super::typed_store_fault(
            "command::receipt-unreadable",
            &error.to_string(),
        )),
    }
}
