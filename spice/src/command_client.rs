//! The native client's sole authenticated path to the current resident.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde::Serialize;
use sill::{Availability, LockFile, LockName};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::header::ORIGIN;
use tungstenite::{Message, WebSocket};
use wire::intent::{CommandAnswer, InboxAnswer};
use wire::{
    Actor, CanonicalUuid, Command, CommandKind, Disposition, Fault, FaultClass, Outcome, Refusal,
    Reply, ReplyPosition, Response, Witness,
};

use crate::cli::{Contract, Guard, Principal, ReplyPositionLaw};

mod browser;
mod deadline_stream;
mod discovery;
mod mapping;
mod session;
mod steering;
mod typed;

#[cfg(test)]
use browser::browser_command;
pub(crate) use browser::{open_canvas_result_with, open_canvas_with, system_browser};
use deadline_stream::{DeadlineStream, IoDirection, IoFailure, remaining};
#[cfg(test)]
use discovery::open_discovery;
use discovery::{
    DiscoveryObserver, DiscoveryRead, DiscoveryWait, resident_lock_path, selected_discovery_path,
};
use session::{AuthenticatedSession, dialable_endpoint, open_session};
#[cfg(test)]
use session::{decode_opening, open_session_with};
pub(crate) use steering::{execute_steering, execute_tell};
pub(crate) use typed::execute_typed;
#[cfg(test)]
use typed::kind_for;

/// Bound for an opening that proves a contract and sends no command, so no
/// registry row governs it.
const OPENING_BOUND: Duration = crate::cli::CANVAS_ANSWER_BOUND;
const MAX_DISCOVERY_BYTES: u64 = 16 * 1024;

type ClientResult<T> = Result<T, Outcome<CommandAnswer>>;

