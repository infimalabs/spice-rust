//! Bounded room delivery to occupied listeners and transcript-backed landing.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use spice::rooms::{CatchUpBudget, CatchUpSource, select_catch_up};
use wire::rooms::{
    CoordinationControl, QuestionResultRef, RoomDelivered, RoomHorizon, RoomQuestionClosed,
    RoomQuestionTimedOut, TextRange, TranscriptHunk,
};
use wire::{
    Actor, AnyFact, CanonicalUuid, Producer, Seq, SequenceCode, SteeringHead, TaskHandle, TaskStamp,
};

use super::*;

const NOTICE_BYTES: usize = spice::rooms::MAX_CATCH_UP_BYTES + 8_192;
/// Most listening reasons one room's line names before counting the rest.
const LISTENING_REASONS: usize = 4;
const NOTICE_DIRECTORY: &str = "room-catch-up";
const LANDED_DIRECTORY: &str = "room-catch-up-landed";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::resident) struct Notice {
    pub(in crate::resident) occupant: CanonicalUuid,
    pub(in crate::resident) through: Seq,
    pub(in crate::resident) text: String,
}

struct Presented {
    room: TaskStamp,
    handle: TaskHandle,
    publication: crate::room_command::Publication,
}

/// One read of the task board, residency, and clock that room projections
/// share.
pub(in crate::resident) struct Planning {
    pub(in crate::resident) residency: wire::ResidencyHead,
    pub(in crate::resident) entries: Vec<board::Entry>,
    rooms: BTreeMap<TaskStamp, usize>,
    pub(in crate::resident) at: Instant,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(in crate::resident) struct Changes {
    pub(in crate::resident) authority: bool,
    pub(in crate::resident) lanes: BTreeSet<CanonicalUuid>,
    pub(in crate::resident) rooms: BTreeSet<TaskStamp>,
}

/// Why `lane` listens to one room and how it leaves, as one notice block.
fn listening(
    handle: &TaskHandle,
    row: Option<&spice::rooms::RoomLaneAttention>,
    entries: &[board::Entry],
) -> String {
    use spice::rooms::AttentionReason as Reason;
    let reasons = row.map_or(&[][..], |row| row.attention.as_slice());
    let task = |task: &TaskStamp| {
        entries
            .iter()
            .find(|entry| entry.born() == task)
            .map_or_else(|| task.to_string(), |entry| entry.handle().to_string())
    };
    let mut named = reasons
        .iter()
        .take(LISTENING_REASONS)
        .map(|reason| match reason {
            Reason::Work { task: work } => format!("work {}", task(work)),
            Reason::Responsibility { task: work } => format!("responsibility {}", task(work)),
            Reason::Activity { publication, .. } => {
                format!("recent contribution {publication}")
            }
            Reason::Invitation { key } => format!("invitation {key}"),
            Reason::Delivery { key } => format!("owed receipt {key}"),
            Reason::Result { key } => format!("owed result {key}"),
            Reason::Coordination { key } => format!("coordination request {key}"),
            Reason::Stop { key } => format!("stop request {key}"),
        })
        .collect::<Vec<_>>();
    if reasons.len() > LISTENING_REASONS {
        named.push(format!("{} more", reasons.len() - LISTENING_REASONS));
    }
    let native = reasons.iter().any(Reason::is_native);
    format!(
        "\nlistening {handle}: {}\nleave: spice room leave {handle} --clause TEXT{}\n",
        named.join(", "),
        if native {
            " (native work and duties keep this lane listening until they end)"
        } else {
            ""
        }
    )
}

/// Publish the current bounded delivery for every occupied lane, then notify
/// only its already-running supervisor. This path never starts or wakes a lane.
pub(in crate::resident) fn reconcile(
    bus: &Bus,
    paths: &RuntimePaths,
    config: &SurfaceConfig,
    seats: &Seats,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    reconcile_selected(
        bus,
        paths,
        config,
        seats,
        true,
        &BTreeSet::new(),
        &BTreeSet::new(),
        diagnostic,
    )
}

/// Reconcile only authority changed by one durable transaction.
///
/// Publications name their indexed rooms, cursor progress names its exact
/// lane, and task/residency changes retain the complete startup projection.
pub(in crate::resident) fn reconcile_commit(
    bus: &Bus,
    paths: &RuntimePaths,
    config: &SurfaceConfig,
    seats: &Seats,
    changes: &Changes,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    if !changes.authority && changes.lanes.is_empty() && changes.rooms.is_empty() {
        return Ok(());
    }
    reconcile_selected(
        bus,
        paths,
        config,
        seats,
        changes.authority,
        &changes.lanes,
        &changes.rooms,
        diagnostic,
    )
}

fn reconcile_selected(
    bus: &Bus,
    paths: &RuntimePaths,
    config: &SurfaceConfig,
    seats: &Seats,
    all: bool,
    lanes: &BTreeSet<CanonicalUuid>,
    rooms: &BTreeSet<TaskStamp>,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let selected_occupant = seats.lanes().into_iter().any(|lane| {
        seats.occupant(lane).is_some() && (all || lanes.contains(&lane) || !rooms.is_empty())
    });
    if !selected_occupant {
        return Ok(());
    }
    let planning = Planning::load(bus, config)?;
    for lane in seats.lanes() {
        let Some(occupant) = seats.occupant(lane).map(lane::occupant::Occupant::occupant) else {
            continue;
        };
        let selected_rooms = if all || lanes.contains(&lane) {
            None
        } else if rooms.is_empty() {
            continue;
        } else {
            Some(rooms)
        };
        if preserve_landed(bus, paths, lane, occupant)? {
            continue;
        }
        let selected_rooms =
            if selected_rooms.is_some() && pending_notice(bus, paths, lane, occupant)? {
                None
            } else {
                selected_rooms
            };
        let path = notice_path(paths, lane);
        match plan(bus, paths, lane, occupant, &planning, selected_rooms) {
            Ok(Some(notice)) => {
                let published = path
                    .parent()
                    .map_or(Ok(()), fs::create_dir_all)
                    .and_then(|()| publish_notice(&path, &notice));
                match published {
                    Ok(()) => super::supervisor::command::room_catch_up(paths, lane),
                    Err(error) => {
                        let _ = writeln!(
                            diagnostic,
                            "lane {lane} room catch-up state could not be published: {error}"
                        );
                    }
                }
            }
            Ok(None) => match fs::remove_file(&path) {
                Ok(()) => super::supervisor::command::room_catch_up(paths, lane),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    super::supervisor::command::room_catch_up(paths, lane);
                }
                Err(error) => {
                    let _ = writeln!(
                        diagnostic,
                        "lane {lane} settled room catch-up state could not be removed: {error}"
                    );
                }
            },
            Err(outcome) => {
                let _ = writeln!(
                    diagnostic,
                    "lane {lane} room catch-up unavailable: {outcome:?}"
                );
            }
        }
    }
    Ok(())
}

