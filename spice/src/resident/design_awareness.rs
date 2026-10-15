//! Background invitations of matching lanes to passive Designs.
//!
//! For every occupied lane whose occupant holds an active claim, each open
//! Design outside every working group whose terms match the claimed task (see
//! [`spice::rooms::awareness`]) is told to that occupant once: the resident
//! records `room::noticed`, the operator's durable read of who was invited,
//! then hands the exact occupant one private background notice. An active or
//! concluded Design is never invited, nor one this lane holds or created, one
//! whose room it already listens to, or one whose room it ever published in;
//! a contribution, packet, or leave therefore ends that lane's invitations.
//! An occupant that ignored an invitation is not told again; a later occupant
//! of the lane may be, at a matching claim it holds. The record is no room
//! publication, so it adds no attention, participation, steering, or wake,
//! and the notice needs no acknowledgment.

use std::collections::{BTreeMap, BTreeSet};

use journal::{GuardError, JournalRead};
use spice::rooms::awareness::{DesignTerms, pitch, task_terms};
use wire::rooms::DesignNoticed;
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, LaneResidency, Producer, Seq, SequenceCode, TaskStamp,
};

use super::design_input::{BackgroundNotice, clip};
use super::room_catch_up::{Planning, unit_outcome};
use super::*;

/// Refusal reason of an invitation a commit after its observation made stale.
const STALE: &str = "room::awareness-stale";

/// Most bytes of the Design title one notice repeats.
const TITLE_BYTES: usize = 200;
/// Most matched terms one notice names; the record keeps them all.
const NOTICE_TERMS: usize = 6;
/// Pitch and title lengths a notice keeps whole while fitting one datagram.
const SHORTEST_CLIP_BYTES: usize = 16;

/// One private Design awareness notice, scoped to the claim's exact occupant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::resident) struct DesignNotice {
    occupant: CanonicalUuid,
    design: String,
    task: String,
    title: String,
    pitch: String,
    pub(in crate::resident) revision: SequenceCode,
    terms: Vec<String>,
}