/// One generated CLI selection with a landed command-envelope mapping.
pub(crate) enum SelectedCommand<'a> {
    /// The operator requests the current resident's bounded Dark transition.
    Down,
    /// Read the current resident's durable consent queue.
    ConsentRead,
    /// Read the tracked consent policy held by the current resident.
    ConsentPolicy,
    /// Terminally answer one request from an exact queue snapshot.
    ConsentDecide {
        request: CanonicalUuid,
        answer: wire::Answer,
        expected: Vec<Witness>,
    },
    /// The operator recognizes one exact, already admitted release publication.
    ReleasePublish {
        release: &'a crate::release_command::AdmittedRelease,
        state: crate::release_command::PublicationState,
    },
    /// Recognize one exact post-proof two-target manifest.
    ProofRecognize {
        manifest: &'a proof::PublishedManifest,
        head: wire::Seq,
    },
    /// Read the current resident-owned lane table.
    LaneRead,
    /// Seat one new lane and launch its first occupant.
    LaneSeat { body: &'a wire::intent::LaneSeat },
    /// Drive one existing lane toward drain, wake, or removal.
    LaneState { body: &'a wire::intent::LaneState },
    /// State the model spelling one seated lane's next launch runs.
    LaneRetune { body: &'a wire::intent::LaneRetune },
    /// Read one bounded continuity account.
    LaneRecall { body: &'a wire::intent::LaneRecall },
    /// The operator appends one durable directive to a seated lane.
    SteerSend {
        lane: CanonicalUuid,
        message: &'a str,
    },
    /// The operator withdraws one live directive.
    SteerCancel { directive: CanonicalUuid },
    /// The operator re-presents one live directive at the urgency now due.
    SteerResend { directive: CanonicalUuid },
    /// The operator restores one exact lapsed or dead-lettered directive.
    SteerRecover { directive: CanonicalUuid },
    /// The authenticated principal reads visible pending direction.
    SteerInbox { lane: Option<CanonicalUuid> },
    /// A seated lane tells another seated lane something, as itself.
    SteerTell {
        lane: CanonicalUuid,
        message: &'a str,
    },
    /// The operator quarantines the exact current executable digest.
    TrustPend {
        digest: &'a wire::Digest,
        worktree: &'a str,
    },
    /// The operator approves one exact pended executable digest.
    TrustGrant { digest: &'a wire::Digest },
    /// The operator withdraws approval from one exact granted digest.
    TrustRevoke { digest: &'a wire::Digest },
}

#[derive(Clone, Copy)]
enum Connection {
    Operator,
    Lane(CanonicalUuid),
}

/// Submit one contract-bound command to the current repository resident.
///
/// `discovery_path` must be the canvas path returned by the caller's single
/// [`spice::repository::Repository::admit_state`] admission.
pub(crate) fn execute(
    discovery_path: &Path,
    contract: &Contract,
    selected: SelectedCommand<'_>,
) -> Outcome<CommandAnswer> {
    let mapped = MappedCommand::selected(contract, selected);
    let connection = match ambient_lane_identity() {
        Ok(Some(lane)) => Connection::Lane(lane),
        Ok(None) => Connection::Operator,
        Err(outcome) => return outcome,
    };
    execute_mapped(discovery_path, contract, connection, mapped)
}

/// Submit one contract-bound command through the operator connection even
/// while a lane identity is ambient, for roots that never seat lanes: the
/// ambient identity names a seat in the repository that launched the agent,
/// and honoring it against another root would select a lane discovery path
/// that root cannot publish.
pub(crate) fn execute_operator(
    discovery_path: &Path,
    contract: &Contract,
    selected: SelectedCommand<'_>,
) -> Outcome<CommandAnswer> {
    let mapped = MappedCommand::selected(contract, selected);
    execute_mapped(discovery_path, contract, Connection::Operator, mapped)
}

fn execute_mapped(
    operator_discovery: &Path,
    contract: &Contract,
    connection: Connection,
    mapped: ClientResult<MappedCommand>,
) -> Outcome<CommandAnswer> {
    execute_mapped_with(operator_discovery, contract, connection, mapped, |_| {})
}

fn execute_mapped_with(
    operator_discovery: &Path,
    contract: &Contract,
    connection: Connection,
    mapped: ClientResult<MappedCommand>,
    after_transient: impl FnMut(&Fault),
) -> Outcome<CommandAnswer> {
    let mapped = match mapped {
        Ok(mapped) => mapped,
        Err(outcome) => return outcome,
    };
    let discovery_path = selected_discovery_path(operator_discovery, connection);
    let bound = answer_bound(mapped.kind, &discovery_path);
    execute_mapped_within(
        operator_discovery,
        &discovery_path,
        contract,
        mapped,
        bound,
        after_transient,
    )
}

/// Submit one mapped command and outlast one resident handoff inside `bound`.
fn execute_mapped_within(
    operator_discovery: &Path,
    discovery_path: &Path,
    contract: &Contract,
    mapped: MappedCommand,
    bound: Duration,
    mut after_transient: impl FnMut(&Fault),
) -> Outcome<CommandAnswer> {
    let deadline = Instant::now() + bound;
    let resident_lock = resident_lock_path(operator_discovery);
    let session = match acquire_session_until(
        contract,
        discovery_path,
        &resident_lock,
        deadline,
        bound,
        false,
        &mut after_transient,
    ) {
        Ok(session) => session,
        Err(outcome) => return outcome,
    };
    let dialed = session.discovery.value.epoch;
    let outcome = submit(contract, &mapped, session);
    let ending = if let Some(fault) = interrupted_submission(&outcome) {
        after_transient(fault);
        None
    } else if refused_admission(&outcome) && mapped.outlasts_restart() {
        // The dialed resident is ending and refused before it enqueued
        // anything, so nothing was committed and the same envelope may be
        // presented to its successor. This is the one moment a restart is
        // observable from a client: a resident that refuses admission is on
        // its way out, and whatever the lock says while it leaves is part of
        // the same handoff, not a resident that is gone for good.
        after_transient(&environment_fault_value(
            NOT_ADMITTING,
            &format!("resident epoch {dialed} stopped admitting the command"),
        ));
        Some(dialed)
    } else {
        return outcome;
    };

    // The resident may have committed before its socket disappeared, or it
    // refused before enqueueing at all. Present the exact command identity
    // again: a replacement generation reconciles a durable receipt or admits
    // the envelope fresh, while moved authority is returned as its current
    // refusal. Both attempts share one client bound, and a second transport
    // interruption or refusal is terminal rather than an unbounded replay loop.
    let session = match acquire_session_until(
        contract,
        discovery_path,
        &resident_lock,
        deadline,
        bound,
        true,
        &mut after_transient,
    ) {
        Ok(session) => session,
        Err(outcome) => {
            return match ending {
                Some(epoch) => restart_unfinished(outcome, epoch, bound),
                None => outcome,
            };
        }
    };
    submit(contract, &mapped, session)
}

/// The refusal a resident sends while it is ending and no longer admits.
const NOT_ADMITTING: &str = "resident::not-admitting";

fn refused_admission(outcome: &Outcome<CommandAnswer>) -> bool {
    matches!(outcome, Outcome::Refusal(refusal) if refusal.reason() == NOT_ADMITTING)
}

/// Keep one answer for a restart the client witnessed and outlasted.
///
/// After the dialed resident refused admission, every reason the handoff
/// loop can name at its deadline describes a successor that never published:
/// discovery absent, a lock nobody holds, or a holder still publishing. Naming
/// any of those would send the caller to repair an environment that is merely
/// between residents. The refusal the resident actually gave stands, extended
/// with the epoch that ended, the bound the client waited, and the one action
/// left: run the command again once a resident is seated. Any other outcome
/// is a different defect and passes through unchanged.
fn restart_unfinished(
    outcome: Outcome<CommandAnswer>,
    epoch: CanonicalUuid,
    bound: Duration,
) -> Outcome<CommandAnswer> {
    match &outcome {
        Outcome::Fault(fault) if fault.reason == "canvas::deadline-elapsed" => {}
        _ => return outcome,
    }
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            NOT_ADMITTING,
            Some("resident-state".to_owned()),
            Some("seated".to_owned()),
            Some("quiescing".to_owned()),
        )
        .with_detail(format!(
            "resident epoch {epoch} stopped admitting during this command and no successor published an authenticated surface within the {bound:?} client bound; run the command again once a resident is seated"
        )),
    ))
}

/// Prove the selected read contract through the resident's sole authenticated
/// opening path and return the generation that accepted it.
pub(crate) fn authenticate(
    discovery_path: &Path,
    contract: &Contract,
) -> ClientResult<CanonicalUuid> {
    authenticate_with(discovery_path, contract, |_| {})
}

fn authenticate_with(
    discovery_path: &Path,
    contract: &Contract,
    after_transient: impl FnMut(&Fault),
) -> ClientResult<CanonicalUuid> {
    if !contract.guards.contains(&Guard::Resident) || matches!(contract.principal, Principal::Local)
    {
        return Err(protocol_fault(
            "command::mapping-unsupported",
            "selected CLI contract has no authenticated resident read mapping",
        ));
    }
    let connection = ambient_lane_identity()?.map_or(Connection::Operator, Connection::Lane);
    let session = acquire_session_with(
        contract,
        &selected_discovery_path(discovery_path, connection),
        &resident_lock_path(discovery_path),
        OPENING_BOUND,
        after_transient,
    )?;
    Ok(session.accepted.epoch)
}

struct MappedCommand {
    id: CanonicalUuid,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: serde_json::Value,
}

struct InboxSnapshot {
    answer: InboxAnswer,
    position: wire::Seq,
}

fn read_inbox(
    operator_discovery: &Path,
    connection: Connection,
    lane: Option<CanonicalUuid>,
) -> Result<InboxSnapshot, Outcome<CommandAnswer>> {
    let contract = crate::cli::contract(&["inbox"])
        .expect("the generated CLI registry carries the inbox contract");
    let mapped = MappedCommand::new(
        contract,
        &["inbox"],
        CommandKind::SteerInbox,
        Vec::new(),
        &wire::intent::SteerInbox {
            lane,
            retired: false,
        },
    )?;
    let outcome = execute_mapped(operator_discovery, contract, connection, Ok(mapped));
    match outcome {
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(position),
            body: CommandAnswer::Inbox(answer),
        }) => Ok(InboxSnapshot { answer, position }),
        Outcome::Reply(_) => Err(protocol_fault(
            "steer::inbox-answer-invalid",
            "resident inbox reply did not retain its snapshot position and body",
        )),
        Outcome::Refusal(refusal) => Err(Outcome::Refusal(refusal)),
        Outcome::Fault(fault) => Err(Outcome::Fault(fault)),
    }
}
pub(crate) fn read_lane_inbox(
    operator_discovery: &Path,
    lane: CanonicalUuid,
) -> Result<InboxAnswer, Outcome<CommandAnswer>> {
    read_inbox(operator_discovery, Connection::Lane(lane), None).map(|snapshot| snapshot.answer)
}

