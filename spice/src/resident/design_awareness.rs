//! Session-profile requests for one contribution to a matching passive Design.
//!
//! Each retained-profile opportunity can owe one request at a session's
//! allocation boundary. The durable notice records that choice so command
//! reads never rescore or invent evidence.

use std::collections::{BTreeMap, BTreeSet};

use journal::{GuardError, JournalRead};
use spice::rooms::awareness::{
    FrequencyProfile, MatchCandidate, TermCounts, pitch, request_questions,
};
use wire::body::TaskDone as CompletedTask;
use wire::rooms::{DesignNoticeEvidence, DesignNoticeOpportunity, DesignNoticed, DesignSession};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, Producer, Seq, SequenceCode, TaskStamp,
    intent::{DesignRequest, TaskDone as TaskDoneIntent},
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
    session: DesignSession,
    opportunity: DesignNoticeOpportunity,
    evidence: DesignNoticeEvidence,
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
/// Nothing is delivered in the background: the lane reads the owed request
/// from its next `task next`.
pub(in crate::resident) fn reconcile(
    bus: &Bus,
    config: &SurfaceConfig,
    seats: &Seats,
    completions: &mut CompletionCursor,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let completed = completions.take(bus)?;
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
    let facts = &planning.facts;
    let mut planned_sessions = BTreeSet::new();
    let mut owed = Vec::new();
    for completed in completions {
        let Some(session) = facts
            .lane_session(completed.lane, completed.occupant)
            .cloned()
        else {
            continue;
        };
        let session_key = (completed.lane, session.clone());
        if planned_sessions.contains(&session_key)
            || session_has_outstanding(facts, entries, completed.lane, &session)
        {
            continue;
        }
        let already_requested = facts
            .latest_design_notice(completed.lane, &session)
            .is_some_and(|(_, notice)| {
                matches!(
                    &notice.opportunity,
                    DesignNoticeOpportunity::TaskCompletion { task, completed: at }
                        if task == &completed.task && at == &completed.at
                )
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
        let candidates = entries
            .iter()
            .filter(|entry| {
                passive(entry)
                    && entry.is_filed()
                    && !facts.design_was_noticed(completed.lane, &session, entry.born())
                    && !facts.published_in_room_for_session(completed.lane, &session, entry.born())
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
        planned_sessions.insert(session_key);
        owed.push(planned_request(
            completed,
            session,
            design,
            matched,
            planning.horizon,
        ));
    }
    owed
}

fn planned_request(
    completed: &Completion,
    session: DesignSession,
    design: &board::Entry,
    terms: Vec<String>,
    horizon: Option<Seq>,
) -> PlannedRequest {
    PlannedRequest {
        lane: completed.lane,
        occupant: completed.occupant,
        session,
        opportunity: DesignNoticeOpportunity::TaskCompletion {
            task: completed.task.clone(),
            completed: completed.at,
        },
        evidence: DesignNoticeEvidence::Task {
            task: completed.task.clone(),
            statement: completed.statement.revision,
            completed: completed.at,
        },
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
    let Some((claim, _cell)) = facts.latest_claim(&actor) else {
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
        statement,
    }))
}

/// Read the durable invitation currently owed to this continuing session.
pub(crate) fn owed_request(
    _bus: &Bus,
    observed: &bus::TaskSnapshot,
    entries: &[board::Entry],
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> Result<Option<DesignRequest>, Outcome<wire::intent::CommandAnswer>> {
    let facts = observed.facts();
    let Some(session) = facts.lane_session(lane, occupant) else {
        return Ok(None);
    };
    let Some((noticed_at, notice)) = facts.latest_design_notice(lane, session) else {
        return Ok(None);
    };
    let Some(design) = entries.iter().find(|entry| entry.born() == &notice.design) else {
        return Ok(None);
    };
    if !request_is_current(facts, design, lane, notice.statement, noticed_at) {
        return Ok(None);
    }
    Ok(Some(design_request(
        design,
        notice.session.clone(),
        notice.opportunity.clone(),
        notice.evidence.clone(),
        wire::rooms::shown_notice_terms(&notice.terms)
            .into_iter()
            .map(str::to_owned)
            .collect(),
    )))
}

/// Select and record one deterministic request for the latest completion.
pub(crate) fn project_request(
    bus: &Bus,
    observed: &bus::TaskSnapshot,
    entries: &[board::Entry],
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> Result<Option<DesignRequest>, Outcome<wire::intent::CommandAnswer>> {
    let facts = observed.facts();
    let Some(session) = facts.lane_session(lane, occupant).cloned() else {
        return Ok(None);
    };
    if let Some(request) = owed_request(bus, observed, entries, lane, occupant)? {
        return Ok(Some(request));
    }
    let Some(current) = current_completion(bus, observed, lane, occupant)? else {
        return Ok(None);
    };
    let completed = &current.completed;
    let task_statement = current.statement;
    if facts
        .latest_design_notice(lane, &session)
        .is_some_and(|(_, notice)| {
            matches!(
                &notice.opportunity,
                DesignNoticeOpportunity::TaskCompletion { task, completed: at }
                    if task == &completed.task && at == &completed.at
            )
        })
    {
        // A standing or publication change released this opportunity's chosen
        // request. It must not retarget the same opportunity to another Design.
        return Ok(None);
    }
    let frequency = FrequencyProfile::recent(
        observed
            .tasks()
            .iter()
            .map(|heads| heads.statement().body()),
    );
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
                && !facts.design_was_noticed(lane, &session, entry.born())
                && !facts.published_in_room_for_session(lane, &session, entry.born())
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
        session: session.clone(),
        opportunity: DesignNoticeOpportunity::TaskCompletion {
            task: completed.task.clone(),
            completed: completed.at,
        },
        evidence: DesignNoticeEvidence::Task {
            task: completed.task.clone(),
            statement: task_statement.revision,
            completed: completed.at,
        },
        design: design.born().clone(),
        statement: design.statement().revision,
        terms: terms.clone(),
        horizon: observed.cursor(),
    };
    match record(bus, &request) {
        Ok(None) => Ok(Some(design_request(
            design,
            session,
            request.opportunity,
            request.evidence,
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

fn session_has_outstanding(
    facts: &bus::TaskFacts,
    entries: &[board::Entry],
    lane: CanonicalUuid,
    session: &DesignSession,
) -> bool {
    let Some((noticed_at, notice)) = facts.latest_design_notice(lane, session) else {
        return false;
    };
    entries
        .iter()
        .find(|entry| entry.born() == &notice.design)
        .is_some_and(|design| request_is_current(facts, design, lane, notice.statement, noticed_at))
}

/// Whether a recorded invitation is still owed. Design closure, statement
/// replacement, standing, or a room publication releases it.
fn request_is_current(
    facts: &bus::TaskFacts,
    design: &board::Entry,
    lane: CanonicalUuid,
    statement: Seq,
    noticed_at: Seq,
) -> bool {
    design.is_filed()
        && passive(design)
        && design.statement().revision == statement
        && !facts.published_in_room_after(lane, design.born(), noticed_at)
}

fn design_request(
    design: &board::Entry,
    session: DesignSession,
    opportunity: DesignNoticeOpportunity,
    evidence: DesignNoticeEvidence,
    terms: Vec<String>,
) -> DesignRequest {
    let statement = design.statement();
    DesignRequest {
        design: wire::TaskHandle::for_filing(&statement.project, &statement.born, statement.filing),
        session,
        opportunity,
        evidence,
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

/// Refuse an invitation whose target session, supporting source, Design, room
/// standing, or competing request moved.
fn still_owed(read: &JournalRead<'_>, request: &PlannedRequest) -> Result<(), GuardError> {
    let declared = current_recipient(read, request)?;
    supporting_source(read, request)?;
    passive_design(read, request, declared)?;
    unseen_request(read, request)
}

fn current_recipient(read: &JournalRead<'_>, request: &PlannedRequest) -> Result<Seq, GuardError> {
    let stale = |why: &str| GuardError::refused(format!("{STALE}: {why}"));
    let occupant = read
        .residency_head()?
        .and_then(|head| {
            head.body()
                .lanes
                .iter()
                .find(|row| row.lane == request.lane && row.disposition == "occupied")
                .and_then(|row| row.occupant)
        })
        .ok_or_else(|| stale("the target lane is no longer occupied"))?;
    let mut after = None;
    let mut current = None;
    loop {
        let page = read.selected_fact_page(
            &[wire::FactKind::LaneSessionDeclared],
            after,
            None,
            journal::FactReadBudget::RETAINED,
        )?;
        for envelope in &page.rows {
            let Some(declared) = envelope.fact.read::<wire::body::LaneSessionDeclared>() else {
                return Err(GuardError::Invalid(
                    "a selected session declaration did not decode".to_owned(),
                ));
            };
            if declared.lane == request.lane && declared.occupant == occupant {
                current = Some((
                    DesignSession {
                        vendor: declared.vendor,
                        identity: declared.identity,
                    },
                    envelope.seq,
                ));
            }
        }
        let Some(continuation) = page.continuation else {
            break;
        };
        after = Some(continuation);
    }
    match current {
        Some((session, declared)) if session == request.session => Ok(declared),
        _ => Err(stale("the target vendor session was replaced")),
    }
}

fn supporting_source(read: &JournalRead<'_>, request: &PlannedRequest) -> Result<(), GuardError> {
    let stale = |why: &str| GuardError::refused(format!("{STALE}: {why}"));
    match &request.evidence {
        DesignNoticeEvidence::Task {
            task,
            statement,
            completed,
        } => {
            let statement_exists = read
                .causal_page(*statement, journal::FactReadBudget::RETAINED)?
                .is_some_and(|page| {
                    page.rows.iter().any(|row| {
                        row.seq == *statement
                            && row.fact.revised_tasks().any(|revised| revised == task)
                    })
                });
            let completion_exists = read
                .causal_page(*completed, journal::FactReadBudget::RETAINED)?
                .is_some_and(|page| {
                    page.rows.iter().any(|row| {
                        row.seq == *completed
                            && row.fact.read::<wire::body::TaskDone>().is_some_and(|done| {
                                done.task == *task
                                    && done.terminal.as_ref().is_some_and(|terminal| {
                                        terminal.status
                                            == wire::body::TaskCompletionStatus::Completed
                                    })
                            })
                    })
                });
            if !statement_exists || !completion_exists {
                return Err(stale("the supporting task evidence was lost or replaced"));
            }
        }
        DesignNoticeEvidence::Conversation { source } => {
            let source_exists = read
                .transcript_source_heads(&request.lane)?
                .iter()
                .any(|head| {
                    head.body().vendor == request.session.vendor
                        && Some(head.body().source) == source.locus.source
                });
            if !source_exists {
                return Err(stale(
                    "the supporting conversation source was lost or replaced",
                ));
            }
        }
    }
    Ok(())
}

fn passive_design(
    read: &JournalRead<'_>,
    request: &PlannedRequest,
    session_declared: Seq,
) -> Result<(), GuardError> {
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
        .is_some_and(|head| head.seq > session_declared)
    {
        return Err(stale("the session published in the room"));
    }
    Ok(())
}

/// Refuse when another invitation for this continuing session was recorded
/// after the planning horizon. Other authority is checked at its exact head,
/// so unrelated journal traffic never makes the plan stale.
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
            if body.lane == request.lane && body.session == request.session {
                return Err(GuardError::refused(format!(
                    "{STALE}: the session already has an invitation"
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
                session: request.session.clone(),
                opportunity: request.opportunity.clone(),
                evidence: request.evidence.clone(),
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
