//! A lane's typed problem-space contribution to an open Design room.

use super::*;

use crate::room_command::{RoomSources, SourceOwners};
use wire::intent::{RoomContribute, RoomContributeAnswer};
use wire::rooms::{RoomContributed, RoomContributionError, RoomSource};

use super::room::mint_publication;

/// Record the commanding lane's contribution in `contribute.room`.
///
/// The anchor must be an open Design and every cited source must resolve
/// through `owners` at the same snapshot. A contribution authorizes nothing:
/// it is one immutable room record, never a task, workflow, claim, group,
/// candidate, decision, or authorization change.
pub(crate) fn room_contribute(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    contribute: &RoomContribute,
    owners: Option<SourceOwners<'_>>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let Actor::Lane(lane) = command.actor().clone() else {
        return refusal(
            "room::contribution-lane-only",
            None,
            "contribute from a lane; the operator writes with `spice room post`",
        );
    };
    let text = contribute.text.trim();
    if let Err(error) = RoomContributed::admit(text, &contribute.sources) {
        return malformed(error);
    }
    let expected = match contribution_witnesses(command, &contribute.sources) {
        Ok(expected) => expected,
        Err(refused) => return refused,
    };
    let snapshot = match validated_working_snapshot(
        bus,
        command.actor().clone(),
        std::slice::from_ref(&contribute.room),
        projects,
        allocation,
    ) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(anchor) = task_entry(&snapshot.entries, &contribute.room) else {
        return refusal(
            "room::anchor-unknown",
            Some(contribute.room.to_string()),
            "read the board and name one current task handle",
        );
    };
    if let Some(refused) = not_open_design(anchor) {
        return refused;
    }
    let room = anchor.born().clone();
    let Some(before) = snapshot.cursor.get().checked_add(1).and_then(Seq::new) else {
        return store_fault(
            "room::cursor-exhausted",
            "the journal cursor has no successor",
        );
    };
    let mut sources = RoomSources::new(owners, &room);
    if let Some(refused) = unavailable_source(&mut sources, &contribute.sources, before) {
        return refused;
    }
    let now = jiff::Timestamp::now();
    let at = match Instant::new(now) {
        Ok(at) => at,
        Err(error) => return environment_fault("room::clock-invalid", &error.to_string()),
    };
    let publication = match mint_publication(bus, &room, now.as_microsecond()) {
        Ok(publication) => publication,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(&room) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let answer = RoomContributeAnswer {
        room: anchor.handle().clone(),
        publication: publication.clone(),
        kind: contribute.kind,
    };
    let mut write = JournalWrite::new(Actor::Lane(lane), origin, at, Producer::CommandDoor);
    guard(&mut write, expected);
    if let Err(error) = write.fact(
        RoomContributed {
            room,
            publication,
            lane,
            contribution: contribute.kind,
            text: text.to_owned(),
            sources: contribute.sources.clone(),
        },
        FactCause::Root,
    ) {
        return store_fault("room::contribution-build", &error.to_string());
    }
    if let Err(error) = write.command_result(
        request(command, contribute.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("room::contribution-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::RoomContribute(answer))
}

fn malformed(error: RoomContributionError) -> Outcome<CommandAnswer> {
    let (reason, hint) = match error {
        RoomContributionError::Empty => {
            ("room::contribution-empty", "state the contribution as TEXT")
        }
        RoomContributionError::Untrimmed => (
            "room::contribution-untrimmed",
            "remove leading and trailing whitespace",
        ),
        RoomContributionError::TextTooLong => (
            "room::contribution-too-long",
            "shorten the contribution and cite longer material as a source",
        ),
        RoomContributionError::TooManySources => (
            "room::contribution-sources-exceeded",
            "cite only the sources this contribution rests on",
        ),
        RoomContributionError::DuplicateSource => (
            "room::contribution-source-duplicate",
            "cite each source once",
        ),
    };
    refusal(reason, Some(error.to_string()), hint)
}
/// The journal-backed witnesses and the refusal when source witnesses do not
/// name exactly the request's citations. Source availability is evaluated
/// separately through its owning authority before these witnesses are stored
/// in the successful command receipt.
fn contribution_witnesses(
    command: &wire::AdmittedCommand,
    cited: &[RoomSource],
) -> Result<Vec<Witness>, Outcome<CommandAnswer>> {
    if !wire::rooms::source_witnesses_match(cited, command.expected()) {
        return Err(refusal(
            "room::contribution-source-witness",
            Some(format!(
                "expected {:?}; cited sources {cited:?}",
                command.expected()
            )),
            "read the Design again and retry with one witness for each exact source",
        ));
    }
    Ok(command
        .expected()
        .iter()
        .filter(|witness| !matches!(witness, Witness::Source { .. }))
        .cloned()
        .collect())
}

/// Why `anchor` is not an open Design: a Design whose workflow is pending
/// with no completion.
fn not_open_design(anchor: &Entry) -> Option<Outcome<CommandAnswer>> {
    if anchor.statement().intent != Some(wire::TaskIntent::Design) {
        return Some(refusal(
            "room::contribution-not-design",
            Some(anchor.handle().to_string()),
            "contribute to the room of an open Design",
        ));
    }
    let workflow = anchor.workflow();
    (workflow.status != "pending" || workflow.completion.is_some()).then(|| {
        refusal(
            "room::contribution-design-concluded",
            Some(format!("{} {}", anchor.handle(), workflow.status)),
            "a concluded Design accepts no new contribution",
        )
    })
}

/// The refusal naming the first cited source unavailable before `before`.
fn unavailable_source(
    sources: &mut RoomSources<'_>,
    cited: &[RoomSource],
    before: Seq,
) -> Option<Outcome<CommandAnswer>> {
    cited.iter().find_map(|source| {
        let unavailable = sources.check(source, before).err()?;
        Some(refusal(
            "room::contribution-source-unavailable",
            Some(format!(
                "{}: {}",
                serde_json::to_string(source).unwrap_or_default(),
                serde_json::to_string(&unavailable).unwrap_or_default()
            )),
            "cite only journal facts, transcript hunks, or Design artifacts that exist now",
        ))
    })
}
