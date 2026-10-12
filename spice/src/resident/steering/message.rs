//! Authored MSG packets published as peer messages.
//!
//! The only source is a complete assistant text block on the author's lane
//! tape (`Event::StreamedProse`, OMP's `text_end`). Settled message prose
//! repeats or paraphrases that same block, reasoning is not authored text,
//! and a tool call's intent is a caption, so none of them is a second source.
//! A block is complete before it is decoded, so a truncated body never sends.
//!
//! One packet resolves every recipient against one residency view and
//! journals one `steer::messaged` fact per seated recipient in one guarded
//! write. Each recipient is honored on its own: every named seated occupant
//! receives the packet, and each address the view does not seat receives
//! nothing and is named, with why, in one private notice to the author. Each
//! fact's directive identity is derived from the packet's transcript locus
//! and its recipient, so rereading the same block — live, at exit, or at
//! startup — finds the published directive instead of sending again, while
//! the same body authored at another locus is another message.
//!
//! A packet that publishes to no one journals one `steer::withheld` fact
//! before its author hears the refusal. That fact is the packet's settled
//! judgment: a later reread finds it and leaves the packet unpublished even
//! when the refusal's cause has since changed. A packet the grammar refuses
//! needs no record, since its text refuses identically on every read.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::ops::{ControlFlow, Range};
use std::time::{Duration, Instant as Clock};

use steer::ack::scan::Route;
use wire::rooms::{RoomAuthorityStamp, RoomMessaged, TextRange, TranscriptHunk};
use wire::{OccupantAddress, TaskHandle, TaskStamp};

use super::transitions::admit_send_envelope;
use super::*;

/// Residency views one packet may resolve against before it gives up on a
/// residency that keeps moving under it.
const PUBLISH_ATTEMPTS: usize = 3;

/// The refusal a packet's author hears for each address that received nothing.
const UNAVAILABLE: &str = "msg::recipient-unavailable";

/// Why a peer directive whose lane left the residency was parked.
pub(in crate::resident) const LANE_REMOVED: &str = "steer::recipient-lane-removed";

/// The recent tail startup recovery rereads for packets a crash left behind.
const RECOVERY_WINDOW_BYTES: u64 = 64 * 1024 * 1024;
const RECOVERY_SOURCE_BYTES: u64 = RECOVERY_WINDOW_BYTES + 64 * 1024 * 1024;
const RECOVERY_RECORDS: usize = 4096;
const RECOVERY_PASSES: usize = 4096;
/// Shared by every lane one startup recovery reads.
const RECOVERY_DURATION: Duration = Duration::from_secs(1);

/// Bytes of header and detail one private refusal notice repeats, so the
/// notice fits one kick datagram.
const NOTICE_TEXT_BYTES: usize = 160;

/// One decoded assistant record and the occupant its capture binding names.
pub(in crate::resident) struct AuthoredRecord<'a> {
    pub(in crate::resident) lane: CanonicalUuid,
    /// Occupant whose captured transcript wrote the record.
    pub(in crate::resident) author: CanonicalUuid,
    /// Tape generation holding the record.
    pub(in crate::resident) source: CanonicalUuid,
    pub(in crate::resident) locus: listen::Locus,
    pub(in crate::resident) events: &'a [listen::Event],
}

/// Who hears which addresses a packet did not reach.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::resident) enum Refusals {
    /// The author is seated and receives a private notice.
    Notify,
    /// The author left or this is recovery; only the diagnostic records it.
    Record,
}

/// One well-formed packet ready to resolve.
#[derive(Clone, Debug)]
pub(in crate::resident) struct Packet<'a> {
    pub(in crate::resident) lane: CanonicalUuid,
    pub(in crate::resident) author: CanonicalUuid,
    pub(in crate::resident) locus: TranscriptLocus,
    /// Every route in source order, as its header wrote it.
    pub(in crate::resident) routes: Vec<Route>,
    pub(in crate::resident) body: &'a str,
    /// The body's byte range within its decoded event's text.
    pub(in crate::resident) text: Range<usize>,
}

/// What publishing one packet did.
#[derive(Debug, Eq, PartialEq)]
pub(in crate::resident) enum Published {
    /// The packet committed now: messages to these recipient lanes and one
    /// record in each named room. Each `unreached` entry names one address
    /// that received nothing, and why.
    Sent {
        lanes: Vec<CanonicalUuid>,
        unreached: Vec<String>,
        rooms: Vec<TaskHandle>,
    },
    /// An earlier read of the same packet already published it.
    Already,
}

/// Why one packet published nothing.
#[derive(Debug, PartialEq)]
pub(in crate::resident) enum Unpublished {
    /// The packet itself, or the residency it named, refuses publication.
    Refused { reason: String, detail: String },
    /// An earlier judgment refused this packet; it stays unpublished.
    Withheld,
    /// The resident could not decide; the packet may publish on a later read.
    Failed(Outcome<CommandAnswer>),
}

/// Publish every MSG packet one authored record holds.
///
/// Returns each packet's outcome in source order. Recipients are told their
/// inboxes changed; each address a packet did not reach is written to
/// `diagnostic` and, under [`Refusals::Notify`], privately to its author.
pub(in crate::resident) fn publish_authored(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    config: &steer::Config,
    record: &AuthoredRecord<'_>,
    refusals: Refusals,
    diagnostic: &mut dyn io::Write,
) -> Vec<Result<Published, Unpublished>> {
    publish_record(
        bus,
        paths,
        epoch,
        config,
        record,
        &BTreeSet::new(),
        refusals,
        diagnostic,
    )
}

