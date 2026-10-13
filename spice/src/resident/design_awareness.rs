//! Background awareness of open Designs that match a lane's claimed work.
//!
//! For every occupied lane whose occupant holds an active claim, each open
//! Design whose terms match the claimed task (see
//! [`spice::rooms::awareness`]) is told once per claim: the resident records
//! `room::noticed`, the operator's durable read of who was told what, then
//! hands the exact occupant one private background notice. A Design is never
//! told when it is settled, when this lane holds or created it, or when this
//! lane already listens to its room. The record is no room publication, so it
//! adds no attention, participation, steering, or wake, and the notice needs
//! no acknowledgment.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use spice::rooms::awareness::{DesignTerms, pitch, task_terms};
use wire::rooms::DesignNoticed;
use wire::{Actor, CanonicalUuid, ClaimDisposition, Producer, Seq, SequenceCode, TaskStamp};

use super::room_catch_up::{Planning, unit_outcome};
use super::*;

/// Most bytes of the Design title one notice repeats.
const TITLE_BYTES: usize = 200;
/// Most matched terms one notice names; the record keeps them all.
const NOTICE_TERMS: usize = 6;
/// Pitch and title lengths a notice keeps whole while fitting one datagram.
const SHORTEST_CLIP_BYTES: usize = 16;

/// One private Design awareness notice, scoped to the claim's exact occupant.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::resident) struct DesignNotice {
    pub(in crate::resident) occupant: CanonicalUuid,
    pub(in crate::resident) design: String,
    pub(in crate::resident) task: String,
    pub(in crate::resident) title: String,
    pub(in crate::resident) pitch: String,
    pub(in crate::resident) terms: Vec<String>,
}

impl DesignNotice {
    /// This notice shrunk until it fits one kick datagram of `bytes`: matched
    /// terms first, then the pitch, then the title.
    fn fit(mut self, bytes: usize) -> Self {
        while serde_json::to_vec(&self).map_or(usize::MAX, |encoded| encoded.len()) > bytes {
            if self.terms.len() > 1 {
                self.terms.pop();
            } else if self.pitch.len() > SHORTEST_CLIP_BYTES {
                self.pitch = clip(&self.pitch, self.pitch.len() / 2);
            } else if self.title.len() > SHORTEST_CLIP_BYTES {
                self.title = clip(&self.title, self.title.len() / 2);
            } else {
                break;
            }
        }
        self
    }

    /// Background purpose: one queued notice per Design.
    pub(in crate::resident) fn purpose(&self) -> String {
        format!("design:{}", self.design)
    }

    /// The notice its lane hears.
    pub(in crate::resident) fn text(&self) -> String {
        let design = &self.design;
        format!(
            "<system-notice>\nOpen Design {design} \"{}\" matches your task {} on: {}.\n\
             Pitch: {}\nRead its room with `spice room history {design}` \
             (`spice://task/{design}/room`); contribute with a visible `MSG #{design}: BODY` \
             packet. This is a private background notice: it needs no acknowledgment key or \
             visible ACK/NAK, starts no work, and does not make this lane a room listener.\n\
             </system-notice>",
            self.title,
            self.task,
            self.terms.join(", "),
            self.pitch,
        )
    }
}

/// One Design to tell one lane about.
struct Told {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    task: TaskStamp,
    claim: Seq,
    design: TaskStamp,
    terms: Vec<String>,
    notice: DesignNotice,
}

/// Tell every occupied claimant about each matching open Design not yet told
/// during its claim. A failed record is reported and retried on the next
/// commit; it never fails the caller, and nothing is told without its record.
pub(in crate::resident) fn reconcile(
    bus: &Bus,
    paths: &RuntimePaths,
    config: &SurfaceConfig,
    seats: &Seats,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let occupied = seats
        .lanes()
        .into_iter()
        .filter_map(|lane| {
            seats
                .occupant(lane)
                .map(|occupant| (lane, occupant.occupant()))
        })
        .collect::<BTreeMap<_, _>>();
    if occupied.is_empty() {
        return Ok(());
    }
    let planning = Planning::load(bus, config)?;
    for told in plan(bus, &planning, &occupied)? {
        match record(bus, &told) {
            Ok(()) => super::supervisor::command::design_notice(paths, told.lane, told.notice),
            Err(error) => {
                let _ = writeln!(
                    diagnostic,
                    "lane {} awareness of Design {} was not recorded: {error:?}",
                    told.lane, told.design
                );
            }
        }
    }
    Ok(())
}

