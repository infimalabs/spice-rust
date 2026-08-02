//! The native client's sole authenticated path to the current resident.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;
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
mod mapping;
mod steering;
mod typed;

pub(crate) use browser::open_canvas;
#[cfg(test)]
use browser::{browser_command, open_canvas_with};
pub(crate) use steering::execute_steering;
pub(crate) use typed::execute_typed;
#[cfg(test)]
use typed::kind_for;

const SOCKET_TIMEOUT: Duration = Duration::from_secs(3);
const LANE_CHANGE_TIMEOUT: Duration = Duration::from_secs(20);
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
    /// Read the current resident-owned lane table.
    LaneRead,
    /// Seat one new lane and launch its first occupant.
    LaneSeat { body: &'a wire::intent::LaneSeat },
    /// Drive one existing lane toward drain or wake.
    LaneState { body: &'a wire::intent::LaneState },
    /// Read one bounded continuity account.
    LaneRecall { body: &'a wire::intent::LaneRecall },
    /// The operator appends one durable directive to a seated lane.
    SteerSend {
        lane: CanonicalUuid,
        message: &'a str,
    },
    /// The operator withdraws one live directive.
    SteerCancel { directive: CanonicalUuid },
    /// The operator restores one exact lapsed directive.
    SteerRecover { directive: CanonicalUuid },
    /// The authenticated principal reads visible pending direction.
    SteerInbox { lane: Option<CanonicalUuid> },
    /// The authenticated lane retires one live key.
    SteerAck {
        key: &'a steer::Key,
        disposition: Disposition,
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

fn execute_mapped(
    operator_discovery: &Path,
    contract: &Contract,
    connection: Connection,
    mapped: ClientResult<MappedCommand>,
) -> Outcome<CommandAnswer> {
    let mapped = match mapped {
        Ok(mapped) => mapped,
        Err(outcome) => return outcome,
    };
    let discovery_path = selected_discovery_path(operator_discovery, connection);
    let discovery = match DiscoveryRead::open(&discovery_path) {
        Ok(discovery) => discovery,
        Err(outcome) => return outcome,
    };
    request(contract, mapped, discovery)
}

/// Prove the selected read contract through the resident's sole authenticated
/// opening path and return the generation that accepted it.
pub(crate) fn authenticate(
    discovery_path: &Path,
    contract: &Contract,
) -> ClientResult<CanonicalUuid> {
    if !contract.guards.contains(&Guard::Resident) || matches!(contract.principal, Principal::Local)
    {
        return Err(protocol_fault(
            "command::mapping-unsupported",
            "selected CLI contract has no authenticated resident read mapping",
        ));
    }
    let connection = ambient_lane_identity()?.map_or(Connection::Operator, Connection::Lane);
    let discovery = DiscoveryRead::open(&selected_discovery_path(discovery_path, connection))?;
    let session = open_session(contract, discovery, SOCKET_TIMEOUT)?;
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
        &wire::intent::SteerInbox { lane },
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

fn selected_discovery_path(operator_discovery: &Path, connection: Connection) -> PathBuf {
    match connection {
        Connection::Operator => operator_discovery.to_path_buf(),
        Connection::Lane(lane) => operator_discovery
            .with_file_name("canvas-lanes")
            .join(lane.to_string()),
    }
}

struct DiscoveryRead {
    path: PathBuf,
    parent: PathBuf,
    parent_identity: FileIdentity,
    file_identity: FileIdentity,
    value: canvas::Discovery,
}

impl DiscoveryRead {
    fn open(path: &Path) -> ClientResult<Self> {
        Self::open_with(path, || {})
    }

    fn open_with(path: &Path, before_open: impl FnOnce()) -> ClientResult<Self> {
        let parent = path.parent().ok_or_else(|| {
            environment_fault(
                "canvas::discovery-unavailable",
                "canvas discovery has no repository runtime parent",
            )
        })?;
        let parent_metadata = secure_metadata(parent, DiscoveryKind::Runtime)?;
        let parent_identity = FileIdentity::from_metadata(&parent_metadata);
        let before = secure_metadata(path, DiscoveryKind::File)?;
        let file_identity = FileIdentity::from_metadata(&before);
        if file_identity.len > MAX_DISCOVERY_BYTES {
            return Err(protocol_fault(
                "canvas::discovery-malformed",
                "canvas discovery exceeds the current bounded document size",
            ));
        }
        before_open();
        let mut file = open_discovery(path)?;
        let opened = file.metadata().map_err(discovery_unavailable)?;
        require_secure(&opened, DiscoveryKind::File)?;
        if FileIdentity::from_metadata(&opened) != file_identity {
            return Err(discovery_replaced());
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_DISCOVERY_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(discovery_unavailable)?;
        if u64::try_from(bytes.len()).ok() != Some(file_identity.len) {
            return Err(discovery_replaced());
        }
        let value = wire::decode(&bytes).map_err(|_| {
            protocol_fault(
                "canvas::discovery-malformed",
                "canvas discovery does not match the current wire shape",
            )
        })?;
        let read = Self {
            path: path.to_path_buf(),
            parent: parent.to_path_buf(),
            parent_identity,
            file_identity,
            value,
        };
        read.verify_current()?;
        Ok(read)
    }

    fn verify_current(&self) -> ClientResult<()> {
        let parent = secure_metadata(&self.parent, DiscoveryKind::Runtime)?;
        let file = secure_metadata(&self.path, DiscoveryKind::File)?;
        if !self
            .parent_identity
            .same_object(&FileIdentity::from_metadata(&parent))
            || FileIdentity::from_metadata(&file) != self.file_identity
        {
            return Err(discovery_replaced());
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum DiscoveryKind {
    Runtime,
    File,
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    mode: u32,
    owner: u32,
    len: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

#[cfg(unix)]
impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            owner: metadata.uid(),
            len: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }

    fn same_object(&self, other: &Self) -> bool {
        self.device == other.device && self.inode == other.inode && self.owner == other.owner
    }
}

#[cfg(not(unix))]
#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

#[cfg(not(unix))]
impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }

    fn same_object(&self, other: &Self) -> bool {
        self == other
    }
}

fn secure_metadata(path: &Path, kind: DiscoveryKind) -> ClientResult<Metadata> {
    let metadata = fs::symlink_metadata(path).map_err(discovery_unavailable)?;
    require_secure(&metadata, kind)?;
    Ok(metadata)
}

#[cfg(unix)]
fn require_secure(metadata: &Metadata, kind: DiscoveryKind) -> ClientResult<()> {
    let shape_matches = match kind {
        DiscoveryKind::Runtime => metadata.file_type().is_dir(),
        DiscoveryKind::File => metadata.file_type().is_file(),
    };
    if !shape_matches {
        return Err(environment_fault(
            "canvas::discovery-insecure",
            "canvas discovery path is not a real repository-owned file",
        ));
    }
    let owner = rustix::process::geteuid().as_raw();
    if metadata.uid() != owner {
        return Err(environment_fault(
            "canvas::discovery-non-owner",
            "canvas discovery path is not owned by the effective user",
        ));
    }
    if matches!(kind, DiscoveryKind::File) && metadata.mode() & 0o077 != 0 {
        return Err(environment_fault(
            "canvas::discovery-insecure",
            "canvas discovery is readable or writable by group or other",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn require_secure(metadata: &Metadata, kind: DiscoveryKind) -> ClientResult<()> {
    let shape_matches = match kind {
        DiscoveryKind::Runtime => metadata.file_type().is_dir(),
        DiscoveryKind::File => metadata.file_type().is_file(),
    };
    if !shape_matches {
        return Err(environment_fault(
            "canvas::discovery-insecure",
            "canvas discovery path is not a real repository-owned file",
        ));
    }
    Ok(())
}

fn open_discovery(path: &Path) -> ClientResult<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    options.open(path).map_err(discovery_unavailable)
}

fn request(
    contract: &Contract,
    mapped: MappedCommand,
    discovery: DiscoveryRead,
) -> Outcome<CommandAnswer> {
    let timeout = match mapped.kind {
        CommandKind::LaneSeat | CommandKind::LaneState => LANE_CHANGE_TIMEOUT,
        _ => SOCKET_TIMEOUT,
    };
    let mut session = match open_session(contract, discovery, timeout) {
        Ok(session) => session,
        Err(outcome) => return outcome,
    };
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

struct AuthenticatedSession {
    socket: WebSocket<DeadlineStream>,
    discovery: DiscoveryRead,
    accepted: wire::Accepted,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum IoDirection {
    Read,
    Write,
}

struct DeadlineStream {
    stream: TcpStream,
    deadline: Instant,
    last_failure: Option<IoDirection>,
}

impl DeadlineStream {
    fn connect(address: &SocketAddr, deadline: Instant) -> io::Result<Self> {
        let stream = TcpStream::connect_timeout(address, remaining(deadline)?)?;
        Ok(Self {
            stream,
            deadline,
            last_failure: None,
        })
    }

    fn read_timeout(&mut self) -> io::Result<Duration> {
        let timeout = remaining(self.deadline)?;
        self.stream.set_read_timeout(Some(timeout))?;
        Ok(timeout)
    }

    fn write_timeout(&mut self) -> io::Result<Duration> {
        let timeout = remaining(self.deadline)?;
        self.stream.set_write_timeout(Some(timeout))?;
        Ok(timeout)
    }
}

impl Read for DeadlineStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        self.last_failure = None;
        if let Err(error) = self.read_timeout() {
            self.last_failure = Some(IoDirection::Read);
            return Err(error);
        }
        match self.stream.read(bytes) {
            Ok(read) => Ok(read),
            Err(error) => {
                self.last_failure = Some(IoDirection::Read);
                Err(error)
            }
        }
    }
}

impl Write for DeadlineStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        self.last_failure = None;
        if let Err(error) = self.write_timeout() {
            self.last_failure = Some(IoDirection::Write);
            return Err(error);
        }
        match self.stream.write(bytes) {
            Ok(written) => Ok(written),
            Err(error) => {
                self.last_failure = Some(IoDirection::Write);
                Err(error)
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.last_failure = None;
        if let Err(error) = self.write_timeout() {
            self.last_failure = Some(IoDirection::Write);
            return Err(error);
        }
        match self.stream.flush() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.last_failure = Some(IoDirection::Write);
                Err(error)
            }
        }
    }
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "resident command deadline elapsed"))
}

fn open_session(
    contract: &Contract,
    discovery: DiscoveryRead,
    timeout: Duration,
) -> ClientResult<AuthenticatedSession> {
    let (authority, address) = loopback_endpoint(&discovery.value.endpoint)?;
    discovery.verify_current()?;
    let deadline = Instant::now() + timeout;
    let stream = match DeadlineStream::connect(&address, deadline) {
        Ok(stream) => stream,
        Err(error) => {
            return Err(environment_fault(
                "canvas::connect-failed",
                &error.to_string(),
            ));
        }
    };
    discovery.verify_current()?;
    let mut request = match format!("ws://{authority}/socket").into_client_request() {
        Ok(request) => request,
        Err(_) => {
            return Err(protocol_fault(
                "canvas::request-invalid",
                "canvas endpoint cannot form the current socket request",
            ));
        }
    };
    let origin = match discovery
        .value
        .endpoint
        .parse::<tungstenite::http::HeaderValue>()
    {
        Ok(origin) => origin,
        Err(_) => {
            return Err(protocol_fault(
                "canvas::origin-invalid",
                "canvas endpoint cannot form the current socket origin",
            ));
        }
    };
    request.headers_mut().insert(ORIGIN, origin);
    let (mut socket, _) = match tungstenite::client(request, stream) {
        Ok(opened) => opened,
        Err(error) => {
            return Err(environment_fault(
                "canvas::opening-failed",
                &error.to_string(),
            ));
        }
    };
    discovery.verify_current()?;
    let hello = wire::Hello {
        wire: wire::WireVersion::CURRENT,
        capability: discovery.value.capability,
        epoch: discovery.value.epoch,
    };
    send(&mut socket, &hello)?;
    let opening = receive(&mut socket)?;
    let accepted = decode_opening(&opening, &discovery.value)?;
    if !principal_matches(contract.principal, &accepted.actor) {
        return Err(protocol_fault(
            "command::principal-mismatch",
            "resident capability resolved to a principal outside the selected CLI contract",
        ));
    }
    discovery.verify_current()?;
    Ok(AuthenticatedSession {
        socket,
        discovery,
        accepted,
    })
}

fn ambient_lane_identity() -> ClientResult<Option<CanonicalUuid>> {
    let Some(value) = crate::bootstrap_environment("SPICE_LANE") else {
        // env-policy: allow
        return Ok(None);
    };
    let Some(value) = value.to_str() else {
        return Err(protocol_fault(
            "hello::lane-identity-invalid",
            "SPICE_LANE is not UTF-8",
        ));
    };
    value.parse().map(Some).map_err(|_| {
        protocol_fault(
            "hello::lane-identity-invalid",
            "SPICE_LANE is not one canonical lane UUID",
        )
    })
}

fn endpoint_authority(endpoint: &str) -> ClientResult<&str> {
    let Some(authority) = endpoint.strip_prefix("http://") else {
        return Err(protocol_fault(
            "canvas::endpoint-invalid",
            "canvas endpoint is not the current HTTP loopback origin",
        ));
    };
    if authority.is_empty() || authority.contains(['/', '?', '#', '@']) {
        return Err(protocol_fault(
            "canvas::endpoint-invalid",
            "canvas endpoint contains more than one socket authority",
        ));
    }
    Ok(authority)
}

fn loopback_endpoint(endpoint: &str) -> ClientResult<(&str, SocketAddr)> {
    let authority = endpoint_authority(endpoint)?;
    match authority.parse::<SocketAddr>() {
        Ok(address) if address.ip().is_loopback() => Ok((authority, address)),
        Ok(_) => Err(protocol_fault(
            "canvas::endpoint-not-loopback",
            "canvas discovery does not name a loopback socket",
        )),
        Err(_) => Err(protocol_fault(
            "canvas::endpoint-invalid",
            "canvas discovery does not name one socket authority",
        )),
    }
}

fn decode_opening(bytes: &[u8], discovery: &canvas::Discovery) -> ClientResult<wire::Accepted> {
    if let Ok(accepted) = wire::decode::<wire::Accepted>(bytes) {
        if accepted.wire.is_current()
            && accepted.epoch == discovery.epoch
            && matches!(accepted.actor, Actor::Operator | Actor::Lane(_))
        {
            return Ok(accepted);
        }
        return Err(protocol_fault(
            "hello::acceptance-invalid",
            "resident acceptance does not bind the current wire, epoch, and principal",
        ));
    }
    if let Ok(refusal) = wire::decode::<Refusal>(bytes) {
        if discloses_capability(&refusal, discovery.capability) {
            return Err(capability_disclosed());
        }
        return Err(Outcome::Refusal(refusal));
    }
    if let Ok(fault) = wire::decode::<Fault>(bytes) {
        if discloses_capability(&fault, discovery.capability) {
            return Err(capability_disclosed());
        }
        return Err(Outcome::Fault(fault));
    }
    Err(protocol_fault(
        "hello::answer-undecodable",
        "resident returned neither a current acceptance, refusal, nor fault",
    ))
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
    if discloses_capability(&response, capability) {
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

fn discloses_capability(value: &impl Serialize, capability: CanonicalUuid) -> bool {
    let needle = capability.to_string();
    wire::encode(value).is_ok_and(|bytes| {
        bytes
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    })
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
        .map_err(|error| environment_fault("canvas::send-failed", &error.to_string()))
}

fn receive(socket: &mut WebSocket<DeadlineStream>) -> ClientResult<Vec<u8>> {
    loop {
        let message = match socket.read() {
            Ok(message) => message,
            Err(error) => {
                let reason = if socket.get_ref().last_failure == Some(IoDirection::Write) {
                    "canvas::send-failed"
                } else {
                    "canvas::receive-failed"
                };
                return Err(environment_fault(reason, &error.to_string()));
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

fn discovery_unavailable(error: io::Error) -> Outcome<CommandAnswer> {
    environment_fault("canvas::discovery-unavailable", &error.to_string())
}

fn discovery_replaced() -> Outcome<CommandAnswer> {
    environment_fault(
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
    remedy: &str,
) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("steering-inbox".to_owned()),
        expected: Some("one live directive in the authenticated lane".to_owned()),
        observed,
        remedy: remedy.to_owned(),
    })
}

fn protocol_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Protocol,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

fn environment_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

#[cfg(test)]
mod tests;
