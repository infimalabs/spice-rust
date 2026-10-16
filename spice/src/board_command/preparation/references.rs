//! Candidate references resolved and scope-checked at the command barrier.
//!
//! A candidate's authority closure is its controlling Design, its governed
//! inputs, its outputs, the tasks whose current preparation names that
//! Design (its gather work), and the Design's own room. A journal-backed
//! reference must resolve to a fact inside that closure: a contribution or
//! other room record of the controlling room, or a task or preparation fact
//! or authored observation of one closure task. Transcript and artifact links
//! keep their own owners.

use std::collections::BTreeSet;

use wire::rooms::{
    DesignPassProposal, RoomContributed, RoomPublicationContent, RoomPublished, RoomSource,
};

use super::*;

/// What one cited source resolves to at the barrier.
enum Resolved {
    /// The link names authority outside the journal.
    External,
    /// No admitted fact or contribution answers the link.
    Missing,
    /// A room record of this Design room.
    Room(TaskStamp),
    /// A task or preparation fact of this task, and whether the task's
    /// current preparation names the candidate's controller.
    Task(TaskStamp, bool),
    /// A fact that belongs to no task or room.
    Unscoped,
}

/// Refuse the first reference outside `candidate`'s authority closure.
pub(super) fn require_scoped_references(
    bus: &Bus,
    candidate: &DesignPassProposal,
) -> Result<(), Outcome<CommandAnswer>> {
    let sources = candidate
        .references
        .iter()
        .map(|reference| reference.source.clone())
        .collect::<Vec<_>>();
    let read_controller = candidate.candidate.controller.clone();
    let resolved = bus
        .read(move |read| {
            sources
                .iter()
                .map(|source| resolve(read, &read_controller, source))
                .collect::<Result<Vec<_>, GuardError>>()
        })
        .map_err(|error| store_fault("preparation::authority-unreadable", &error.to_string()))?;
    let controller = &candidate.candidate.controller;
    let closure = std::iter::once(controller)
        .chain(&candidate.governed_inputs)
        .chain(&candidate.outputs)
        .collect::<BTreeSet<_>>();
    for (reference, resolved) in candidate.references.iter().zip(resolved) {
        let admitted = match &resolved {
            Resolved::External => true,
            Resolved::Room(room) => room == controller,
            Resolved::Task(task, prepared) => *prepared || closure.contains(task),
            Resolved::Missing | Resolved::Unscoped => false,
        };
        if admitted {
            continue;
        }
        let source = serde_json::to_string(&reference.source)
            .unwrap_or_else(|_| format!("{:?}", reference.source));
        if matches!(resolved, Resolved::Missing) {
            return Err(refusal(
                "preparation::reference-missing",
                Some(source),
                "cite an admitted journal fact, or paste the `source` a `spice room contribute` \
                 answer returned",
            ));
        }
        return Err(refusal(
            "preparation::reference-out-of-scope",
            Some(source),
            &format!(
                "cite a contribution in Design {controller}'s room, or a fact of {controller}, \
                 its governed inputs, its outputs, or work prepared under it"
            ),
        ));
    }
    Ok(())
}

fn resolve(
    read: &journal::JournalRead<'_>,
    controller: &TaskStamp,
    source: &RoomSource,
) -> Result<Resolved, GuardError> {
    match source {
        RoomSource::Transcript(_) | RoomSource::Artifact { .. } => Ok(Resolved::External),
        RoomSource::Journal { fact } => {
            let Some(envelope) = Seq::new(fact.get())
                .map(|seq| read.room_fact(seq))
                .transpose()?
                .flatten()
            else {
                return Ok(Resolved::Missing);
            };
            let fact = &envelope.fact;
            // An authored observation is its task's note, scoped like the
            // task's own facts rather than as a record of a foreign room.
            let observed = fact
                .read::<RoomPublished>()
                .filter(|published| {
                    matches!(published.content, RoomPublicationContent::Authored { .. })
                })
                .map(|published| published.room);
            if observed.is_none()
                && let Some(room) = fact.room_record()
            {
                return Ok(Resolved::Room(room.clone()));
            }
            let Some(task) = observed.or_else(|| {
                fact.preparation_controller()
                    .or_else(|| fact.task_subject())
                    .cloned()
            }) else {
                return Ok(Resolved::Unscoped);
            };
            let prepared = read.task_heads_for(&task)?.is_some_and(|heads| {
                heads
                    .statement()
                    .body()
                    .preparation
                    .as_ref()
                    .is_some_and(|preparation| {
                        preparation.parent == *controller || preparation.controller == *controller
                    })
            });
            Ok(Resolved::Task(task, prepared))
        }
        RoomSource::Contribution { room, publication } => {
            let Some(seq) = read.room_publication_seq(room, publication)? else {
                return Ok(Resolved::Missing);
            };
            let contributed = read
                .room_fact(seq)?
                .and_then(|envelope| envelope.fact.read::<RoomContributed>())
                .is_some_and(|contributed| contributed.publication == *publication);
            Ok(if contributed {
                Resolved::Room(room.clone())
            } else {
                Resolved::Missing
            })
        }
    }
}
