//! Organic background invitations of relevant lanes to passive Designs.
//!
//! Every matching claimant receives one notice, recorded before delivery.
//! Open standing, exact statement identity, claim, occupant, engagement, and
//! once-per-occupant checks suppress stale or duplicate notices. Records are
//! background information only, adding no attention, participation, steering,
//! claim, wake, or acknowledgment.

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
                "<system-notice>\nPassive Design {design} \"{}\" at statement revision {} \
                 matches your task {} on: {}.\nPitch: {}\nRead its room with \
                 `spice room history {design}` (`spice://task/{design}/room`). Optional \
                 commands: `spice task modify {design} --title TITLE`; `spice task modify \
                 {design} --description DESCRIPTION`; `spice room contribute {design} \
                 reason-for|reason-against|evidence|question TEXT`; or `spice room leave \
                 {design} --clause TEXT` to decline further invitations. This private \
                 background invitation needs no acknowledgment key or visible ACK/NAK, \
                 starts no work, grants no claim, creates no assignment, requires no response, \
                 and does not make this lane a room listener.\n</system-notice>",
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
    pub(in crate::resident) occupant: CanonicalUuid,
    pub(in crate::resident) task: TaskStamp,
    pub(in crate::resident) task_statement: Seq,
    pub(in crate::resident) claim: Seq,
    pub(in crate::resident) design: TaskStamp,
    pub(in crate::resident) statement: Seq,
    pub(in crate::resident) terms: Vec<String>,
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
    let (history, horizon) = noticed(bus, planning.residency.lanes.clone(), claimants)?;
    let mut owed = Vec::new();
    for (design, design_terms) in designs {
        for (entry, claim, occupant) in &claims {
            if design.born() == entry.born()
                || history
                    .told
                    .get(occupant)
                    .is_some_and(|designs| designs.contains(design.born()))
                || held_or_created(design, claim.lane)
            {
                continue;
            }
            let Some(matched) = design_terms.matched(&claimed_terms(entry)) else {
                continue;
            };
            // Engagement projects the Design's attention: read it only for a
            // claimant this pass could tell.
            if engaged(bus, planning, design.born(), claim.lane)? {
                continue;
            }
            owed.push(invitation(
                entry,
                claim.lane,
                *occupant,
                claim.claimed,
                design,
                matched,
                horizon,
            ));
        }
    }
    owed.sort_unstable_by(|left, right| {
        left.task
            .cmp(&right.task)
            .then_with(|| left.design.cmp(&right.design))
    });
    Ok(owed)
}

fn invitation(
    entry: &board::Entry,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    claim: Seq,
    design: &board::Entry,
    terms: Vec<String>,
    horizon: Option<Seq>,
) -> Told {
    let statement = entry.statement();
    let design_statement = design.statement();
    Told {
        lane,
        occupant,
        task: entry.born().clone(),
        task_statement: statement.revision,
        claim,
        design: design.born().clone(),
        statement: design_statement.revision,
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
            terms: wire::rooms::shown_notice_terms(&terms)
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
        .fit(),
        terms,
        horizon,
    }
}

/// Distinctive terms of one claimed task's statement.
fn claimed_terms(entry: &board::Entry) -> BTreeSet<String> {
    let statement = entry.statement();
    task_terms(
        &statement.title,
        statement.description.as_deref(),
        &statement.acceptance,
    )
}

