//! Bounded room delivery to occupied listeners and transcript-backed landing.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use spice::rooms::{CatchUpBudget, CatchUpSource, select_catch_up};
use wire::rooms::{RoomDelivered, RoomHorizon, TextRange, TranscriptHunk};
use wire::{Actor, CanonicalUuid, Producer, Seq, SequenceCode, TaskHandle, TaskStamp};

use super::*;

const NOTICE_BYTES: usize = spice::rooms::MAX_CATCH_UP_BYTES + 4_096;
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

/// Publish the current bounded delivery for every occupied lane, then notify
/// only its already-running supervisor. This path never starts or wakes a lane.
pub(in crate::resident) fn reconcile(
    bus: &Bus,
    paths: &RuntimePaths,
    config: &SurfaceConfig,
    seats: &Seats,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    for lane in seats.lanes() {
        let Some(occupant) = seats.occupant(lane).map(lane::occupant::Occupant::occupant) else {
            continue;
        };
        if preserve_landed(bus, paths, lane, occupant)? {
            continue;
        }
        let path = notice_path(paths, lane);
        match plan(bus, paths, config, lane, occupant) {
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

fn plan(
    bus: &Bus,
    paths: &RuntimePaths,
    config: &SurfaceConfig,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> CommandResult<Option<Notice>> {
    let snapshot = bus
        .task_snapshot()
        .map_err(|error| store_fault("room::catch-up-snapshot-unreadable", &error.to_string()))?;
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
    let at = Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("room::catch-up-clock-invalid", &error.to_string()))?;
    let after = start(bus, lane)?;
    let mut sources = Vec::new();
    let mut attention = Vec::new();
    let mut presented = BTreeMap::new();
    for entry in &entries {
        let room = entry.born().clone();
        let publications =
            crate::board_command::room_publications(bus, &room).map_err(unit_outcome)?;
        if publications.is_empty() {
            continue;
        }
        let room_attention =
            crate::board_command::project_attention(&entries, &residency, &room, &publications, at)
                .map_err(unit_outcome)?;
        let unseen = publications
            .into_iter()
            .filter(|publication| publication.seq > after)
            .collect();
        let projections = crate::room_command::project_publications(bus, &paths.tape, unseen)
            .map_err(|detail| store_fault("room::catch-up-publication-unreadable", &detail))?;
        for publication in projections {
            let seq = publication.seq();
            let display = publication.to_string();
            sources.push(CatchUpSource {
                seq,
                room: room.clone(),
                publication: publication.publication().clone(),
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
    }
    let selected = select_catch_up(lane, after, &attention, sources, CatchUpBudget::STANDARD)
        .map_err(|error| store_fault("room::catch-up-authority-invalid", &error.to_string()))?;
    let Some(through) = selected.through else {
        return Ok(None);
    };
    let mut text = String::from(
        "<system-notice>\n# Room catch-up\nBounded background publications for rooms this lane currently listens to:\n",
    );
    for item in &selected.items {
        let item = presented
            .get(&item.seq)
            .expect("a selected source retains its presentation");
        text.push_str(&format!(
            "\nroom {} anchor {}\n{}\n",
            item.handle, item.room, item.publication
        ));
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
    Ok(Some(Notice {
        occupant,
        through,
        text,
    }))
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
        None => commit_fact(bus, lane, RoomHorizon { lane }),
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

fn unit_outcome(outcome: Outcome<wire::intent::CommandAnswer>) -> Outcome<()> {
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
) -> CommandResult<()> {
    let Some(notice) = read_notice(&landed_notice_path(paths, record.lane))
        .or_else(|| read_notice(&notice_path(paths, record.lane)))
    else {
        return Ok(());
    };
    let current = bus
        .read({
            let lane = record.lane;
            move |read| read.room_cursor(lane)
        })
        .map_err(|error| store_fault("room::catch-up-cursor-unreadable", &error.to_string()))?;
    let current = current.as_ref().map(|head| &head.body().through);
    if notice.occupant != record.occupant {
        return Ok(());
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
        commit_fact(
            bus,
            record.lane,
            RoomDelivered {
                through: SequenceCode::new(notice.through.get()),
                hunk,
            },
        )?;
        return Ok(());
    }
    Ok(())
}

/// Commit one resident-recognized room catch-up fact about `lane`, returning
/// its journal position.
fn commit_fact(bus: &Bus, lane: CanonicalUuid, body: impl wire::Body) -> CommandResult<Seq> {
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
        CommitOutcome::Committed(commit) => Seq::new(commit.last()).ok_or_else(|| {
            store_fault("room::catch-up-commit", "a committed position is positive")
        }),
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