/// Publish one record's packets, leaving those `withheld` names unpublished.
#[expect(
    clippy::too_many_arguments,
    reason = "one record's publication carries its judged packets beside the shared resident inputs"
)]
fn publish_record(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    config: &steer::Config,
    record: &AuthoredRecord<'_>,
    withheld: &BTreeSet<TranscriptLocus>,
    refusals: Refusals,
    diagnostic: &mut dyn io::Write,
) -> Vec<Result<Published, Unpublished>> {
    let mut outcomes = Vec::new();
    for (event, text) in authored_blocks(record.events) {
        for message in steer::ack::scan::scan_messages(text) {
            let place = steer::Locus::at(text, message.header.start);
            let outcome = match packet_locus(record, event, place) {
                Err(outcome) => Err(Unpublished::Failed(outcome)),
                Ok(locus) if withheld.contains(&locus) => Err(Unpublished::Withheld),
                Ok(locus) => {
                    let outcome = match &message.publication {
                        Err(refusal) => Err(Unpublished::Refused {
                            reason: refusal.reason().to_owned(),
                            detail: refusal_detail(text, refusal),
                        }),
                        Ok(publication) => settle(
                            bus,
                            epoch,
                            record,
                            &locus,
                            publish_resolved(
                                bus,
                                epoch,
                                config,
                                &Packet {
                                    lane: record.lane,
                                    author: record.author,
                                    locus: locus.clone(),
                                    routes: publication.routes.clone(),
                                    body: &text[publication.body.clone()],
                                    text: publication.body.clone(),
                                },
                            ),
                        ),
                    };
                    report(
                        paths,
                        record,
                        &locus,
                        &text[message.header.clone()],
                        &outcome,
                        refusals,
                        diagnostic,
                    );
                    outcome
                }
            };
            if let Err(Unpublished::Failed(failure)) = &outcome {
                let _ = writeln!(
                    diagnostic,
                    "lane {} MSG at record {}:{} event {event} line {place} not published: {}",
                    record.lane,
                    record.locus.member,
                    record.locus.offset,
                    describe(failure).1,
                );
            }
            outcomes.push(outcome);
        }
    }
    outcomes
}

