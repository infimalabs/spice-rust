//! Session-profile requests for one contribution to a matching open Design.
//!
//! Each retained-profile opportunity can owe one request at a session's
//! allocation boundary. The durable notice records that choice so command
//! reads never rescore or invent evidence.

use std::collections::{BTreeMap, BTreeSet};

use board::similarity::{
    FrequencyProfile, GenerationMismatch, MATCH_SCORE_BASE, Prepared, TermCounts,
};
use journal::{GuardError, JournalRead};
use spice::rooms::awareness::request_questions;
use spice::rooms::evidence::{DesignCandidate, Digest as ProfileDigest};
use spice::rooms::profile::{EvidenceKey, ProfileOpportunity, ProfileOpportunityKind};
use wire::body::TaskDone as CompletedTask;
use wire::rooms::{
    DesignAsk, DesignConversationEvidence, DesignItemSeries, DesignItems, DesignNoticeEvidence,
    DesignNoticeOpportunity, DesignNoticed, DesignSession, DesignSupport, RoomContributed,
    RoomLeft, RoomQuestionItem, WorkingGroupStance,
};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, Domain, Producer, Seq, SequenceCode, TaskStamp,
    intent::{DesignRequest, DesignRequestAsk, TaskDone as TaskDoneIntent},
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
/// Denominator shared by the exact stance weighting ratios.
const MATCH_SCORE_PARTS: f64 = 3.0;

/// Candidate-owned relevance floor for one Design's effective stance.
#[must_use]
pub(in crate::resident) fn match_score_floor(stance: WorkingGroupStance) -> f64 {
    let parts = match stance {
        WorkingGroupStance::Passive => 3.0,
        WorkingGroupStance::Explore => 2.0,
        WorkingGroupStance::Converge => 4.0,
    };
    MATCH_SCORE_BASE * parts / MATCH_SCORE_PARTS
}

/// The stance whose floor one Design's invitations clear: its group's,
/// raised to Converge once its current items steep. A steeped Design is
/// close, so it asks only the lanes Converge would; the floor falls back with
/// the fold as soon as a later charge contests an item or a statement edit
/// un-steeps it. The notice records this stance as the one that admitted the
/// match.
fn admitting_stance(facts: &bus::TaskFacts, design: &board::Entry) -> WorkingGroupStance {
    let born = design.born();
    let steeped = facts
        .design_items(born)
        .is_some_and(|items| items.steeped(&design.statement().acceptance));
    if steeped {
        WorkingGroupStance::Converge
    } else {
        facts.design_stance(born)
    }
}

/// The floor one Design's invitations clear.
pub(in crate::resident) fn invitation_floor(facts: &bus::TaskFacts, design: &board::Entry) -> f64 {
    match_score_floor(admitting_stance(facts, design))
}

/// One Design request and the journal position its planning read covered.
#[derive(Clone)]
struct PlannedRequest {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    session: DesignSession,
    compaction: Option<Seq>,
    opportunity: DesignNoticeOpportunity,
    evidence: DesignNoticeEvidence,
    excerpt: String,
    design: TaskStamp,
    statement: Seq,
    terms: Vec<String>,
    stance: Option<WorkingGroupStance>,
    asks: Vec<DesignAsk>,
    horizon: Option<Seq>,
}

