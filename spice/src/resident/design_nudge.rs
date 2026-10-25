//! The operator's push of one open Design to proposal, delivered to the one
//! lane best placed to take it.

use std::collections::BTreeSet;

use board::similarity::MatchCache;
use spice::rooms::nudge::{FreeLanes, Recipient, choose};
use spice::rooms::profile::SessionProfiles;
use wire::intent::CommandAnswer;
use wire::{Actor, CanonicalUuid, ClaimDisposition, TaskStamp};

use super::room_catch_up::Planning;
use super::*;

/// Record one nudge and message the lane [`recipient`] chooses; returns the
/// lane to notify.
pub(super) fn nudge(
    bus: &Bus,
    config: &SurfaceConfig,
    profiles: Option<&std::sync::Mutex<message_surface::MessageState>>,
    command: &wire::AdmittedCommand,
    request: &wire::rooms::DesignNudge,
) -> (Outcome<CommandAnswer>, Option<CanonicalUuid>) {
    crate::board_command::design_nudge(bus, command, request, &config.steering, |design, holder| {
        recipient(bus, profiles, design, holder).map_err(design_awareness::command_answer_outcome)
    })
}

/// The lane one nudge of `design` messages. A lane is free to take it while it
/// holds no active claim but the Design's; an active `holder` is the only lane
/// able to take it, and no lane can take a passive Design. A lane the
/// residency seats scores its occupant's continuing session; a vacant lane
/// the lifecycle may wake scores its retained sessions. `profiles` is absent
/// until the resident's message startup completes, when no lane has a
/// retained profile to score.
fn recipient(
    bus: &Bus,
    profiles: Option<&std::sync::Mutex<message_surface::MessageState>>,
    design: &TaskStamp,
    holder: Option<CanonicalUuid>,
) -> CommandResult<Option<Recipient>> {
    let planning = Planning::load(bus)?;
    let Some(entry) = planning.entries.iter().find(|entry| entry.born() == design) else {
        return Ok(None);
    };
    if board::passive_design(entry) {
        return Ok(None);
    }
    let busy = planning
        .entries
        .iter()
        .filter(|entry| entry.born() != design)
        .filter_map(|entry| match entry.claim() {
            Some(claim) if claim.disposition == ClaimDisposition::Active => match claim.actor {
                Actor::Lane(lane) => Some(lane),
                _ => None,
            },
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let mut free = FreeLanes::default();
    for row in &planning.residency.lanes {
        if holder.map_or(busy.contains(&row.lane), |holder| holder != row.lane) {
            continue;
        }
        match row.occupant {
            Some(occupant) => {
                free.seated.insert(row.lane, occupant);
            }
            None if row.disposition != "occupied" => {
                free.vacant.insert(row.lane);
            }
            None => {}
        }
    }
    let matcher = MatchCache::shared();
    let frequency = matcher.fit(planning.entries.iter().map(board::Entry::statement));
    let vector = matcher.prepare(&frequency, entry.statement());
    let chosen = match profiles {
        Some(profiles) => {
            design_awareness::bind_current_sessions(
                planning.snapshot.facts(),
                &free.seated,
                profiles,
            )?;
            let mut state = message_surface::lock_state(profiles)?;
            choose(
                &mut state.profiles,
                &frequency,
                vector.as_deref(),
                &free,
                design,
            )
        }
        None => choose(
            &mut SessionProfiles::default(),
            &frequency,
            vector.as_deref(),
            &free,
            design,
        ),
    };
    chosen.map_err(|error| {
        store_fault(
            "preparation::nudge-profile-generation-mismatch",
            &error.to_string(),
        )
    })
}
