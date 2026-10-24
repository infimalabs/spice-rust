//! Authored MSG packets published as peer messages and room conversation
//! records.
//!
//! Visible assistant text is the only source: each complete streamed
//! assistant text block, including a turn's final message, in presentation
//! order. Settled message prose repeats or paraphrases those blocks, reasoning
//! is not visible text, and a tool call's intent and arguments are call
//! metadata, so none of them publishes, even when an intent repeats a packet
//! header. A block is complete before it is decoded, so a truncated body never
//! sends.
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
//! A packet that publishes to no one atomically journals one
//! `steer::withheld` fact and one resident control directive that wakes its
//! author with the refusal. Their source-locus identity makes every later
//! reread a no-op. A packet that reaches anyone still reports only its missed
//! addresses in a private background notice. A packet the grammar refuses
//! needs no record, since its text refuses identically on every read.
//!
//! An MSG naming the exact key of an operator direction also writes one
//! `room::conversed` record in that same write; its identity is the packet's
//! exact transcript hunk, so a reread finds it instead of publishing again.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io;
use std::ops::{ControlFlow, Range};
use std::time::Duration;

use steer::ack::scan::{Audience, Route};
use wire::rooms::{
    RoomAuthorityStamp, RoomConversed, RoomDestination, RoomMessaged, RoomPublicationKind,
    RoomRoute, TextRange, TranscriptHunk,
};
use wire::{OccupantAddress, TaskHandle, TaskStamp};

use super::transitions::admit_send_envelope;
use super::*;

mod conversation;
mod notice;

use conversation::{conversation_stamp, publish_conversation};
pub(in crate::resident) use notice::{MessageRecordedNotice, MessageRefusalNotice};
use notice::{refusal_detail, report};

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
/// Each lane's own wall-time allowance; its tail scan measures it from that
/// scan's start, so no lane spends another's.
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
    /// The routes the header spelled, in source order.
    pub(in crate::resident) routes: Vec<Route>,
    pub(in crate::resident) body: &'a str,
    /// The body's byte range within its decoded event's text.
    pub(in crate::resident) text: Range<usize>,
}

/// What publishing one packet did.
#[derive(Debug, Eq, PartialEq)]
pub(in crate::resident) enum Published {
    /// The packet committed now: immediate messages to these recipient lanes,
    /// queued routes to these exact historical addresses, and one record in
    /// each named room. Each `redirected` entry names one address an earlier
    /// occupant held whose proved successor heard the packet instead, and
    /// each `unreached` entry names one address that received or queued
    /// nothing, and why.
    Sent {
        lanes: Vec<CanonicalUuid>,
        queued: Vec<OccupantAddress>,
        redirected: Vec<String>,
        unreached: Vec<String>,
        rooms: Vec<TaskHandle>,
        /// The packet's conversation record opened one operator item.
        operator: bool,
        /// Some `#TASK` the packet names is the task its author's lane holds
        /// an active claim on.
        held_room: bool,
    },
    /// An earlier read of the same packet already published it.
    Already,
}

/// Why one packet published nothing.
#[derive(Debug, PartialEq)]
pub(in crate::resident) enum Unpublished {
    /// The packet itself, or the residency it named, refuses publication.
    Refused { reason: String, detail: String },
    /// The refusal was journaled with one waking directive to its author.
    AuthorDirected { reason: String, detail: String },
    /// An earlier judgment refused this packet; it stays unpublished.
    Withheld,
    /// The resident could not decide; the packet may publish on a later read.
    Failed(Outcome<CommandAnswer>),
}

