//! Durable peer-review feedback delivered to the still-running phase author.

use std::collections::BTreeMap;
use std::time::Duration;

use journal::FactCause;
use wire::{Actor, CanonicalUuid, Seq, TaskStamp};

use super::*;

#[derive(Clone, Copy)]
struct ClaimAuthor {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
}

/// Who completed the phase one finding judged, and who judged it.
struct PhaseAuthors {
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

pub(super) fn reconcile(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    snapshot: &bus::TaskSnapshot,
    cursor: Option<Seq>,
    lifetime: Duration,
) -> CommandResult<bool> {
    let mut refreshed = None;
    let mut changed = false;
    loop {
        let observed = refreshed.as_ref().unwrap_or(snapshot);
        let findings = pending_findings(observed.facts(), cursor)?;
        let mut appended = false;
        for envelope in observed.facts().after(cursor) {
            let Some(reviewed) = envelope.fact.read::<wire::body::TaskReviewed>() else {
                continue;
            };
            let Some(authors) = findings.get(&envelope.seq) else {
                continue;
            };
            let Some(feedback) = feedback(observed, envelope, &reviewed, authors)? else {
                continue;
            };
            let control = wire::review_feedback_control(&feedback.task, feedback.reviewed);
            let directive = identity(&control);
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
            return Ok(changed);
        }
        refreshed = Some(bus.task_snapshot().map_err(|error| {
            store_fault("review-feedback::snapshot-unreadable", &error.to_string())
        })?);
    }
}

fn feedback(
    snapshot: &bus::TaskSnapshot,
    envelope: &wire::Envelope<wire::AnyFact>,
    reviewed: &wire::body::TaskReviewed,
    authors: &PhaseAuthors,
) -> CommandResult<Option<Feedback>> {
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
    if author.occupant == reviewer.occupant {
        return Ok(None);
    }
    let Some(residency) = snapshot.residency().cloned() else {
        return Ok(None);
    };
    let Some(target) = residency.body().lanes.iter().find(|lane| {
        lane.disposition == "occupied"
            && lane.occupant == Some(author.occupant)
            && lane
                .session
                .as_ref()
                .is_some_and(|session| session.occupant == author.occupant)
    }) else {
        return Ok(None);
    };
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
    Ok(Some(Feedback {
        reviewed: envelope.seq,
        task: reviewed.task.clone(),
        lane: target.lane,
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

/// Every finding after the cursor, with the claim authors it judged.
///
/// Authorship is folded once, forward across the task history, rather than
/// once per finding: a resident that starts against years of reviews would
/// otherwise decode the whole history again for each of them.
fn pending_findings(
    facts: &bus::TaskFacts,
    cursor: Option<Seq>,
) -> CommandResult<BTreeMap<Seq, PhaseAuthors>> {
    let mut wanted = BTreeMap::new();
    for envelope in facts.after(cursor) {
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
            wanted.insert(reviewed.task, envelope.seq);
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
                if !finding || cursor.is_some_and(|cursor| envelope.seq <= cursor) {
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
        "[REVIEW] Peer feedback for {handle}:\n\n{quoted}\n\n\
         🌶️ Verbalize any noteworthy lessons and internalize this as guidance—any follow-ups \
         it raises are already on the board, so there is nothing to capture or act on now! \
         Keep working your current task; do not re-file what the system already filed."
    )
}

fn identity(control: &str) -> CanonicalUuid {
    let digest = blake3::hash(format!("spice:{control}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    CanonicalUuid::new(uuid::Uuid::from_bytes(bytes))
}

#[cfg(test)]
mod tests;
