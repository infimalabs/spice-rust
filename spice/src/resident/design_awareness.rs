//! Organic background invitations of relevant lanes to passive Designs.
//!
//! Every matching claimant receives one notice, recorded before delivery.
//! Open standing, exact statement identity, claim, occupant, engagement, and
//! once-per-occupant checks suppress stale or duplicate notices. Records are
//! background information only, adding no attention, participation, steering,
//! claim, wake, or acknowledgment.

use std::collections::{BTreeMap, BTreeSet};

use journal::{GuardError, JournalRead};
use spice::rooms::awareness::{
    COMPLETION_VALIDATION_BYTES, CommonTerms, DesignTerms, completion_terms, pitch, task_terms,
};
use wire::body::TaskDone as CompletedTask;
use wire::rooms::DesignNoticed;
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, LaneResidency, Producer, Seq, SequenceCode, TaskStamp,
    intent::TaskDone as TaskDoneIntent,
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
/// Completions admitted to one resident reconciliation pass.
const COMPLETION_PAGE_ROWS: usize = 64;
/// Aggregate completion envelope bytes decoded in one reconciliation pass.
const COMPLETION_PAGE_BYTES: usize = 256 * 1024;

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
    trigger: Trigger,
    pub(in crate::resident) design: TaskStamp,
    pub(in crate::resident) statement: Seq,
    pub(in crate::resident) terms: Vec<String>,
    pub(in crate::resident) notice: DesignNotice,
    horizon: Option<Seq>,
}

/// Authority that made one invitation candidate current.
#[derive(Clone, Copy)]
enum Trigger {
    Claim,
    Completion(Seq),
}

/// One newly observed task completion and its bounded matching input.
#[derive(Clone)]
pub(in crate::resident) struct Completion {
    at: Seq,
    task: TaskStamp,
    operation: CanonicalUuid,
    validation: String,
}

/// Process-local completion horizon. Starting at the resident's current
/// journal cursor deliberately prevents historical completion backfill.
pub(in crate::resident) struct CompletionCursor {
    after: Option<Seq>,
}

impl CompletionCursor {
    pub(in crate::resident) fn start(bus: &Bus) -> CommandResult<Self> {
        let after = bus.cursor().map_err(|error| {
            store_fault("room::awareness-cursor-unreadable", &error.to_string())
        })?;
        Ok(Self { after })
    }

    /// Consume at most one bounded page, advancing only after a successful
    /// read. A continuation advances to the last admitted row rather than the
    /// page horizon, so no completion can be skipped.
    pub(in crate::resident) fn take(&mut self, bus: &Bus) -> CommandResult<Vec<Completion>> {
        let after = self.after;
        let page = bus
            .read(move |read| {
                read.selected_fact_page(
                    &[wire::FactKind::TaskDone],
                    after,
                    None,
                    journal::FactReadBudget::new(COMPLETION_PAGE_ROWS, COMPLETION_PAGE_BYTES)?,
                )
            })
            .map_err(|error| {
                store_fault("room::awareness-completions-unreadable", &error.to_string())
            })?;
        self.after = page.continuation.or(page.horizon);
        Ok(page
            .rows
            .into_iter()
            .filter_map(|envelope| {
                let done = envelope.fact.read::<CompletedTask>()?;
                let operation = done.terminal.as_ref()?.operation;
                let mut validation_end = done.validation.len().min(COMPLETION_VALIDATION_BYTES);
                while !done.validation.is_char_boundary(validation_end) {
                    validation_end -= 1;
                }
                Some(Completion {
                    at: envelope.seq,
                    task: done.task.clone(),
                    operation,
                    validation: done.validation[..validation_end].to_owned(),
                })
            })
            .collect())
    }
}

/// Invite every occupied claimant, and every newly completing occupant, to
/// each matching passive Design its occupant was never told about. An
/// invitation a later commit made stale is dropped. Claim observations retry
/// on later commits; each completion is consumed by its first bounded pass.
/// Nothing is told without its record.
pub(in crate::resident) fn reconcile(
    bus: &Bus,
    paths: &RuntimePaths,
    config: &SurfaceConfig,
    seats: &Seats,
    completions: &mut CompletionCursor,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let completed = completions.take(bus)?;
    reconcile_observed(bus, paths, config, seats, &completed, diagnostic)
}

/// Claim-only seam retained for focused in-process resident contracts.
#[cfg(test)]
pub(in crate::resident) fn reconcile_claims(
    bus: &Bus,
    paths: &RuntimePaths,
    config: &SurfaceConfig,
    seats: &Seats,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    reconcile_observed(bus, paths, config, seats, &[], diagnostic)
}