/// One newly observed task completion and its matching input.
#[derive(Clone)]
pub(in crate::resident) struct Completion {
    at: Seq,
    task: TaskStamp,
    validation: String,
    support: DesignSupport,
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
                        support: DesignSupport::for_phase(&claim.cell.phase),
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

/// Record one best matching open Design for every retained-profile
/// opportunity, replacing a session's owed request only with a strictly better
/// match. Nothing is pushed into a lane's turn here: room catch-up carries the
/// owed request into the lane's next model input, and `task next` shows it
/// while the lane holds no claim.
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
    bind_current_sessions(planning.snapshot.facts(), &occupied, profiles)?;
    retain_completed_evidence(&completed, profiles)?;
    let requests = plan_profile_requests(bus, &planning, &occupied, cursor, profiles)?;
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
                    support: completed.support,
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
    bus: &Bus,
    planning: &Planning,
    occupied: &BTreeMap<CanonicalUuid, CanonicalUuid>,
    cursor: &mut CompletionCursor,
    profiles: &std::sync::Mutex<message_surface::MessageState>,
) -> CommandResult<Vec<PlannedRequest>> {
    let entries = &planning.entries;
    let facts = planning.snapshot.facts();
    let opportunities = {
        let mut state = message_surface::lock_state(profiles)?;
        record_design_changes(entries, cursor, &mut state.profiles);
        take_profile_opportunities(&state.profiles, occupied, cursor)
    };
    if opportunities.is_empty() {
        return Ok(Vec::new());
    }
    let frequency = FrequencyProfile::recent(entries.iter().map(board::Entry::statement));
    let prepared = prepare_open_designs(entries, &frequency);
    let scoring = Scoring {
        facts,
        frequency: &frequency,
        prepared: &prepared,
    };

    let mut state = message_surface::lock_state(profiles)?;
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
        let compaction = facts.session_compaction(lane, &session);
        let session_identity = (lane, session.clone());
        if planned_sessions.contains(&session_identity) {
            continue;
        }
        let owed = owed_notice(bus, facts, entries, lane, &session)?;
        if let Some(owed) = owed
            && let Some(restated) =
                restated_request(bus, planning, occupant, &session, compaction, owed)?
        {
            planned_sessions.insert(session_identity);
            planned.push(restated);
            continue;
        }
        let owed = owed.map(|(_, notice)| notice.design.clone());
        let Some(durable_opportunity) = durable_opportunity(&opportunity, facts, occupant) else {
            continue;
        };
        let selected = {
            let Some(profile) = state.profiles.profile_mut(&opportunity.session) else {
                continue;
            };
            remove_stale_profile_evidence(bus, lane, &session, profile)?;
            select_design(
                &scoring,
                profile,
                lane,
                &session,
                &opportunity.kind,
                owed.as_ref(),
            )?
        };
        let Some(Selection {
            design,
            statement,
            evidence,
            excerpt,
            terms,
            stance,
            asks,
        }) = selected
        else {
            continue;
        };
        planned_sessions.insert(session_identity);
        planned.push(PlannedRequest {
            lane,
            occupant,
            session,
            compaction,
            opportunity: durable_opportunity,
            evidence,
            excerpt,
            design,
            statement,
            terms,
            stance: Some(stance),
            asks,
            horizon: planning.horizon,
        });
    }
    Ok(planned)
}

/// Each filed open Design weighted under `frequency`.
fn prepare_open_designs<'e>(
    entries: &'e [board::Entry],
    frequency: &FrequencyProfile,
) -> Vec<(&'e board::Entry, Prepared)> {
    entries
        .iter()
        .filter(|entry| entry.is_filed() && open_design(entry))
        .filter_map(|entry| {
            frequency
                .prepare(&TermCounts::task(entry.statement()))
                .map(|vector| (entry, vector))
        })
        .collect()
}

/// Open Designs weighted under one planning read's frequency generation.
struct Scoring<'e> {
    facts: &'e bus::TaskFacts,
    frequency: &'e FrequencyProfile,
    prepared: &'e [(&'e board::Entry, Prepared)],
}

/// The Design one session's current cards select for an opportunity.
struct Selection {
    design: TaskStamp,
    statement: Seq,
    evidence: DesignNoticeEvidence,
    excerpt: String,
    terms: Vec<String>,
    stance: WorkingGroupStance,
    asks: Vec<DesignAsk>,
}