/// Reread each lane's recent tail for packets a crash left unpublished.
///
/// Live publication commits after the sweep that accounted its record, so a
/// resident that stopped between the two leaves a packet on the tape with no
/// message. Every record after a lane's current session declaration was
/// written by the occupant that declared it; rereading them publishes what is
/// missing, finds the rest already published, and leaves every packet an
/// earlier judgment withheld unpublished. Refusals are diagnostics only: the
/// author heard them live, or is gone.
///
/// Recovery runs before this resident seats any new occupant, so a packet a
/// crash left unjudged resolves against no occupant born after the previous
/// resident stopped.
pub(in crate::resident) fn publish_at_startup(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    config: &steer::Config,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let residency = current_residency(bus, epoch)?;
    let view = bus
        .task_snapshot()
        .map_err(|error| store_fault("steer::residency-unreadable", &error.to_string()))?;
    let deadline = Clock::now() + RECOVERY_DURATION;
    for row in &residency.body().lanes {
        let Some(session) = row
            .session
            .as_ref()
            .filter(|session| session.lane == row.lane)
        else {
            continue;
        };
        if let Err(detail) = recover_lane(
            bus, paths, epoch, config, &view, row, session, deadline, diagnostic,
        ) {
            let _ = writeln!(
                diagnostic,
                "lane {} MSG recovery incomplete: {detail}",
                row.lane
            );
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "one lane's recovery reads the residency row its session names"
)]
fn recover_lane(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    config: &steer::Config,
    view: &bus::TaskSnapshot,
    row: &wire::LaneResidency,
    session: &wire::LaneSession,
    deadline: Clock,
    diagnostic: &mut dyn io::Write,
) -> Result<(), String> {
    // An occupant with no birth on its lane authors nothing publishable.
    let Ok(birth) = view.facts().occupant_address(row.lane, session.occupant) else {
        return Ok(());
    };
    let withheld = withheld_packets(bus, row.lane, session.occupant, birth)?;
    let path = paths
        .tape
        .join(row.lane.to_string())
        .join(row.vendor.as_str());
    if !path.is_dir() {
        return Ok(());
    }
    let registry = spice::extension::registry().map_err(|error| error.to_string())?;
    let Some(vendor) = registry.vendor(row.vendor.as_str()) else {
        return Ok(());
    };
    let remaining = deadline.saturating_duration_since(Clock::now());
    let budget = listen::TailBudget::new(
        RECOVERY_WINDOW_BYTES,
        RECOVERY_SOURCE_BYTES,
        RECOVERY_RECORDS,
        RECOVERY_PASSES,
        remaining,
    )
    .map_err(|error| error.to_string())?;
    let declared = (session.locus.member, session.locus.offset);
    let mut newest_first = Vec::new();
    let search = listen::search_tail(
        &path,
        vendor.dialect(),
        steer::ack::scan::message::MARKER.as_bytes(),
        budget,
        |record| {
            if (record.locus.member, record.locus.offset) <= declared {
                return ControlFlow::Break(());
            }
            if let listen::RecordPayload::Events(events) = record.payload {
                newest_first.push((
                    record.locus,
                    events
                        .into_iter()
                        .map(|sourced| sourced.event)
                        .collect::<Vec<_>>(),
                ));
            }
            ControlFlow::Continue(())
        },
    )
    .map_err(|error| error.to_string())?;
    let source = search
        .source
        .as_str()
        .parse::<CanonicalUuid>()
        .map_err(|error| error.to_string())?;
    for (locus, events) in newest_first.iter().rev() {
        publish_record(
            bus,
            paths,
            epoch,
            config,
            &AuthoredRecord {
                lane: row.lane,
                author: session.occupant,
                source,
                locus: *locus,
                events,
            },
            &withheld,
            Refusals::Record,
            diagnostic,
        );
    }
    Ok(())
}

/// Every packet locus an earlier judgment withheld from one occupant.
///
/// An occupant's packets are judged only after its birth, so the kind index
/// is read from there.
fn withheld_packets(
    bus: &Bus,
    lane: CanonicalUuid,
    author: CanonicalUuid,
    birth: OccupantAddress,
) -> Result<BTreeSet<TranscriptLocus>, String> {
    bus.read(move |read| {
        let mut withheld = BTreeSet::new();
        let mut after = Some(birth.birth());
        loop {
            let page = read.selected_fact_page(
                &[wire::FactKind::SteerWithheld],
                after,
                None,
                journal::FactReadBudget::RETAINED,
            )?;
            withheld.extend(page.rows.iter().filter_map(|envelope| {
                let body = envelope.fact.read::<wire::body::SteerWithheld>()?;
                (body.from == lane && body.sender == author).then_some(body.locus)
            }));
            match page.continuation {
                Some(continuation) => after = Some(continuation),
                None => return Ok(withheld),
            }
        }
    })
    .map_err(|error| error.to_string())
}

/// Record a packet the residency refused as withheld before anyone hears it.
///
/// A refusal that cannot be recorded is undecided: the author is not told,
/// and a later read may judge the packet again.
fn settle(
    bus: &Bus,
    epoch: CanonicalUuid,
    record: &AuthoredRecord<'_>,
    locus: &TranscriptLocus,
    outcome: Result<Published, Unpublished>,
) -> Result<Published, Unpublished> {
    let Err(Unpublished::Refused { reason, detail }) = outcome else {
        return outcome;
    };
    let at = current_instant().map_err(Unpublished::Failed)?;
    let origin =
        resident_origin(epoch).map_err(|outcome| Unpublished::Failed(typed_outcome(outcome)))?;
    let mut write = JournalWrite::new(Actor::Lane(record.lane), origin, at, Producer::AckScanner);
    write
        .fact(
            wire::body::SteerWithheld {
                from: record.lane,
                sender: record.author,
                locus: *locus,
                reason: reason.clone(),
                detail: detail.clone(),
            },
            FactCause::Root,
        )
        .map_err(|error| {
            Unpublished::Failed(typed_store_fault("steer::fact-invalid", &error.to_string()))
        })?;
    commit(bus, write).map_err(|outcome| Unpublished::Failed(typed_outcome(outcome)))?;
    Err(Unpublished::Refused { reason, detail })
}

/// Publish one packet, resolving again when the residency moves under it.
fn publish_resolved(
    bus: &Bus,
    epoch: CanonicalUuid,
    config: &steer::Config,
    packet: &Packet<'_>,
) -> Result<Published, Unpublished> {
    let mut attempt = 1;
    loop {
        let view = bus.task_snapshot().map_err(|error| {
            Unpublished::Failed(typed_store_fault(
                "steer::residency-unreadable",
                &error.to_string(),
            ))
        })?;
        match publish_packet(bus, epoch, config, packet, &view) {
            Err(Unpublished::Refused { reason, .. })
                if attempt < PUBLISH_ATTEMPTS && moved(&reason) =>
            {
                attempt += 1;
            }
            outcome => return outcome,
        }
    }
}

/// Whether a commit refusal means the view went stale rather than the packet
/// being wrong.
fn moved(reason: &str) -> bool {
    matches!(reason, "steer::residency-moved" | "steer::identity-taken")
}

/// Resolve one packet against one residency view and journal a message to
/// every seated recipient and a record in every named room in one guarded
/// write.
///
/// The guard re-proves at commit that the residency is the one the view
/// read and that every recipient lane it publishes to still seats the
/// resolved occupant, so a seat change between read and commit refuses the
/// whole write instead of redirecting any message. A recipient the view does
/// not seat receives nothing and is named in the outcome. Every `#` context
/// must name a task the view holds, or the whole packet refuses; each room a
/// packet names records the packet's exact transcript bytes once, whether or
/// not a recipient hears it. A packet that reaches no recipient and names no
/// room publishes nothing.
pub(in crate::resident) fn publish_packet(
    bus: &Bus,
    epoch: CanonicalUuid,
    config: &steer::Config,
    packet: &Packet<'_>,
    view: &bus::TaskSnapshot,
) -> Result<Published, Unpublished> {
    let directed = packet
        .routes
        .iter()
        .filter_map(|route| Some((route.address?, route.context.as_ref())))
        .collect::<Vec<_>>();
    let identities = directed
        .iter()
        .map(|(recipient, _)| message_identity(&packet.locus, *recipient))
        .collect::<Vec<_>>();
    let mut minted_micros = None;
    for directive in &identities {
        let (existing, minted) = keyspace(bus, *directive).map_err(Unpublished::Failed)?;
        if existing.is_some() {
            return Ok(Published::Already);
        }
        minted_micros = minted;
    }
    let Some(residency) = view
        .residency()
        .filter(|residency| residency.body().epoch == epoch)
    else {
        return Err(Unpublished::Failed(typed_store_fault(
            "steer::residency-missing",
            "no current residency names this resident generation",
        )));
    };
    let sender = view
        .facts()
        .occupant_address(packet.lane, packet.author)
        .map_err(|refusal| Unpublished::Refused {
            reason: "msg::sender-unproved".to_owned(),
            detail: refusal.to_string(),
        })?;
    if directed.iter().any(|(recipient, _)| *recipient == sender) {
        return Err(Unpublished::Refused {
            reason: "msg::recipient-self".to_owned(),
            detail: format!("{sender} is the authoring occupant's own address"),
        });
    }
    let Some(record) = resolve_rooms(bus, packet, view, sender)? else {
        return Ok(Published::Already);
    };
    let (targets, unreached) =
        resolve_targets(bus, &residency.body().lanes, view, &directed, &identities)?;
    if targets.is_empty() && record.rooms.is_empty() {
        return Err(Unpublished::Refused {
            reason: UNAVAILABLE.to_owned(),
            detail: unreached.join("; "),
        });
    }
    let at = current_instant().map_err(Unpublished::Failed)?;
    let expires = expiry(at, config.expiry().get()).map_err(Unpublished::Failed)?;
    let origin =
        resident_origin(epoch).map_err(|outcome| Unpublished::Failed(typed_outcome(outcome)))?;
    let mut proposal = JournalWrite::new(
        Actor::Lane(packet.lane),
        origin.clone(),
        at,
        Producer::AckScanner,
    );
    for target in &targets {
        let key = mint_key(minted_micros).map_err(Unpublished::Failed)?;
        minted_micros = key.minted_micros();
        proposal.guard(send_guard(
            residency.position(),
            Witness::Residency {
                lane: target.lane,
                occupant: Some(target.occupant),
                epoch,
            },
            target.directive,
            key.clone(),
        ));
        let messaged = wire::body::SteerMessaged {
            directive: target.directive,
            key: key.to_string(),
            lane: target.lane,
            occupant: target.occupant,
            from: packet.lane,
            sender: packet.author,
            text: packet.body.to_owned(),
            locus: packet.locus.clone(),
            room: target.room.clone(),
            expires,
        };
        let pending = proposal
            .fact(messaged.clone(), FactCause::Existing(residency.position()))
            .map_err(fact_invalid)?;
        let origin = origin.clone();
        proposal.steering_head(pending, move |seq| {
            steer::Ledger::sent_head(&messaged.as_sent(), origin, at, seq)
                .expect("the resolved message body builds its authority head")
        });
    }
    stage_rooms(&mut proposal, residency.position(), &record, &targets)?;
    admit_send_envelope(&proposal).map_err(unpublished)?;
    commit(bus, proposal).map_err(|outcome| unpublished(typed_outcome(outcome)))?;
    Ok(Published::Sent {
        lanes: targets.iter().map(|target| target.lane).collect(),
        unreached,
        rooms: record.rooms.into_iter().map(|(handle, _)| handle).collect(),
    })
}

fn fact_invalid(error: impl std::fmt::Display) -> Unpublished {
    Unpublished::Failed(typed_store_fault("steer::fact-invalid", &error.to_string()))
}

/// Each distinct room a packet names, as its header spelled the task and by
/// the task's birth stamp, in source order.
type Rooms = Vec<(TaskHandle, TaskStamp)>;

/// One seated recipient a directed route reaches.
struct Target {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    directive: CanonicalUuid,
    /// The room the route's context freezes into its message.
    room: Option<TaskStamp>,
}

/// What the rooms a packet names record of it: each room, and, when it names
/// any, the packet's exact transcript hunk and authoring occupant.
struct RoomRecord {
    rooms: Rooms,
    hunk: Option<TranscriptHunk>,
    author: CanonicalUuid,
}

/// Every room the packet names and, when it names any, the packet's exact
/// transcript hunk; `None` when the packet already recorded these bytes.
///
/// Callers find a published directed route first; a packet whose directed
/// routes all missed seated recipients leaves its room record as the only
/// proof that it published.
fn resolve_rooms(
    bus: &Bus,
    packet: &Packet<'_>,
    view: &bus::TaskSnapshot,
    sender: OccupantAddress,
) -> Result<Option<RoomRecord>, Unpublished> {
    let rooms = packet_rooms(packet, view)?;
    let hunk = match rooms.first() {
        None => None,
        Some((_, room)) => {
            let hunk = packet_hunk(bus, packet)?;
            if recorded(bus, room, sender, &hunk)? {
                return Ok(None);
            }
            Some(hunk)
        }
    };
    Ok(Some(RoomRecord {
        rooms,
        hunk,
        author: packet.author,
    }))
}

/// Each directed route's seated target, in route order, and why each
/// unseated address receives nothing.
fn resolve_targets(
    bus: &Bus,
    residency: &[wire::LaneResidency],
    view: &bus::TaskSnapshot,
    directed: &[(OccupantAddress, Option<&TaskHandle>)],
    identities: &[CanonicalUuid],
) -> Result<(Vec<Target>, Vec<String>), Unpublished> {
    let lanes = seats(residency, view);
    let seated = lanes
        .iter()
        .filter_map(|(lane, seat)| {
            let (occupant, address) = (*seat)?;
            Some((address?, (*lane, occupant)))
        })
        .collect::<BTreeMap<_, _>>();
    let mut targets = Vec::with_capacity(directed.len());
    let mut unreached = Vec::new();
    for ((recipient, context), directive) in directed.iter().zip(identities) {
        match seated.get(recipient) {
            Some((lane, occupant)) => targets.push(Target {
                lane: *lane,
                occupant: *occupant,
                directive: *directive,
                room: context_room(*context),
            }),
            None => unreached.push(unavailable(bus, &lanes, *recipient)?),
        }
    }
    Ok((targets, unreached))
}

/// Stage one `room::messaged` record of the packet in each room, naming the
/// directives its seated routes into that room opened.
fn stage_rooms(
    proposal: &mut JournalWrite,
    residency: wire::Seq,
    record: &RoomRecord,
    targets: &[Target],
) -> Result<(), Unpublished> {
    let Some(hunk) = &record.hunk else {
        return Ok(());
    };
    let moment = jiff::Timestamp::now().as_microsecond();
    for ((_, room), offset) in record.rooms.iter().zip(0_i64..) {
        let publication = RoomAuthorityStamp::for_micros(moment + offset).map_err(|error| {
            Unpublished::Failed(typed_store_fault(
                "msg::room-stamp-invalid",
                &error.to_string(),
            ))
        })?;
        proposal
            .fact(
                RoomMessaged {
                    room: room.clone(),
                    publication,
                    hunk: hunk.clone(),
                    author: record.author,
                    directed: targets
                        .iter()
                        .filter(|target| target.room.as_ref() == Some(room))
                        .map(|target| target.directive)
                        .collect(),
                },
                FactCause::Existing(residency),
            )
            .map_err(fact_invalid)?;
    }
    Ok(())
}

/// The room a directed route's context freezes into its message.
fn context_room(context: Option<&TaskHandle>) -> Option<TaskStamp> {
    context.and_then(|handle| handle.born().ok())
}

/// Every distinct room the packet's routes name, in source order, each
/// resolved by birth stamp against `view`; a context naming no task refuses
/// the whole packet.
fn packet_rooms(packet: &Packet<'_>, view: &bus::TaskSnapshot) -> Result<Rooms, Unpublished> {
    let mut rooms = Rooms::new();
    for handle in packet
        .routes
        .iter()
        .filter_map(|route| route.context.as_ref())
    {
        let born = handle
            .born()
            .ok()
            .filter(|born| view.task_heads(born).is_some())
            .ok_or_else(|| Unpublished::Refused {
                reason: "msg::context-unknown".to_owned(),
                detail: format!("{handle} names no task"),
            })?;
        if rooms.iter().all(|(_, known)| *known != born) {
            rooms.push((handle.clone(), born));
        }
    }
    Ok(rooms)
}

/// The packet body's exact bytes in its author's accounted transcript.
///
/// Publication follows the sweep that accounted the record, so a record the
/// lane's accounted generation does not yet cover is undecided, not refused:
/// a later read publishes it.
fn packet_hunk(bus: &Bus, packet: &Packet<'_>) -> Result<TranscriptHunk, Unpublished> {
    let locus = &packet.locus;
    let unaccounted = || {
        Unpublished::Failed(typed_store_fault(
            "msg::transcript-unaccounted",
            &format!(
                "lane {} record {}:{} has no accounted transcript generation",
                packet.lane, locus.member, locus.offset
            ),
        ))
    };
    let source = locus.source.ok_or_else(unaccounted)?;
    let lane = packet.lane;
    let head = bus
        .read(move |read| read.transcript_source_heads(&lane))
        .map_err(|error| {
            Unpublished::Failed(typed_store_fault(
                "steer::transcript-unreadable",
                &error.to_string(),
            ))
        })?
        .into_iter()
        .map(journal::JournalHead::into_body)
        .find(|head| {
            let end = u64::from(locus.offset) + u64::from(locus.length);
            head.source == source
                && (locus.member < head.next.member
                    || (locus.member == head.next.member && end <= head.next.offset))
        })
        .ok_or_else(unaccounted)?;
    let offset = |at: usize| {
        u64::try_from(at).map_err(|error| {
            Unpublished::Failed(typed_protocol_fault(
                "steer::transcript-locus-invalid",
                &error.to_string(),
            ))
        })
    };
    Ok(TranscriptHunk {
        head,
        locus: wire::MessageLocus {
            member: locus.member,
            offset: locus.offset,
            length: locus.length,
            event: locus.event,
        },
        text: TextRange {
            start: offset(packet.text.start)?,
            end: offset(packet.text.end)?,
        },
    })
}

/// Whether `room` already records these exact bytes since the author's birth.
///
/// One packet's routes commit in one write, so finding its record in any one
/// room proves the whole packet published.
fn recorded(
    bus: &Bus,
    room: &TaskStamp,
    author: OccupantAddress,
    hunk: &TranscriptHunk,
) -> Result<bool, Unpublished> {
    let room = room.clone();
    let hunk = hunk.clone();
    bus.read(move |read| {
        let mut after = Some(author.birth());
        loop {
            let page = read.room_publications(&room, after, journal::FactReadBudget::RETAINED)?;
            if page.rows.iter().any(|envelope| {
                envelope
                    .fact
                    .read::<RoomMessaged>()
                    .is_some_and(|body| body.hunk.same_bytes(&hunk))
            }) {
                return Ok(true);
            }
            match page.continuation {
                Some(continuation) => after = Some(continuation),
                None => return Ok(false),
            }
        }
    })
    .map_err(|error| {
        Unpublished::Failed(typed_store_fault(
            "steer::room-unreadable",
            &error.to_string(),
        ))
    })
}

/// The one directive identity a packet's message to one recipient can own.
fn message_identity(locus: &TranscriptLocus, recipient: OccupantAddress) -> CanonicalUuid {
    let source = locus
        .source
        .map_or_else(String::new, |source| source.to_string());
    let identity = format!(
        "spice:msg:{source}:{}:{}:{}:{}:{}:{recipient}",
        locus.member, locus.offset, locus.event, locus.line, locus.column,
    );
    let digest = blake3::hash(identity.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    CanonicalUuid::new(uuid::Uuid::from_bytes(bytes))
}

/// Each residency lane's occupant and, when proved, that occupant's address.
type Seats = BTreeMap<CanonicalUuid, Option<(CanonicalUuid, Option<OccupantAddress>)>>;

/// Every residency lane's occupant and its address as `view` proves it.
fn seats(lanes: &[wire::LaneResidency], view: &bus::TaskSnapshot) -> Seats {
    lanes
        .iter()
        .map(|row| {
            let seat = row.occupant.map(|occupant| {
                let address = view.facts().occupant_address(row.lane, occupant).ok();
                (occupant, address)
            });
            (row.lane, seat)
        })
        .collect()
}

/// Why a recipient the view does not seat cannot hear the packet, led by its
/// address so a bounded notice still names it. An expired address also names
/// what its birth lane seats in the same view, so its author can address the
/// lane's current occupant; the packet itself never follows the lane.
fn unavailable(
    bus: &Bus,
    lanes: &Seats,
    recipient: OccupantAddress,
) -> Result<String, Unpublished> {
    match bus.resolve_occupant(recipient) {
        Ok(Err(refusal @ bus::AddressRefusal::Expired { birth })) => {
            let now = match lanes.get(&birth.lane) {
                Some(Some((_, Some(current)))) => format!("its lane seats {current}"),
                Some(Some((_, None))) => "its lane seats an unproved occupant".to_owned(),
                Some(None) => "its lane is vacant".to_owned(),
                None => "its lane left the residency".to_owned(),
            };
            Ok(format!("{recipient} {} ({now})", refusal.reason()))
        }
        Ok(Err(refusal)) => Ok(format!("{recipient} {}", refusal.reason())),
        Ok(Ok(_)) => Ok(format!("{recipient} msg::recipient-unseated")),
        Err(error) => Err(Unpublished::Failed(typed_store_fault(
            "steer::address-unreadable",
            &error.to_string(),
        ))),
    }
}

fn unpublished(outcome: Outcome<CommandAnswer>) -> Unpublished {
    match outcome {
        Outcome::Refusal(_) => {
            let (reason, detail) = describe(&outcome);
            Unpublished::Refused { reason, detail }
        }
        outcome => Unpublished::Failed(outcome),
    }
}

fn describe(outcome: &Outcome<CommandAnswer>) -> (String, String) {
    match outcome {
        Outcome::Refusal(refusal) => (
            refusal.reason().to_owned(),
            refusal
                .diagnostic()
                .and_then(wire::RefusalDiagnostic::detail)
                .unwrap_or_else(|| refusal.reason())
                .to_owned(),
        ),
        Outcome::Fault(fault) => (
            fault.reason.clone(),
            format!("{}: {}", fault.reason, fault.detail),
        ),
        Outcome::Reply(_) => ("reply".to_owned(), "unexpected reply".to_owned()),
    }
}

/// Each complete assistant text block that could hold a packet, by event.
fn authored_blocks(events: &[listen::Event]) -> impl Iterator<Item = (usize, &str)> {
    events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| match event {
            listen::Event::StreamedProse(prose)
                if prose.role == listen::ProseRole::Assistant
                    && prose.text.contains(steer::ack::scan::message::MARKER) =>
            {
                Some((index, prose.text.as_str()))
            }
            _ => None,
        })
}

fn packet_locus(
    record: &AuthoredRecord<'_>,
    event: usize,
    place: steer::Locus,
) -> Result<TranscriptLocus, Outcome<CommandAnswer>> {
    let part = |part: usize| {
        u32::try_from(part).map_err(|error| {
            typed_protocol_fault("steer::transcript-locus-invalid", &error.to_string())
        })
    };
    Ok(TranscriptLocus {
        source: Some(record.source),
        member: record.locus.member,
        offset: record.locus.offset,
        length: record.locus.length,
        event: part(event)?,
        line: part(place.line)?,
        column: part(place.column)?,
    })
}

fn refusal_detail(text: &str, refusal: &steer::ack::scan::MessageRefusal) -> String {
    use steer::ack::scan::MessageRefusal;
    match refusal {
        MessageRefusal::Unterminated => "the selector list is not closed by a colon".to_owned(),
        MessageRefusal::Recipient { span, error } => {
            format!(
                "`{}` is not an occupant address: {error}",
                &text[span.clone()]
            )
        }
        MessageRefusal::Context { span, .. } => {
            format!("`#{}` is not a task handle", &text[span.clone()])
        }
        MessageRefusal::Repeated { address, .. } => format!("{address} is named more than once"),
        MessageRefusal::RepeatedContext { context, .. } => {
            format!("#{context} names a room another room-only route already names")
        }
        MessageRefusal::EmptyBody => "the packet has no body".to_owned(),
    }
}

/// Tell the recipients, or the author and the diagnostic, what one packet did.
fn report(
    paths: &RuntimePaths,
    record: &AuthoredRecord<'_>,
    locus: &TranscriptLocus,
    header: &str,
    outcome: &Result<Published, Unpublished>,
    refusals: Refusals,
    diagnostic: &mut dyn io::Write,
) {
    let teller = Teller {
        paths,
        record,
        locus,
        header,
        refusals,
    };
    match outcome {
        Ok(Published::Sent {
            lanes,
            unreached,
            rooms,
        }) => {
            for lane in lanes {
                notify_inbox(paths, *lane);
            }
            if !rooms.is_empty() {
                teller.recorded(rooms, lanes.len(), diagnostic);
            }
            if !unreached.is_empty() {
                teller.tell(UNAVAILABLE, &unreached.join("; "), lanes.len(), diagnostic);
            }
        }
        Ok(Published::Already) | Err(Unpublished::Withheld | Unpublished::Failed(_)) => {}
        Err(Unpublished::Refused { reason, detail }) => {
            teller.tell(reason, detail, 0, diagnostic);
        }
    }
}

/// Where one packet's outcome is told.
struct Teller<'a> {
    paths: &'a RuntimePaths,
    record: &'a AuthoredRecord<'a>,
    locus: &'a TranscriptLocus,
    header: &'a str,
    refusals: Refusals,
}