/// Acquire one authenticated session across a resident publication handoff.
///
/// A synchronous re-read admits a replacement that has already published.
/// Otherwise the observer is armed and followed by another read, so an atomic
/// discovery rename cannot land between the failed read and the wait. Only
/// opening is retried; the command envelope remains untouched until this
/// function succeeds.
fn acquire_session_with(
    contract: &Contract,
    discovery_path: &Path,
    resident_lock: &Path,
    bound: Duration,
    mut after_transient: impl FnMut(&Fault),
) -> ClientResult<AuthenticatedSession> {
    let deadline = Instant::now() + bound;
    acquire_session_until(
        contract,
        discovery_path,
        resident_lock,
        deadline,
        bound,
        false,
        &mut after_transient,
    )
}

fn acquire_session_until(
    contract: &Contract,
    discovery_path: &Path,
    resident_lock: &Path,
    deadline: Instant,
    bound: Duration,
    recovering_submission: bool,
    after_transient: &mut impl FnMut(&Fault),
) -> ClientResult<AuthenticatedSession> {
    let attempt = acquire_session_attempt(contract, discovery_path, resident_lock, deadline, bound);
    let Err(outcome) = attempt else {
        return attempt;
    };
    let fault = match refreshable_fault(&outcome, resident_lock, recovering_submission) {
        Ok(Some(fault)) => fault,
        Ok(None) => return Err(outcome),
        Err(fault) => return Err(Outcome::Fault(fault)),
    };
    after_transient(fault);

    // A replacement may already have published. Re-read before constructing
    // the observer so a present authenticated surface has no watcher latency.
    let retry = acquire_session_attempt(contract, discovery_path, resident_lock, deadline, bound);
    let Err(outcome) = retry else {
        return retry;
    };
    match refreshable_fault(&outcome, resident_lock, recovering_submission) {
        Ok(Some(_)) => {}
        Ok(None) => return Err(outcome),
        Err(fault) => return Err(Outcome::Fault(fault)),
    }
    let Some(mut observer) = DiscoveryObserver::arm(resident_lock) else {
        return Err(outcome);
    };
    loop {
        // The first iteration closes the interval between the last read and
        // arming the observer; later iterations consume signalled changes.
        let attempt =
            acquire_session_attempt(contract, discovery_path, resident_lock, deadline, bound);
        let Err(outcome) = attempt else {
            return attempt;
        };
        let fault = match refreshable_fault(&outcome, resident_lock, recovering_submission) {
            Ok(Some(fault)) => fault,
            Ok(None) => return Err(outcome),
            Err(fault) => return Err(Outcome::Fault(fault)),
        };
        match observer.wait(deadline) {
            DiscoveryWait::Changed => {}
            DiscoveryWait::Deadline => {
                return Err(acquisition_deadline(bound, &fault.reason));
            }
            DiscoveryWait::Failed => return Err(outcome),
        }
    }
}