/// The best Design `kind` admits that still owes this session an ask, unless
/// it fails to strictly outrank the Design still owed to the session under the
/// same frequency generation, current cards, and stance floors. An owed Design
/// that no longer clears its floor bars nothing. Whether a Design is selected
/// is judged on its whole statement; the selection then asks every pending
/// item, the one scoring best against the cards first.
fn select_design(
    scoring: &Scoring<'_>,
    profile: &mut spice::rooms::profile::SessionProfile,
    lane: CanonicalUuid,
    session: &DesignSession,
    kind: &ProfileOpportunityKind,
    owed: Option<&TaskStamp>,
) -> CommandResult<Option<Selection>> {
    let Scoring {
        facts,
        frequency,
        prepared,
    } = *scoring;
    let mismatch = |error: GenerationMismatch| {
        store_fault(
            "room::awareness-profile-generation-mismatch",
            &error.to_string(),
        )
    };
    let candidate = |prepared| candidate(facts, prepared);
    let bar = match owed {
        Some(owed) => profile
            .evidence_mut()
            .select(
                frequency,
                prepared
                    .iter()
                    .filter(|(design, _)| design.born() == owed)
                    .map(candidate),
            )
            .map_err(mismatch)?
            .map(|matched| matched.score),
        None => None,
    };
    let candidates = prepared
        .iter()
        .filter(|(design, _)| {
            opportunity_design_matches(kind, design)
                && pending_asks(facts, lane, session, design).is_some()
        })
        .map(candidate);
    let Some(matched) = profile
        .evidence_mut()
        .select(frequency, candidates)
        .map_err(mismatch)?
    else {
        return Ok(None);
    };
    if bar.is_some_and(|bar| matched.score <= bar) {
        return Ok(None);
    }
    let design: &board::Entry = matched.design;
    let evidence = durable_evidence(matched.evidence, matched.digest);
    let excerpt = matched.excerpt.to_owned();
    let terms = matched.terms;
    let stance = admitting_stance(facts, design);
    let asks = pending_asks(facts, lane, session, design).unwrap_or_default();
    let asks =
        rank_asks(profile, frequency, design, asks, match_score_floor(stance)).map_err(mismatch)?;
    Ok(Some(Selection {
        design: design.born().clone(),
        statement: design.statement().revision,
        evidence,
        excerpt,
        terms,
        stance,
        asks,
    }))
}

/// What `design` still owes this continuing session: each item the stance
/// makes askable of the lane that the session was not yet asked and did not
/// answer, or, when no current item is askable of the lane, the whole Design
/// (an empty list) unless asked or answered. `None` when it owes nothing,
/// including once every current item concurs: nothing is left to weigh.
fn pending_asks(
    facts: &bus::TaskFacts,
    lane: CanonicalUuid,
    session: &DesignSession,
    design: &board::Entry,
) -> Option<Vec<DesignAsk>> {
    let born = design.born();
    let acceptance = &design.statement().acceptance;
    let stance = facts.design_stance(born);
    let quiet = DesignItems::default();
    let items = facts.design_items(born).unwrap_or(&quiet);
    if items.all_concurred(acceptance) {
        return None;
    }
    let askable = items.askable(acceptance, stance, lane);
    let open = |item: Option<&RoomQuestionItem>| {
        !facts.design_was_asked_in_session(lane, session, born, item)
            && !facts.design_ask_answered_in_session(lane, session, born, item)
    };
    if askable.is_empty() {
        return open(None).then(Vec::new);
    }
    let pending = askable
        .into_iter()
        .filter(|ask| open(Some(&ask.item)))
        .collect::<Vec<_>>();
    (!pending.is_empty()).then_some(pending)
}