impl Planning {
    pub(in crate::resident) fn load(bus: &Bus, config: &SurfaceConfig) -> CommandResult<Self> {
        let snapshot = bus.task_snapshot().map_err(|error| {
            store_fault("room::catch-up-snapshot-unreadable", &error.to_string())
        })?;
        let residency = snapshot
            .residency()
            .ok_or_else(|| {
                store_fault(
                    "room::catch-up-residency-missing",
                    "the resident has no residency head",
                )
            })?
            .body()
            .clone();
        let entries = snapshot
            .tasks()
            .iter()
            .map(|heads| {
                board::Entry::new(
                    heads.statement().body().clone(),
                    heads.workflow().body().clone(),
                    heads.claim().map(|head| head.body().clone()),
                    heads.lease().map(|head| head.body().clone()),
                    &config.projects,
                )
                .map_err(|error| store_fault("room::catch-up-task-invalid", &error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let rooms = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.born().clone(), index))
            .collect();
        let at = Instant::new(jiff::Timestamp::now()).map_err(|error| {
            environment_fault("room::catch-up-clock-invalid", &error.to_string())
        })?;
        Ok(Self {
            residency,
            entries,
            rooms,
            at,
        })
    }
}

pub(in crate::resident) fn changes(
    bus: &Bus,
    commit: bus::JournalCommit,
) -> CommandResult<Changes> {
    let first = Seq::new(commit.first()).expect("a committed position is positive");
    let last = Seq::new(commit.last()).expect("a committed position is positive");
    bus.read(move |read| {
        let mut changes = Changes {
            authority: read
                .residency_head()?
                .is_some_and(|head| head.position() >= first && head.position() <= last),
            rooms: read
                .changed_room_publications(first, last)?
                .into_iter()
                .collect(),
            ..Changes::default()
        };
        let mut start = journal::FactPageStart::At(first);
        loop {
            let page = read.fact_page(start, last, journal::FactReadBudget::RETAINED)?;
            for envelope in &page.rows {
                let fact = &envelope.fact;
                changes.authority |= changes_task_attention(fact);
                if let Some(delivered) = fact.read::<RoomDelivered>() {
                    changes.lanes.insert(delivered.hunk.head.lane);
                }
                if let Some(key) = steer::directives::affected_key(envelope)
                    && let Some(head) = read.steering_key(key.as_str())?
                {
                    extend_steering_rooms(&mut changes.rooms, head.body());
                }
                if let Some(result) = changed_question_result(fact)
                    && let Some((record, _)) = read.question_result(&result)?
                    && let Some(room) = record.context()
                {
                    changes.rooms.insert(room.clone());
                }
            }
            let Some(overflow) = page.overflow else {
                break;
            };
            start = journal::FactPageStart::At(overflow.continuation);
        }
        Ok(changes)
    })
    .map_err(|error| store_fault("room::catch-up-changes-unreadable", &error.to_string()))
}

fn changes_task_attention(fact: &AnyFact) -> bool {
    fact.task_subject().is_some()
        && !matches!(
            fact.kind(),
            wire::FactKind::TaskNoted
                | wire::FactKind::ClaimRenewed
                | wire::FactKind::ClaimLeaseWarned
        )
}

fn extend_steering_rooms(rooms: &mut BTreeSet<TaskStamp>, head: &SteeringHead) {
    rooms.extend(head.room.iter().cloned());
    let Some(control) = head
        .control
        .as_deref()
        .and_then(|control| control.parse::<CoordinationControl>().ok())
    else {
        return;
    };
    rooms.insert(match control {
        CoordinationControl::Coordinate(room) | CoordinationControl::Stop { room, .. } => room,
    });
}

fn changed_question_result(fact: &AnyFact) -> Option<QuestionResultRef> {
    fact.read::<RoomQuestionClosed>()
        .map(|closed| closed.result)
        .or_else(|| {
            fact.read::<RoomQuestionTimedOut>()
                .map(|timed_out| timed_out.result)
        })
        .or_else(|| {
            fact.read::<wire::body::SteerDeclined>()
                .and_then(|declined| {
                    declined
                        .key
                        .parse::<wire::rooms::RoomAuthorityStamp>()
                        .ok()
                        .map(QuestionResultRef::new)
                })
        })
}

fn plan(
    bus: &Bus,
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    planning: &Planning,
    rooms: Option<&BTreeSet<TaskStamp>>,
) -> CommandResult<Option<Notice>> {
    let after = start(bus, lane)?;
    let mut sources = Vec::new();
    let mut attention = Vec::new();
    let mut presented = BTreeMap::new();
    let mut handles = BTreeMap::new();
    if let Some(rooms) = rooms {
        for room in rooms {
            let index = planning.rooms.get(room).ok_or_else(|| {
                store_fault(
                    "room::catch-up-anchor-missing",
                    &format!("publication index names unknown room {room}"),
                )
            })?;
            let entry = &planning.entries[*index];
            append_room(
                bus,
                paths,
                lane,
                planning,
                entry,
                after,
                &mut sources,
                &mut attention,
                &mut presented,
            )?;
            handles.insert(room.clone(), entry.handle().clone());
        }
    } else {
        for entry in &planning.entries {
            let room = entry.born().clone();
            append_room(
                bus,
                paths,
                lane,
                planning,
                entry,
                after,
                &mut sources,
                &mut attention,
                &mut presented,
            )?;
            handles.insert(room, entry.handle().clone());
        }
    }
    let selected = select_catch_up(lane, after, &attention, sources, CatchUpBudget::STANDARD)
        .map_err(|error| store_fault("room::catch-up-authority-invalid", &error.to_string()))?;
    let Some(through) = selected.through else {
        return Ok(None);
    };
    let text = render(
        lane,
        through,
        &selected,
        &presented,
        &attention,
        &handles,
        &planning.entries,
    );
    Ok(Some(Notice {
        occupant,
        through,
        text,
    }))
}

/// The notice text for `selected`: each publication under its room, then why
/// `lane` listens to each room shown and how it leaves.
fn render(
    lane: CanonicalUuid,
    through: Seq,
    selected: &spice::rooms::CatchUp,
    presented: &BTreeMap<Seq, Presented>,
    attention: &[spice::rooms::RoomAttention],
    handles: &BTreeMap<TaskStamp, TaskHandle>,
    entries: &[board::Entry],
) -> String {
    let mut text = String::from(
        "<system-notice>\n# Room catch-up\nBounded background publications for rooms this lane currently listens to:\n",
    );
    let mut rooms = Vec::new();
    for item in &selected.items {
        let item = presented
            .get(&item.seq)
            .expect("a selected source retains its presentation");
        if !rooms.contains(&&item.room) {
            rooms.push(&item.room);
        }
        text.push_str(&format!(
            "\nroom {} anchor {}\n{}\n",
            item.handle, item.room, item.publication
        ));
    }
    for room in rooms {
        let row = attention
            .iter()
            .find(|view| view.room == *room)
            .and_then(|view| view.lanes.iter().find(|row| row.lane == lane));
        text.push_str(&listening(&handles[room], row, entries));
    }
    if let Some(continuation) = selected.continuation {
        let withheld = presented
            .get(&continuation)
            .expect("a continuation retains its presentation");
        text.push_str(&format!(
            "\ncontinue: spice://task/{}/room?after={}\n",
            withheld.handle,
            withheld.publication.publication()
        ));
    }
    text.push_str(&format!(
        "\nroom-delivery lane={lane} through={through}\nThis background delivery needs no acknowledgment and starts no work.\n</system-notice>"
    ));
    text
}

#[allow(clippy::too_many_arguments)]
fn append_room(
    bus: &Bus,
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    planning: &Planning,
    entry: &board::Entry,
    after: Seq,
    sources: &mut Vec<CatchUpSource>,
    attention: &mut Vec<spice::rooms::RoomAttention>,
    presented: &mut BTreeMap<Seq, Presented>,
) -> CommandResult<()> {
    let room = entry.born().clone();
    let publications =
        crate::board_command::room_publications_after(bus, &room, after).map_err(unit_outcome)?;
    if publications.is_empty() {
        return Ok(());
    }
    let room_attention = crate::board_command::project_attention(
        bus,
        &planning.entries,
        &planning.residency,
        &room,
        latest_lane_activity(bus, &room, lane)?.as_slice(),
        planning.at,
    )
    .map_err(unit_outcome)?;
    let artifacts = paths.artifacts();
    let owners = crate::room_command::SourceOwners {
        tape: &paths.tape,
        artifacts: &artifacts,
    };
    let projections =
        crate::room_command::project_publications(bus, owners, &room, publications)
            .map_err(|detail| store_fault("room::catch-up-publication-unreadable", &detail))?;
    for publication in projections {
        let seq = publication.seq();
        let display = publication.to_string();
        sources.push(CatchUpSource {
            seq,
            room: room.clone(),
            publication: publication.publication().clone(),
            at: publication.at(),
            author: publication.author_lane(),
            recipients: publication
                .recipients()
                .map_err(|detail| store_fault("room::catch-up-route-invalid", &detail))?,
            bytes: display.len(),
        });
        presented.insert(
            seq,
            Presented {
                room: room.clone(),
                handle: entry.handle().clone(),
                publication,
            },
        );
    }
    attention.push(room_attention);
    Ok(())
}

fn latest_lane_activity(
    bus: &Bus,
    room: &TaskStamp,
    lane: CanonicalUuid,
) -> CommandResult<Option<wire::Envelope<wire::AnyFact>>> {
    let room = room.clone();
    let actor = Actor::Lane(lane);
    bus.read(move |read| read.latest_room_publication(&room, &actor))
        .map_err(|error| store_fault("room::catch-up-activity-unreadable", &error.to_string()))
}

/// Where `lane`'s catch-up starts: its delivery cursor or, before any delivery,
/// its horizon, recorded at the journal head the first time the lane is
/// reconciled without either, so retained history it predates is never owed.
fn start(bus: &Bus, lane: CanonicalUuid) -> CommandResult<Seq> {
    if let Some(cursor) = cursor(bus, lane)? {
        return Ok(cursor);
    }
    let horizon = bus
        .read(move |read| read.room_catch_up_horizon(lane))
        .map_err(|error| store_fault("room::catch-up-horizon-unreadable", &error.to_string()))?;
    match horizon {
        Some(head) => Ok(head.position()),
        None => commit_fact(bus, lane, RoomHorizon { lane })
            .map(|commit| Seq::new(commit.last()).expect("a committed position is positive")),
    }
}

fn cursor(bus: &Bus, lane: CanonicalUuid) -> CommandResult<Option<Seq>> {
    bus.read(move |read| read.room_cursor(lane))
        .map_err(|error| store_fault("room::catch-up-cursor-unreadable", &error.to_string()))
        .map(|cursor| {
            cursor
                .map(|head| Seq::new(head.body().through.get()).expect("a cursor code is positive"))
        })
}

/// Whether the lane still has a published page that has not advanced its
/// durable cursor. A changed-room pass must retain full projection in this
/// case so it cannot replace unrelated pending content.
fn pending_notice(
    bus: &Bus,
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> CommandResult<bool> {
    let Some(notice) = read_notice(&notice_path(paths, lane)) else {
        return Ok(false);
    };
    Ok(notice.occupant == occupant
        && cursor(bus, lane)?.is_none_or(|cursor| cursor < notice.through))
}

/// Keep an exact landed page available until its durable cursor proves
/// recognition. Recomputed attention and newly appended publications cannot
/// replace transcript evidence while accounting catches up.
fn preserve_landed(
    bus: &Bus,
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> CommandResult<bool> {
    let landed_path = landed_notice_path(paths, lane);
    let Some(landed) = read_notice(&landed_path) else {
        return Ok(false);
    };
    if landed.occupant != occupant {
        remove_landed(&landed_path)?;
        return Ok(false);
    }
    if cursor(bus, lane)?.is_none_or(|cursor| cursor < landed.through) {
        let path = notice_path(paths, lane);
        path.parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| publish_notice(&path, &landed))
            .map_err(|error| {
                environment_fault("room::catch-up-state-unavailable", &error.to_string())
            })?;
        return Ok(true);
    }
    remove_landed(&landed_path)?;
    Ok(false)
}

fn remove_landed(path: &Path) -> CommandResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(environment_fault(
            "room::catch-up-landed-state-unavailable",
            &error.to_string(),
        )),
    }
}

pub(in crate::resident) fn unit_outcome(
    outcome: Outcome<wire::intent::CommandAnswer>,
) -> Outcome<()> {
    match outcome {
        Outcome::Reply(_) => unreachable!("a room authority helper never replies on failure"),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

/// Commit a cursor only after the exact resident-produced notice appears as
/// operator prose in the lane's accounted transcript.
pub(in crate::resident) fn recognize(
    bus: &Bus,
    paths: &RuntimePaths,
    record: &lane::capture::CaptureRecord,
    source: CanonicalUuid,
) -> CommandResult<Option<bus::JournalCommit>> {
    let Some(notice) = read_notice(&landed_notice_path(paths, record.lane))
        .or_else(|| read_notice(&notice_path(paths, record.lane)))
    else {
        return Ok(None);
    };
    let current = bus
        .read({
            let lane = record.lane;
            move |read| read.room_cursor(lane)
        })
        .map_err(|error| store_fault("room::catch-up-cursor-unreadable", &error.to_string()))?;
    let current = current.as_ref().map(|head| &head.body().through);
    if notice.occupant != record.occupant {
        return Ok(None);
    }
    for (event, prose) in
        record
            .decoded
            .events()
            .iter()
            .enumerate()
            .filter_map(|(event, event_body)| {
                event_body
                    .prose()
                    .filter(|prose| prose.role == listen::ProseRole::Operator)
                    .map(|prose| (event, prose))
            })
    {
        if !matches_delivery(&notice, record.occupant, current, &prose.text) {
            continue;
        }
        let Some(hunk) = delivery_hunk(
            bus,
            record.lane,
            source,
            record.locus,
            event,
            prose.text.len(),
        )?
        else {
            continue;
        };
        let commit = commit_fact(
            bus,
            record.lane,
            RoomDelivered {
                through: SequenceCode::new(notice.through.get()),
                hunk,
            },
        )?;
        return Ok(Some(commit));
    }
    Ok(None)
}

/// Commit one resident-recognized room catch-up fact about `lane`.
fn commit_fact(
    bus: &Bus,
    lane: CanonicalUuid,
    body: impl wire::Body,
) -> CommandResult<bus::JournalCommit> {
    let at = Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("room::catch-up-clock-invalid", &error.to_string()))?;
    let origin = format!("lane:{lane}")
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("room::catch-up-origin-invalid", &error.to_string()))?;
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::ResidentRecognition);
    write
        .fact(body, FactCause::Root)
        .map_err(|error| store_fault("room::catch-up-build", &error.to_string()))?;
    match bus
        .commit(write)
        .map_err(|error| store_fault("room::catch-up-commit", &error.to_string()))?
    {
        CommitOutcome::Committed(commit) => Ok(commit),
        CommitOutcome::Refused(refusal) => {
            Err(store_fault("room::catch-up-refused", &refusal.to_string()))
        }
        CommitOutcome::Fault(fault) => Err(store_fault("room::catch-up-fault", &fault.to_string())),
    }
}