fn acquire_session_attempt(
    contract: &Contract,
    discovery_path: &Path,
    resident_lock: &Path,
    deadline: Instant,
    bound: Duration,
) -> ClientResult<AuthenticatedSession> {
    DiscoveryRead::open(discovery_path, resident_lock).and_then(|discovery| {
        let attempt_bound = remaining(deadline)
            .map_err(|_| acquisition_deadline(bound, "opening the current discovery"))?;
        open_session(contract, discovery, attempt_bound)
    })
}

fn refreshable_fault<'a>(
    outcome: &'a Outcome<CommandAnswer>,
    resident_lock: &Path,
    recovering_submission: bool,
) -> Result<Option<&'a Fault>, Fault> {
    let Outcome::Fault(fault) = outcome else {
        return Ok(None);
    };
    match fault.reason.as_str() {
        "canvas::publisher-replaced" => Ok(Some(fault)),
        "canvas::discovery-unavailable" | "canvas::discovery-replaced" => {
            if recovering_submission {
                return Ok(Some(fault));
            }
            match publisher_availability(resident_lock)? {
                Availability::Held => Ok(Some(fault)),
                Availability::Available => Err(environment_fault_value(
                    "canvas::publisher-gone",
                    "no resident holds the runtime lock or publishes Canvas discovery; run `spice up` from this repository to restore resident commands",
                )),
            }
        }
        "canvas::publisher-gone"
        | "canvas::connect-failed"
        | "canvas::opening-failed"
        | "canvas::send-failed"
        | "canvas::receive-failed"
            if recovering_submission =>
        {
            Ok(Some(fault))
        }
        _ => Ok(None),
    }
}

