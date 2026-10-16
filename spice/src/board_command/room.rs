use super::*;

use spice::rooms::{
    Declared, LaneAvailability, RoomDuty, RoomDutyKind, RoomDutyStanding, RoomProjectionBudget,
    RoomProjectionError, RoomProjectionSources, RoomWork, derive_room_attention,
};
use wire::rooms::{
    OOPS_GROOMING_REPORT_LIMIT, ROOM_ACTIVITY_SECONDS, ROOM_PUBLICATION_BYTES_LIMIT,
    RoomAuthorityStamp,
};

const ROOM_PUBLICATION_PAGES: usize =
    spice::rooms::MAX_ROOM_ATTENTION_SIGNALS.div_ceil(journal::MAX_RETAINED_FACT_PAGE_ROWS);

pub(super) fn room_post(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    post: &RoomPost,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let content = match room_post_content(post) {
        Ok(content) => content,
        Err(outcome) => return outcome,
    };
    let snapshot = match validated_working_snapshot(
        bus,
        command.actor().clone(),
        std::slice::from_ref(&post.room),
        projects,
        allocation,
    ) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(anchor) = task_entry(&snapshot.entries, &post.room) else {
        return refusal(
            "room::anchor-unknown",
            Some(post.room.to_string()),
            "read the board and name one current task handle",
        );
    };
    if matches!(
        anchor.workflow().status.conclusion(),
        Some(wire::WorkflowConclusion::Negative(
            wire::WorkflowNegative::Deleted
        ))
    ) {
        return refusal(
            "room::anchor-deleted",
            Some(format!("{} deleted", anchor.handle())),
            "a deleted task's room stays readable; publish to a task that is not deleted",
        );
    }
    if post.grooming.is_some() && matches!(command.actor(), Actor::Lane(_)) {
        return refusal(
            "room::oops-grooming-operator",
            Some(anchor.handle().to_string()),
            "only the operator records OOPS grooming; publish evidence as an observation",
        );
    }
    if let Some(grooming) = &post.grooming
        && let Err(outcome) = validate_oops_grooming(anchor, &snapshot.entries, command, grooming)
    {
        return outcome;
    }
    let room = anchor.born().clone();
    let publications = match room_publications(bus, &room) {
        Ok(publications) => publications,
        Err(outcome) => return outcome,
    };
    let now = jiff::Timestamp::now();
    let at = match Instant::new(now) {
        Ok(at) => at,
        Err(error) => return environment_fault("room::clock-invalid", &error.to_string()),
    };
    let attention = match project_attention(
        bus,
        &snapshot.entries,
        &snapshot.answer.residency.body,
        &room,
        &publications,
        at,
    ) {
        Ok(attention) => attention,
        Err(outcome) => return outcome,
    };
    let publication = match mint_publication(bus, &room, now.as_microsecond()) {
        Ok(publication) => publication,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(&room) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let answer = RoomPostAnswer {
        room: anchor.handle().clone(),
        publication: publication.clone(),
        outcome: post_outcome(attention),
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    if let Err(error) = write.fact(
        RoomPublished {
            room,
            publication,
            content,
        },
        FactCause::Root,
    ) {
        return store_fault("room::post-build", &error.to_string());
    }
    if let Err(error) = write.command_result(
        request(command, post.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("room::post-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::RoomPost(answer))
}

/// The terminal standing that settled a room's anchor, or `None` while the
/// anchor is unresolved. Only an unresolved anchor's room takes new
/// coordination; a settled room still takes observations, and duties already
/// open in it still settle.
pub(super) fn anchor_settlement(workflow: &wire::WorkflowHead) -> Option<&'static str> {
    match workflow.status.conclusion()? {
        wire::WorkflowConclusion::Positive { .. } => Some("completed"),
        wire::WorkflowConclusion::Negative(wire::WorkflowNegative::Closed) => Some("closed"),
        wire::WorkflowConclusion::Negative(wire::WorkflowNegative::Deleted) => Some("deleted"),
    }
}

fn room_post_content(post: &RoomPost) -> Result<RoomPublicationContent, Outcome<CommandAnswer>> {
    match (post.text.trim(), &post.grooming) {
        ("", None) => Err(refusal(
            "room::post-empty",
            Some(post.room.to_string()),
            "supply complete authored text or one typed OOPS disposition",
        )),
        (text, None) if text.len() > ROOM_PUBLICATION_BYTES_LIMIT => Err(refusal(
            "room::post-too-large",
            Some(text.len().to_string()),
            &format!("shorten the observation to at most {ROOM_PUBLICATION_BYTES_LIMIT} bytes"),
        )),
        (text, None) => Ok(RoomPublicationContent::Authored {
            text: text.to_owned(),
        }),
        ("", Some(grooming)) => Ok(RoomPublicationContent::OopsGroomed {
            grooming: grooming.clone(),
        }),
        (_, Some(_)) => Err(refusal(
            "room::post-shape",
            Some(post.room.to_string()),
            "supply either authored text or one OOPS disposition, never both",
        )),
    }
}

fn validate_oops_grooming(
    anchor: &Entry,
    entries: &[Entry],
    command: &wire::AdmittedCommand,
    grooming: &OopsGrooming,
) -> Result<(), Outcome<CommandAnswer>> {
    if !wire::intent::is_open_design(anchor.statement().intent, anchor.workflow()) {
        return Err(refusal(
            "room::oops-batch-not-open",
            Some(anchor.handle().to_string()),
            "record grooming under an open Design room",
        ));
    }
    if grooming.reports.is_empty() {
        return Err(refusal(
            "room::oops-reports-missing",
            Some(anchor.handle().to_string()),
            "name at least one parked OOPS report",
        ));
    }
    if grooming.reports.len() > OOPS_GROOMING_REPORT_LIMIT {
        return Err(refusal(
            "room::oops-batch-too-large",
            Some(format!(
                "{} reports; limit={OOPS_GROOMING_REPORT_LIMIT}",
                grooming.reports.len()
            )),
            "split the evidence into separate bounded grooming commitments",
        ));
    }
    if grooming.cause.trim().is_empty() {
        return Err(refusal(
            "room::oops-cause-missing",
            Some(anchor.handle().to_string()),
            "retain the evidenced common cause or negative basis",
        ));
    }
    if grooming.disposition == wire::rooms::OopsDisposition::ProposedWork
        && grooming.reports.len() != 1
    {
        return Err(refusal(
            "room::oops-proposal-ambiguous",
            Some(anchor.handle().to_string()),
            "record proposed-work against exactly one original report before controlled filing",
        ));
    }
    let mut seen = Vec::with_capacity(grooming.reports.len());
    for report in &grooming.reports {
        if seen.contains(report) {
            return Err(refusal(
                "room::oops-report-duplicate",
                Some(report.to_string()),
                "name each source report once",
            ));
        }
        seen.push(report.clone());
        let Some(entry) = entries.iter().find(|entry| entry.born() == report) else {
            return Err(refusal(
                "room::oops-report-missing",
                Some(report.to_string()),
                "read the board and name a retained parked OOPS report",
            ));
        };
        if !entry.is_oops() || !entry.workflow().status.is_pending() {
            return Err(refusal(
                "room::oops-report-not-parked",
                Some(entry.handle().to_string()),
                "groom only pending reports still held by typed OOPS filing state",
            ));
        }
        let witness = Witness::Task {
            task: report.clone(),
            head: entry.statement().revision,
        };
        if !command.expected().contains(&witness) {
            return Err(refusal(
                "room::oops-report-witness-moved",
                Some(entry.handle().to_string()),
                "read the board again and retry with every report's current statement witness",
            ));
        }
    }
    Ok(())
}

pub(crate) fn room_publications(
    bus: &Bus,
    room: &wire::TaskStamp,
) -> Result<Vec<wire::Envelope<wire::AnyFact>>, Outcome<CommandAnswer>> {
    bounded_room_publications(bus, room, None)
}

pub(crate) fn room_publications_after(
    bus: &Bus,
    room: &wire::TaskStamp,
    after: wire::Seq,
) -> Result<Vec<wire::Envelope<wire::AnyFact>>, Outcome<CommandAnswer>> {
    bounded_room_publications(bus, room, Some(after))
}

fn bounded_room_publications(
    bus: &Bus,
    room: &wire::TaskStamp,
    after: Option<wire::Seq>,
) -> Result<Vec<wire::Envelope<wire::AnyFact>>, Outcome<CommandAnswer>> {
    let room = room.clone();
    let (rows, more) = bus
        .read(move |read| {
            let mut rows = Vec::new();
            let mut after = after;
            for _ in 0..ROOM_PUBLICATION_PAGES {
                let page =
                    read.room_publications(&room, after, journal::FactReadBudget::RETAINED)?;
                rows.extend(page.rows);
                let Some(continuation) = page.continuation else {
                    return Ok((rows, false));
                };
                after = Some(continuation);
            }
            Ok((rows, true))
        })
        .map_err(|error| store_fault("room::publications-unreadable", &error.to_string()))?;
    if more {
        return Err(refusal(
            "room::attention-budget",
            Some(format!(
                "room publications exceed {} rows or {} bounded pages",
                spice::rooms::MAX_ROOM_ATTENTION_SIGNALS,
                ROOM_PUBLICATION_PAGES
            )),
            "reduce retained room traffic before publishing another message",
        ));
    }
    Ok(rows)
}

/// One room's listeners: lanes with native work or duties in it, lanes that
/// posted in it within the activity window, lanes a `MSG ADDRESS#ROOM` route
/// addressed within that window, and lanes a recorded Design request asked
/// about it. An addressed or asked lane hears the room without becoming a
/// participant until it posts; a lane's leave ends its activity, earlier
/// invitations, and earlier requests, never its native work or duties.
pub(crate) fn project_attention(
    bus: &Bus,
    entries: &[Entry],
    residency: &wire::ResidencyHead,
    room: &wire::TaskStamp,
    publications: &[wire::Envelope<wire::AnyFact>],
    at: Instant,
) -> Result<spice::rooms::RoomAttention, Outcome<CommandAnswer>> {
    let work = entries
        .iter()
        .filter_map(|entry| RoomWork::from_task(room, entry))
        .collect::<Vec<_>>();
    let since = at
        .get()
        .checked_sub(jiff::SignedDuration::from_secs(ROOM_ACTIVITY_SECONDS))
        .map_err(|error| environment_fault("room::activity-window-invalid", &error.to_string()))?;
    let since = Instant::new(since)
        .map_err(|error| environment_fault("room::activity-window-invalid", &error.to_string()))?;
    let mut duties = room_invitations(bus, room, residency, since)?;
    duties.extend(room_asks(bus, room)?);
    duties.extend(room_native_duties(bus, room)?);
    derive_room_attention(
        room,
        since,
        &RoomProjectionSources {
            publications: Declared::Available(publications),
            work: Declared::Available(&work),
            duties: Declared::Available(&duties),
            responsibilities: Declared::Available(&[]),
            residency: Declared::Available(residency),
        },
        RoomProjectionBudget::STANDARD,
    )
    .map_err(projection_refusal)
}

/// Each lane whose newest `MSG`/`ASK` route into `room` was told at or after
/// `since`; the bounded read touches no room publication history.
fn room_invitations(
    bus: &Bus,
    room: &wire::TaskStamp,
    residency: &wire::ResidencyHead,
    since: Instant,
) -> Result<Vec<RoomDuty>, Outcome<CommandAnswer>> {
    let lanes = residency
        .lanes
        .iter()
        .map(|row| row.lane)
        .collect::<Vec<_>>();
    let probe = room.clone();
    let heads = bus
        .read(move |read| {
            lanes
                .into_iter()
                .map(|lane| read.latest_room_route(lane, &probe))
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|error| store_fault("room::invitation-unreadable", &error.to_string()))?;
    Ok(heads
        .into_iter()
        .flatten()
        .filter(|head| head.body().published >= since)
        .map(|head| RoomDuty {
            room: room.clone(),
            lane: head.body().lane,
            kind: RoomDutyKind::Invitation {
                key: head.body().key.clone(),
                at: head.body().published,
            },
            standing: RoomDutyStanding::Owed,
        })
        .collect())
}

/// The newest recorded Design request that asked each lane about `room`,
/// read through the per-Design notice index; a non-Design room has none.
fn room_asks(bus: &Bus, room: &wire::TaskStamp) -> Result<Vec<RoomDuty>, Outcome<CommandAnswer>> {
    let probe = room.clone();
    let (rows, more) = bus
        .read(move |read| {
            let mut rows = Vec::new();
            let mut after = None;
            for _ in 0..ROOM_PUBLICATION_PAGES {
                let page = read.design_notices(&probe, after, journal::FactReadBudget::RETAINED)?;
                rows.extend(page.rows);
                let Some(continuation) = page.continuation else {
                    return Ok((rows, false));
                };
                after = Some(continuation);
            }
            Ok((rows, true))
        })
        .map_err(|error| store_fault("room::asks-unreadable", &error.to_string()))?;
    if more {
        return Err(refusal(
            "room::attention-budget",
            Some(format!(
                "Design requests for {room} exceed {} bounded pages",
                ROOM_PUBLICATION_PAGES
            )),
            "reduce retained room traffic before retrying",
        ));
    }
    let mut newest = std::collections::BTreeMap::new();
    for envelope in rows {
        if let Some(noticed) = envelope.fact.read::<wire::rooms::DesignNoticed>()
            && noticed.design == *room
        {
            newest.insert(noticed.lane, (noticed.task, envelope.at));
        }
    }
    Ok(newest
        .into_iter()
        .map(|(lane, (task, at))| RoomDuty {
            room: room.clone(),
            lane,
            kind: RoomDutyKind::Asked { task, at },
            standing: RoomDutyStanding::Owed,
        })
        .collect())
}

/// Every native duty bound to `room` that no leave can end: each live route
/// told into it is an owed receipt, each live coordination or stop request
/// addresses its lane, and each pending ASK result a lane owes in it.
fn room_native_duties(
    bus: &Bus,
    room: &wire::TaskStamp,
) -> Result<Vec<RoomDuty>, Outcome<CommandAnswer>> {
    let limit = spice::rooms::MAX_ROOM_ATTENTION_SIGNALS;
    let probe = room.clone();
    let (steering, results) = bus
        .read(move |read| {
            Ok((
                read.live_room_steering(&probe, limit)?,
                read.pending_question_results()?,
            ))
        })
        .map_err(|error| store_fault("room::duties-unreadable", &error.to_string()))?;
    if steering.len() >= limit {
        return Err(refusal(
            "room::attention-budget",
            Some(format!("room {room} holds {limit} or more live directives")),
            "settle or cancel live room directives before retrying",
        ));
    }
    let duty = |lane, kind| RoomDuty {
        room: room.clone(),
        lane,
        kind,
        standing: RoomDutyStanding::Owed,
    };
    let mut duties = steering
        .into_iter()
        .map(|head| {
            let head = head.into_body();
            let kind = match head.control.as_deref().map(str::parse) {
                Some(Ok(wire::rooms::CoordinationControl::Coordinate(_))) => {
                    RoomDutyKind::Coordination { key: head.key }
                }
                Some(Ok(wire::rooms::CoordinationControl::Stop { .. })) => {
                    RoomDutyKind::Stop { key: head.key }
                }
                _ => RoomDutyKind::Delivery { key: head.key },
            };
            duty(head.lane, kind)
        })
        .collect::<Vec<_>>();
    duties.extend(results.into_iter().filter_map(|(record, _)| {
        match (record.respondent(), record.context()) {
            (wire::rooms::RoomDestination::Lane { lane }, Some(context)) if context == room => {
                Some(duty(
                    *lane,
                    RoomDutyKind::Result {
                        key: record.result().to_string(),
                    },
                ))
            }
            _ => None,
        }
    }));
    Ok(duties)
}

fn projection_refusal(error: RoomProjectionError) -> Outcome<CommandAnswer> {
    refusal(
        "room::attention-unavailable",
        Some(error.to_string()),
        "repair the named room authority source, then retry the post",
    )
}

/// The first unused publication identity in `room` at or after `micros`.
///
/// Each occupied candidate is a distinct earlier publication, so the probe
/// ends within the room's size; the clock range is the only other limit.
pub(super) fn mint_publication(
    bus: &Bus,
    room: &wire::TaskStamp,
    mut micros: i64,
) -> Result<RoomAuthorityStamp, Outcome<CommandAnswer>> {
    loop {
        let candidate = RoomAuthorityStamp::for_micros(micros).map_err(|error| {
            environment_fault("room::publication-clock-invalid", &error.to_string())
        })?;
        let (probe_room, probe) = (room.clone(), candidate.clone());
        let occupied = bus
            .read(move |read| read.room_publication_seq(&probe_room, &probe))
            .map_err(|error| store_fault("room::publication-unreadable", &error.to_string()))?;
        if occupied.is_none() {
            return Ok(candidate);
        }
        micros = micros.checked_add(1).ok_or_else(|| {
            environment_fault(
                "room::publication-clock-exhausted",
                "room publication clock exceeded the native stamp range",
            )
        })?;
    }
}

fn post_outcome(attention: spice::rooms::RoomAttention) -> RoomPostOutcome {
    let listeners = attention
        .lanes
        .into_iter()
        .filter(|lane| lane.listens())
        .map(|lane| {
            let (availability, delivery) = match lane.availability {
                LaneAvailability::Occupied => (
                    RoomListenerAvailability::Occupied,
                    RoomListenerDelivery::AtSafeBoundary,
                ),
                LaneAvailability::Vacant => (
                    RoomListenerAvailability::Vacant,
                    RoomListenerDelivery::WaitsForNextStart,
                ),
                LaneAvailability::Unavailable { disposition } => (
                    RoomListenerAvailability::Unavailable { disposition },
                    RoomListenerDelivery::PendingAvailability,
                ),
                LaneAvailability::Unknown => (
                    RoomListenerAvailability::Unknown,
                    RoomListenerDelivery::PendingAvailability,
                ),
            };
            RoomPostListener {
                lane: lane.lane,
                availability,
                delivery,
            }
        })
        .collect::<Vec<_>>();
    if listeners.is_empty() {
        RoomPostOutcome::RecordedNoCurrentListener {
            next: RoomPostNext::StartOrDirect,
        }
    } else {
        RoomPostOutcome::RecordedWithListeners { listeners }
    }
}
