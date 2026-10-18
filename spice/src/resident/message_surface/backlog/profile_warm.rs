//! Bounded warming of continuing-session evidence profiles from admitted
//! history.
//!
//! Warming walks two histories newest first: one completion page shared by
//! every owing profile, and per profile, older windows of its lane's tape.
//! Startup takes the first step; each later commit turn takes at most one more
//! step of each, resuming from the recorded cursors, until a profile finds both
//! beginnings or already holds its limit of newer cards in a class. Reads run
//! outside the message-state lock; only folding the found cards holds it.

use std::collections::BTreeSet;

use spice::rooms::evidence::{Conversation, Evidence};
use spice::rooms::profile::{EvidenceKey, SessionKey, SessionProfiles, TranscriptCursor};

use super::*;

/// Completion facts one task-history step reads.
const TASK_ROWS: usize = 256;
/// Encoded completion bytes one task-history step reads.
const TASK_BYTES: usize = 1024 * 1024;

/// Completion evidence one task-history page found, newest first.
pub(super) struct TaskPage {
    cards: Vec<(SessionKey, Evidence<EvidenceKey>)>,
    next: Option<wire::Seq>,
    complete: bool,
}

/// Conversation evidence one tape window found for one profile, newest first.
pub(super) struct TranscriptWindow {
    key: SessionKey,
    cards: Vec<Evidence<EvidenceKey>>,
    crossed: TranscriptCursor,
    complete: bool,
}

/// Take at most one further completion page and one older tape window for
/// profiles that still owe history.
pub(in crate::resident) fn advance(
    bus: &bus::BusReader,
    paths: &RuntimePaths,
    state: &Mutex<MessageState>,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let (facts, transcript) = {
        let mut state = lock_state(state)?;
        state.profiles.settle_full();
        let facts = state
            .profiles
            .owed_facts()
            .map(|before| (before, sessions(&state.profiles)));
        let transcript = state
            .profiles
            .owed_transcript()
            .map(|(key, cursor)| (key.clone(), cursor));
        (facts, transcript)
    };
    if let Some((before, sessions)) = facts {
        let page = read_task_page(bus, before, &sessions);
        let mut state = lock_state(state)?;
        match page {
            Ok(page) => fold_task_page(&mut state.profiles, page),
            Err(error) => fail_task_history(&mut state.profiles, &error, diagnostic),
        }
    }
    if let Some((key, cursor)) = transcript {
        let window = read_older_window(bus, paths, &key, cursor);
        let mut state = lock_state(state)?;
        match window {
            Ok(window) => fold_transcript_window(&mut state.profiles, window),
            Err(detail) => {
                let _ = writeln!(
                    diagnostic,
                    "session profile transcript unavailable for lane {}: {detail}",
                    key.lane
                );
                state.profiles.warm_failed(&key, detail);
            }
        }
    }
    Ok(())
}

/// Every known continuing session, as completion ownership resolves them.
pub(super) fn sessions(profiles: &SessionProfiles) -> BTreeSet<SessionKey> {
    profiles.iter().map(|(key, _)| key.clone()).collect()
}

pub(super) fn fail_task_history(
    profiles: &mut SessionProfiles,
    error: &Outcome<()>,
    diagnostic: &mut dyn io::Write,
) {
    profiles.facts_failed(format!("task history: {error:?}"));
    let _ = writeln!(
        diagnostic,
        "session profile task history unavailable: {error:?}"
    );
}

/// Read one newest-first completion page strictly before `before`, attributing
/// each completion to the newest known session its source boundary names.
///
/// A task whose statement was revised after it completed is skipped: its card
/// must carry the statement it was observed with, not a later edit. So is a
/// completion whose terminal operation names no won claim, exactly as live
/// retention admits only claim-owned completions: the card's support is the
/// phase that claim held.
pub(super) fn read_task_page(
    bus: &bus::BusReader,
    before: Option<wire::Seq>,
    sessions: &BTreeSet<SessionKey>,
) -> CommandResult<TaskPage> {
    let budget = journal::FactReadBudget::new(TASK_ROWS, TASK_BYTES).map_err(|error| {
        store_fault(
            "room::profile-task-history-budget-invalid",
            &error.to_string(),
        )
    })?;
    let page = bus
        .recent_fact_page(wire::FactKind::TaskDone, before, budget)
        .map_err(|error| {
            store_fault("room::profile-task-history-unreadable", &error.to_string())
        })?;
    let snapshot = bus
        .task_snapshot()
        .map_err(|error| store_fault("room::profile-task-heads-unreadable", &error.to_string()))?;
    let mut cards = Vec::new();
    for envelope in page.rows {
        let completion = envelope.seq;
        let Some(done) = envelope.fact.read::<wire::body::TaskDone>() else {
            continue;
        };
        let Some(statement) = snapshot
            .task_heads(&done.task)
            .map(|heads| heads.statement().body())
        else {
            continue;
        };
        if statement.revision > completion {
            continue;
        }
        let Some(key) = completion_session(bus, &done.source, sessions)? else {
            continue;
        };
        let Some(support) = done
            .terminal
            .as_ref()
            .and_then(|terminal| wire::intent::TaskDone::claim_for_operation(terminal.operation))
            .and_then(|claim| snapshot.facts().at(claim))
            .and_then(|claimed| claimed.fact.read::<wire::body::ClaimWon>())
            .map(|won| wire::rooms::DesignSupport::for_phase(&won.cell.phase))
        else {
            continue;
        };
        cards.push((
            key,
            Evidence::task(
                EvidenceKey::Task {
                    task: done.task,
                    statement: statement.revision,
                    completion,
                    support,
                },
                &statement.title,
                statement.description.as_deref(),
                &statement.acceptance,
                &done.validation,
            ),
        ));
    }
    Ok(TaskPage {
        cards,
        next: page.continuation,
        complete: page.continuation.is_none(),
    })
}