/// Every Design each occupied claimant is owed, in lane then Design order.
fn plan(
    bus: &Bus,
    planning: &Planning,
    occupied: &BTreeMap<CanonicalUuid, CanonicalUuid>,
) -> CommandResult<Vec<Told>> {
    let entries = &planning.entries;
    let claims = entries
        .iter()
        .filter_map(|entry| {
            let claim = entry.claim()?;
            let occupant = *occupied.get(&claim.lane)?;
            (claim.disposition == ClaimDisposition::Active).then_some((entry, claim, occupant))
        })
        .collect::<Vec<_>>();
    if claims.is_empty() {
        return Ok(Vec::new());
    }
    let designs = entries
        .iter()
        .filter(|entry| {
            entry.statement().intent == Some(wire::TaskIntent::Design)
                && entry.workflow().status == "pending"
                && entry.workflow().completion.is_none()
        })
        .map(|entry| {
            let statement = entry.statement();
            (
                entry,
                DesignTerms::new(&statement.title, statement.description.as_deref()),
            )
        })
        .collect::<Vec<_>>();
    if designs.is_empty() {
        return Ok(Vec::new());
    }
    let since = claims
        .iter()
        .map(|(_, claim, _)| claim.claimed)
        .min()
        .expect("claims are nonempty");
    let told = noticed(bus, since)?;
    let mut owed = Vec::new();
    for (entry, claim, occupant) in claims {
        let statement = entry.statement();
        let terms = task_terms(
            &statement.title,
            statement.description.as_deref(),
            &statement.acceptance,
        );
        for (design, design_terms) in &designs {
            if design.born() == entry.born()
                || told
                    .get(&(claim.lane, claim.claimed))
                    .is_some_and(|designs| designs.contains(design.born()))
                || held_or_created(design, claim.lane)
            {
                continue;
            }
            let Some(matched) = design_terms.matched(&terms) else {
                continue;
            };
            if listens(bus, planning, design.born(), claim.lane)? {
                continue;
            }
            let design_statement = design.statement();
            owed.push(Told {
                lane: claim.lane,
                occupant,
                task: entry.born().clone(),
                claim: claim.claimed,
                design: design.born().clone(),
                notice: DesignNotice {
                    occupant,
                    design: design.handle().to_string(),
                    task: entry.handle().to_string(),
                    title: clip(&design_statement.title, TITLE_BYTES),
                    pitch: pitch(
                        &design_statement.title,
                        design_statement.description.as_deref(),
                    ),
                    terms: matched.iter().take(NOTICE_TERMS).cloned().collect(),
                }
                .fit(super::supervisor::command::KICK_BYTES),
                terms: matched,
            });
        }
    }
    Ok(owed)
}

/// Whether `lane` holds an active claim on `design` or created it.
fn held_or_created(design: &board::Entry, lane: CanonicalUuid) -> bool {
    let held = design
        .claim()
        .is_some_and(|claim| claim.disposition == ClaimDisposition::Active && claim.lane == lane);
    let created = matches!(
        &design.statement().creation,
        wire::body::TaskCreationContext::Transcript { boundary, .. } if boundary.lane == lane
    );
    held || created
}

/// Whether `lane` already listens to `design`'s room for any reason.
fn listens(
    bus: &Bus,
    planning: &Planning,
    design: &TaskStamp,
    lane: CanonicalUuid,
) -> CommandResult<bool> {
    let room = design.clone();
    let actor = Actor::Lane(lane);
    let activity = bus
        .read(move |read| read.latest_room_publication(&room, &actor))
        .map_err(|error| store_fault("room::awareness-activity-unreadable", &error.to_string()))?;
    let attention = crate::board_command::project_attention(
        bus,
        &planning.entries,
        &planning.residency,
        design,
        activity.as_slice(),
        planning.at,
    )
    .map_err(unit_outcome)?;
    Ok(attention
        .lanes
        .iter()
        .any(|row| row.lane == lane && row.listens()))
}

/// The Designs told to each lane during each claim, keyed by the claim's win.
type Noticed = BTreeMap<(CanonicalUuid, Seq), BTreeSet<TaskStamp>>;

/// The Designs already told to each `(lane, claim)` won at or after `since`.
fn noticed(bus: &Bus, since: Seq) -> CommandResult<Noticed> {
    bus.read(move |read| {
        let mut told = Noticed::new();
        let mut after = since.get().checked_sub(1).and_then(Seq::new);
        loop {
            let page = read.selected_fact_page(
                &[wire::FactKind::DesignNoticed],
                after,
                None,
                journal::FactReadBudget::RETAINED,
            )?;
            for envelope in page.rows {
                if let Some(body) = envelope.fact.read::<DesignNoticed>()
                    && let Some(claim) = Seq::new(body.claim.get())
                {
                    told.entry((body.lane, claim))
                        .or_default()
                        .insert(body.design);
                }
            }
            let Some(continuation) = page.continuation else {
                return Ok(told);
            };
            after = Some(continuation);
        }
    })
    .map_err(|error| store_fault("room::awareness-unreadable", &error.to_string()))
}

/// Commit the durable record of one notice before it is told.
fn record(bus: &Bus, told: &Told) -> CommandResult<()> {
    let at = Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("room::awareness-clock-invalid", &error.to_string()))?;
    let origin = format!("lane:{}", told.lane)
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("room::awareness-origin-invalid", &error.to_string()))?;
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::ResidentRecognition);
    write
        .fact(
            DesignNoticed {
                design: told.design.clone(),
                lane: told.lane,
                occupant: told.occupant,
                task: told.task.clone(),
                claim: SequenceCode::new(told.claim.get()),
                terms: told.terms.clone(),
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("room::awareness-build", &error.to_string()))?;
    match bus
        .commit(write)
        .map_err(|error| store_fault("room::awareness-commit", &error.to_string()))?
    {
        CommitOutcome::Committed(_) => Ok(()),
        CommitOutcome::Refused(refusal) => {
            Err(store_fault("room::awareness-refused", &refusal.to_string()))
        }
        CommitOutcome::Fault(fault) => {
            Err(store_fault("room::awareness-fault", &fault.to_string()))
        }
    }
}

/// `text` cut at a character boundary to at most `bytes` bytes.
fn clip(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_owned();
    }
    let mut end = bytes - '…'.len_utf8();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}
