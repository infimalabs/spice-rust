//! Session-profile requests for one contribution to a matching passive Design.
//!
//! Each retained-profile opportunity can owe one request at a session's
//! allocation boundary. The durable notice records that choice so command
//! reads never rescore or invent evidence.

use std::collections::{BTreeMap, BTreeSet};

use journal::{GuardError, JournalRead};
use spice::rooms::awareness::{FrequencyProfile, TermCounts, pitch, request_questions};
use spice::rooms::evidence::{DesignCandidate, Digest as ProfileDigest};
use spice::rooms::profile::{EvidenceKey, ProfileOpportunity, ProfileOpportunityKind};
use wire::body::TaskDone as CompletedTask;
use wire::rooms::{
    DesignConversationEvidence, DesignNoticeEvidence, DesignNoticeOpportunity, DesignNoticed,
    DesignSession,
};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, Domain, Producer, Seq, SequenceCode, TaskStamp,
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
/// Profile opportunities admitted to one in-memory page.
const PROFILE_OPPORTUNITY_PAGE: usize = 64;

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

/// Process-local horizons for task evidence acquisition, profile opportunities,
/// and Design statements already offered to the profile store.
pub(in crate::resident) struct CompletionCursor {
    after: Option<Seq>,
    opportunities: Option<u64>,
    designs: BTreeMap<TaskStamp, Seq>,
}