/// Publish every MSG packet one authored record holds.
///
/// Returns each packet's outcome in source order. Recipients are told their
/// inboxes changed. Partial misses and grammar refusals reach `diagnostic` and,
/// under [`Refusals::Notify`], a private background notice; a wholly withheld
/// packet instead opens its one waking directive.
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
                            config.expiry().get(),
                            record,
                            &locus,
                            &text[message.header.clone()],
                            publish_resolved(
                                bus,
                                epoch,
                                config,
                                &Packet {
                                    lane: record.lane,
                                    author: record.author,
                                    locus,
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
/// earlier judgment withheld unpublished. A newly judged whole refusal opens
/// its durable author directive; grammar refusals remain diagnostics only.
///
/// Recovery runs before this resident seats any new occupant, so a packet a
/// crash left unjudged resolves against no occupant born after the previous
/// resident stopped. Each lane owns a fresh wall-time allowance; work on one
/// tape never spends another tape's scan budget.
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
    for row in &residency.body().lanes {
        let Some(session) = row
            .session
            .as_ref()
            .filter(|session| session.lane == row.lane)
        else {
            continue;
        };
        if let Err(detail) =
            recover_lane(bus, paths, epoch, config, &view, row, session, diagnostic)
        {
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
    let budget = listen::TailBudget::new(
        RECOVERY_WINDOW_BYTES,
        RECOVERY_SOURCE_BYTES,
        RECOVERY_RECORDS,
        RECOVERY_PASSES,
        RECOVERY_DURATION,
    )
    .map_err(|error| error.to_string())?;
    let declared = (session.locus.member, session.locus.offset);
    let mut newest_first = Vec::new();
    let search = listen::search_tail(
        &path,
        vendor.dialect(),
        &[steer::ack::scan::message::MARKER.as_bytes()],
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

/// Atomically record a packet the residency refused and direct its author.
///
/// A refusal that cannot be recorded is undecided: the author is not told,
/// and a later read may judge the packet again.
fn settle(
    bus: &Bus,
    epoch: CanonicalUuid,
    lifetime: Duration,
    record: &AuthoredRecord<'_>,
    locus: &TranscriptLocus,
    header: &str,
    outcome: Result<Published, Unpublished>,
) -> Result<Published, Unpublished> {
    let Err(Unpublished::Refused { reason, detail }) = outcome else {
        return outcome;
    };
    let Some(control) = wire::packet_withheld_control(locus) else {
        return Err(Unpublished::Failed(typed_store_fault(
            "steer::packet-locus-unsourced",
            "a withheld authored packet has no transcript source",
        )));
    };
    let directive = internal_directive_identity(&control);
    if control_standing(bus, directive, record.lane, &control)
        .map_err(|outcome| Unpublished::Failed(typed_outcome(outcome)))?
        .is_some()
    {
        return Err(Unpublished::Withheld);
    }
    let at = current_instant().map_err(Unpublished::Failed)?;
    let origin =
        resident_origin(epoch).map_err(|outcome| Unpublished::Failed(typed_outcome(outcome)))?;
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::AckScanner);
    let withheld = write
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
    append_internal_directive(
        bus,
        &mut write,
        epoch,
        InternalDirective {
            directive,
            lane: record.lane,
            text: format!(
                "Your packet `{}` was withheld: {reason}: {}",
                notice::bounded(header),
                notice::bounded(&detail)
            ),
            control,
            lifetime,
            cause: FactCause::Pending(withheld),
            published: at,
        },
    )
    .map_err(|outcome| Unpublished::Failed(typed_outcome(outcome)))?;
    commit(bus, write).map_err(|outcome| Unpublished::Failed(typed_outcome(outcome)))?;
    Err(Unpublished::AuthorDirected { reason, detail })
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
    matches!(
        reason,
        "steer::residency-moved" | "steer::identity-taken" | "steer::directive-moved"
    )
}

/// One directed lane route: the address it names and the context it carries.
type Directed = (OccupantAddress, Option<TaskHandle>);

/// Resolve one packet against one residency view and publish it in one
/// guarded write.
///
/// A plain MSG, one naming no operator, is native peer messaging plus room
/// records. An operator-routed MSG also writes the conversation record that
/// owns its operator item; see [`publish_conversation`].
///
/// The guard re-proves at commit that the residency is the one the view
/// read and that every route still names either its resolved occupant or its
/// proved vacant lane. A seat change between read and commit refuses the whole
/// write instead of resolving again. A valid address for a lane whose latest
/// occupant has departed queues one lane-scoped directive. An expired address
/// whose lane now seats a later proved occupant delivers to that occupant,
/// recording the address it named so both ends hear about the change; once
/// that successor departs too, the older address stays expired. A route that
/// resolves onto the author's own lane under any code, or two routes that
/// resolve onto one seat, refuse the whole packet. Every `#` context must name
/// a task the view holds, or the whole packet refuses; each room a packet
/// names records the packet's exact transcript bytes once. A packet that can
/// neither deliver nor queue and names no room publishes nothing.
pub(in crate::resident) fn publish_packet(
    bus: &Bus,
    epoch: CanonicalUuid,
    config: &steer::Config,
    packet: &Packet<'_>,
    view: &bus::TaskSnapshot,
) -> Result<Published, Unpublished> {
    if packet
        .routes
        .iter()
        .any(|route| route.audience == Some(Audience::Operator))
    {
        publish_conversation(bus, epoch, config, packet, view)
    } else {
        publish_message(bus, epoch, config, packet, view)
    }
}

fn publish_message(
    bus: &Bus,
    epoch: CanonicalUuid,
    config: &steer::Config,
    packet: &Packet<'_>,
    view: &bus::TaskSnapshot,
) -> Result<Published, Unpublished> {
    let directed = occupant_routes(&packet.routes);
    let identities = identities(packet, &directed);
    let mut minted_micros = None;
    for directive in &identities {
        let (existing, minted) = keyspace(bus, *directive).map_err(Unpublished::Failed)?;
        if existing.is_some() {
            return Ok(Published::Already);
        }
        minted_micros = minted;
    }
    let residency = current(view, epoch)?;
    let sender = proved_sender(view, packet)?;
    refuse_self(&directed, sender)?;
    let Some(record) = resolve_rooms(bus, packet, view, sender)? else {
        return Ok(Published::Already);
    };
    let (targets, unreached) = resolve_targets(
        bus,
        &residency.body().lanes,
        view,
        packet.lane,
        &directed,
        &identities,
    )?;
    if targets.is_empty() && record.rooms.is_empty() {
        return Err(Unpublished::Refused {
            reason: UNAVAILABLE.to_owned(),
            detail: unreached.join("; "),
        });
    }
    let at = current_instant().map_err(Unpublished::Failed)?;
    let expires = expiry(at, config.expiry().get()).map_err(Unpublished::Failed)?;
    let mut proposal = lane_write(epoch, packet, at)?;
    let keys = mint_keys(&mut minted_micros, targets.len())?;
    let sending = Sending {
        epoch,
        residency: residency.position(),
        packet,
        at,
        expires,
    };
    sending.stage(&mut proposal, &targets, keys)?;
    stage_rooms(
        &mut proposal,
        residency.position(),
        &record,
        &targets,
        moment(minted_micros),
    )?;
    admit_send_envelope(&proposal).map_err(unpublished)?;
    commit(bus, proposal).map_err(|outcome| unpublished(typed_outcome(outcome)))?;
    Ok(Published::Sent {
        lanes: targets
            .iter()
            .filter(|target| target.occupant.is_some())
            .map(|target| target.lane)
            .collect(),
        queued: targets
            .iter()
            .filter(|target| target.occupant.is_none())
            .map(|target| target.recipient)
            .collect(),
        redirected: targets
            .iter()
            .filter_map(|target| target.redirected.as_ref().map(Redirected::note))
            .collect(),
        unreached,
        held_room: names_held_task(&record.rooms, view, packet.lane),
        rooms: record.rooms.into_iter().map(|(handle, _)| handle).collect(),
        operator: false,
    })
}

/// Whether any of `rooms` is the task `lane` holds an active claim on.
fn names_held_task(rooms: &Rooms, view: &bus::TaskSnapshot, lane: CanonicalUuid) -> bool {
    rooms.iter().any(|(_, room)| {
        view.task_heads(room)
            .and_then(|heads| heads.claim())
            .is_some_and(|claim| {
                claim.body().disposition == wire::ClaimDisposition::Active
                    && claim.body().lane == lane
            })
    })
}

/// Each occupant-addressed route, in route order, with its own context.
fn occupant_routes(routes: &[Route]) -> Vec<Directed> {
    routes
        .iter()
        .filter_map(|route| match route.audience {
            Some(Audience::Occupant(address)) => Some((address, route.context.clone())),
            Some(Audience::Operator) | None => None,
        })
        .collect()
}

/// The one directive identity each directed route can own.
fn identities(packet: &Packet<'_>, directed: &[Directed]) -> Vec<CanonicalUuid> {
    directed
        .iter()
        .map(|(recipient, _)| message_identity(&packet.locus, *recipient))
        .collect()
}

/// The residency head this resident generation publishes against.
fn current(
    view: &bus::TaskSnapshot,
    epoch: CanonicalUuid,
) -> Result<&journal::JournalHead<journal::crossing::ResidencyHead>, Unpublished> {
    view.residency()
        .filter(|residency| residency.body().epoch == epoch)
        .ok_or_else(|| {
            Unpublished::Failed(typed_store_fault(
                "steer::residency-missing",
                "no current residency names this resident generation",
            ))
        })
}

fn proved_sender(
    view: &bus::TaskSnapshot,
    packet: &Packet<'_>,
) -> Result<OccupantAddress, Unpublished> {
    view.facts()
        .occupant_address(packet.lane, packet.author)
        .map_err(|refusal| Unpublished::Refused {
            reason: "msg::sender-unproved".to_owned(),
            detail: refusal.to_string(),
        })
}

fn refuse_self(directed: &[Directed], sender: OccupantAddress) -> Result<(), Unpublished> {
    if directed.iter().any(|(recipient, _)| *recipient == sender) {
        return Err(Unpublished::Refused {
            reason: "msg::recipient-self".to_owned(),
            detail: format!("{sender} is the authoring occupant's own address"),
        });
    }
    Ok(())
}

fn lane_write(
    epoch: CanonicalUuid,
    packet: &Packet<'_>,
    at: Instant,
) -> Result<JournalWrite, Unpublished> {
    let origin =
        resident_origin(epoch).map_err(|outcome| Unpublished::Failed(typed_outcome(outcome)))?;
    Ok(JournalWrite::new(
        Actor::Lane(packet.lane),
        origin,
        at,
        Producer::AckScanner,
    ))
}

/// The durable key clock's high-water, which every minted key follows.
fn minted(bus: &Bus) -> Result<Option<i64>, Unpublished> {
    bus.read(|read| read.steering_minted_micros())
        .map_err(|error| {
            Unpublished::Failed(typed_store_fault(
                "steer::index-unreadable",
                &error.to_string(),
            ))
        })
}

/// `count` fresh steering keys, each after the last.
fn mint_keys(
    minted_micros: &mut Option<i64>,
    count: usize,
) -> Result<Vec<steer::Key>, Unpublished> {
    (0..count)
        .map(|_| {
            let key = mint_key(*minted_micros).map_err(Unpublished::Failed)?;
            *minted_micros = key.minted_micros();
            Ok(key)
        })
        .collect()
}

/// The first microsecond a packet's room-authority stamps may use: now, or
/// past every key this write minted, so no stamp spells one of its keys.
fn moment(minted_micros: Option<i64>) -> i64 {
    let now = jiff::Timestamp::now().as_microsecond();
    minted_micros.map_or(now, |minted| now.max(minted.saturating_add(1)))
}

/// What every peer message one packet opens shares.
struct Sending<'a, 'p> {
    epoch: CanonicalUuid,
    residency: wire::Seq,
    packet: &'a Packet<'p>,
    at: Instant,
    expires: Instant,
}

impl Sending<'_, '_> {
    /// Stage one guarded peer directive to each seated or vacant-lane target.
    fn stage(
        &self,
        proposal: &mut JournalWrite,
        targets: &[Target],
        keys: Vec<steer::Key>,
    ) -> Result<(), Unpublished> {
        let origin = resident_origin(self.epoch)
            .map_err(|outcome| Unpublished::Failed(typed_outcome(outcome)))?;
        for (target, key) in targets.iter().zip(keys) {
            proposal.guard(send_guard(
                self.residency,
                Witness::Residency {
                    lane: target.lane,
                    occupant: target.occupant,
                    epoch: self.epoch,
                },
                target.directive,
                key.clone(),
            ));
            match target.occupant {
                Some(occupant) => {
                    let messaged = wire::body::SteerMessaged {
                        directive: target.directive,
                        key: key.to_string(),
                        lane: target.lane,
                        occupant,
                        from: self.packet.lane,
                        sender: self.packet.author,
                        text: self.packet.body.to_owned(),
                        locus: self.packet.locus,
                        room: target.room.clone(),
                        addressed: target.redirected.map(|route| route.addressed.address),
                        expires: self.expires,
                    };
                    let pending = proposal
                        .fact(messaged.clone(), FactCause::Existing(self.residency))
                        .map_err(fact_invalid)?;
                    let (origin, at) = (origin.clone(), self.at);
                    proposal.steering_head(pending, move |seq| {
                        steer::Ledger::sent_head(&messaged.as_sent(), origin, at, seq)
                            .expect("the resolved message body builds its authority head")
                    });
                }
                None => {
                    let queued = wire::body::SteerQueued {
                        directive: target.directive,
                        key: key.to_string(),
                        lane: target.lane,
                        recipient: target.recipient,
                        from: self.packet.lane,
                        sender: self.packet.author,
                        text: self.packet.body.to_owned(),
                        locus: self.packet.locus,
                        room: target.room.clone(),
                        expires: self.expires,
                    };
                    let pending = proposal
                        .fact(queued.clone(), FactCause::Existing(self.residency))
                        .map_err(fact_invalid)?;
                    let (origin, at) = (origin.clone(), self.at);
                    proposal.steering_head(pending, move |seq| {
                        steer::Ledger::sent_head(&queued.as_sent(), origin, at, seq)
                            .expect("the queued message body builds its authority head")
                    });
                }
            }
        }
        Ok(())
    }
}

fn fact_invalid(error: impl std::fmt::Display) -> Unpublished {
    Unpublished::Failed(typed_store_fault("steer::fact-invalid", &error.to_string()))
}

/// Each distinct room a packet names, as its header spelled the task and by
/// the task's birth stamp, in source order.
type Rooms = Vec<(TaskHandle, TaskStamp)>;

/// One seated or vacant-lane target a directed route reaches.
struct Target {
    lane: CanonicalUuid,
    /// Occupant whose current seat admits immediate delivery; absent while the
    /// proved logical lane is vacant.
    occupant: Option<CanonicalUuid>,
    /// Exact historical address that proved the destination lane.
    recipient: OccupantAddress,
    directive: CanonicalUuid,
    /// The room the route's context freezes into its message.
    room: Option<TaskStamp>,
    /// The earlier occupant the route named, when `occupant` is its proved
    /// successor on the same lane.
    redirected: Option<Redirected>,
}

/// A route that named an earlier occupant of a lane and reached its proved
/// successor there.
#[derive(Clone, Copy)]
struct Redirected {
    addressed: bus::OccupantBirth,
    successor: bus::OccupantBirth,
}

impl Redirected {
    /// What the author hears about this route: the named address, the address
    /// that heard it, and how long after the named occupant's birth the
    /// successor was born.
    fn note(&self) -> String {
        format!(
            "{} expired; its lane now seats {}, born {} after it",
            self.addressed.address,
            self.successor.address,
            steer::inbox::compact_age(self.addressed.at, self.successor.at)
        )
    }
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
/// Callers resolve directed routes separately; a packet with only room routes
/// still uses its room record as the proof that it published.
fn resolve_rooms(
    bus: &Bus,
    packet: &Packet<'_>,
    view: &bus::TaskSnapshot,
    sender: OccupantAddress,
) -> Result<Option<RoomRecord>, Unpublished> {
    let contexts = packet
        .routes
        .iter()
        .filter_map(|route| route.context.as_ref())
        .collect();
    let rooms = packet_rooms(contexts, view)?;
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

/// Each directed route's target, in route order, and why each unproved,
/// removed, or expired address receives nothing. A proved address whose birth
/// lane is currently vacant opens a lane-scoped queued directive; one whose
/// birth lane now seats a later proved occupant delivers to that occupant,
/// keeping the named address beside the route. A route that resolves onto
/// `author`'s own lane, or two routes that resolve onto one seat, refuse the
/// whole packet as a self or repeated recipient.
fn resolve_targets(
    bus: &Bus,
    residency: &[wire::LaneResidency],
    view: &bus::TaskSnapshot,
    author: CanonicalUuid,
    directed: &[Directed],
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
        let room = context_room(context.as_ref());
        let target = |lane, occupant, redirected| Target {
            lane,
            occupant,
            recipient: *recipient,
            directive: *directive,
            room: room.clone(),
            redirected,
        };
        if let Some((lane, occupant)) = seated.get(recipient) {
            targets.push(target(*lane, Some(*occupant), None));
            continue;
        }
        let Ok(birth) = occupant_birth(bus, *recipient)? else {
            unreached.push(unavailable(bus, &lanes, view, *recipient)?);
            continue;
        };
        let vacant = lanes.get(&birth.lane).is_some_and(Option::is_none)
            && view.facts().latest_lane_occupant(birth.lane) == Some(birth.occupant);
        if vacant {
            targets.push(target(birth.lane, None, None));
        } else if let Some(successor) = successor_seat(bus, &lanes, &birth)? {
            let redirected = Redirected {
                addressed: birth,
                successor,
            };
            targets.push(target(
                birth.lane,
                Some(successor.occupant),
                Some(redirected),
            ));
        } else {
            unreached.push(unavailable(bus, &lanes, view, *recipient)?);
        }
    }
    admit_seats(author, &targets)?;
    Ok((targets, unreached))
}

/// Refuse the packet when a resolved route reaches its author's own lane under
/// any code, or when two routes reach one seat under different codes, so a
/// packet never reaches its author and opens at most one directive per seat.
fn admit_seats(author: CanonicalUuid, targets: &[Target]) -> Result<(), Unpublished> {
    if let Some(own) = targets.iter().find(|target| target.lane == author) {
        return Err(Unpublished::Refused {
            reason: "msg::recipient-self".to_owned(),
            detail: format!(
                "{} is an earlier address of the authoring occupant's own lane",
                own.recipient
            ),
        });
    }
    let mut seats = BTreeMap::new();
    for target in targets {
        let Some(occupant) = target.occupant else {
            continue;
        };
        if let Some(first) = seats.insert((target.lane, occupant), target) {
            let current = [first, target]
                .into_iter()
                .find_map(|named| named.redirected.map(|route| route.successor.address))
                .unwrap_or(target.recipient);
            return Err(Unpublished::Refused {
                reason: "msg::recipient-repeated".to_owned(),
                detail: format!(
                    "{} and {} both name the seat {current} holds",
                    first.recipient, target.recipient
                ),
            });
        }
    }
    Ok(())
}

/// The proved occupant now seated on an expired address's birth lane, when the
/// residency seats one with a proved address of its own.
fn successor_seat(
    bus: &Bus,
    lanes: &Seats,
    birth: &bus::OccupantBirth,
) -> Result<Option<bus::OccupantBirth>, Unpublished> {
    let Some(Some((occupant, Some(address)))) = lanes.get(&birth.lane) else {
        return Ok(None);
    };
    if *occupant == birth.occupant {
        return Ok(None);
    }
    Ok(occupant_birth(bus, *address)?
        .ok()
        .filter(|successor| successor.lane == birth.lane && successor.occupant == *occupant))
}

fn occupant_birth(bus: &Bus, address: OccupantAddress) -> Result<bus::AddressProof, Unpublished> {
    bus.occupant_birth(address).map_err(|error| {
        Unpublished::Failed(typed_store_fault(
            "steer::address-unreadable",
            &error.to_string(),
        ))
    })
}

/// Stage one `room::messaged` record of the packet in each room, naming the
/// directives its delivered or queued routes into that room opened.
fn stage_rooms(
    proposal: &mut JournalWrite,
    residency: wire::Seq,
    record: &RoomRecord,
    targets: &[Target],
    moment: i64,
) -> Result<(), Unpublished> {
    let Some(hunk) = &record.hunk else {
        return Ok(());
    };
    for ((_, room), offset) in record.rooms.iter().zip(0_usize..) {
        let publication = conversation_stamp(moment, offset)?;
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

/// Every distinct room `contexts` name, in source order, each resolved by
/// birth stamp against `view`; a context naming no task refuses the whole
/// packet.
fn packet_rooms(
    contexts: Vec<&TaskHandle>,
    view: &bus::TaskSnapshot,
) -> Result<Rooms, Unpublished> {
    let mut rooms = Rooms::new();
    for handle in contexts {
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
            let end = locus.offset + locus.length;
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

/// Why a recipient the view neither seats, redirects to a proved successor,
/// nor admits onto a vacant proved lane cannot hear the packet, led by its
/// address so a bounded notice still names it. An expired address names the
/// unproved current seat, or the latest address whose successor birth expired
/// it once that successor left.
fn unavailable(
    bus: &Bus,
    lanes: &Seats,
    view: &bus::TaskSnapshot,
    recipient: OccupantAddress,
) -> Result<String, Unpublished> {
    match bus.resolve_occupant(recipient) {
        Ok(Err(refusal @ bus::AddressRefusal::Expired { birth })) => {
            let latest = view
                .facts()
                .latest_lane_occupant(birth.lane)
                .and_then(|occupant| view.facts().occupant_address(birth.lane, occupant).ok())
                .filter(|address| *address != recipient);
            let now = match lanes.get(&birth.lane) {
                Some(Some((_, Some(current)))) => format!("its lane seats {current}"),
                Some(Some((_, None))) => "its lane seats an unproved occupant".to_owned(),
                Some(None) => latest.map_or_else(
                    || "its lane is vacant".to_owned(),
                    |current| format!("its lane is vacant after {current}"),
                ),
                None => latest.map_or_else(
                    || "its lane left the residency".to_owned(),
                    |current| format!("its lane left the residency after {current}"),
                ),
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
        .filter_map(|(index, event)| Some((index, packet_prose(event)?)))
}

/// The complete assistant text block `event` carries, when it holds a placed
/// packet.
fn packet_prose(event: &listen::Event) -> Option<&str> {
    spice::quote::packet_text(event).filter(|text| steer::ack::scan::has_message(text))
}

/// Whether `record` carries an assistant text block that could hold a packet,
/// whose publication must cite the record's accounted bytes.
pub(in crate::resident) fn carries_packet(record: &listen::Record) -> bool {
    match &record.payload {
        listen::RecordPayload::Events(events) => events
            .iter()
            .any(|sourced| packet_prose(&sourced.event).is_some()),
        listen::RecordPayload::Residue(_) => false,
    }
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
