//! Durable peer-review feedback delivered to the phase author's vendor session.
//!
//! A finding reaches the lane seat that continues the session which completed
//! the judged phase: the same occupant, or a later occupant that resumed the
//! same vendor session after a seat boundary or compaction. While the author
//! lane is between seats the finding is held and judged again on the next
//! reconcile; a renewal that starts another session never receives it.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use journal::FactCause;
use wire::rooms::DesignSession;
use wire::{Actor, CanonicalUuid, Instant, LaneResidency, Seq, TaskStamp};

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ClaimAuthor {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
}

/// Who completed the phase one finding judged, and who judged it. Fixed once
/// the finding is recorded, because history is append-only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PhaseAuthors {
    author: ClaimAuthor,
    reviewer: ClaimAuthor,
}

struct Feedback {
    reviewed: Seq,
    task: TaskStamp,
    lane: CanonicalUuid,
    text: String,
    residency: journal::JournalHead<wire::ResidencyHead>,
}

/// What one finding's author can receive at this reconcile.
enum Judgment {
    /// A seat continuing the author's session is live.
    Deliver(Feedback),
    /// The author lane is between seats; judge the finding again later.
    Hold,
    /// Never deliverable: judged by its own author's session, continued by no
    /// seat because another session holds the lane, or past its lifetime.
    Drop,
}

/// Which findings one reconcile judges, and the clock and directive lifetime
/// it judges them by.
pub(super) struct FindingWindow<'a> {
    /// Lifecycle projection cursor; every finding after it is new.
    pub(super) cursor: Option<Seq>,
    /// Findings still waiting for a seat, with the authors folded when they
    /// were first judged; replaced by those that still wait.
    pub(super) held: &'a mut BTreeMap<Seq, PhaseAuthors>,
    pub(super) now: Instant,
    pub(super) lifetime: Duration,
}

/// Deliver every finding after the window's cursor or held for a seat whose
/// author session is seated, and leave held exactly the findings still
/// waiting for a seat. Held findings are judged from their held authors; only
/// findings after the cursor that are not held fold task history.
pub(super) fn reconcile(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    snapshot: &bus::TaskSnapshot,
    window: FindingWindow<'_>,
) -> CommandResult<bool> {
    let FindingWindow {
        cursor,
        held,
        now,
        lifetime,
    } = window;
    let mut refreshed = None;
    let mut changed = false;
    loop {
        let observed = refreshed.as_ref().unwrap_or(snapshot);
        let mut findings = pending_findings(observed.facts(), cursor, held)?;
        findings.extend(held.iter().map(|(seq, authors)| (*seq, *authors)));
        let mut waiting = BTreeMap::new();
        let mut appended = false;
        for (seq, authors) in &findings {
            let envelope = observed.facts().at(*seq).ok_or_else(|| {
                store_fault(
                    "review-feedback::finding-missing",
                    "a folded finding has no fact in the snapshot it was folded from",
                )
            })?;
            let Some(reviewed) = envelope.fact.read::<wire::body::TaskReviewed>() else {
                continue;
            };
            let feedback = match judge(observed, envelope, &reviewed, authors, now, lifetime)? {
                Judgment::Deliver(feedback) => feedback,
                Judgment::Hold => {
                    waiting.insert(*seq, *authors);
                    continue;
                }
                Judgment::Drop => continue,
            };
            let control = wire::review_feedback_control(&feedback.task, feedback.reviewed);
            let directive = super::super::steering::internal_directive_identity(&control);
            if super::super::steering::control_standing(bus, directive, feedback.lane, &control)?
                .is_some()
            {
                continue;
            }
            let (mut proposal, published) = super::super::steering::internal_write(epoch)?;
            guard_residency(&mut proposal, &feedback.residency);
            super::super::steering::append_internal_directive(
                bus,
                &mut proposal,
                epoch,
                super::super::steering::InternalDirective {
                    directive,
                    lane: feedback.lane,
                    text: feedback.text,
                    control,
                    lifetime,
                    cause: FactCause::Root,
                    published,
                },
            )?;
            match commit(bus, proposal) {
                Ok(_) => {
                    super::super::steering::notify_inbox(paths, feedback.lane);
                    changed = true;
                }
                Err(Outcome::Refusal(refusal)) if refusal.reason() == "resident::head-moved" => {}
                Err(outcome) => return Err(outcome),
            }
            appended = true;
            break;
        }
        if !appended {
            *held = waiting;
            return Ok(changed);
        }
        refreshed = Some(bus.task_snapshot().map_err(|error| {
            store_fault("review-feedback::snapshot-unreadable", &error.to_string())
        })?);
    }
}