impl Teller<'_> {
    /// Stable identity of the packet's source locus.
    fn packet(&self) -> String {
        let locus = self.locus;
        format!(
            "{}:{}:{}:{}:{}",
            locus.member, locus.offset, locus.event, locus.line, locus.column
        )
    }

    /// Record the refusal and, under [`Refusals::Notify`], tell the author
    /// privately, with how many recipients the packet did reach.
    fn tell(&self, reason: &str, detail: &str, reached: usize, diagnostic: &mut dyn io::Write) {
        let (record, locus) = (self.record, self.locus);
        let _ = writeln!(
            diagnostic,
            "lane {} MSG at record {}:{} event {} line {}:{} reached {reached} and refused: \
             {reason}: {detail}",
            record.lane, locus.member, locus.offset, locus.event, locus.line, locus.column,
        );
        if self.refusals == Refusals::Notify {
            crate::resident::supervisor::command::message_refused(
                self.paths,
                record.lane,
                MessageRefusalNotice {
                    occupant: record.author,
                    packet: self.packet(),
                    header: bounded(self.header),
                    line: locus.line,
                    column: locus.column,
                    reached,
                    reason: reason.to_owned(),
                    detail: bounded(detail),
                },
            );
        }
    }

    /// Record which rooms the packet recorded in and, under
    /// [`Refusals::Notify`], tell the author privately; no participant
    /// receipt is implied.
    fn recorded(&self, rooms: &[TaskHandle], reached: usize, diagnostic: &mut dyn io::Write) {
        let (record, locus) = (self.record, self.locus);
        let rooms = rooms.iter().map(ToString::to_string).collect::<Vec<_>>();
        let _ = writeln!(
            diagnostic,
            "lane {} MSG at record {}:{} event {} line {}:{} recorded in {}",
            record.lane,
            locus.member,
            locus.offset,
            locus.event,
            locus.line,
            locus.column,
            rooms.join(", "),
        );
        if self.refusals == Refusals::Notify {
            crate::resident::supervisor::command::message_recorded(
                self.paths,
                record.lane,
                MessageRecordedNotice {
                    occupant: record.author,
                    packet: self.packet(),
                    header: bounded(self.header),
                    line: locus.line,
                    column: locus.column,
                    rooms,
                    reached,
                },
            );
        }
    }
}

