//! Durable peer-review feedback delivered to the still-running phase author.

use std::time::Duration;

use journal::FactCause;
use wire::{Actor, CanonicalUuid, Seq, TaskStamp};

use super::*;

/// Prefix identifying resident-authored peer-review feedback directives.
pub(super) const CONTROL_PREFIX: &str = "review-feedback:";

struct ClaimAuthor {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
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
        let mut appended = false;
        for envelope in observed.facts().after(cursor) {
            let Some(reviewed) = envelope.fact.read::<wire::body::TaskReviewed>() else {
                continue;
            };
            let Some(feedback) = feedback(observed, envelope, &reviewed)? else {
                continue;
            };
            let control = control(&feedback.task, feedback.reviewed);
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
                    let residency = current_residency(bus, epoch)?;
                    super::super::steering::publish_inbox(
                        bus,
                        paths,
                        residency.body(),
                        feedback.lane,
                    )?;
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
) -> CommandResult<Option<Feedback>> {
    if reviewed.reviewer != envelope.actor {
        return Err(store_fault(
            "review-feedback::reviewer-mismatched",
            "the review body and authenticated fact actor disagree",
        ));
    }
    if reviewed.verdict == wire::body::Verdict::Clean {
        return Ok(None);
    }
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
    let (author, reviewer) = phase_authors(snapshot.facts(), &reviewed.task, envelope.seq)?;
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

fn phase_authors(
    facts: &bus::TaskFacts,
    task: &TaskStamp,
    reviewed: Seq,
) -> CommandResult<(ClaimAuthor, ClaimAuthor)> {
    let mut active = None;
    let mut completed = None;
    for envelope in facts.iter().take_while(|envelope| envelope.seq < reviewed) {
        if let Some(won) = envelope.fact.read::<wire::body::ClaimWon>()
            && &won.task == task
        {
            active = Some(ClaimAuthor {
                lane: won.lane,
                occupant: won.occupant,
            });
        } else if let Some(carried) = envelope.fact.read::<wire::body::ClaimCarried>()
            && &carried.task == task
        {
            active = Some(ClaimAuthor {
                lane: carried.lane,
                occupant: carried.occupant,
            });
        } else if let Some(rebound) = envelope.fact.read::<wire::body::ClaimRebound>()
            && &rebound.task == task
        {
            active = Some(ClaimAuthor {
                lane: rebound.lane,
                occupant: rebound.occupant,
            });
        } else if (envelope.fact.read::<wire::body::ClaimFreed>())
            .is_some_and(|freed| &freed.task == task)
            || (envelope.fact.read::<wire::body::ClaimLapsed>())
                .is_some_and(|lapsed| &lapsed.task == task)
        {
            active = None;
        } else if (envelope.fact.read::<wire::body::TaskDone>())
            .is_some_and(|done| &done.task == task)
        {
            let author = active.take().ok_or_else(|| {
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
            completed = Some(author);
        } else if (envelope.fact.read::<wire::body::TaskReviewed>())
            .is_some_and(|prior| &prior.task == task)
        {
            active = None;
        }
    }
    let author = completed.ok_or_else(|| {
        store_fault(
            "review-feedback::author-missing",
            "review has no preceding completed phase author",
        )
    })?;
    let reviewer = active.ok_or_else(|| {
        store_fault(
            "review-feedback::reviewer-missing",
            "review has no preceding active reviewer claim",
        )
    })?;
    Ok((author, reviewer))
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

fn control(task: &TaskStamp, reviewed: Seq) -> String {
    format!("{CONTROL_PREFIX}{task}:{reviewed}")
}

fn identity(control: &str) -> CanonicalUuid {
    let digest = blake3::hash(format!("spice:{control}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    CanonicalUuid::new(uuid::Uuid::from_bytes(bytes))
}

#[cfg(test)]
mod tests;