impl CompletionCursor {
    pub(in crate::resident) fn start(bus: &Bus) -> CommandResult<Self> {
        let after = bus.cursor().map_err(|error| {
            store_fault("room::awareness-cursor-unreadable", &error.to_string())
        })?;
        Ok(Self {
            after,
            opportunities: None,
            designs: BTreeMap::new(),
        })
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

/// Record one best matching passive Design for every retained-profile
/// opportunity. Nothing is delivered in the background: the lane reads the
/// owed request from its next `task next`.
pub(in crate::resident) fn reconcile(
    bus: &Bus,
    config: &SurfaceConfig,
    seats: &Seats,
    cursor: &mut CompletionCursor,
    profiles: &std::sync::Mutex<message_surface::MessageState>,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let completed = cursor.take(bus)?;
    let occupied = occupied(seats);
    if occupied.is_empty() {
        return Ok(());
    }
    let planning = Planning::load(bus, config)?;
    bind_current_sessions(&planning.facts, &occupied, profiles)?;
    retain_completed_evidence(&completed, profiles)?;
    let requests = plan_profile_requests(&planning, &occupied, cursor, profiles)?;
    for request in requests {
        match record(bus, &request) {
            Ok(None) => {}
            Ok(Some(stale)) => {
                let _ = writeln!(
                    diagnostic,
                    "lane {} request for Design {} became stale: {stale}",
                    request.lane, request.design
                );
            }
            Err(error) => {
                let _ = writeln!(
                    diagnostic,
                    "lane {} request for Design {} was not recorded: {error:?}",
                    request.lane, request.design
                );
            }
        }
    }
    Ok(())
}

fn bind_current_sessions(
    facts: &bus::TaskFacts,
    occupied: &BTreeMap<CanonicalUuid, CanonicalUuid>,
    profiles: &std::sync::Mutex<message_surface::MessageState>,
) -> CommandResult<()> {
    let mut state = message_surface::lock_state(profiles)?;
    for (&lane, &occupant) in occupied {
        let Some(session) = facts.lane_session(lane, occupant) else {
            continue;
        };
        state.profiles.bind(
            occupant,
            spice::rooms::profile::SessionKey::new(
                lane,
                session.vendor.clone(),
                session.identity.clone(),
            ),
        );
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
                EvidenceKey::Task {
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

fn plan_profile_requests(
    planning: &Planning,
    occupied: &BTreeMap<CanonicalUuid, CanonicalUuid>,
    cursor: &mut CompletionCursor,
    profiles: &std::sync::Mutex<message_surface::MessageState>,
) -> CommandResult<Vec<PlannedRequest>> {
    let entries = &planning.entries;
    let facts = &planning.facts;
    let frequency = FrequencyProfile::recent(entries.iter().map(board::Entry::statement));
    let prepared = entries
        .iter()
        .filter(|entry| passive(entry) && entry.is_filed())
        .filter_map(|entry| {
            let statement = entry.statement();
            frequency
                .prepare(&TermCounts::statement(
                    &statement.title,
                    statement.description.as_deref(),
                    &statement.acceptance,
                ))
                .map(|vector| (entry, vector))
        })
        .collect::<Vec<_>>();

    let mut state = message_surface::lock_state(profiles)?;
    record_design_changes(entries, cursor, &mut state.profiles);
    let opportunities = take_profile_opportunities(&state.profiles, occupied, cursor);
    let mut planned_sessions = BTreeSet::new();
    let mut planned = Vec::new();
    for opportunity in opportunities {
        let lane = opportunity.session.lane;
        let Some(&occupant) = occupied.get(&lane) else {
            continue;
        };
        if state.profiles.session(lane, occupant) != Some(&opportunity.session) {
            continue;
        }
        let session = DesignSession {
            vendor: opportunity.session.vendor.clone(),
            identity: opportunity.session.identity.clone(),
        };
        let session_identity = (lane, session.clone());
        if planned_sessions.contains(&session_identity)
            || session_has_outstanding(facts, entries, lane, &session)
        {
            continue;
        }
        let Some(durable_opportunity) = durable_opportunity(&opportunity, facts, occupant) else {
            continue;
        };
        let candidates = prepared
            .iter()
            .filter(|(design, _)| {
                opportunity_design_matches(&opportunity.kind, design)
                    && !facts.design_was_noticed(lane, &session, design.born())
                    && !facts.published_in_room_for_session(lane, &session, design.born())
            })
            .map(|(design, vector)| DesignCandidate::new(design.born(), *design, vector));
        let selected = {
            let Some(profile) = state.profiles.profile_mut(&opportunity.session) else {
                continue;
            };
            profile
                .evidence_mut()
                .select(&frequency, candidates)
                .map_err(|error| {
                    store_fault(
                        "room::awareness-profile-generation-mismatch",
                        &error.to_string(),
                    )
                })?
                .map(|matched| {
                    (
                        matched.design.born().clone(),
                        matched.design.statement().revision,
                        matched.evidence.clone(),
                        matched.digest,
                        matched.terms,
                    )
                })
        };
        let Some((design, statement, evidence, digest, terms)) = selected else {
            continue;
        };
        planned_sessions.insert(session_identity);
        planned.push(PlannedRequest {
            lane,
            occupant,
            session,
            opportunity: durable_opportunity,
            evidence: durable_evidence(&evidence, digest),
            design,
            statement,
            terms,
            horizon: planning.horizon,
        });
    }
    Ok(planned)
}

fn record_design_changes(
    entries: &[board::Entry],
    cursor: &mut CompletionCursor,
    profiles: &mut spice::rooms::profile::SessionProfiles,
) {
    let current = entries
        .iter()
        .filter(|entry| {
            entry
                .statement()
                .intent
                .is_some_and(wire::TaskIntent::is_design)
        })
        .map(|entry| (entry.born().clone(), entry.statement().revision))
        .collect::<BTreeMap<_, _>>();
    for (design, statement) in &current {
        if cursor.designs.get(design) != Some(statement) {
            profiles.design_changed(*statement);
        }
    }
    cursor.designs = current;
}

fn take_profile_opportunities(
    profiles: &spice::rooms::profile::SessionProfiles,
    occupied: &BTreeMap<CanonicalUuid, CanonicalUuid>,
    cursor: &mut CompletionCursor,
) -> Vec<ProfileOpportunity> {
    let mut page = profiles.opportunities(cursor.opportunities, PROFILE_OPPORTUNITY_PAGE);
    let mut rows = Vec::new();
    if page.missed {
        rows.extend(profiles.iter().filter_map(|(session, _)| {
            let occupant = occupied.get(&session.lane)?;
            (profiles.session(session.lane, *occupant) == Some(session)).then(|| {
                ProfileOpportunity {
                    cursor: 0,
                    session: session.clone(),
                    kind: ProfileOpportunityKind::Declared {
                        occupant: *occupant,
                    },
                }
            })
        }));
    }
    loop {
        rows.append(&mut page.rows);
        cursor.opportunities = page.next;
        if !page.more {
            break;
        }
        page = profiles.opportunities(cursor.opportunities, PROFILE_OPPORTUNITY_PAGE);
    }
    rows
}

fn opportunity_design_matches(kind: &ProfileOpportunityKind, design: &board::Entry) -> bool {
    match kind {
        ProfileOpportunityKind::DesignChanged { statement } => {
            design.statement().revision == *statement
        }
        ProfileOpportunityKind::Declared { .. }
        | ProfileOpportunityKind::Stored { .. }
        | ProfileOpportunityKind::Ready { .. } => true,
    }
}

fn durable_opportunity(
    opportunity: &ProfileOpportunity,
    facts: &bus::TaskFacts,
    current_occupant: CanonicalUuid,
) -> Option<DesignNoticeOpportunity> {
    match &opportunity.kind {
        ProfileOpportunityKind::Declared { occupant } => {
            if *occupant != current_occupant {
                return None;
            }
            facts
                .lane_session_declaration(opportunity.session.lane, *occupant)
                .map(|declaration| DesignNoticeOpportunity::SessionResumed { declaration })
        }
        ProfileOpportunityKind::Stored { key, digest } => match key {
            EvidenceKey::Task {
                task, completion, ..
            } => Some(DesignNoticeOpportunity::TaskCompletion {
                task: task.clone(),
                completed: *completion,
            }),
            EvidenceKey::Conversation { .. } => Some(DesignNoticeOpportunity::Conversation {
                source: durable_conversation(key, *digest)?,
            }),
        },
        ProfileOpportunityKind::Ready { through } => Some(DesignNoticeOpportunity::ProfileReady {
            horizon: through.facts,
        }),
        ProfileOpportunityKind::DesignChanged { .. } => {
            Some(DesignNoticeOpportunity::DesignStatement)
        }
    }
}

fn durable_evidence(key: &EvidenceKey, digest: ProfileDigest) -> DesignNoticeEvidence {
    match key {
        EvidenceKey::Task {
            task,
            statement,
            completion,
        } => DesignNoticeEvidence::Task {
            task: task.clone(),
            statement: *statement,
            completed: *completion,
        },
        EvidenceKey::Conversation { .. } => DesignNoticeEvidence::Conversation {
            source: durable_conversation(key, digest)
                .expect("a conversation evidence key always has a durable locus"),
        },
    }
}

fn durable_conversation(
    key: &EvidenceKey,
    digest: ProfileDigest,
) -> Option<DesignConversationEvidence> {
    let EvidenceKey::Conversation {
        source,
        member,
        offset,
        length,
    } = key
    else {
        return None;
    };
    Some(DesignConversationEvidence {
        locus: wire::body::TranscriptLocus {
            source: Some(*source),
            member: *member,
            offset: *offset,
            length: *length,
            event: 0,
            line: 1,
            column: 1,
        },
        digest: wire::Digest::compute(Domain::RoomProfileEvidence, digest.as_bytes()),
    })
}

/// Whether `entry` is an open Design inactive under its group stance.
fn passive(entry: &board::Entry) -> bool {
    board::passive_design(entry)
}

/// Read the durable invitation currently owed to this continuing session.
pub(crate) fn owed_request(
    bus: &Bus,
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
    if !supporting_evidence_is_current(bus, lane, session, &notice.evidence)? {
        return Ok(None);
    }
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

fn supporting_evidence_is_current(
    bus: &Bus,
    lane: CanonicalUuid,
    session: &DesignSession,
    evidence: &DesignNoticeEvidence,
) -> Result<bool, Outcome<wire::intent::CommandAnswer>> {
    let DesignNoticeEvidence::Conversation { source } = evidence else {
        return Ok(true);
    };
    let Some(source_identity) = source.locus.source else {
        return Ok(false);
    };
    let vendor = session.vendor.clone();
    bus.read(move |read| read.transcript_source_heads(&lane))
        .map(|heads| {
            heads
                .into_iter()
                .any(|head| head.body().vendor == vendor && head.body().source == source_identity)
        })
        .map_err(|error| {
            command_answer_outcome(store_fault(
                "board::design-request-source-unreadable",
                &error.to_string(),
            ))
        })
}

fn command_answer_outcome(outcome: Outcome<()>) -> Outcome<wire::intent::CommandAnswer> {
    match outcome {
        Outcome::Reply(_) => unreachable!("Design recognition does not reply on failure"),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

/// Return only a request the resident already selected from the retained
/// session profile. Command reads never rescore one incidental completion.
pub(crate) fn project_request(
    bus: &Bus,
    observed: &bus::TaskSnapshot,
    entries: &[board::Entry],
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> Result<Option<DesignRequest>, Outcome<wire::intent::CommandAnswer>> {
    owed_request(bus, observed, entries, lane, occupant)
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