/// One private record of the rooms an MSG packet recorded in, scoped to the
/// packet's authoring occupant.
#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::resident) struct MessageRecordedNotice {
    pub(in crate::resident) occupant: CanonicalUuid,
    /// Stable identity of the recorded packet's source locus.
    pub(in crate::resident) packet: String,
    /// The packet header as authored, bounded.
    pub(in crate::resident) header: String,
    pub(in crate::resident) line: u32,
    pub(in crate::resident) column: u32,
    /// Each room the packet recorded in, as its header spelled the task.
    pub(in crate::resident) rooms: Vec<String>,
    /// How many named recipients the packet reached.
    pub(in crate::resident) reached: usize,
}

impl MessageRecordedNotice {
    /// The notice its author hears.
    pub(in crate::resident) fn text(&self) -> String {
        let reached = match self.reached {
            0 => String::new(),
            1 => " and reached 1 recipient".to_owned(),
            reached => format!(" and reached {reached} recipients"),
        };
        format!(
            "<system-notice>\nYour MSG packet `{}` at line {}, column {} of its assistant \
             text block was recorded in room {}{reached}. Recorded means the room holds it; \
             no participant has received or acknowledged it, and nothing woke anyone. This \
             is a private notice; no acknowledgment key or visible ACK/NAK is \
             required.\n</system-notice>",
            self.header,
            self.line,
            self.column,
            self.rooms.join(", "),
        )
    }
}