fn matches_delivery(
    notice: &Notice,
    occupant: CanonicalUuid,
    current: Option<&SequenceCode>,
    text: &str,
) -> bool {
    notice.occupant == occupant
        && text == notice.text
        && current.is_none_or(|through| through.get() < notice.through.get())
}

fn delivery_hunk(
    bus: &Bus,
    lane: CanonicalUuid,
    source: CanonicalUuid,
    locus: listen::Locus,
    event: usize,
    text_bytes: usize,
) -> CommandResult<Option<TranscriptHunk>> {
    let head = bus
        .read(move |read| read.transcript_source_heads(&lane))
        .map_err(|error| store_fault("room::catch-up-transcript-unreadable", &error.to_string()))?
        .into_iter()
        .map(journal::JournalHead::into_body)
        .find(|head| {
            let end = u64::from(locus.offset) + u64::from(locus.length);
            head.source == source
                && (locus.member < head.next.member
                    || (locus.member == head.next.member && end <= head.next.offset))
        });
    let Some(head) = head else {
        return Ok(None);
    };
    Ok(Some(TranscriptHunk {
        head,
        locus: wire::MessageLocus {
            member: locus.member,
            offset: locus.offset,
            length: locus.length,
            event: u32::try_from(event)
                .map_err(|error| store_fault("room::catch-up-event-invalid", &error.to_string()))?,
        },
        text: TextRange {
            start: 0,
            end: u64::try_from(text_bytes)
                .map_err(|error| store_fault("room::catch-up-text-invalid", &error.to_string()))?,
        },
    }))
}

