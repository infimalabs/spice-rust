//! Completion-bound requests for one contribution to a matching passive Design.
//!
//! A completed claim can owe one request at the lane's natural break. The
//! durable notice records that choice, while `task next` can project the same
//! choice directly when it races the resident recognition pass. Room catch-up
//! carries the owed request to the occupant's next ordinary input.

use std::collections::{BTreeMap, BTreeSet};

use journal::{GuardError, JournalRead};
use spice::rooms::awareness::{
    FrequencyProfile, MatchCandidate, TermCounts, excerpt, pitch, request_questions,
};
use wire::body::TaskDone as CompletedTask;
use wire::rooms::DesignNoticed;
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, Producer, Seq, SequenceCode, TaskStamp,
    intent::{DesignRequest, DesignSupport, TaskDone as TaskDoneIntent},
};

use super::room_catch_up::Planning;
use super::*;

/// Refusal reason of a request a commit after its observation made stale.
const STALE: &str = "room::awareness-stale";
/// Completions admitted to one resident reconciliation pass.
const COMPLETION_PAGE_ROWS: usize = 64;
/// Aggregate completion envelope bytes decoded in one reconciliation pass.
const COMPLETION_PAGE_BYTES: usize = 256 * 1024;

/// One Design request and the journal position its planning read covered.
#[derive(Clone)]
struct PlannedRequest {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    task: TaskStamp,
    task_statement: Seq,
    claim: Seq,
    /// Exact `TaskDone` that owes the request.
    completed: Seq,
    design: TaskStamp,
    statement: Seq,
    terms: Vec<String>,
    horizon: Option<Seq>,
}

/// One newly observed task completion and its matching input.
#[derive(Clone)]
pub(in crate::resident) struct Completion {
    at: Seq,
    task: TaskStamp,
    validation: String,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    claim: Seq,
    statement: wire::TaskHead,
}

/// Process-local completion horizon. `task next` independently projects an
/// unrecorded latest completion, so command ordering cannot lose the request.
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
    /// read. A continuation advances to the last admitted row.
    pub(in crate::resident) fn take(&mut self, bus: &Bus) -> CommandResult<Vec<Completion>> {
        let after = self.after;
        let (continuation, horizon, completed) = bus
            .read(move |read| {
                let page = read.selected_fact_page(
                    &[wire::FactKind::TaskDone],
                    after,
                    None,
                    journal::FactReadBudget::new(COMPLETION_PAGE_ROWS, COMPLETION_PAGE_BYTES)?,
                )?;
                let mut completed = Vec::new();
                for envelope in &page.rows {
                    let Some(done) = envelope.fact.read::<CompletedTask>() else {
                        continue;
                    };
                    let Some(terminal) = done.terminal else {
                        continue;
                    };
                    let Some(claim) = read.claim_head(&done.task)?.map(|head| head.into_body())
                    else {
                        continue;
                    };
                    let Actor::Lane(lane) = claim.actor else {
                        continue;
                    };
                    if claim.disposition != ClaimDisposition::Freed
                        || terminal.operation != TaskDoneIntent::operation_for_claim(claim.claimed)
                    {
                        continue;
                    }
                    let Some(statement) = read.task_head(&done.task)?.map(|head| head.into_body())
                    else {
                        continue;
                    };
                    completed.push(Completion {
                        at: envelope.seq,
                        task: done.task,
                        validation: done.validation,
                        lane,
                        occupant: claim.occupant,
                        claim: claim.claimed,
                        statement,
                    });
                }
                Ok((page.continuation, page.horizon, completed))
            })
            .map_err(|error| {
                store_fault("room::awareness-completions-unreadable", &error.to_string())
            })?;
        self.after = continuation.or(horizon);
        Ok(completed)
    }
}

/// Record one best matching passive Design for every newly completed claim.
/// Room catch-up delivers the owed request with the occupant's next ordinary
/// input; `task next` shows the same request.
pub(in crate::resident) fn reconcile(
    bus: &Bus,
    config: &SurfaceConfig,
    seats: &Seats,
    completions: &mut CompletionCursor,
    profiles: &std::sync::Mutex<message_surface::MessageState>,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let completed = completions.take(bus)?;
    retain_completed_evidence(&completed, profiles)?;
    if completed.is_empty() {
        return Ok(());
    }
    let occupied = occupied(seats);
    if occupied.is_empty() {
        return Ok(());
    }
    let planning = Planning::load(bus, config)?;
    for request in plan(&planning, &occupied, &completed) {
        if let Err(error) = record(bus, &request) {
            let _ = writeln!(
                diagnostic,
                "lane {} request for Design {} was not recorded: {error:?}",
                request.lane, request.design
            );
        }
    }
    Ok(())
}