/// One private MSG refusal, scoped to the packet's authoring occupant.
#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::resident) struct MessageRefusalNotice {
    pub(in crate::resident) occupant: CanonicalUuid,
    /// Stable identity of the refused packet's source locus.
    pub(in crate::resident) packet: String,
    /// The packet header as authored, bounded.
    pub(in crate::resident) header: String,
    pub(in crate::resident) line: u32,
    pub(in crate::resident) column: u32,
    /// How many named recipients the packet did reach.
    pub(in crate::resident) reached: usize,
    pub(in crate::resident) reason: String,
    /// Which addresses the packet missed, or why it was refused, bounded.
    pub(in crate::resident) detail: String,
}

impl MessageRefusalNotice {
    /// The notice its author hears.
    pub(in crate::resident) fn text(&self) -> String {
        let (outcome, missed) = match self.reached {
            0 => ("published nothing".to_owned(), "No recipient received it"),
            reached => (
                format!(
                    "reached {reached} recipient{}, but not every address it named",
                    if reached == 1 { "" } else { "s" }
                ),
                "Only the addresses named here did not receive it",
            ),
        };
        format!(
            "<system-notice>\nYour MSG packet `{}` at line {}, column {} of its assistant \
             text block {outcome}: {}: {}. {missed}. Name only exact addresses of other \
             seated occupants (`spice lane list` shows each lane's), each once, qualify a \
             route with `#TASK` only for an existing task's handle, close the list with a \
             colon, and author a packet to the routes that still need it in new visible \
             text. This is a private refusal notice; no acknowledgment key or visible \
             ACK/NAK is required.\n</system-notice>",
            self.header, self.line, self.column, self.reason, self.detail,
        )
    }
}