/// `asks` with the one whose item text, read with the Design's problem
/// sentence, scores best against the session's cards at or above `floor`
/// moved first and the rest kept in statement order; unchanged when none
/// clears it. Empty asks about the whole Design.
fn rank_asks(
    profile: &mut spice::rooms::profile::SessionProfile,
    frequency: &FrequencyProfile,
    design: &board::Entry,
    mut asks: Vec<DesignAsk>,
    floor: f64,
) -> Result<Vec<DesignAsk>, GenerationMismatch> {
    let statement = design.statement();
    let problem = statement.pitch.as_str();
    let mut best: Option<(f64, usize)> = None;
    for (index, ask) in asks.iter().enumerate() {
        let Some(text) = ask.question.item(&statement.acceptance) else {
            continue;
        };
        let Some(vector) = frequency.prepare(&TermCounts::from_texts([text, problem])) else {
            continue;
        };
        let Some(score) = profile.evidence_mut().best_score(frequency, &vector)? else {
            continue;
        };
        if score >= floor && best.is_none_or(|(known, _)| score > known) {
            best = Some((score, index));
        }
    }
    if let Some((_, index)) = best {
        asks[..=index].rotate_right(1);
    }
    Ok(asks)
}

/// One prepared open Design weighted under its invitation floor.
fn candidate<'a>(
    facts: &bus::TaskFacts,
    (design, vector): &'a (&'a board::Entry, Prepared),
) -> DesignCandidate<'a, board::Entry> {
    DesignCandidate::new(
        design.born(),
        *design,
        vector,
        invitation_floor(facts, design),
    )
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
            support,
        } => DesignNoticeEvidence::Task {
            task: task.clone(),
            statement: *statement,
            completed: *completion,
            support: *support,
        },
        EvidenceKey::Conversation { .. } => DesignNoticeEvidence::Conversation {
            source: durable_conversation(key, digest)
                .expect("a conversation evidence key always has a durable locus"),
        },
    }
}

