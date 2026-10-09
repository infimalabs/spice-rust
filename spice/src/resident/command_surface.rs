//! The resident's single exhaustive command dispatcher.
//! Every admitted command reaches exactly one domain handler from this table.

use std::io;

use bus::Bus;
use lane::occupant::Seats;
use spice::placeholder::PlaceholderReason;
use spice::repository::Repository;
use wire::intent::{CommandAnswer, Intent};
use wire::{Actor, CanonicalUuid, CommandKind, Outcome, Refusal, Reply};

use super::{
    CommandDispatch, CommandEffects, RuntimePaths, SurfaceConfig, consent_surface,
    current_residency, dispatch_proof_recognize, dispatch_release_publish, dispatch_up_change,
    lane_recovery, lane_surface, lifecycle, policy_surface, read_materializations::CurrentReads,
    settings_surface, steering, vendor_surface,
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

    /// Record a seated occupant's consent request and answer what policy
    /// answers.
    pub(super) fn ask_consent(
        &self,
        residency: &wire::ResidencyHead,
        ask: consent_surface::LaneAsk,
        diagnostic: &mut dyn io::Write,
    ) -> super::CommandResult<()> {
        consent_surface::ask(
            self.bus,
            self.reads,
            self.paths,
            &self.config.consent,
            residency,
            ask,
            diagnostic,
        )
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
    ) -> CommandDispatch {
        if let Some(outcome) = crate::board_command::dispatch_with_repository(
            self.bus,
            command,
            Some(self.repository),
            &self.config.projects,
            &self.config.allocation,
        ) {
            return CommandDispatch::ordinary(outcome);
        }
        if let Some(dispatched) = self.dispatch_lane(seats, command) {
            return dispatched;
        }
        if let Some(dispatched) = self.dispatch_controls(command) {
            return dispatched;
        }
        CommandDispatch::ordinary(Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                PlaceholderReason::HandlerNotLanded.to_reason(),
                None,
                Some(command.kind().to_string()),
                None,
            ),
        )))
    }

    fn dispatch_lane(
        &self,
        seats: &mut Seats,
        command: &wire::AdmittedCommand,
    ) -> Option<CommandDispatch> {
        let mut effects = CommandEffects::default();
        let ordinary = |outcome| Some(CommandDispatch::ordinary(outcome));
        match (command.kind(), command.body()) {
            (CommandKind::UpChange, Intent::UpChange(change)) => ordinary(dispatch_up_change(
                self.bus, self.paths, self.epoch, seats, command, change,
            )),
            (CommandKind::ReleasePublish, Intent::ReleasePublish(publish)) => ordinary(
                dispatch_release_publish(self.bus, self.paths, self.epoch, command, publish),
            ),
            (CommandKind::ProofRecognize, Intent::Recognize(recognize)) => ordinary(
                dispatch_proof_recognize(self.bus, self.paths, self.epoch, command, recognize),
            ),
            (CommandKind::LaneRead, Intent::LaneRead(_)) => ordinary(lane_surface::read(
                self.bus, self.paths, self.epoch, seats, command,
            )),
            (CommandKind::LaneSeat, Intent::LaneSeat(seat)) => ordinary(lane_surface::seat(
                self.bus, self.paths, self.epoch, seats, &self.lane, command, seat,
            )),
            (CommandKind::LaneArrange, Intent::LaneArrange(arrange)) => ordinary(
                lane_surface::arrange(self.bus, self.paths, self.epoch, seats, command, arrange),
            ),
            (CommandKind::LaneState, Intent::LaneState(state)) if state.worktree.is_some() => Some(
                match lane_recovery::begin_import(
                    self.bus,
                    self.paths,
                    self.epoch,
                    seats,
                    &self.recovery,
                    state,
                ) {
                    Ok(launch) => CommandDispatch::import(launch),
                    Err(outcome) => CommandDispatch::ordinary(outcome),
                },
            ),
            (CommandKind::LaneState, Intent::LaneState(state))
                if state.action == wire::intent::LaneAction::Wake =>
            {
                Some(
                    match lane_surface::begin_wake(
                        self.bus,
                        self.paths,
                        self.epoch,
                        seats,
                        &self.lane,
                        &self.config.steering,
                        state,
                    ) {
                        Ok(launch) => CommandDispatch::wake(launch),
                        Err(outcome) => CommandDispatch::ordinary(outcome),
                    },
                )
            }
            (CommandKind::LaneState, Intent::LaneState(state)) => ordinary(lane_surface::state(
                self.bus,
                self.paths,
                self.epoch,
                seats,
                &self.lane,
                &self.config.steering,
                command,
                state,
            )),
            (CommandKind::LaneRenewal, Intent::LaneRenewal(renewal)) => {
                ordinary(lane_surface::renew(
                    self.bus,
                    self.paths,
                    self.epoch,
                    seats,
                    &self.config.steering,
                    command,
                    renewal,
                ))
            }
            (CommandKind::LaneRetune, Intent::LaneRetune(retune)) => {
                let outcome = lane_surface::retune(
                    self.bus,
                    self.paths,
                    self.epoch,
                    seats,
                    command,
                    retune,
                    &mut effects,
                );
                Some(CommandDispatch::with_effects(outcome, effects))
            }
            (CommandKind::LaneEffortRetune, Intent::LaneEffortRetune(retune)) => {
                let outcome = lane_surface::retune_effort(
                    self.bus,
                    self.paths,
                    self.epoch,
                    seats,
                    command,
                    retune,
                    &mut effects,
                );
                Some(CommandDispatch::with_effects(outcome, effects))
            }
            (CommandKind::LaneFastRetune, Intent::LaneFastRetune(retune)) => {
                let outcome = lane_surface::retune_fast(
                    self.bus,
                    self.paths,
                    self.epoch,
                    seats,
                    command,
                    retune,
                    &mut effects,
                );
                Some(CommandDispatch::with_effects(outcome, effects))
            }
            (CommandKind::TeamArrange, Intent::TeamArrange(arrange)) => {
                let outcome = lane_surface::arrange_teams(
                    self.bus,
                    self.paths,
                    self.epoch,
                    seats,
                    command,
                    arrange,
                    &mut effects,
                );
                Some(CommandDispatch::with_effects(outcome, effects))
            }
            (CommandKind::TeamConfigure, Intent::TeamConfigure(configure)) => {
                let outcome = lane_surface::configure(
                    self.bus,
                    self.paths,
                    self.epoch,
                    seats,
                    command,
                    configure,
                    &mut effects,
                );
                Some(CommandDispatch::with_effects(outcome, effects))
            }
            (CommandKind::LaneRecall, Intent::LaneRecall(recall)) => ordinary(
                lane_surface::recall(self.bus, self.paths, self.epoch, seats, command, recall),
            ),
            _ => None,
        }
    }

    fn dispatch_controls(&self, command: &wire::AdmittedCommand) -> Option<CommandDispatch> {
        self.dispatch_steering(command)
            .or_else(|| self.dispatch_policy(command).map(CommandDispatch::ordinary))
    }

    fn dispatch_steering(&self, command: &wire::AdmittedCommand) -> Option<CommandDispatch> {
        let mut effects = CommandEffects::default();
        let outcome = match (command.kind(), command.body()) {
            (CommandKind::SteerInbox, Intent::SteerInbox(inbox)) => Some(steering::inbox(
                self.bus, self.paths, self.epoch, command, inbox,
            )),
            (CommandKind::SteerHistory, Intent::SteerHistory(request)) => {
                Some(steering::history(self.bus, self.paths, request))
            }
            (CommandKind::SteerSend, Intent::SteerSend(send)) => Some(steering::send(
                self.bus,
                self.epoch,
                &self.config.steering,
                command,
                send,
                &mut effects,
            )),
            (CommandKind::SteerTell, Intent::SteerTell(tell)) => Some(steering::tell(
                self.bus,
                self.epoch,
                &self.config.steering,
                command,
                tell,
                &mut effects,
            )),
            (CommandKind::SteerCancel, Intent::DirectiveRef(cancel)) => Some(steering::cancel(
                self.bus,
                self.epoch,
                command,
                cancel,
                &mut effects,
            )),
            (CommandKind::SteerResend, Intent::DirectiveRef(resend)) => Some(steering::resend(
                self.bus,
                self.paths,
                self.epoch,
                &self.config.steering,
                command,
                resend,
                &mut effects,
            )),
            (CommandKind::SteerRecover, Intent::DirectiveRef(recover)) => Some(steering::recover(
                self.bus,
                self.epoch,
                &self.config.steering,
                command,
                recover,
                &mut effects,
            )),
            _ => None,
        }?;
        Some(CommandDispatch::with_effects(outcome, effects))
    }

    fn dispatch_policy(&self, command: &wire::AdmittedCommand) -> Option<Outcome<CommandAnswer>> {
        match (command.kind(), command.body()) {
            (CommandKind::GlobalSettingsRead, Intent::GlobalSettingsRead(_)) => {
                Some(settings_surface::read(self.bus, self.repository))
            }
            (CommandKind::VendorCatalog, Intent::VendorCatalog(_)) => {
                Some(vendor_surface::catalog(self.bus, self.repository))
            }
            (CommandKind::GlobalSettingsSet, Intent::GlobalSettingsSet(settings)) => Some(
                settings_surface::set(self.bus, self.repository, self.epoch, command, settings),
            ),
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
                    self.paths,
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

    pub(super) fn inbox_residency(
        &self,
    ) -> super::CommandResult<journal::JournalHead<wire::ResidencyHead>> {
        current_residency(self.bus, self.epoch)
    }

    pub(super) fn complete_command_launch(
        &self,
        seats: &mut Seats,
        command: &wire::AdmittedCommand,
        completion: super::CommandLaunchCompletion,
        launched: super::CommandResult<super::supervisor::Launch>,
    ) -> Outcome<CommandAnswer> {
        match completion {
            super::CommandLaunchCompletion::Wake(pending) => lane_surface::finish_wake(
                self.bus, self.paths, self.epoch, seats, command, pending, launched,
            ),
            super::CommandLaunchCompletion::Import(pending) => lane_recovery::finish_import(
                self.bus, self.paths, self.epoch, seats, command, pending, launched,
            ),
        }
    }

    pub(super) fn complete_automatic_launch(
        &self,
        lifecycle: &mut lifecycle::Runtime,
        seats: &mut Seats,
        diagnostic: &mut dyn std::io::Write,
        pending: lifecycle::PendingLaunch,
        launched: super::CommandResult<super::supervisor::Launch>,
    ) -> super::CommandResult<()> {
        lifecycle.complete_launch(
            self.bus,
            self.repository,
            self.paths,
            self.epoch,
            seats,
            diagnostic,
            pending,
            launched,
        )
    }

    pub(super) fn notify_inbox(&self, lane: CanonicalUuid) {
        steering::notify_inbox(self.paths, lane);
    }

    pub(super) const fn epoch(&self) -> CanonicalUuid {
        self.epoch
    }

    pub(super) fn reconcile_lifecycle(
        &self,
        lifecycle: &mut lifecycle::Runtime,
        seats: &mut Seats,
        launches: &mut super::launch_runtime::Runtime,
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
            launches,
            diagnostic,
        )
    }

    pub(super) fn observe_lifecycle_death(
        &self,
        lifecycle: &mut lifecycle::Runtime,
        seats: &Seats,
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        diagnostic: &mut dyn std::io::Write,
    ) -> super::CommandResult<()> {
        let seated = seats
            .started(lane)
            .filter(|activity| activity.occupant() == occupant);
        lifecycle.observe_death(self.bus, self.epoch, seated, lane, false, diagnostic)
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
        {
            if lifecycle.launch_pending(state.lane) {
                return Err(Outcome::Refusal(Refusal::bare(
                    wire::RefusalDiagnostic::new(
                        "lifecycle::launch-pending",
                        Some("lane".to_owned()),
                        Some("no launch proof pending for this lane".to_owned()),
                        Some(state.lane.to_string()),
                    )
                    .with_detail(
                        "wait for the current wake to settle before starting another occupant",
                    ),
                )));
            }
            if state.worktree.is_none() {
                lifecycle.operator_wake(self.bus, self.epoch, state.lane)?;
                released = true;
            }
        }
        if matches!(command.body(), Intent::LaneRenewal(_)) {
            lifecycle.release_all();
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