/// At most [`NOTICE_TEXT_BYTES`] of `text`, cut at a character boundary.
fn bounded(text: &str) -> String {
    if text.len() <= NOTICE_TEXT_BYTES {
        return text.to_owned();
    }
    let mut end = NOTICE_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Dead-letter every live peer directive whose lane left the residency.
///
/// A peer message or reply is bound to its logical lane, not to the occupant
/// seated when it published: it stays live through vacancy, resume, and
/// renewal for whichever occupant sits there next, with its key and expiry
/// unchanged. Only removing the lane ends that destination, and a lane that
/// later takes its place is another destination that inherits nothing, so
/// each such directive moves to the existing `deadlettered` disposition with
/// `steer::recipient-lane-removed`. Lane removal and startup run this; a lane
/// never returns to the residency, so the removal needs no residency guard,
/// and each parked head is guarded unchanged. A write carries one origin, so
/// directives are parked in one write per origin they were published under.
pub(in crate::resident) fn dead_letter_removed_lanes(
    bus: &Bus,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let view = bus
        .task_snapshot()
        .map_err(|error| store_fault("steer::residency-unreadable", &error.to_string()))?;
    let residency = view
        .residency()
        .ok_or_else(|| store_fault("resident::head-missing", "residency head is absent"))?;
    let heads = bus
        .read(move |read| crate::steering_read::pending(read, None))
        .map_err(|error| store_fault("steer::index-unreadable", &error.to_string()))?;
    let mut removed = BTreeMap::<wire::Origin, Vec<_>>::new();
    for head in heads {
        let seated = residency
            .body()
            .lanes
            .iter()
            .any(|row| row.lane == head.body().lane);
        if head.body().from.is_some() && !seated {
            removed
                .entry(head.body().origin.clone())
                .or_default()
                .push(head);
        }
    }
    for (origin, heads) in removed {
        let at = current_instant().map_err(unit_outcome)?;
        let mut proposal =
            JournalWrite::new(Actor::Spice, origin, at, Producer::ResidentRecognition);
        let standing = heads.clone();
        proposal.guard(JournalGuard::new(move |read| {
            for head in standing {
                if read.steering_head(head.body().directive)? != Some(head) {
                    return Err(GuardError::refused("steer::dead-letter-standing-moved"));
                }
            }
            Ok(())
        }));
        for head in &heads {
            let body = wire::steering::SteerDeadLettered {
                directive: head.body().directive,
                key: head.body().key.clone(),
                lane: head.body().lane,
                reason: LANE_REMOVED.to_owned(),
                requeue: format!("spice steer recover {}", head.body().directive),
            };
            let pending = proposal
                .fact(body.clone(), FactCause::Existing(head.body().created))
                .map_err(|error| {
                    store_fault("steer::dead-letter-fact-invalid", &error.to_string())
                })?;
            let next = steer::Ledger::transition_head(
                head.body(),
                steer::Steering::DeadLettered(&body),
                at,
            )
            .map_err(|error| store_fault("steer::index-invalid", &error.to_string()))?;
            proposal.steering_head(pending, move |_| next);
        }
        commit(bus, proposal)?;
        for head in &heads {
            let _ = writeln!(
                diagnostic,
                "lane {} peer directive {} dead-lettered: {LANE_REMOVED}",
                head.body().lane,
                head.body().key,
            );
        }
    }
    Ok(())
}