fn remove_stale_profile_evidence(
    bus: &Bus,
    lane: CanonicalUuid,
    session: &DesignSession,
    profile: &mut spice::rooms::profile::SessionProfile,
) -> CommandResult<()> {
    let evidence = profile
        .evidence()
        .evidence()
        .map(|card| (card.digest(), durable_evidence(card.key(), card.digest())))
        .collect::<Vec<_>>();
    if evidence.is_empty() {
        return Ok(());
    }
    let session = session.clone();
    let stale = bus
        .read(move |read| {
            let mut stale = Vec::new();
            for (digest, evidence) in evidence {
                if !evidence_source_is_current(read, lane, &session, &evidence)? {
                    stale.push(digest);
                }
            }
            Ok(stale)
        })
        .map_err(|error| {
            store_fault(
                "room::awareness-profile-source-unreadable",
                &error.to_string(),
            )
        })?;
    for digest in stale {
        profile.evidence_mut().remove(digest);
    }
    Ok(())
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

/// Whether `entry` is currently an open Design, regardless of its stance.
fn open_design(entry: &board::Entry) -> bool {
    wire::intent::is_open_design(entry.statement().intent, entry.workflow())
}

/// One request owed to a continuing session.
pub(crate) struct OwedRequest {
    /// Position of the notice that chose it.
    pub(crate) at: Seq,
    /// Design whose room accepts the answer.
    pub(crate) design: TaskStamp,
    pub(crate) request: DesignRequest,
}

/// Read the durable invitation currently owed to this continuing session.
pub(crate) fn owed_request(
    bus: &Bus,
    observed: &bus::TaskSnapshot,
    entries: &[board::Entry],
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> Result<Option<OwedRequest>, Outcome<wire::intent::CommandAnswer>> {
    let facts = observed.facts();
    let Some(session) = facts.lane_session(lane, occupant) else {
        return Ok(None);
    };
    let Some((noticed_at, notice)) =
        owed_notice(bus, facts, entries, lane, session).map_err(command_answer_outcome)?
    else {
        return Ok(None);
    };
    let design = entries
        .iter()
        .find(|entry| entry.born() == &notice.design)
        .expect("an owed notice has a current Design");
    Ok(Some(OwedRequest {
        at: noticed_at,
        design: notice.design.clone(),
        request: design_request(facts, design, notice),
    }))
}

/// The one owed-ness rule shared by selection and delivery.
fn owed_notice<'a>(
    bus: &Bus,
    facts: &'a bus::TaskFacts,
    entries: &[board::Entry],
    lane: CanonicalUuid,
    session: &DesignSession,
) -> CommandResult<Option<(Seq, &'a DesignNoticed)>> {
    let Some((noticed_at, notice)) = facts.latest_design_notice(lane, session) else {
        return Ok(None);
    };
    let Some(design) = entries.iter().find(|entry| entry.born() == &notice.design) else {
        return Ok(None);
    };
    if !request_is_current(facts, design, notice, noticed_at)
        || !supporting_evidence_is_current(bus, lane, session, &notice.evidence)?
    {
        return Ok(None);
    }
    Ok(Some((noticed_at, notice)))
}

fn supporting_evidence_is_current(
    bus: &Bus,
    lane: CanonicalUuid,
    session: &DesignSession,
    evidence: &DesignNoticeEvidence,
) -> CommandResult<bool> {
    let session = session.clone();
    let evidence = evidence.clone();
    bus.read(move |read| evidence_source_is_current(read, lane, &session, &evidence))
        .map_err(|error| {
            store_fault(
                "board::design-request-source-unreadable",
                &error.to_string(),
            )
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
    Ok(owed_request(bus, observed, entries, lane, occupant)?.map(|owed| owed.request))
}

/// The owed request restated for a new occupant of its continuing session.
///
/// Once an earlier occupant's catch-up carried the request, the lane's cursor
/// has passed its record, so the resumed occupant would never be shown it.
/// Recording it again at the resume makes catch-up present it once more; the
/// restated record names this occupant, so it is never restated for it again.
fn restated_request(
    bus: &Bus,
    planning: &Planning,
    occupant: CanonicalUuid,
    session: &DesignSession,
    compaction: Option<Seq>,
    (noticed_at, notice): (Seq, &DesignNoticed),
) -> CommandResult<Option<PlannedRequest>> {
    let lane = notice.lane;
    if notice.occupant == occupant {
        return Ok(None);
    }
    let Some(declaration) = planning
        .snapshot
        .facts()
        .lane_session_declaration(lane, occupant)
    else {
        return Ok(None);
    };
    if super::room_catch_up::cursor(bus, lane)?.is_none_or(|cursor| cursor < noticed_at) {
        return Ok(None);
    }
    Ok(Some(PlannedRequest {
        lane,
        occupant,
        session: session.clone(),
        compaction,
        opportunity: DesignNoticeOpportunity::SessionResumed { declaration },
        evidence: notice.evidence.clone(),
        excerpt: notice.excerpt.clone(),
        design: notice.design.clone(),
        statement: notice.statement,
        terms: notice.terms.clone(),
        stance: notice.stance,
        asks: notice.asks.clone(),
        horizon: planning.horizon,
    }))
}

/// Whether a recorded invitation is still owed. Design closure or statement
/// replacement releases it, and so does the lane's answer: a contribution
/// against any asked item or a leave, or any room record for a whole-Design
/// ask. Another lane's post or a change in an item's standing never does.
fn request_is_current(
    facts: &bus::TaskFacts,
    design: &board::Entry,
    notice: &DesignNoticed,
    noticed_at: Seq,
) -> bool {
    let answered =
        |item| facts.design_ask_answered_after(notice.lane, design.born(), item, noticed_at);
    let settled = if notice.asks.is_empty() {
        answered(None)
    } else {
        notice.asks.iter().any(|ask| answered(Some(&ask.item)))
    };
    design.is_filed()
        && open_design(design)
        && design.statement().revision == notice.statement
        && !settled
}

fn design_request(
    facts: &bus::TaskFacts,
    design: &board::Entry,
    notice: &DesignNoticed,
) -> DesignRequest {
    let statement = design.statement();
    let asks = notice
        .asks
        .iter()
        .filter_map(|ask| {
            let series = facts
                .design_items(design.born())
                .and_then(|items| items.series(&ask.item));
            Some(DesignRequestAsk {
                question: ask.question,
                text: ask.question.item(&statement.acceptance)?.to_owned(),
                state: ask.state,
                charges: series.map_or(0, |series| series.charges),
                lanes: series.map_or(0, DesignItemSeries::lanes),
            })
        })
        .collect();
    DesignRequest {
        design: wire::TaskHandle::for_filing(&statement.project, &statement.born, statement.filing),
        lane: notice.lane,
        session: notice.session.clone(),
        opportunity: notice.opportunity.clone(),
        evidence: notice.evidence.clone(),
        excerpt: notice.excerpt.clone(),
        title: statement.title.clone(),
        statement: SequenceCode::new(statement.revision.get()),
        pitch: statement.pitch.clone(),
        questions: request_questions(&statement.acceptance),
        terms: wire::rooms::shown_notice_terms(&notice.terms)
            .into_iter()
            .map(str::to_owned)
            .collect(),
        stance: notice.stance,
        asks,
    }
}

/// Refuse an invitation whose target session, supporting source, Design,
/// room standing, or competing request moved.
fn still_owed(read: &JournalRead<'_>, request: &PlannedRequest) -> Result<(), GuardError> {
    let session_declared = current_context(read, request)?;
    supporting_source(read, request)?;
    current_design(read, request, session_declared)?;
    unseen_request(read, request)
}

fn current_context(read: &JournalRead<'_>, request: &PlannedRequest) -> Result<Seq, GuardError> {
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
    let mut occupant_sessions = BTreeMap::new();
    let mut current_session = None;
    let mut session_declared = None;
    let mut compaction = None;
    loop {
        let page = read.selected_fact_page(
            &[
                wire::FactKind::LaneSessionDeclared,
                wire::FactKind::LaneCompacted,
            ],
            after,
            None,
            journal::FactReadBudget::RETAINED,
        )?;
        for envelope in &page.rows {
            if let Some(declared) = envelope.fact.read::<wire::body::LaneSessionDeclared>() {
                let session = DesignSession {
                    vendor: declared.vendor,
                    identity: declared.identity,
                };
                occupant_sessions.insert((declared.lane, declared.occupant), session.clone());
                if declared.lane == request.lane && session == request.session {
                    session_declared.get_or_insert(envelope.seq);
                }
                if declared.lane == request.lane && declared.occupant == occupant {
                    current_session = Some(session);
                }
                continue;
            }
            let Some(compacted) = envelope.fact.read::<wire::body::LaneCompacted>() else {
                return Err(GuardError::Invalid(
                    "a selected context fact did not decode".to_owned(),
                ));
            };
            if compacted.lane == request.lane
                && occupant_sessions.get(&(compacted.lane, compacted.occupant))
                    == Some(&request.session)
            {
                compaction = Some(envelope.seq);
            }
        }
        let Some(continuation) = page.continuation else {
            break;
        };
        after = Some(continuation);
    }
    if current_session.as_ref() != Some(&request.session) {
        return Err(stale("the target vendor session was replaced"));
    }
    if compaction != request.compaction {
        return Err(stale("the target session context was compacted"));
    }
    session_declared.ok_or_else(|| stale("the target vendor session has no declaration"))
}

fn evidence_source_is_current(
    read: &JournalRead<'_>,
    lane: CanonicalUuid,
    session: &DesignSession,
    evidence: &DesignNoticeEvidence,
) -> Result<bool, GuardError> {
    match evidence {
        DesignNoticeEvidence::Task {
            task,
            statement,
            completed,
            ..
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
            Ok(statement_exists && completion_exists)
        }
        DesignNoticeEvidence::TaskClaim {
            task,
            statement,
            claim,
            terminal,
            ..
        } => {
            let statement_exists = read
                .causal_page(*statement, journal::FactReadBudget::RETAINED)?
                .is_some_and(|page| {
                    page.rows.iter().any(|row| {
                        row.seq == *statement
                            && row.fact.revised_tasks().any(|revised| revised == task)
                    })
                });
            let claim_exists = match Seq::new(claim.get()) {
                Some(claim) => read
                    .causal_page(claim, journal::FactReadBudget::RETAINED)?
                    .is_some_and(|page| {
                        page.rows.iter().any(|row| {
                            row.seq == claim
                                && row.fact.task_subject() == Some(task)
                                && matches!(
                                    row.fact.kind(),
                                    wire::FactKind::ClaimWon
                                        | wire::FactKind::ClaimCarried
                                        | wire::FactKind::ClaimRebound
                                )
                        })
                    }),
                None => false,
            };
            let terminal_exists = match terminal {
                Some(terminal) => read
                    .causal_page(*terminal, journal::FactReadBudget::RETAINED)?
                    .is_some_and(|page| {
                        page.rows.iter().any(|row| {
                            row.seq == *terminal
                                && row.fact.read::<wire::body::TaskDone>().is_some_and(|done| {
                                    done.task == *task && done.terminal.is_some()
                                })
                        })
                    }),
                None => true,
            };
            Ok(statement_exists && claim_exists && terminal_exists)
        }
        DesignNoticeEvidence::Conversation { source } => {
            let source_exists = read.transcript_source_heads(&lane)?.iter().any(|head| {
                head.body().vendor == session.vendor
                    && Some(head.body().source) == source.locus.source
            });
            Ok(source_exists)
        }
    }
}

fn supporting_source(read: &JournalRead<'_>, request: &PlannedRequest) -> Result<(), GuardError> {
    if evidence_source_is_current(read, request.lane, &request.session, &request.evidence)? {
        return Ok(());
    }
    let evidence = match request.evidence {
        DesignNoticeEvidence::Conversation { .. } => "conversation",
        DesignNoticeEvidence::Task { .. } | DesignNoticeEvidence::TaskClaim { .. } => "task",
    };
    Err(GuardError::refused(format!(
        "{STALE}: the supporting {evidence} evidence was lost or replaced"
    )))
}

fn current_design(
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
    if answered_in_session(read, request, session_declared)? {
        return Err(stale("the session already answered this ask"));
    }
    Ok(())
}

/// Whether the lane answered this ask since its session was declared: any
/// room record for a whole-Design ask, else a contribution against any asked
/// item or a leave of the room.
fn answered_in_session(
    read: &JournalRead<'_>,
    request: &PlannedRequest,
    session_declared: Seq,
) -> Result<bool, GuardError> {
    let published = read
        .latest_room_publication(&request.design, &Actor::Lane(request.lane))?
        .is_some_and(|head| head.seq > session_declared);
    if !published || request.asks.is_empty() {
        return Ok(published);
    }
    let mut after = Some(session_declared);
    loop {
        let page =
            read.room_publications(&request.design, after, journal::FactReadBudget::RETAINED)?;
        for envelope in &page.rows {
            let answered = match envelope.fact.kind() {
                wire::FactKind::RoomContributed => {
                    envelope.fact.read::<RoomContributed>().map(|contributed| {
                        contributed.lane == request.lane
                            && contributed.item.as_ref().is_some_and(|item| {
                                request.asks.iter().any(|ask| &ask.item == item)
                            })
                    })
                }
                wire::FactKind::RoomLeft => envelope
                    .fact
                    .read::<RoomLeft>()
                    .map(|left| left.lane == request.lane),
                _ => Some(false),
            }
            .ok_or_else(|| GuardError::Invalid("a room answer did not decode".to_owned()))?;
            if answered {
                return Ok(true);
            }
        }
        let Some(continuation) = page.continuation else {
            return Ok(false);
        };
        after = Some(continuation);
    }
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
                compaction: request.compaction,
                opportunity: request.opportunity.clone(),
                evidence: request.evidence.clone(),
                excerpt: request.excerpt.clone(),
                terms: request.terms.clone(),
                stance: request.stance,
                asks: request.asks.clone(),
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