/// The newest known session whose transcript reached one completion's proven
/// source boundary.
fn completion_session(
    bus: &bus::BusReader,
    source: &wire::body::SourceBoundary,
    sessions: &BTreeSet<SessionKey>,
) -> CommandResult<Option<SessionKey>> {
    let wire::body::SourceBoundary::Proven { heads } = source;
    for head in heads.iter().rev() {
        if head.next.offset == 0 {
            continue;
        }
        let session = bus
            .transcript_session(
                head.lane,
                head.source,
                head.next.member,
                head.next.offset - 1,
            )
            .map_err(|error| store_fault("room::profile-session-unreadable", &error.to_string()))?;
        let Some(session) = session else {
            continue;
        };
        let key = SessionKey::new(head.lane, head.vendor.clone(), session.identity);
        if sessions.contains(&key) {
            return Ok(Some(key));
        }
    }
    Ok(None)
}

pub(super) fn fold_task_page(profiles: &mut SessionProfiles, page: TaskPage) {
    for (key, card) in page.cards {
        profiles.retain_older(&key, card);
    }
    profiles.warmed_facts(page.next, page.complete);
}

/// Read the tape window just older than `cursor`, or the newest window when the
/// walk has not begun.
fn read_older_window(
    bus: &bus::BusReader,
    paths: &RuntimePaths,
    key: &SessionKey,
    cursor: Option<TranscriptCursor>,
) -> Result<TranscriptWindow, String> {
    let vendor = ingest_vendor(&key.vendor)
        .ok_or_else(|| format!("vendor {} has no transcript dialect", key.vendor))?;
    let tape = paths
        .tape
        .join(key.lane.to_string())
        .join(key.vendor.as_str());
    let tail = match cursor {
        None => listen::scan_tail(&tape, vendor.dialect(), startup_budget()),
        Some(cursor) => {
            let before = listen::Checkpoint {
                source: cursor
                    .source
                    .to_string()
                    .parse()
                    .map_err(|error| format!("{error}"))?,
                next: listen::Position {
                    member: cursor.member,
                    offset: cursor.offset,
                },
            };
            listen::scan_before(&tape, vendor.dialect(), &before, startup_budget())
        }
    }
    .map_err(|error| error.to_string())?;
    read_transcript_window(bus, key, &tail.scan, &tail.start).map_err(|error| format!("{error:?}"))
}

/// Collect the conversation evidence `key`'s own declared session wrote in one
/// scanned window, newest first.
pub(super) fn read_transcript_window(
    bus: &bus::BusReader,
    key: &SessionKey,
    scan: &listen::Scan,
    start: &listen::Checkpoint,
) -> CommandResult<TranscriptWindow> {
    let source = scan_source(scan)?;
    let mut cards = Vec::new();
    for record in scan.records.iter().rev() {
        let listen::RecordPayload::Events(events) = &record.payload else {
            continue;
        };
        for sourced in events.iter().rev() {
            let listen::Event::Prose(prose) = &sourced.event else {
                continue;
            };
            if prose.role != listen::ProseRole::Assistant {
                continue;
            }
            let Some(session) = bus
                .transcript_session(key.lane, source, prose.locus.member, prose.locus.offset)
                .map_err(|error| {
                    store_fault("room::profile-session-unreadable", &error.to_string())
                })?
            else {
                continue;
            };
            if session.vendor != key.vendor.as_str() || session.identity != key.identity {
                continue;
            }
            let Some(conversation) = Conversation::read(&prose.text) else {
                continue;
            };
            cards.push(Evidence::conversation(
                EvidenceKey::Conversation {
                    source,
                    member: prose.locus.member,
                    offset: prose.locus.offset,
                    length: prose.locus.length,
                },
                &conversation,
            ));
        }
    }
    Ok(TranscriptWindow {
        key: key.clone(),
        cards,
        crossed: TranscriptCursor {
            source,
            member: start.next.member,
            offset: start.next.offset,
        },
        complete: start.next == listen::Position::start(),
    })
}

pub(super) fn fold_transcript_window(profiles: &mut SessionProfiles, window: TranscriptWindow) {
    for card in window.cards {
        profiles.retain_older(&window.key, card);
    }
    profiles.warmed_transcript(&window.key, window.crossed, window.complete);
}