fn retain_completed_evidence(
    completed: &[Completion],
    profiles: &std::sync::Mutex<message_surface::MessageState>,
) -> CommandResult<()> {
    let mut state = message_surface::lock_state(profiles)?;
    for completed in completed {
        state.profiles.insert(
            completed.lane,
            completed.occupant,
            spice::rooms::evidence::Evidence::task(
                spice::rooms::profile::EvidenceKey::Task {
                    task: completed.task.clone(),
                    statement: completed.statement.revision,
                    completion: completed.at,
                },
                &completed.statement.title,
                completed.statement.description.as_deref(),
                &completed.statement.acceptance,
                &completed.validation,
            ),
        );
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

/// At most one best Design for each completion, in completion order.
fn plan(
    planning: &Planning,
    occupied: &BTreeMap<CanonicalUuid, CanonicalUuid>,
    completions: &[Completion],
) -> Vec<PlannedRequest> {
    if completions.is_empty() {
        return Vec::new();
    }
    let entries = &planning.entries;
    let frequency = FrequencyProfile::recent(entries.iter().map(board::Entry::statement));
    let facts = planning.snapshot.facts();
    let mut requested: BTreeMap<CanonicalUuid, BTreeSet<TaskStamp>> = BTreeMap::new();
    let mut owed = Vec::new();
    for completed in completions {
        let already_requested = facts
            .latest_design_notice(completed.lane, completed.occupant)
            .is_some_and(|(at, notice)| {
                at > completed.at
                    && notice.task == completed.task
                    && notice.claim.get() == completed.claim.get()
            });
        if occupied.get(&completed.lane) != Some(&completed.occupant) || already_requested {
            continue;
        }
        let completion = TermCounts::completion(
            &completed.statement.title,
            completed.statement.description.as_deref(),
            &completed.statement.acceptance,
            &completed.validation,
        );
        let actor = Actor::Lane(completed.lane);
        let candidates = entries
            .iter()
            .filter(|entry| {
                passive(entry)
                    && entry.is_filed()
                    && entry.born() != &completed.task
                    && !facts.design_was_noticed(completed.occupant, entry.born())
                    && !requested
                        .get(&completed.occupant)
                        .is_some_and(|designs| designs.contains(entry.born()))
                    && !facts.published_in_room(&actor, entry.born())
            })
            .map(|design| {
                let statement = design.statement();
                MatchCandidate::new(
                    design.born(),
                    design,
                    TermCounts::statement(
                        &statement.title,
                        statement.description.as_deref(),
                        &statement.acceptance,
                    ),
                )
            });
        let Some((design, matched)) = frequency.select(&completion, candidates) else {
            continue;
        };
        requested
            .entry(completed.occupant)
            .or_default()
            .insert(design.born().clone());
        owed.push(planned_request(
            completed,
            design,
            matched,
            planning.horizon,
        ));
    }
    owed
}

fn planned_request(
    completed: &Completion,
    design: &board::Entry,
    terms: Vec<String>,
    horizon: Option<Seq>,
) -> PlannedRequest {
    PlannedRequest {
        lane: completed.lane,
        occupant: completed.occupant,
        task: completed.task.clone(),
        task_statement: completed.statement.revision,
        claim: completed.claim,
        completed: completed.at,
        design: design.born().clone(),
        statement: design.statement().revision,
        terms,
        horizon,
    }
}

/// Whether `entry` is an open Design inactive under its group stance.
fn passive(entry: &board::Entry) -> bool {
    board::passive_design(entry)
}

struct CurrentCompletion {
    completed: bus::LaneCompletion,
    claim: Seq,
    support: DesignSupport,
    statement: wire::TaskHead,
}

fn current_completion(
    bus: &Bus,
    observed: &bus::TaskSnapshot,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> Result<Option<CurrentCompletion>, Outcome<wire::intent::CommandAnswer>> {
    let facts = observed.facts();
    let actor = Actor::Lane(lane);
    let Some(completed) = facts.latest_lane_completion(lane).cloned() else {
        return Ok(None);
    };
    if completed.occupant != occupant {
        return Ok(None);
    }
    let Some((claim, cell)) = facts.latest_claim(&actor) else {
        return Ok(None);
    };
    if completed.operation != TaskDoneIntent::operation_for_claim(claim) {
        return Ok(None);
    }
    let Some(statement) = current_task_statement(bus, observed, &completed.task)? else {
        return Ok(None);
    };
    Ok(Some(CurrentCompletion {
        completed,
        claim,
        support: DesignSupport::for_phase(&cell.phase),
        statement,
    }))
}

/// One request owed to a lane occupant.
pub(crate) struct OwedRequest {
    /// Position of the notice that chose it.
    pub(crate) at: Seq,
    /// Design whose room accepts the answer.
    pub(crate) design: TaskStamp,
    pub(crate) request: DesignRequest,
}

/// Read the durable request currently owed to this lane occupant.
pub(crate) fn owed_request(
    bus: &Bus,
    observed: &bus::TaskSnapshot,
    entries: &[board::Entry],
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> Result<Option<OwedRequest>, Outcome<wire::intent::CommandAnswer>> {
    let Some(current) = current_completion(bus, observed, lane, occupant)? else {
        return Ok(None);
    };
    let facts = observed.facts();
    let Some((noticed_at, notice)) = facts.latest_design_notice(lane, occupant) else {
        return Ok(None);
    };
    if noticed_at <= current.completed.at
        || notice.task != current.completed.task
        || notice.claim.get() != current.claim.get()
    {
        return Ok(None);
    }
    let Some(design) = entries.iter().find(|entry| entry.born() == &notice.design) else {
        return Ok(None);
    };
    if !request_is_current(facts, design, lane) {
        return Ok(None);
    }
    Ok(Some(OwedRequest {
        at: noticed_at,
        design: notice.design.clone(),
        request: design_request(
            design,
            &current,
            wire::rooms::shown_notice_terms(&notice.terms)
                .into_iter()
                .map(str::to_owned)
                .collect(),
        ),
    }))
}

/// Select and record one deterministic request for the latest completion.
pub(crate) fn project_request(
    bus: &Bus,
    observed: &bus::TaskSnapshot,
    entries: &[board::Entry],
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> Result<Option<DesignRequest>, Outcome<wire::intent::CommandAnswer>> {
    let Some(current) = current_completion(bus, observed, lane, occupant)? else {
        return Ok(None);
    };
    let facts = observed.facts();
    let actor = Actor::Lane(lane);
    let completed = &current.completed;
    let claim = current.claim;

    if let Some(owed) = owed_request(bus, observed, entries, lane, occupant)? {
        return Ok(Some(owed.request));
    }
    if facts
        .latest_design_notice(lane, occupant)
        .is_some_and(|(at, notice)| {
            at > completed.at && notice.task == completed.task && notice.claim.get() == claim.get()
        })
    {
        // A standing or publication change released this completion's chosen
        // request. It must not retarget the same completion to another Design.
        return Ok(None);
    }

    let frequency = FrequencyProfile::recent(
        observed
            .tasks()
            .iter()
            .map(|heads| heads.statement().body()),
    );
    let task_statement = &current.statement;
    let completion = TermCounts::completion(
        &task_statement.title,
        task_statement.description.as_deref(),
        &task_statement.acceptance,
        &completed.validation,
    );
    let candidates = entries
        .iter()
        .filter(|entry| {
            passive(entry)
                && entry.is_filed()
                && entry.born() != &completed.task
                && !facts.design_was_noticed(occupant, entry.born())
                && !facts.published_in_room(&actor, entry.born())
        })
        .map(|design| {
            let statement = design.statement();
            MatchCandidate::new(
                design.born(),
                design,
                TermCounts::statement(
                    &statement.title,
                    statement.description.as_deref(),
                    &statement.acceptance,
                ),
            )
        });
    let Some((design, terms)) = frequency.select(&completion, candidates) else {
        return Ok(None);
    };
    let request = PlannedRequest {
        lane,
        occupant,
        task: completed.task.clone(),
        task_statement: task_statement.revision,
        claim,
        completed: completed.at,
        design: design.born().clone(),
        statement: design.statement().revision,
        terms: terms.clone(),
        horizon: observed.cursor(),
    };
    match record(bus, &request) {
        Ok(None) => Ok(Some(design_request(
            design,
            &current,
            wire::rooms::shown_notice_terms(&terms)
                .into_iter()
                .map(str::to_owned)
                .collect(),
        ))),
        Ok(Some(stale)) => Err(Outcome::Refusal(wire::Refusal::bare(
            wire::RefusalDiagnostic::new(
                "board::witness-moved",
                Some(stale),
                None,
                Some("board::witness-moved: Design request projection changed".to_owned()),
            ),
        ))),
        Err(outcome) => Err(command_answer_outcome(outcome)),
    }
}

fn current_task_statement(
    bus: &Bus,
    observed: &bus::TaskSnapshot,
    task: &TaskStamp,
) -> Result<Option<wire::TaskHead>, Outcome<wire::intent::CommandAnswer>> {
    if let Some(heads) = observed.task_heads(task) {
        return Ok(Some(heads.statement().body().clone()));
    }
    let task = task.clone();
    bus.read(move |read| read.task_head(&task))
        .map(|head| head.map(|head| head.into_body()))
        .map_err(|error| {
            command_answer_outcome(store_fault(
                "board::design-request-task-unreadable",
                &error.to_string(),
            ))
        })
}

/// Whether a recorded request is still owed. Only the Design's standing or the
/// lane's own room publication releases it; a statement edit does not.
fn request_is_current(facts: &bus::TaskFacts, design: &board::Entry, lane: CanonicalUuid) -> bool {
    design.is_filed()
        && passive(design)
        && !facts.published_in_room(&Actor::Lane(lane), design.born())
}

fn design_request(
    design: &board::Entry,
    current: &CurrentCompletion,
    terms: Vec<String>,
) -> DesignRequest {
    let statement = design.statement();
    let task = &current.statement;
    DesignRequest {
        design: wire::TaskHandle::for_filing(&statement.project, &statement.born, statement.filing),
        task: wire::TaskHandle::for_filing(&task.project, &task.born, task.filing),
        support: current.support,
        excerpt: excerpt(&current.completed.validation),
        title: statement.title.clone(),
        statement: SequenceCode::new(statement.revision.get()),
        pitch: pitch(&statement.title, statement.description.as_deref()),
        questions: request_questions(&statement.acceptance),
        terms,
    }
}

fn command_answer_outcome(outcome: Outcome<()>) -> Outcome<wire::intent::CommandAnswer> {
    match outcome {
        Outcome::Reply(_) => unreachable!("Design recognition does not reply on failure"),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

/// Refuse a request whose completion, occupant, statement, standing,
/// publication, newer lane work, or competing request moved.
fn still_owed(read: &JournalRead<'_>, request: &PlannedRequest) -> Result<(), GuardError> {
    current_recipient(read, request)?;
    idle_since_completion(read, request)?;
    passive_design(read, request)?;
    unseen_request(read, request)
}

fn current_recipient(read: &JournalRead<'_>, request: &PlannedRequest) -> Result<(), GuardError> {
    let stale = |why: &str| GuardError::refused(format!("{STALE}: {why}"));
    let occupied = read.residency_head()?.is_some_and(|head| {
        head.body().lanes.iter().any(|row| {
            row.lane == request.lane
                && row.occupant == Some(request.occupant)
                && row.disposition == "occupied"
        })
    });
    if !occupied {
        return Err(stale("the occupant moved"));
    }
    let recipient_current = read.claim_head(&request.task)?.is_some_and(|head| {
        let claim = head.body();
        claim.disposition == ClaimDisposition::Freed
            && claim.lane == request.lane
            && claim.occupant == request.occupant
            && claim.claimed == request.claim
    });
    if !recipient_current {
        return Err(stale("the completed claim moved"));
    }
    if !read
        .task_head(&request.task)?
        .is_some_and(|head| head.body().revision == request.task_statement)
    {
        return Err(stale("the task statement moved"));
    }
    Ok(())
}

/// Refuse once the lane won any claim after the owing completion: its natural
/// break ended whether that newer work is still active or already completed.
/// Only claim wins are read, so other lanes' and unrelated traffic stay
/// admissible.
fn idle_since_completion(
    read: &JournalRead<'_>,
    request: &PlannedRequest,
) -> Result<(), GuardError> {
    let mut after = Some(request.completed);
    loop {
        let page = read.selected_fact_page(
            &[wire::FactKind::ClaimWon],
            after,
            None,
            journal::FactReadBudget::RETAINED,
        )?;
        for envelope in &page.rows {
            let Some(won) = envelope.fact.read::<wire::body::ClaimWon>() else {
                return Err(GuardError::Invalid(
                    "a selected claim win did not decode".to_owned(),
                ));
            };
            if won.lane == request.lane {
                return Err(GuardError::refused(format!(
                    "{STALE}: the lane claimed newer work"
                )));
            }
        }
        let Some(continuation) = page.continuation else {
            return Ok(());
        };
        after = Some(continuation);
    }
}

fn passive_design(read: &JournalRead<'_>, request: &PlannedRequest) -> Result<(), GuardError> {
    let stale = |why: &str| GuardError::refused(format!("{STALE}: {why}"));
    let Some(statement) = read.task_head(&request.design)? else {
        return Err(stale("the Design disappeared"));
    };
    if statement.body().revision != request.statement {
        return Err(stale("the Design statement moved"));
    }
    let Some(workflow) = read.workflow_head(&request.design)? else {
        return Err(stale("the Design workflow disappeared"));
    };
    if !statement.body().filing.is_filed()
        || !wire::intent::is_open_design(statement.body().intent, workflow.body())
    {
        return Err(stale("the Design is no longer open"));
    }
    if read
        .group_partition()?
        .is_some_and(|head| head.body().design_is_active(&request.design))
    {
        return Err(stale("the Design is no longer passive"));
    }
    if read
        .latest_room_publication(&request.design, &Actor::Lane(request.lane))?
        .is_some()
    {
        return Err(stale("the lane published in the room"));
    }
    Ok(())
}

/// Refuse when another request for this occupant's Design or completion was
/// recorded after the planning horizon. Only Design notices are read: every
/// other authority that can change the chosen request is checked at its
/// current head, so unrelated journal traffic never makes the plan stale.
fn unseen_request(read: &JournalRead<'_>, request: &PlannedRequest) -> Result<(), GuardError> {
    let mut after = request.horizon;
    loop {
        let page = read.selected_fact_page(
            &[wire::FactKind::DesignNoticed],
            after,
            None,
            journal::FactReadBudget::RETAINED,
        )?;
        for envelope in &page.rows {
            let Some(body) = envelope.fact.read::<DesignNoticed>() else {
                return Err(GuardError::Invalid(
                    "a selected Design notice did not decode".to_owned(),
                ));
            };
            if body.occupant == request.occupant
                && (body.design == request.design
                    || (body.task == request.task && body.claim.get() == request.claim.get()))
            {
                return Err(GuardError::refused(format!(
                    "{STALE}: the occupant already has this request"
                )));
            }
        }
        let Some(continuation) = page.continuation else {
            return Ok(());
        };
        after = Some(continuation);
    }
}

/// Commit the durable record before `task next` returns it: the guard's stale
/// cause when a relevant authority moved after its observation.
fn record(bus: &Bus, request: &PlannedRequest) -> CommandResult<Option<String>> {
    let at = Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("room::awareness-clock-invalid", &error.to_string()))?;
    let origin = format!("lane:{}", request.lane)
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("room::awareness-origin-invalid", &error.to_string()))?;
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::ResidentRecognition);
    write
        .fact(
            DesignNoticed {
                statement: request.statement,
                design: request.design.clone(),
                lane: request.lane,
                occupant: request.occupant,
                task: request.task.clone(),
                claim: SequenceCode::new(request.claim.get()),
                terms: request.terms.clone(),
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("room::awareness-build", &error.to_string()))?;
    let observed = request.clone();
    write.guard(journal::JournalGuard::new(move |read| {
        still_owed(read, &observed)
    }));
    match bus
        .commit(write)
        .map_err(|error| store_fault("room::awareness-commit", &error.to_string()))?
    {
        CommitOutcome::Committed(_) => Ok(None),
        CommitOutcome::Refused(refusal) if refusal.reason().starts_with(STALE) => {
            Ok(Some(refusal.reason().to_owned()))
        }
        CommitOutcome::Refused(refusal) => {
            Err(store_fault("room::awareness-refused", &refusal.to_string()))
        }
        CommitOutcome::Fault(fault) => {
            Err(store_fault("room::awareness-fault", &fault.to_string()))
        }
    }
}