fn interrupted_submission(outcome: &Outcome<CommandAnswer>) -> Option<&Fault> {
    let Outcome::Fault(fault) = outcome else {
        return None;
    };
    matches!(
        fault.reason.as_str(),
        "canvas::send-failed" | "canvas::receive-failed" | "canvas::closed-without-answer"
    )
    .then_some(fault)
}

fn publisher_availability(resident_lock: &Path) -> Result<Availability, Fault> {
    let name = LockName::new("resident").expect("resident is a non-empty lock name");
    LockFile::new(name, resident_lock)
        .availability()
        .map_err(|error| environment_fault_value("canvas::publisher-unknown", &error.to_string()))
}

fn acquisition_deadline(bound: Duration, prior: &str) -> Outcome<CommandAnswer> {
    environment_fault(
        "canvas::deadline-elapsed",
        &format!(
            "resident did not publish an authenticated surface within the {bound:?} client bound after {prior}"
        ),
    )
}

fn submit(
    contract: &Contract,
    mapped: &MappedCommand,
    mut session: AuthenticatedSession,
) -> Outcome<CommandAnswer> {
    let command = mapped.envelope(session.discovery.value.epoch);
    if let Err(outcome) = send(&mut session.socket, &command) {
        return outcome;
    }
    let response = match receive(&mut session.socket) {
        Ok(response) => response,
        Err(outcome) => return outcome,
    };
    decode_response(
        &response,
        contract,
        mapped.id,
        mapped.kind,
        session.discovery.value.epoch,
        session.discovery.value.capability,
    )
}

pub(crate) fn answer_bound(kind: CommandKind, _discovery_path: &Path) -> Duration {
    crate::cli::answer_bound(kind)
}

