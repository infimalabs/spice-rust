//! A Design made passive by a working-group change asks its holder to park.
//!
//! When a claimed Design enters a Paused working group or its group pauses, the change sends the
//! lane holding it one operator directive carrying a
//! [`wire::rooms::StopControl`] for the exact claim win, asking it to
//! checkpoint and free. The control makes the request idempotent: a later
//! change that leaves the same holder on the same win sends nothing new.

use super::*;

use wire::CanonicalUuid;
use wire::rooms::StopControl;

/// A Design's room as read before or after a partition change makes the Design
/// passive. The change asks the lane holding the Design to park, either in the
/// move write or in a following task-origin write.
pub(super) struct Deactivation {
    anchor: Entry,
    heads: wire::intent::BoardTask,
    /// Whether a stop already stands for the holder's current claim win.
    stopped: bool,
}

impl Deactivation {
    /// Read `design`'s room, or `None` when the board no longer shows it.
    pub(super) fn read(
        bus: &Bus,
        design: &TaskHandle,
    ) -> Result<Option<Self>, Outcome<CommandAnswer>> {
        let snapshot =
            validated_working_snapshot(bus, Actor::Operator, std::slice::from_ref(design))?;
        let Some(anchor) = task_entry(&snapshot.entries, design).cloned() else {
            return Ok(None);
        };
        let heads = task_heads(&snapshot.answer, design)?.clone();
        let stopped = match stop_control(&anchor, &heads) {
            Some(control) => stop_stands(bus, &control)?,
            None => false,
        };
        Ok(Some(Self {
            anchor,
            heads,
            stopped,
        }))
    }

    /// Whether the room owes nothing: no lane holds the Design, or a stop
    /// already stands for the holder's current claim win.
    pub(super) fn is_quiescent(&self) -> bool {
        self.stopped || active_lane_claim(&self.heads).is_none()
    }

    /// The origin the partition change commits under: the partition's own,
    /// unless it asks the Design's holder to park, whose claim the request
    /// cites and whose origin a cause cannot leave.
    pub(super) fn origin(&self) -> Result<wire::Origin, Outcome<CommandAnswer>> {
        if self.is_quiescent() {
            Ok(wire::rooms::working_group_origin())
        } else {
            task_origin(self.anchor.born())
        }
    }

    /// Stage the stop request into the partition change's `write`, guarding
    /// the claim it read and the absence of a standing stop, so a claim won or
    /// a stop sent meanwhile refuses the change. Returns the lane asked to
    /// park.
    pub(super) fn stage(
        self,
        bus: &Bus,
        command: &wire::AdmittedCommand,
        config: &steer::Config,
        write: &mut JournalWrite,
        at: Instant,
        directive: Option<(CanonicalUuid, CanonicalUuid)>,
    ) -> Result<Option<CanonicalUuid>, Outcome<CommandAnswer>> {
        guard(
            write,
            vec![Witness::Claim {
                task: self.anchor.born().clone(),
                head: self.heads.claim.as_ref().map(|claim| claim.position),
            }],
        );
        if self.is_quiescent() {
            return Ok(None);
        }
        let (lane, claim) = active_lane_claim(&self.heads)
            .expect("a non-quiescent deactivation holds an active lane claim");
        let control = StopControl {
            room: self.anchor.born().clone(),
            claim: wire::SequenceCode::new(claim.body.claimed.get()),
        };
        let guarded = control.clone();
        write.guard(journal::JournalGuard::new(move |read| {
            if read.room_stop(&guarded)?.is_none() {
                Ok(())
            } else {
                Err(journal::GuardError::refused("room::stop-standing"))
            }
        }));
        let origin = self.origin()?;
        let message = stop_message(self.anchor.handle());
        let control = control.to_string();
        let (directive, fanout) = directive.map_or((command.id(), None), |(directive, fanout)| {
            (directive, Some(fanout))
        });
        crate::resident::append_operator_directive(
            bus,
            origin,
            config,
            write,
            crate::resident::OperatorDirective {
                directive,
                fanout,
                lane,
                text: &message,
                control: Some(&control),
                room: None,
            },
            at,
            FactCause::Existing(claim.body.claimed),
        )?;
        crate::resident::steering_envelope_admits(write)?;
        Ok(Some(lane))
    }

    /// Commit this room's deactivation under its own task origin after the
    /// shared partition has made it passive. A guard refusal means the room
    /// moved since it was read, so the caller reads it again.
    pub(super) fn commit(
        self,
        bus: &Bus,
        command: &wire::AdmittedCommand,
        config: &steer::Config,
        directive: (CanonicalUuid, CanonicalUuid),
    ) -> Result<Shelving, Outcome<CommandAnswer>> {
        let origin = self.origin()?;
        let at = current_instant()?;
        let mut write =
            JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
        let lane = self.stage(bus, command, config, &mut write, at, Some(directive))?;
        match bus
            .commit(write)
            .map_err(|error| store_fault("room::deactivation-commit", &error.to_string()))?
        {
            CommitOutcome::Committed(commit) => Ok(Shelving::Committed {
                range: SeqRange {
                    first: seq(commit.first()),
                    last: seq(commit.last()),
                },
                lane,
            }),
            CommitOutcome::Refused(_) => Ok(Shelving::Moved),
            CommitOutcome::Fault(fault) => {
                Err(store_fault("room::deactivation-fault", &fault.to_string()))
            }
        }
    }
}

/// What one task-origin deactivation write did.
pub(super) enum Shelving {
    /// Its stop request committed over `range`, asking `lane` to park.
    Committed {
        range: SeqRange,
        lane: Option<CanonicalUuid>,
    },
    /// The room moved since it was read; read it again.
    Moved,
}

fn stop_message(anchor: &TaskHandle) -> String {
    format!(
        "Design {anchor} became passive because its working group is Paused. Checkpoint its \
         current work and run `spice task free {anchor}` at the next safe boundary; free \
         parks unpublished commits and dirty work under the task, and your claim stays \
         authoritative until then. Do not release child-plan claims or unrelated \
         steering/ASK duties."
    )
}

/// The stop control a deactivation of `anchor` would send its holder, absent
/// while no lane holds it.
fn stop_control(anchor: &Entry, heads: &wire::intent::BoardTask) -> Option<StopControl> {
    active_lane_claim(heads).map(|(_, claim)| StopControl {
        room: anchor.born().clone(),
        claim: wire::SequenceCode::new(claim.body.claimed.get()),
    })
}

/// Whether a stop carrying exactly `control` was ever sent, whatever its
/// disposition: a holder asked once is not asked again for the same win.
fn stop_stands(bus: &Bus, control: &StopControl) -> Result<bool, Outcome<CommandAnswer>> {
    let control = control.clone();
    bus.read(move |read| read.room_stop(&control).map(|head| head.is_some()))
        .map_err(|error| store_fault("room::stop-unreadable", &error.to_string()))
}

fn active_lane_claim(
    heads: &wire::intent::BoardTask,
) -> Option<(CanonicalUuid, &wire::intent::Positioned<wire::ClaimHead>)> {
    let claim = heads
        .claim
        .as_ref()
        .filter(|claim| claim.body.disposition == ClaimDisposition::Active)?;
    let Actor::Lane(lane) = claim.body.actor else {
        return None;
    };
    Some((lane, claim))
}