pub(in crate::resident) fn notice_path(paths: &RuntimePaths, lane: CanonicalUuid) -> PathBuf {
    paths
        .runtime
        .join(NOTICE_DIRECTORY)
        .join(format!("{lane}.json"))
}
pub(in crate::resident) fn landed_notice_path(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
) -> PathBuf {
    paths
        .runtime
        .join(LANDED_DIRECTORY)
        .join(format!("{lane}.json"))
}

pub(in crate::resident) fn read_notice(path: &Path) -> Option<Notice> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() > NOTICE_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

pub(in crate::resident) fn publish_notice(path: &Path, notice: &Notice) -> io::Result<()> {
    let bytes = serde_json::to_vec(notice).map_err(io::Error::other)?;
    atomic::Destination::new(path)
        .map_err(io::Error::other)?
        .publish(|file| file.write_all(&bytes))
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
            .parse()
            .expect("fixture UUID")
    }

    #[test]
    fn only_the_exact_occupant_delivery_advances_an_older_cursor() {
        let occupant = uuid(1);
        let notice = Notice {
            occupant,
            through: Seq::new(12).expect("fixture position"),
            text: "source-backed delivery".to_owned(),
        };
        let older = SequenceCode::new(11);
        let landed = SequenceCode::new(12);

        assert!(matches_delivery(
            &notice,
            occupant,
            Some(&older),
            "source-backed delivery"
        ));
        assert!(!matches_delivery(
            &notice,
            uuid(2),
            Some(&older),
            "source-backed delivery"
        ));
        assert!(!matches_delivery(
            &notice,
            occupant,
            Some(&older),
            "interrupted delivery"
        ));
        assert!(!matches_delivery(
            &notice,
            occupant,
            Some(&landed),
            "source-backed delivery"
        ));
    }
}

#[cfg(test)]
mod listening_tests {
    use spice::rooms::{AttentionReason, LaneAvailability, RoomLaneAttention};

    use super::*;

    #[test]
    fn each_room_names_why_the_lane_listens_and_how_it_leaves() {
        let handle: TaskHandle = "ROOMS-dVxHkZhQY".parse().expect("fixture handle");
        let row = RoomLaneAttention {
            lane: "018f82d4-6f59-7d06-92e3-bce913116301"
                .parse()
                .expect("fixture lane"),
            availability: LaneAvailability::Occupied,
            participation: Vec::new(),
            attention: vec![
                AttentionReason::Invitation {
                    key: "dVxJpSbV9".to_owned(),
                },
                AttentionReason::Result {
                    key: "dVxJkvVCV".to_owned(),
                },
            ],
            left: None,
            leave_intervals: Vec::new(),
        };
        let block = listening(&handle, Some(&row), &[]);
        assert!(block.contains("invitation dVxJpSbV9"), "{block}");
        assert!(block.contains("owed result dVxJkvVCV"), "{block}");
        assert!(
            block.contains("spice room leave ROOMS-dVxHkZhQY --clause"),
            "{block}"
        );
    }
}