fn ambient_lane_identity() -> ClientResult<Option<CanonicalUuid>> {
    let Some(value) = crate::bootstrap_environment(steer::hook::LANE) else {
        return Ok(None);
    };
    let Some(value) = value.to_str() else {
        return Err(protocol_fault(
            "hello::lane-identity-invalid",
            &format!("{} is not UTF-8", steer::hook::LANE),
        ));
    };
    value.parse().map(Some).map_err(|_| {
        protocol_fault(
            "hello::lane-identity-invalid",
            &format!("{} is not one canonical lane UUID", steer::hook::LANE),
        )
    })
}

fn decode_response(
    bytes: &[u8],
    contract: &Contract,
    id: CanonicalUuid,
    kind: CommandKind,
    epoch: CanonicalUuid,
    capability: CanonicalUuid,
) -> Outcome<CommandAnswer> {
    let response = match wire::decode::<Response<serde_json::Value>>(bytes) {
        Ok(response) => response,
        Err(_) => {
            return protocol_fault(
                "command::answer-undecodable",
                "resident returned no current command response envelope",
            );
        }
    };
    if discloses_capability(bytes, capability) {
        return capability_disclosed();
    }
    if !response.v.is_current() || response.id != id || response.epoch != epoch {
        return protocol_fault(
            "command::answer-mismatch",
            "resident response does not bind the current command identity and epoch",
        );
    }
    match response.outcome {
        Outcome::Reply(reply) => {
            if !position_matches(contract.reply_position, reply.position) {
                return protocol_fault(
                    "command::position-mismatch",
                    "resident reply position violates the selected CLI contract",
                );
            }
            let body = match wire::encode(&reply.body)
                .ok()
                .and_then(|body| kind.binding().answer.decode(&body).ok())
            {
                Some(body) => body,
                None => {
                    return protocol_fault(
                        "command::answer-binding",
                        "resident reply body violates the command registry binding",
                    );
                }
            };
            Outcome::Reply(Reply {
                position: reply.position,
                body,
            })
        }
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

fn position_matches(law: ReplyPositionLaw, position: ReplyPosition) -> bool {
    matches!(
        (law, position),
        (ReplyPositionLaw::Direct, ReplyPosition::Direct {})
            | (ReplyPositionLaw::Snapshot, ReplyPosition::Snapshot(_))
            | (ReplyPositionLaw::Committed, ReplyPosition::Committed(_))
            | (
                ReplyPositionLaw::SnapshotOrCommitted,
                ReplyPosition::Snapshot(_)
            )
            | (
                ReplyPositionLaw::SnapshotOrCommitted,
                ReplyPosition::Committed(_)
            )
            | (
                ReplyPositionLaw::DirectOrCommitted,
                ReplyPosition::Direct {}
            )
            | (
                ReplyPositionLaw::DirectOrCommitted,
                ReplyPosition::Committed(_)
            )
    )
}

fn principal_matches(principal: Principal, actor: &Actor) -> bool {
    match principal {
        Principal::Local => false,
        Principal::Operator => matches!(actor, Actor::Operator),
        Principal::Lane => matches!(actor, Actor::Lane(_)),
        Principal::OperatorOrLane => matches!(actor, Actor::Operator | Actor::Lane(_)),
    }
}

/// Whether one received frame carries the discovery capability.
///
/// The frame is searched as received: a board answer runs to megabytes, and
/// re-encoding it only to scan the copy doubled the cost of every reply.
fn discloses_capability(frame: &[u8], capability: CanonicalUuid) -> bool {
    let needle = capability.to_string();
    std::str::from_utf8(frame).is_ok_and(|text| text.contains(&needle))
}

fn send<T: Serialize>(socket: &mut WebSocket<DeadlineStream>, value: &T) -> ClientResult<()> {
    let bytes = wire::encode(value).map_err(|_| {
        protocol_fault(
            "canvas::request-unencodable",
            "current client request did not retain its wire representation",
        )
    })?;
    socket
        .send(Message::Binary(bytes.into()))
        .map_err(|error| transport_fault(socket.get_ref(), &error.to_string(), IoDirection::Write))
}

/// Name a transport failure by what ended it.
///
/// The bound elapsing is not a peer that failed: the resident may still be
/// composing the answer, and a reader deciding whether to retry or to
/// investigate the resident needs those two apart. The direction is only
/// consulted once the bound is ruled out, because a socket implementation
/// writes control frames underneath a read the caller asked for.
fn transport_fault(
    stream: &DeadlineStream,
    detail: &str,
    assumed: IoDirection,
) -> Outcome<CommandAnswer> {
    let failure = stream.last_failure.unwrap_or(IoFailure {
        direction: assumed,
        deadline_elapsed: false,
    });
    if failure.deadline_elapsed {
        return environment_fault(
            "canvas::deadline-elapsed",
            &format!(
                "resident did not answer within the {:?} client bound: {detail}",
                stream.bound
            ),
        );
    }
    match failure.direction {
        IoDirection::Read => environment_fault("canvas::receive-failed", detail),
        IoDirection::Write => environment_fault("canvas::send-failed", detail),
    }
}

/// What the opening handshake was bounded by, kept past the stream it came from.
#[derive(Clone, Copy)]
struct OpeningBound {
    bound: Duration,
    deadline: Instant,
}

/// Name a failed opening by whether the client's own bound is what ended it.
///
/// The handshake consumes the stream, so the direction `transport_fault` reads
/// is out of reach by the time the failure surfaces; the deadline the stream
/// was configured from outlives it. A resident under load accepts the socket
/// long before it answers the upgrade, so this is the first place the bound can
/// elapse, and tungstenite spells that arrival as an interrupted handshake --
/// the same would-block shape a reader must not mistake for a peer that failed.
fn opening_fault(opening: OpeningBound, detail: &str) -> Outcome<CommandAnswer> {
    if Instant::now() >= opening.deadline {
        return environment_fault(
            "canvas::deadline-elapsed",
            &format!(
                "resident did not open a socket within the {:?} client bound: {detail}",
                opening.bound
            ),
        );
    }
    environment_fault("canvas::opening-failed", detail)
}

fn receive(socket: &mut WebSocket<DeadlineStream>) -> ClientResult<Vec<u8>> {
    loop {
        let message = match socket.read() {
            Ok(message) => message,
            Err(error) => {
                return Err(transport_fault(
                    socket.get_ref(),
                    &error.to_string(),
                    IoDirection::Read,
                ));
            }
        };
        match message {
            Message::Binary(bytes) => return Ok(bytes.to_vec()),
            Message::Text(text) => return Ok(text.as_bytes().to_vec()),
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            Message::Close(_) => {
                return Err(protocol_fault(
                    "canvas::closed-without-answer",
                    "resident closed the socket before answering",
                ));
            }
        }
    }
}

fn discovery_unavailable(error: io::Error) -> Fault {
    environment_fault_value("canvas::discovery-unavailable", &error.to_string())
}

fn discovery_replaced() -> Fault {
    environment_fault_value(
        "canvas::discovery-replaced",
        "canvas discovery changed identity while the current generation was acquired",
    )
}

fn capability_disclosed() -> Outcome<CommandAnswer> {
    protocol_fault(
        "canvas::capability-disclosed",
        "resident response attempted to disclose the opening capability",
    )
}

fn steering_refusal(
    reason: &str,
    observed: Option<String>,
    detail: &str,
) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            reason.to_owned(),
            Some("steering-inbox".to_owned()),
            Some("one live directive in the authenticated lane".to_owned()),
            observed,
        )
        .with_detail(detail),
    ))
}

fn protocol_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(protocol_fault_value(reason, detail))
}

fn protocol_fault_value(reason: &str, detail: &str) -> Fault {
    Fault {
        class: FaultClass::Protocol,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    }
}

fn environment_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(environment_fault_value(reason, detail))
}

fn environment_fault_value(reason: &str, detail: &str) -> Fault {
    Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    }
}

#[cfg(test)]
mod tests;
