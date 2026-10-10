//! Authored MSG packets published as peer messages.
//!
//! The only source is a complete assistant text block on the author's lane
//! tape (`Event::StreamedProse`, OMP's `text_end`). Settled message prose
//! repeats or paraphrases that same block, reasoning is not authored text,
//! and a tool call's intent is a caption, so none of them is a second source.
//! A block is complete before it is decoded, so a truncated body never sends.
//!
//! One packet resolves every recipient against one residency view and
//! journals one `steer::messaged` fact per recipient in one guarded write, so
//! it publishes to every named seated occupant or to none. Each fact's
//! directive identity is derived from the packet's transcript locus and its
//! recipient, so rereading the same block — live, at exit, or at startup —
//! finds the published directive instead of sending again, while the same
//! body authored at another locus is another message.

use std::collections::BTreeMap;
use std::io;
use std::ops::ControlFlow;
use std::time::{Duration, Instant as Clock};

use wire::OccupantAddress;

use super::transitions::admit_send_envelope;
use super::*;

/// Residency views one packet may resolve against before it gives up on a
/// residency that keeps moving under it.
const PUBLISH_ATTEMPTS: usize = 3;

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

/// Who hears that a packet published nothing.
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
    pub(in crate::resident) recipients: Vec<OccupantAddress>,
    pub(in crate::resident) body: &'a str,
}

/// What publishing one packet did.
#[derive(Debug, Eq, PartialEq)]
pub(in crate::resident) enum Published {
    /// The packet's messages committed now, to these recipient lanes.
    Sent(Vec<CanonicalUuid>),
    /// An earlier read of the same packet already published it.
    Already,
}

/// Why one packet published nothing.
#[derive(Debug, PartialEq)]
pub(in crate::resident) enum Unpublished {
    /// The packet itself, or the residency it named, refuses publication.
    Refused { reason: String, detail: String },
    /// The resident could not decide; the packet may publish on a later read.
    Failed(Outcome<CommandAnswer>),
}

