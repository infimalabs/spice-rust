//! Automatic-launch authority retained across off-loop session proof.

use super::*;

pub(in crate::resident) struct PendingLaunch {
    trigger: Trigger,
    lane: CanonicalUuid,
    /// The steering an inbox wake was admitted to deliver.
    woken: Option<super::super::steering::WokenInbox>,
    claim: Option<crate::board_command::AutomaticClaim>,
    wake: lane_surface::PendingAutomaticWake,
}

pub(super) struct Launching {
    pub(super) launch: super::super::supervisor::PendingLaunch,
    pub(super) pending: PendingLaunch,
}

pub(super) fn pending_launch(
    trigger: Trigger,
    lane: CanonicalUuid,
    woken: Option<super::super::steering::WokenInbox>,
    claim: Option<crate::board_command::AutomaticClaim>,
    launch: lane_surface::AutomaticLaunch,
) -> Attempted {
    let (launch, wake) = launch.into_parts();
    Attempted::Launching(Launching {
        launch,
        pending: PendingLaunch {
            trigger,
            lane,
            woken,
            claim,
            wake,
        },
    })
}

impl Runtime {
    /// Settle one automatic launch, answering the lane it seated and started.
    #[allow(
        clippy::too_many_arguments,
        reason = "one completion settles the lifecycle authority that admitted its launch"
    )]
    pub(in crate::resident) fn complete_launch(
        &mut self,
        bus: &Bus,
        repository: &Repository,
        paths: &RuntimePaths,
        epoch: CanonicalUuid,
        seats: &mut Seats,
        diagnostic: &mut dyn std::io::Write,
        pending: PendingLaunch,
        launched: CommandResult<super::super::supervisor::Launch>,
    ) -> CommandResult<Option<CanonicalUuid>> {
        self.launching.remove(&pending.lane);
        let result =
            lane_surface::finish_automatic_wake(bus, paths, epoch, seats, pending.wake, launched);
        let attempted = settle_launch(
            bus,
            repository,
            paths,
            epoch,
            pending.trigger,
            pending.lane,
            pending.woken.as_ref(),
            pending.claim.as_ref(),
            result,
        );
        match attempted {
            Ok(Attempted::Started) => {
                self.backoff.release(pending.lane);
                let _ = writeln!(
                    diagnostic,
                    "lifecycle started trigger={} lane={}",
                    pending.trigger.as_str(),
                    pending.lane
                );
                Ok(Some(pending.lane))
            }
            Ok(Attempted::DeadLettered(parked)) => {
                self.backoff.release(pending.lane);
                for directive in parked {
                    let _ = writeln!(
                        diagnostic,
                        "lifecycle dead-lettered trigger={} lane={} key={} directive={} requeue={}",
                        pending.trigger.as_str(),
                        pending.lane,
                        directive.key,
                        directive.directive,
                        directive.requeue,
                    );
                }
                Ok(None)
            }
            Ok(Attempted::Launching(_)) => {
                unreachable!("one launch completion cannot begin another launch")
            }
            Err(Outcome::Refusal(refusal)) => {
                let now = current_instant()?;
                let _ = self.backoff.refused(
                    pending.trigger,
                    pending.lane,
                    refusal.reason(),
                    now,
                    diagnostic,
                )?;
                Ok(None)
            }
            Err(Outcome::Fault(fault)) => Err(Outcome::Fault(fault)),
            Err(Outcome::Reply(_)) => {
                unreachable!("unit lifecycle failures carry no reply")
            }
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one completed launch retains the exact trigger compensation contract"
)]
fn settle_launch(
    bus: &Bus,
    repository: &Repository,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    trigger: Trigger,
    lane: CanonicalUuid,
    woken: Option<&super::super::steering::WokenInbox>,
    claim: Option<&crate::board_command::AutomaticClaim>,
    launched: CommandResult<lane_surface::AutomaticWake>,
) -> CommandResult<Attempted> {
    match trigger {
        Trigger::PendingInbox => {
            let woken = woken.expect("an inbox wake retains the steering it was woken for");
            match launched {
                Ok(lane_surface::AutomaticWake::Started) => Ok(Attempted::Started),
                Ok(lane_surface::AutomaticWake::Unstarted(failure)) => {
                    super::super::steering::dead_letter_pending(
                        bus, paths, epoch, lane, woken, &failure,
                    )
                    .map(Attempted::DeadLettered)
                }
                Err(failure) if pending_start_failure_is_terminal(&failure) => {
                    super::super::steering::dead_letter_pending(
                        bus, paths, epoch, lane, woken, &failure,
                    )
                    .map(Attempted::DeadLettered)
                }
                Err(failure) => Err(failure),
            }
        }
        Trigger::HeldClaim => match launched? {
            lane_surface::AutomaticWake::Started => Ok(Attempted::Started),
            lane_surface::AutomaticWake::Unstarted(failure) => Err(failure),
        },
        Trigger::DrainQueue | Trigger::AvailableWork => match launched {
            Ok(lane_surface::AutomaticWake::Started) => Ok(Attempted::Started),
            Ok(lane_surface::AutomaticWake::Unstarted(failure)) | Err(failure) => {
                let claim = claim.expect("work launch retains its reserved claim");
                match crate::board_command::release_automatic(bus, repository, claim) {
                    Ok(()) => Err(failure),
                    Err(release) => Err(unit_outcome(release)),
                }
            }
        },
    }
}
