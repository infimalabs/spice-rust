//! Durable claimant steering derived from task generation advances.
//!
//! The journal records each transaction that changes a task after creation in
//! its generation index. Every index row newer than an active claim's
//! acknowledgement becomes one resident notice to the claimant lane, bound to
//! the claim that trails it. A claimant's own changes need no filter: the
//! journal keeps a current holder's acknowledgement current, so they never
//! trail it.
//!
//! Notices are derived from current state on every pass, so a publication that
//! fails leaves the change journaled and a later pass delivers it. A notice
//! stays pending until its claim acknowledges that generation by reclaiming,
//! ends, or is replaced; the resident then cancels it. Answering its key never
//! retires it.

use std::collections::BTreeSet;
use std::time::Duration;

use journal::{JournalGuard, JournalHead, TaskHeads};
use wire::{ClaimHead, Disposition, TaskHandle};

use super::*;

/// Index rows one read returns while a claim trails its task.
const ADVANCE_PAGE: usize = 64;
const CLAIM_MOVED: &str = "task-generation::claim-moved";

/// The (task, claim, generation) a notice control names; its whole identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::resident) struct NoticeAuthority {
    pub(in crate::resident) task: TaskStamp,
    pub(in crate::resident) claim: Seq,
    pub(in crate::resident) generation: u64,
}

impl NoticeAuthority {
    fn control(&self) -> String {
        format!(
            "{}{}:{}:{}",
            spice::session_meter::TASK_GENERATION_CONTROL_PREFIX,
            self.task,
            self.claim,
            self.generation
        )
    }

    /// Whether `claim` is still the claim this notice names on `lane`, and
    /// still trails its generation.
    fn pending_under(&self, lane: CanonicalUuid, claim: &ClaimHead) -> bool {
        claim.disposition == ClaimDisposition::Active
            && claim.lane == lane
            && claim.claimed == self.claim
            && claim.acknowledged < self.generation
    }
}

pub(super) fn reconcile(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    snapshot: &bus::TaskSnapshot,
    cursor: Option<Seq>,
    lifetime: Duration,
) -> CommandResult<bool> {
    let mut changed = retire(bus, paths, epoch, snapshot)?;
    for heads in snapshot.tasks() {
        changed |= publish_trailing(bus, paths, epoch, snapshot.facts(), heads, lifetime)?;
    }
    let refreshed = if changed {
        Some(bus.task_snapshot().map_err(|error| {
            store_fault("task-generation::snapshot-unreadable", &error.to_string())
        })?)
    } else {
        None
    };
    let observed = refreshed.as_ref().unwrap_or(snapshot);
    let feedback =
        super::review_feedback::reconcile(bus, paths, epoch, observed, cursor, lifetime)?;
    Ok(changed || feedback)
}

/// Cancel every live notice whose claim acknowledged it, ended, or was replaced.
fn retire(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    snapshot: &bus::TaskSnapshot,
) -> CommandResult<bool> {
    let pending = bus
        .read(|read| crate::steering_read::pending(read, None))
        .map_err(|error| store_fault("steer::index-unreadable", &error.to_string()))?;
    let mut lanes = BTreeSet::new();
    for head in &pending {
        let body = head.body();
        let Some(authority) = notice_authority(body.control.as_deref())
            .map_err(|error| store_fault("task-generation::control-invalid", &error))?
        else {
            continue;
        };
        if task_heads(snapshot, &authority.task)
            .and_then(TaskHeads::claim)
            .is_some_and(|claim| authority.pending_under(body.lane, claim.body()))
        {
            continue;
        }
        let (mut proposal, published) = super::super::steering::internal_write(epoch)?;
        if super::super::steering::append_control_cancellation(
            bus,
            &mut proposal,
            body.directive,
            body.lane,
            &authority.control(),
            published,
        )?
        .is_some()
        {
            commit(bus, proposal)?;
            lanes.insert(body.lane);
        }
    }
    if lanes.is_empty() {
        return Ok(false);
    }
    for lane in lanes {
        super::super::steering::notify_inbox(paths, lane);
    }
    Ok(true)
}

/// Publish every unpublished notice for one active claim trailing its task.
fn publish_trailing(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    facts: &bus::TaskFacts,
    heads: &TaskHeads,
    lifetime: Duration,
) -> CommandResult<bool> {
    let Some(claim) = heads
        .claim()
        .map(JournalHead::body)
        .filter(|claim| claim.disposition == ClaimDisposition::Active)
    else {
        return Ok(false);
    };
    if claim.acknowledged >= heads.workflow().body().generation {
        return Ok(false);
    }
    let statement = heads.statement().body();
    let handle = TaskHandle::for_project(&statement.project, &statement.born);
    let mut after = claim.acknowledged;
    let mut changed = false;
    loop {
        let task = statement.born.clone();
        let advances = bus
            .read(move |read| read.task_generations_after(&task, after, ADVANCE_PAGE))
            .map_err(|error| {
                store_fault("task-generation::index-unreadable", &error.to_string())
            })?;
        for advance in &advances {
            let authority = NoticeAuthority {
                task: statement.born.clone(),
                claim: claim.claimed,
                generation: advance.generation,
            };
            let text = text(&handle, &authority, advance.position, facts);
            changed |= publish(bus, paths, epoch, claim.lane, &authority, text, lifetime)?;
        }
        match advances.last() {
            Some(last) if advances.len() == ADVANCE_PAGE => after = last.generation,
            _ => return Ok(changed),
        }
    }
}