fn reconcile_observed(
    bus: &Bus,
    paths: &RuntimePaths,
    config: &SurfaceConfig,
    seats: &Seats,
    completions: &[Completion],
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let occupied = occupied(seats);
    if occupied.is_empty() {
        return Ok(());
    }
    let planning = Planning::load(bus, config)?;
    for told in plan(bus, &planning, &occupied, completions)? {
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
/// Design order. Completion profiles are built once before the Design cross
/// product.
pub(in crate::resident) fn plan(
    bus: &Bus,
    planning: &Planning,
    occupied: &BTreeMap<CanonicalUuid, CanonicalUuid>,
    completions: &[Completion],
) -> CommandResult<Vec<Told>> {
    let entries = &planning.entries;
    let mut candidates = entries
        .iter()
        .filter_map(|entry| {
            let claim = entry.claim()?;
            let occupant = *occupied.get(&claim.lane)?;
            (claim.disposition == ClaimDisposition::Active).then(|| Candidate {
                entry,
                lane: claim.lane,
                occupant,
                claim: claim.claimed,
                trigger: Trigger::Claim,
                terms: statement_terms(entry),
            })
        })
        .collect::<Vec<_>>();
    for completed in completions {
        let Some(entry) = entries.iter().find(|entry| entry.born() == &completed.task) else {
            continue;
        };
        let Some(claim) = entry.claim() else {
            continue;
        };
        if claim.disposition != ClaimDisposition::Freed
            || claim.fact != completed.at
            || completed.operation != TaskDoneIntent::operation_for_claim(claim.claimed)
            || occupied.get(&claim.lane) != Some(&claim.occupant)
        {
            continue;
        }
        let statement = entry.statement();
        candidates.push(Candidate {
            entry,
            lane: claim.lane,
            occupant: claim.occupant,
            claim: claim.claimed,
            trigger: Trigger::Completion(completed.at),
            terms: completion_terms(
                &statement.title,
                statement.description.as_deref(),
                &statement.acceptance,
                &completed.validation,
            ),
        });
    }
    if candidates.is_empty() {
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
    let common = CommonTerms::of(
        entries
            .iter()
            .filter(|entry| open(entry))
            .map(statement_terms),
    );
    let claimants = candidates
        .iter()
        .map(|candidate| (candidate.lane, candidate.occupant))
        .collect();
    let (history, horizon) = noticed(bus, planning.residency.lanes.clone(), claimants)?;
    let mut owed = Vec::new();
    for (design, design_terms) in designs {
        for candidate in &candidates {
            if design.born() == candidate.entry.born()
                || history
                    .told
                    .get(&candidate.occupant)
                    .is_some_and(|designs| designs.contains(design.born()))
                || held_or_created(design, candidate.lane)
            {
                continue;
            }
            let Some(matched) = design_terms.matched(&candidate.terms, &common) else {
                continue;
            };
            // Engagement projects the Design's attention: read it only for a
            // candidate this pass could tell.
            if engaged(bus, planning, design.born(), candidate.lane)? {
                continue;
            }
            owed.push(invitation(
                candidate.entry,
                candidate.lane,
                candidate.occupant,
                candidate.claim,
                candidate.trigger,
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

struct Candidate<'a> {
    entry: &'a board::Entry,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    claim: Seq,
    trigger: Trigger,
    terms: BTreeSet<String>,
}

fn invitation(
    entry: &board::Entry,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    claim: Seq,
    trigger: Trigger,
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
        trigger,
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

/// Distinctive terms of one task's statement.
fn statement_terms(entry: &board::Entry) -> BTreeSet<String> {
    let statement = entry.statement();
    task_terms(
        &statement.title,
        statement.description.as_deref(),
        &statement.acceptance,
    )
}

/// Whether `entry` is open work.
fn open(entry: &board::Entry) -> bool {
    entry.workflow().status.is_pending()
}

/// Whether `entry` is an open Design inactive under its group stance.
fn passive(entry: &board::Entry) -> bool {
    board::passive_design(entry)
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
    let recipient_current = read.claim_head(&told.task)?.is_some_and(|head| {
        let claim = head.body();
        match told.trigger {
            Trigger::Claim => {
                claim.disposition == ClaimDisposition::Active
                    && claim.lane == told.lane
                    && claim.claimed == told.claim
            }
            Trigger::Completion(done) => {
                claim.disposition == ClaimDisposition::Freed
                    && claim.lane == told.lane
                    && claim.occupant == told.occupant
                    && claim.claimed == told.claim
                    && claim.fact == done
            }
        }
    });
    if !recipient_current {
        return Err(stale("the claim or completion moved"));
    }
    if !read
        .task_head(&told.task)?
        .is_some_and(|head| head.body().revision == told.task_statement)
    {
        return Err(stale("the task statement moved"));
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
    let active = read
        .group_partition()?
        .is_some_and(|head| head.body().design_is_active(&told.design));
    let held = read.claim_head(&told.design)?.is_some_and(|head| {
        head.body().disposition == ClaimDisposition::Active && head.body().lane == told.lane
    });
    let created = matches!(
        &statement.body().creation,
        wire::body::TaskCreationContext::Transcript { boundary, .. }
            if boundary.lane == told.lane
    );
    if active || held || created {
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