fn judge(
    snapshot: &bus::TaskSnapshot,
    envelope: &wire::Envelope<wire::AnyFact>,
    reviewed: &wire::body::TaskReviewed,
    authors: &PhaseAuthors,
    now: Instant,
    lifetime: Duration,
) -> CommandResult<Judgment> {
    let finding = reviewed
        .finding
        .as_deref()
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| {
            store_fault(
                "review-feedback::finding-missing",
                "a finding review carries no final disposition",
            )
        })?;
    let PhaseAuthors { author, reviewer } = *authors;
    let author_session = snapshot.facts().lane_session(author.lane, author.occupant);
    if same_session(snapshot, author, author_session, reviewer) {
        return Ok(Judgment::Drop);
    }
    if outlived(envelope.at, now, lifetime) {
        return Ok(Judgment::Drop);
    }
    let Some(residency) = snapshot.residency().cloned() else {
        return Ok(Judgment::Hold);
    };
    let Some(row) = residency
        .body()
        .lanes
        .iter()
        .find(|lane| lane.lane == author.lane)
    else {
        return Ok(Judgment::Drop);
    };
    let seated = row.disposition == "occupied"
        && row
            .session
            .as_ref()
            .is_some_and(|session| row.occupant == Some(session.occupant));
    if !seated {
        return Ok(Judgment::Hold);
    }
    if !continues(row, author, author_session) {
        return Ok(Judgment::Drop);
    }
    let target = row.lane;
    let heads = snapshot
        .tasks()
        .binary_search_by(|heads| heads.statement().body().born.cmp(&reviewed.task))
        .ok()
        .map(|index| &snapshot.tasks()[index])
        .ok_or_else(|| {
            store_fault(
                "review-feedback::task-missing",
                "the reviewed task has no current statement head",
            )
        })?;
    Ok(Judgment::Deliver(Feedback {
        reviewed: envelope.seq,
        task: reviewed.task.clone(),
        lane: target,
        text: feedback_text(
            &wire::TaskHandle::for_project(
                &heads.statement().body().project,
                &heads.statement().body().born,
            ),
            finding,
        ),
        residency,
    }))
}

/// Whether the seat on the author's lane `row` continues the vendor session
/// that completed the judged phase: the author's own occupant, or a later one
/// that resumed the same session. Widening delivery to every seat in the
/// lane's renewal lineage replaces this body with
/// `row.lineage.contains(&author.occupant)`.
fn continues(
    row: &LaneResidency,
    author: ClaimAuthor,
    author_session: Option<&DesignSession>,
) -> bool {
    row.session.as_ref().is_some_and(|seated| {
        seated.occupant == author.occupant
            || author_session.is_some_and(|session| {
                session.vendor == seated.vendor && session.identity == seated.identity
            })
    })
}

/// Whether `reviewer` judged from the same vendor session as `author`.
fn same_session(
    snapshot: &bus::TaskSnapshot,
    author: ClaimAuthor,
    author_session: Option<&DesignSession>,
    reviewer: ClaimAuthor,
) -> bool {
    author.occupant == reviewer.occupant
        || (author.lane == reviewer.lane
            && author_session.is_some_and(|session| {
                snapshot
                    .facts()
                    .lane_session(reviewer.lane, reviewer.occupant)
                    == Some(session)
            }))
}

/// Whether a finding recorded `at` is past the directive lifetime it would be
/// delivered with.
fn outlived(at: Instant, now: Instant, lifetime: Duration) -> bool {
    let age = now.get().duration_since(at.get());
    age.is_positive() && age.unsigned_abs() >= lifetime
}