/// Publish every MSG packet one authored record holds.
///
/// Returns each packet's outcome in source order. Recipients are told their
/// inboxes changed; a refused packet is written to `diagnostic` and, under
/// [`Refusals::Notify`], privately to its author.
pub(in crate::resident) fn publish_authored(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    config: &steer::Config,
    record: &AuthoredRecord<'_>,
    refusals: Refusals,
    diagnostic: &mut dyn io::Write,
) -> Vec<Result<Published, Unpublished>> {
    let mut outcomes = Vec::new();
    for (event, text) in authored_blocks(record.events) {
        for message in steer::ack::scan::scan_messages(text) {
            let place = steer::Locus::at(text, message.header.start);
            let outcome = match packet_locus(record, event, place) {
                Err(outcome) => Err(Unpublished::Failed(outcome)),
                Ok(locus) => {
                    let outcome = match &message.publication {
                        Err(refusal) => Err(Unpublished::Refused {
                            reason: refusal.reason().to_owned(),
                            detail: refusal_detail(text, refusal),
                        }),
                        Ok(publication) => publish_resolved(
                            bus,
                            epoch,
                            config,
                            &Packet {
                                lane: record.lane,
                                author: record.author,
                                locus: locus.clone(),
                                recipients: publication
                                    .recipients
                                    .iter()
                                    .map(|recipient| recipient.address)
                                    .collect(),
                                body: &text[publication.body.clone()],
                            },
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
/// missing and finds the rest already published. Refusals are diagnostics
/// only: the author heard them live, or is gone.
pub(in crate::resident) fn publish_at_startup(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    config: &steer::Config,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let residency = current_residency(bus, epoch)?;
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
            bus, paths, epoch, config, row, session, deadline, diagnostic,
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
    row: &wire::LaneResidency,
    session: &wire::LaneSession,
    deadline: Clock,
    diagnostic: &mut dyn io::Write,
) -> Result<(), String> {
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
        publish_authored(
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
            Refusals::Record,
            diagnostic,
        );
    }
    Ok(())
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

/// Resolve one packet against one residency view and journal all of its
/// messages in one guarded write, or none of them.
///
/// The guard re-proves at commit that the residency is the one the view
/// read and that every recipient lane still seats the resolved occupant, so
/// a seat change between read and commit refuses the whole write instead of
/// redirecting any message.
pub(in crate::resident) fn publish_packet(
    bus: &Bus,
    epoch: CanonicalUuid,
    config: &steer::Config,
    packet: &Packet<'_>,
    view: &bus::TaskSnapshot,
) -> Result<Published, Unpublished> {
    let identities = packet
        .recipients
        .iter()
        .map(|recipient| message_identity(&packet.locus, *recipient))
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
    let seated = residency
        .body()
        .lanes
        .iter()
        .filter_map(|row| {
            let occupant = row.occupant?;
            let address = view.facts().occupant_address(row.lane, occupant).ok()?;
            Some((address, (row.lane, occupant)))
        })
        .collect::<BTreeMap<_, _>>();
    let mut targets = Vec::with_capacity(packet.recipients.len());
    for recipient in &packet.recipients {
        if *recipient == sender {
            return Err(Unpublished::Refused {
                reason: "msg::recipient-self".to_owned(),
                detail: format!("{recipient} is the authoring occupant's own address"),
            });
        }
        match seated.get(recipient) {
            Some(target) => targets.push(*target),
            None => return Err(unavailable(bus, *recipient)),
        }
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
    let mut lanes = Vec::with_capacity(targets.len());
    for ((lane, occupant), directive) in targets.into_iter().zip(identities) {
        let key = mint_key(minted_micros).map_err(Unpublished::Failed)?;
        minted_micros = key.minted_micros();
        proposal.guard(send_guard(
            residency.position(),
            Witness::Residency {
                lane,
                occupant: Some(occupant),
                epoch,
            },
            directive,
            key.clone(),
        ));
        let messaged = wire::body::SteerMessaged {
            directive,
            key: key.to_string(),
            lane,
            occupant,
            from: packet.lane,
            sender: packet.author,
            text: packet.body.to_owned(),
            locus: packet.locus.clone(),
            expires,
        };
        let pending = proposal
            .fact(messaged.clone(), FactCause::Existing(residency.position()))
            .map_err(|error| {
                Unpublished::Failed(typed_store_fault("steer::fact-invalid", &error.to_string()))
            })?;
        let origin = origin.clone();
        proposal.steering_head(pending, move |seq| {
            steer::Ledger::sent_head(&messaged.as_sent(), origin, at, seq)
                .expect("the resolved message body builds its authority head")
        });
        lanes.push(lane);
    }
    admit_send_envelope(&proposal).map_err(unpublished)?;
    commit(bus, proposal).map_err(|outcome| unpublished(typed_outcome(outcome)))?;
    Ok(Published::Sent(lanes))
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

/// Why a recipient the view does not seat cannot hear the packet.
fn unavailable(bus: &Bus, recipient: OccupantAddress) -> Unpublished {
    let detail = match bus.resolve_occupant(recipient) {
        Ok(Err(refusal)) => refusal.to_string(),
        Ok(Ok(birth)) => format!(
            "occupant {} at {recipient} was not seated in the residency this packet resolved against",
            birth.occupant
        ),
        Err(error) => {
            return Unpublished::Failed(typed_store_fault(
                "steer::address-unreadable",
                &error.to_string(),
            ));
        }
    };
    Unpublished::Refused {
        reason: "msg::recipient-unavailable".to_owned(),
        detail,
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
        MessageRefusal::Unterminated => "the recipient list is not closed by a colon".to_owned(),
        MessageRefusal::Recipient { span, error } => {
            format!(
                "`{}` is not an occupant address: {error}",
                &text[span.clone()]
            )
        }
        MessageRefusal::Repeated { address, .. } => format!("{address} is named more than once"),
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
    match outcome {
        Ok(Published::Sent(lanes)) => {
            for lane in lanes {
                notify_inbox(paths, *lane);
            }
        }
        Ok(Published::Already) | Err(Unpublished::Failed(_)) => {}
        Err(Unpublished::Refused { reason, detail }) => {
            let _ = writeln!(
                diagnostic,
                "lane {} MSG at record {}:{} event {} line {}:{} refused: {reason}: {detail}",
                record.lane, locus.member, locus.offset, locus.event, locus.line, locus.column,
            );
            if refusals == Refusals::Notify {
                crate::resident::supervisor::command::message_refused(
                    paths,
                    record.lane,
                    crate::resident::supervisor::command::MessageRefusalNotice {
                        occupant: record.author,
                        packet: format!(
                            "{}:{}:{}:{}:{}",
                            locus.member, locus.offset, locus.event, locus.line, locus.column
                        ),
                        header: bounded(header),
                        line: locus.line,
                        column: locus.column,
                        reason: reason.clone(),
                        detail: bounded(detail),
                    },
                );
            }
        }
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