fn publish(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    authority: &NoticeAuthority,
    text: String,
    lifetime: Duration,
) -> CommandResult<bool> {
    let control = authority.control();
    let directive = identity(&control);
    let standing = super::super::steering::control_standing(bus, directive, lane, &control)?;
    let (mut proposal, published) = super::super::steering::internal_write(epoch)?;
    proposal.guard(claim_guard(authority.clone(), lane));
    match standing {
        None => {
            let notice = super::super::steering::InternalDirective {
                directive,
                lane,
                text,
                control,
                lifetime,
                cause: FactCause::Root,
                published,
            };
            super::super::steering::append_internal_directive(bus, &mut proposal, epoch, notice)?;
        }
        // Expiry is not retirement: a notice outlives its lifetime while its
        // claim still trails the generation it names.
        Some(Disposition::Lapsed | Disposition::DeadLettered) => {
            super::super::steering::append_control_recovery(
                bus,
                &mut proposal,
                directive,
                published,
                lifetime,
            )?;
        }
        Some(_) => return Ok(false),
    }
    match commit(bus, proposal) {
        Ok(_) => {
            super::super::steering::notify_inbox(paths, lane);
        }
        Err(Outcome::Refusal(refusal)) if refusal.reason() == CLAIM_MOVED => {}
        Err(outcome) => return Err(outcome),
    }
    Ok(true)
}

/// Whether a steering control names a task generation notice, which only its
/// claim's reclaim, end, or replacement retires.
pub(in crate::resident) fn is_notice(control: Option<&str>) -> bool {
    control.is_some_and(|control| {
        control.starts_with(spice::session_meter::TASK_GENERATION_CONTROL_PREFIX)
    })
}

/// Refuse a notice once its claim stopped trailing the generation it names.
fn claim_guard(authority: NoticeAuthority, lane: CanonicalUuid) -> JournalGuard {
    JournalGuard::new(move |read| {
        if read
            .claim_head(&authority.task)?
            .is_some_and(|head| authority.pending_under(lane, head.body()))
        {
            Ok(())
        } else {
            Err(journal::GuardError::refused(CLAIM_MOVED))
        }
    })
}

fn task_heads<'a>(snapshot: &'a bus::TaskSnapshot, task: &TaskStamp) -> Option<&'a TaskHeads> {
    snapshot
        .tasks()
        .binary_search_by(|heads| heads.statement().body().born.cmp(task))
        .ok()
        .map(|index| &snapshot.tasks()[index])
}

/// The notice authority a steering control names, if it names one.
pub(in crate::resident) fn notice_authority(
    control: Option<&str>,
) -> Result<Option<NoticeAuthority>, String> {
    let Some(encoded) = control.and_then(|control| {
        control.strip_prefix(spice::session_meter::TASK_GENERATION_CONTROL_PREFIX)
    }) else {
        return Ok(None);
    };
    let mut fields = encoded.split(':');
    let task = fields
        .next()
        .ok_or_else(|| "task-generation control omitted its task".to_owned())?
        .parse()
        .map_err(|error| format!("task-generation control has an invalid task: {error}"))?;
    let claim = fields
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(Seq::new)
        .ok_or_else(|| "task-generation control has an invalid claim position".to_owned())?;
    let generation = fields
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|generation| *generation > 0)
        .ok_or_else(|| "task-generation control has an invalid generation".to_owned())?;
    if fields.next().is_some() {
        return Err("task-generation control carries trailing fields".to_owned());
    }
    Ok(Some(NoticeAuthority {
        task,
        claim,
        generation,
    }))
}

fn identity(control: &str) -> CanonicalUuid {
    let digest = blake3::hash(format!("spice:{control}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    CanonicalUuid::new(uuid::Uuid::from_bytes(bytes))
}

fn text(
    handle: &TaskHandle,
    authority: &NoticeAuthority,
    position: Seq,
    facts: &bus::TaskFacts,
) -> String {
    let change = facts.at(position).map_or_else(
        || format!("the change at position {position}"),
        |envelope| {
            format!(
                "{} by {} at position {position}",
                envelope.fact.kind(),
                envelope.actor
            )
        },
    );
    format!(
        "Task {handle} changed after this claim last acknowledged it: generation {} is {change}. \
         Run `spice task reclaim {handle}` to take the current task before continuing. \
         Answering this key does not retire the notice, and completion and review stay \
         refused until the reclaim.",
        authority.generation,
    )
}

#[cfg(test)]
mod tests;