impl DesignNotice {
    /// This notice shrunk until it fits one kick datagram: matched terms
    /// first, then the pitch, then the title.
    fn fit(mut self) -> Self {
        while !self.background().fits() {
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

    /// The invitation its lane hears, queued once per Design.
    pub(in crate::resident) fn background(&self) -> BackgroundNotice {
        let design = &self.design;
        BackgroundNotice {
            occupant: self.occupant,
            purpose: format!("design:{design}"),
            text: format!(
                "<system-notice>\nYou joined the early statement sample for passive Design \
                 {design} \"{}\" at statement revision {}; it matches your task {} on: {}.\n\
                 Pitch: {}\nRead its room with `spice room history {design}` \
                 (`spice://task/{design}/room`). Optional commands: \
                 `spice task modify {design} --title TITLE`; \
                 `spice task modify {design} --description DESCRIPTION`; \
                 `spice room contribute {design} \
                 reason-for|reason-against|evidence|question TEXT`; or `spice room leave {design} \
                 --clause TEXT` to decline further invitations. This private background \
                 invitation needs no acknowledgment key or visible ACK/NAK, starts no work, \
                 grants no claim, creates no assignment, requires no response, and does not make \
                 this lane a room listener.\n</system-notice>",
                self.title,
                self.revision,
                self.task,
                self.terms.join(", "),
                self.pitch,
            ),
        }
    }
}

/// One Design to tell one occupant about, and the journal position its
/// invitation read covered.
#[derive(Clone)]
pub(in crate::resident) struct Told {
    pub(in crate::resident) lane: CanonicalUuid,
    occupant: CanonicalUuid,
    task: TaskStamp,
    claim: Seq,
    pub(in crate::resident) design: TaskStamp,
    terms: Vec<String>,
    pub(in crate::resident) notice: DesignNotice,
    horizon: Option<Seq>,
}

/// Invite every occupied claimant to each matching passive Design its
/// occupant was never told about. An invitation a later commit made stale is
/// dropped; any other failed record is reported and retried on the next
/// commit. It never fails the caller, and nothing is told without its record.
pub(in crate::resident) fn reconcile(
    bus: &Bus,
    paths: &RuntimePaths,
    config: &SurfaceConfig,
    seats: &Seats,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let occupied = occupied(seats);
    if occupied.is_empty() {
        return Ok(());
    }
    let planning = Planning::load(bus, config)?;
    for told in plan(bus, &planning, &occupied)? {
        match record(bus, &told) {
            Ok(true) => super::supervisor::command::background_notice(
                paths,
                told.lane,
                told.notice.background(),
            ),
            Ok(false) => {}
            Err(error) => {
                let _ = writeln!(
                    diagnostic,
                    "lane {} invitation to Design {} was not recorded: {error:?}",
                    told.lane, told.design
                );
            }
        }
    }
    Ok(())
}

/// Each seated lane and its current occupant.
pub(super) fn occupied(seats: &Seats) -> BTreeMap<CanonicalUuid, CanonicalUuid> {
    seats
        .lanes()
        .into_iter()
        .filter_map(|lane| {
            seats
                .occupant(lane)
                .map(|occupant| (lane, occupant.occupant()))
        })
        .collect()
}

/// Every invitation each occupied claimant's occupant is owed, in task then
/// Design order.
pub(in crate::resident) fn plan(
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
        .filter(|entry| passive(entry))
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
    let claimants = claims
        .iter()
        .map(|(_, claim, occupant)| (claim.lane, *occupant))
        .collect();
    let (told, horizon) = noticed(bus, planning.residency.lanes.clone(), claimants)?;
    let mut owed = Vec::new();
    for (entry, claim, occupant) in claims {
        let statement = entry.statement();
        let terms = task_terms(
            &statement.title,
            statement.description.as_deref(),
            &statement.acceptance,
        );
        let told = told.get(&(claim.lane, occupant));
        for (design, design_terms) in &designs {
            if design.born() == entry.born()
                || told.is_some_and(|designs| designs.contains(design.born()))
                || held_or_created(design, claim.lane)
            {
                continue;
            }
            let Some(matched) = design_terms.matched(&terms) else {
                continue;
            };
            if engaged(bus, planning, design.born(), claim.lane)? {
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
                    revision: SequenceCode::new(design_statement.revision.get()),
                    terms: matched.iter().take(NOTICE_TERMS).cloned().collect(),
                }
                .fit(),
                terms: matched,
                horizon,
            });
        }
    }
    Ok(owed)
}