/// Whether `entry` is an open Design outside every working group.
fn passive(entry: &board::Entry) -> bool {
    wire::intent::is_open_design(entry.statement().intent, entry.workflow()) && !entry.grouped()
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

/// Durable invitation state needed for once-per-occupant suppression.
#[derive(Default)]
struct NoticeHistory {
    told: BTreeMap<CanonicalUuid, BTreeSet<TaskStamp>>,
}

/// The Designs already told to each claimant occupant and the exact horizon
/// these reads cover.
fn noticed(
    bus: &Bus,
    lanes: Vec<LaneResidency>,
    claimants: BTreeMap<CanonicalUuid, CanonicalUuid>,
) -> CommandResult<(NoticeHistory, Option<Seq>)> {
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
        let mut after = births.and_then(|starts| {
            starts
                .into_iter()
                .min()
                .and_then(|position| position.get().checked_sub(1))
                .and_then(Seq::new)
        });
        let mut history = NoticeHistory::default();
        loop {
            let page = noticed_page(read, after)?;
            for body in page
                .rows
                .iter()
                .filter_map(|row| row.fact.read::<DesignNoticed>())
            {
                history
                    .told
                    .entry(body.occupant)
                    .or_default()
                    .insert(body.design.clone());
            }
            let Some(continuation) = page.continuation else {
                return Ok((history, page.horizon));
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

/// Refuse an invitation whose claim, occupant, statement, standing,
/// engagement, or once-per-occupant observation went stale.
fn still_owed(read: &JournalRead<'_>, told: &Told) -> Result<(), GuardError> {
    current_recipient(read, told)?;
    passive_design(read, told)?;
    unseen_invitation(read, told)
}

fn current_recipient(read: &JournalRead<'_>, told: &Told) -> Result<(), GuardError> {
    let stale = |why: &str| GuardError::refused(format!("{STALE}: {why}"));
    let occupied = read.residency_head()?.is_some_and(|head| {
        head.body().lanes.iter().any(|row| {
            row.lane == told.lane
                && row.occupant == Some(told.occupant)
                && row.disposition == "occupied"
        })
    });
    if !occupied {
        return Err(stale("the occupant moved"));
    }
    let claimed = read.claim_head(&told.task)?.is_some_and(|head| {
        let claim = head.body();
        claim.disposition == ClaimDisposition::Active
            && claim.lane == told.lane
            && claim.claimed == told.claim
    });
    if !claimed {
        return Err(stale("the claim moved"));
    }
    if !read
        .task_head(&told.task)?
        .is_some_and(|head| head.body().revision == told.task_statement)
    {
        return Err(stale("the claimed task statement moved"));
    }
    Ok(())
}

fn passive_design(read: &JournalRead<'_>, told: &Told) -> Result<(), GuardError> {
    let stale = |why: &str| GuardError::refused(format!("{STALE}: {why}"));
    let Some(statement) = read.task_head(&told.design)? else {
        return Err(stale("the Design disappeared"));
    };
    if statement.body().revision != told.statement {
        return Err(stale("the Design statement moved"));
    }
    let Some(workflow) = read.workflow_head(&told.design)? else {
        return Err(stale("the Design workflow disappeared"));
    };
    if !statement.body().filing.is_filed()
        || !wire::intent::is_open_design(statement.body().intent, workflow.body())
    {
        return Err(stale("the Design is no longer open"));
    }
    let grouped = read.group_partition()?.is_some_and(|head| {
        head.body()
            .groups
            .iter()
            .any(|group| group.anchors.contains(&told.design))
    });
    let held = read.claim_head(&told.design)?.is_some_and(|head| {
        head.body().disposition == ClaimDisposition::Active && head.body().lane == told.lane
    });
    let created = matches!(
        &statement.body().creation,
        wire::body::TaskCreationContext::Transcript { boundary, .. }
            if boundary.lane == told.lane
    );
    if grouped || held || created {
        return Err(stale("the Design is no longer passive for this lane"));
    }
    if read
        .latest_room_publication(&told.design, &Actor::Lane(told.lane))?
        .is_some()
    {
        return Err(stale("the lane published in the room"));
    }
    Ok(())
}

fn unseen_invitation(read: &JournalRead<'_>, told: &Told) -> Result<(), GuardError> {
    let stale = |why: &str| GuardError::refused(format!("{STALE}: {why}"));
    let Some(current) = read.cursor()? else {
        return Err(stale("the journal horizon disappeared"));
    };
    let mut start = journal::FactPageStart::After(told.horizon);
    loop {
        let page = read.fact_page(start, current, journal::FactReadBudget::RETAINED)?;
        for envelope in &page.rows {
            let Some(body) = envelope.fact.read::<DesignNoticed>() else {
                return Err(stale("authority changed after the invitation was planned"));
            };
            if body.occupant == told.occupant && body.design == told.design {
                return Err(stale("the occupant was already invited"));
            }
        }
        let Some(overflow) = page.overflow else {
            break;
        };
        start = journal::FactPageStart::At(overflow.continuation);
    }
    Ok(())
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
                statement: told.statement,
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