/// Every finding after the cursor that is not already held, with the claim
/// authors it judged.
///
/// Authorship is folded once, forward across the task history, rather than
/// once per finding: a resident that starts against years of reviews would
/// otherwise decode the whole history again for each of them. A reconcile
/// whose only candidates are held findings folds nothing.
fn pending_findings(
    facts: &bus::TaskFacts,
    cursor: Option<Seq>,
    held: &BTreeMap<Seq, PhaseAuthors>,
) -> CommandResult<BTreeMap<Seq, PhaseAuthors>> {
    let mut wanted = BTreeMap::new();
    let mut candidates = BTreeSet::new();
    for envelope in facts.after(cursor) {
        if held.contains_key(&envelope.seq) {
            continue;
        }
        let Some(reviewed) = envelope.fact.read::<wire::body::TaskReviewed>() else {
            continue;
        };
        if reviewed.reviewer != envelope.actor {
            return Err(store_fault(
                "review-feedback::reviewer-mismatched",
                "the review body and authenticated fact actor disagree",
            ));
        }
        if reviewed.verdict == wire::body::Verdict::Finding {
            candidates.insert(envelope.seq);
            let newest = wanted.entry(reviewed.task).or_insert(envelope.seq);
            *newest = (*newest).max(envelope.seq);
        }
    }
    let Some(newest) = wanted.values().max().copied() else {
        return Ok(BTreeMap::new());
    };
    let mut active = BTreeMap::new();
    let mut completed = BTreeMap::new();
    let mut findings = BTreeMap::new();
    for envelope in facts.iter().take_while(|envelope| envelope.seq <= newest) {
        let Some((task, step)) = phase_step(envelope) else {
            continue;
        };
        if !wanted.get(&task).is_some_and(|last| envelope.seq <= *last) {
            continue;
        }
        match step {
            PhaseStep::Claimed(author) => {
                active.insert(task, author);
            }
            PhaseStep::Released => {
                active.remove(&task);
            }
            PhaseStep::Done => {
                let author = active.remove(&task).ok_or_else(|| {
                    store_fault(
                        "review-feedback::author-missing",
                        "task completion has no preceding active claim author",
                    )
                })?;
                if envelope.actor != Actor::Lane(author.lane) {
                    return Err(store_fault(
                        "review-feedback::author-mismatched",
                        "task completion actor disagrees with its active claim lane",
                    ));
                }
                completed.insert(task, author);
            }
            PhaseStep::Reviewed(finding) => {
                let reviewer = active.remove(&task);
                if !finding || !candidates.contains(&envelope.seq) {
                    continue;
                }
                let author = completed.get(&task).copied().ok_or_else(|| {
                    store_fault(
                        "review-feedback::author-missing",
                        "review has no preceding completed phase author",
                    )
                })?;
                let reviewer = reviewer.ok_or_else(|| {
                    store_fault(
                        "review-feedback::reviewer-missing",
                        "review has no preceding active reviewer claim",
                    )
                })?;
                findings.insert(envelope.seq, PhaseAuthors { author, reviewer });
            }
        }
    }
    Ok(findings)
}

/// What one fact does to a task's phase authorship.
enum PhaseStep {
    Claimed(ClaimAuthor),
    Released,
    Done,
    Reviewed(bool),
}

fn phase_step(envelope: &wire::Envelope<wire::AnyFact>) -> Option<(TaskStamp, PhaseStep)> {
    let fact = &envelope.fact;
    if let Some(won) = fact.read::<wire::body::ClaimWon>() {
        let author = ClaimAuthor {
            lane: won.lane,
            occupant: won.occupant,
        };
        Some((won.task, PhaseStep::Claimed(author)))
    } else if let Some(carried) = fact.read::<wire::body::ClaimCarried>() {
        let author = ClaimAuthor {
            lane: carried.lane,
            occupant: carried.occupant,
        };
        Some((carried.task, PhaseStep::Claimed(author)))
    } else if let Some(rebound) = fact.read::<wire::body::ClaimRebound>() {
        let author = ClaimAuthor {
            lane: rebound.lane,
            occupant: rebound.occupant,
        };
        Some((rebound.task, PhaseStep::Claimed(author)))
    } else if let Some(freed) = fact.read::<wire::body::ClaimFreed>() {
        Some((freed.task, PhaseStep::Released))
    } else if let Some(lapsed) = fact.read::<wire::body::ClaimLapsed>() {
        Some((lapsed.task, PhaseStep::Released))
    } else if let Some(done) = fact.read::<wire::body::TaskDone>() {
        Some((done.task, PhaseStep::Done))
    } else {
        let reviewed = fact.read::<wire::body::TaskReviewed>()?;
        let finding = reviewed.verdict == wire::body::Verdict::Finding;
        Some((reviewed.task, PhaseStep::Reviewed(finding)))
    }
}

fn feedback_text(handle: &wire::TaskHandle, finding: &str) -> String {
    let quoted = finding
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{handle}\n\n{quoted}\n\n\
         Verbalize and internalize all noteworthy findings as working guidance—there is \
         nothing else to capture or act on because follow-ups are already boarded!—continue \
         working your current task."
    )
}

#[cfg(test)]
mod tests;