/// Whether `entry` is an open Design outside every working group.
fn passive(entry: &board::Entry) -> bool {
    entry.statement().intent == Some(wire::TaskIntent::Design)
        && entry.workflow().status == "pending"
        && entry.workflow().completion.is_none()
        && !entry.grouped()
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

/// Whether `lane` ever published in `design`'s room (a contribution, packet,
/// or leave), whichever working groups the Design has stood in since, or
/// listens to it now for any other reason.
fn engaged(
    bus: &Bus,
    planning: &Planning,
    design: &TaskStamp,
    lane: CanonicalUuid,
) -> CommandResult<bool> {
    let room = design.clone();
    let actor = Actor::Lane(lane);
    let published = bus
        .read(move |read| read.latest_room_publication(&room, &actor))
        .map_err(|error| store_fault("room::awareness-activity-unreadable", &error.to_string()))?;
    if published.is_some() {
        return Ok(true);
    }
    let attention = crate::board_command::project_attention(
        bus,
        &planning.entries,
        &planning.residency,
        design,
        &[],
        planning.at,
    )
    .map_err(unit_outcome)?;
    Ok(attention
        .lanes
        .iter()
        .any(|row| row.lane == lane && row.listens()))
}

/// The Designs told to each occupant, keyed by lane and occupant.
type Noticed = BTreeMap<(CanonicalUuid, CanonicalUuid), BTreeSet<TaskStamp>>;

/// The Designs already told to each claimant occupant, read from the earliest
/// such occupant's birth on its lane, and the horizon the read covers. A
/// claimant whose birth is unproved widens the read to every retained record,
/// so no invitation repeats.
fn noticed(
    bus: &Bus,
    lanes: Vec<LaneResidency>,
    claimants: BTreeMap<CanonicalUuid, CanonicalUuid>,
) -> CommandResult<(Noticed, Option<Seq>)> {
    bus.read(move |read| {
        let addresses = bus::journal_seat_addresses(read, &lanes)?;
        let births = claimants
            .iter()
            .map(|(lane, occupant)| match addresses.get(lane) {
                Some(wire::SeatAddress::Seated {
                    occupant: seated,
                    address,
                }) if seated == occupant => Some(address.birth()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>();
        let mut after = births
            .and_then(|births| births.into_iter().min())
            .and_then(|birth| birth.get().checked_sub(1))
            .and_then(Seq::new);
        let mut told = Noticed::new();
        loop {
            let page = noticed_page(read, after)?;
            for body in page
                .rows
                .iter()
                .filter_map(|row| row.fact.read::<DesignNoticed>())
            {
                told.entry((body.lane, body.occupant))
                    .or_default()
                    .insert(body.design);
            }
            let Some(continuation) = page.continuation else {
                return Ok((told, page.horizon));
            };
            after = Some(continuation);
        }
    })
    .map_err(|error| store_fault("room::awareness-unreadable", &error.to_string()))
}

/// One retained page of `room::noticed` records after `after`.
fn noticed_page(
    read: &JournalRead<'_>,
    after: Option<Seq>,
) -> Result<journal::SelectedFactPage, GuardError> {
    read.selected_fact_page(
        &[wire::FactKind::DesignNoticed],
        after,
        None,
        journal::FactReadBudget::RETAINED,
    )
}

/// Refuse an invitation a commit after its observation made stale: the claim
/// moved, the Design is no longer passive, the lane published in its room, or
/// the occupant was already invited to it.
fn still_owed(read: &JournalRead<'_>, told: &Told) -> Result<(), GuardError> {
    let stale = |why: &str| GuardError::refused(format!("{STALE}: {why}"));
    let claimed = read.claim_head(&told.task)?.is_some_and(|head| {
        let claim = head.body();
        claim.disposition == ClaimDisposition::Active
            && claim.lane == told.lane
            && claim.claimed == told.claim
    });
    if !claimed {
        return Err(stale("the claim moved"));
    }
    let open = read
        .workflow_head(&told.design)?
        .is_some_and(|head| head.body().status == "pending" && head.body().completion.is_none());
    let grouped = read.group_partition()?.is_some_and(|head| {
        head.body()
            .groups
            .iter()
            .any(|group| group.anchors.contains(&told.design))
    });
    if !open || grouped {
        return Err(stale("the Design is no longer passive"));
    }
    if read
        .latest_room_publication(&told.design, &Actor::Lane(told.lane))?
        .is_some()
    {
        return Err(stale("the lane published in the room"));
    }
    let mut after = told.horizon;
    loop {
        let page = noticed_page(read, after)?;
        if page
            .rows
            .iter()
            .filter_map(|row| row.fact.read::<DesignNoticed>())
            .any(|body| {
                body.lane == told.lane
                    && body.occupant == told.occupant
                    && body.design == told.design
            })
        {
            return Err(stale("the occupant was already invited"));
        }
        let Some(continuation) = page.continuation else {
            return Ok(());
        };
        after = Some(continuation);
    }
}

/// Commit the durable record of one invitation before it is told: `false`
/// when a commit after its observation made it stale.
pub(in crate::resident) fn record(bus: &Bus, told: &Told) -> CommandResult<bool> {
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
    let observed = told.clone();
    write.guard(journal::JournalGuard::new(move |read| {
        still_owed(read, &observed)
    }));
    match bus
        .commit(write)
        .map_err(|error| store_fault("room::awareness-commit", &error.to_string()))?
    {
        CommitOutcome::Committed(_) => Ok(true),
        CommitOutcome::Refused(refusal) if refusal.reason().starts_with(STALE) => Ok(false),
        CommitOutcome::Refused(refusal) => {
            Err(store_fault("room::awareness-refused", &refusal.to_string()))
        }
        CommitOutcome::Fault(fault) => {
            Err(store_fault("room::awareness-fault", &fault.to_string()))
        }
    }
}
