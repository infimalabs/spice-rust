//! The native client's sole authenticated path to the current resident.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read};
use std::net::{SocketAddr, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use spice::repository::Repository;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::header::ORIGIN;
use tungstenite::{Message, WebSocket};
use wire::intent::CommandAnswer;
use wire::{
    Actor, CanonicalUuid, Command, CommandKind, Fault, FaultClass, Outcome, Refusal, Reply,
    ReplyPosition, Response, Witness,
};

use crate::cli::{Contract, Guard, Principal, ReplyPositionLaw};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_DISCOVERY_BYTES: u64 = 16 * 1024;

type ClientResult<T> = Result<T, Outcome<CommandAnswer>>;

/// One generated CLI selection with a landed command-envelope mapping.
pub(crate) enum SelectedCommand<'a> {
    /// The operator requests the current resident's bounded Dark transition.
    Down,
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
}

/// Submit one contract-bound command to the current repository resident.
pub(crate) fn execute(
    repository: &Repository,
    contract: &Contract,
    selected: SelectedCommand<'_>,
) -> Outcome<CommandAnswer> {
    let mapped = match MappedCommand::selected(contract, selected) {
        Ok(mapped) => mapped,
        Err(outcome) => return outcome,
    };
    let discovery_path = repository.state_root().join("canvas");
    let discovery = match DiscoveryRead::open(&discovery_path) {
        Ok(discovery) => discovery,
        Err(outcome) => return outcome,
    };
    request(contract, mapped, discovery)
}

struct MappedCommand {
    id: CanonicalUuid,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: serde_json::Value,
}

impl MappedCommand {
    fn selected(contract: &Contract, selected: SelectedCommand<'_>) -> ClientResult<Self> {
        match (contract.path, selected) {
            (["down"], SelectedCommand::Down) => Self::new(
                contract,
                &["down"],
                CommandKind::UpChange,
                vec![Witness::Transition {
                    from: wire::ResidentState::Seated,
                    to: wire::ResidentState::Dark,
                }],
                &wire::intent::UpChange {
                    state: wire::ResidentState::Dark,
                },
            ),
            (["release", "publish"], SelectedCommand::ReleasePublish { release, state }) => {
                let mut expected = vec![Witness::Observation {
                    key: release.key().clone(),
                    head: state.observation(),
                }];
                expected.extend(release.receipts().map(|proof| Witness::Sealed {
                    proof: proof.clone(),
                    manifest: release.manifest().clone(),
                }));
                Self::new(
                    contract,
                    &["release", "publish"],
                    CommandKind::ReleasePublish,
                    expected,
                    &wire::intent::ReleasePublish {
                        key: release.key().clone(),
                        manifest: release.manifest().clone(),
                    },
                )
            }
            (["lane"], SelectedCommand::LaneRead) => Self::new(
                contract,
                &["lane"],
                CommandKind::LaneRead,
                Vec::new(),
                &wire::intent::LaneRead {},
            ),
            (["lane", "add"], SelectedCommand::LaneSeat { body }) => Self::new(
                contract,
                &["lane", "add"],
                CommandKind::LaneSeat,
                Vec::new(),
                body,
            ),
            (["lane", "drain"] | ["lane", "wake"], SelectedCommand::LaneState { body }) => {
                Self::new(
                    contract,
                    contract.path,
                    CommandKind::LaneState,
                    Vec::new(),
                    body,
                )
            }
            (["recall"], SelectedCommand::LaneRecall { body }) => Self::new(
                contract,
                &["recall"],
                CommandKind::LaneRecall,
                Vec::new(),
                body,
            ),
            _ => Err(protocol_fault(
                "command::mapping-unsupported",
                "selected CLI contract has no resident command mapping",
            )),
        }
    }

    fn new<B: Serialize>(
        contract: &Contract,
        mapped_path: &[&str],
        kind: CommandKind,
        expected: Vec<Witness>,
        body: &B,
    ) -> ClientResult<Self> {
        if contract.path != mapped_path
            || !contract.guards.contains(&Guard::Resident)
            || matches!(contract.principal, Principal::Local)
        {
            return Err(protocol_fault(
                "command::mapping-unsupported",
                "selected CLI contract has no resident command mapping",
            ));
        }
        let encoded = wire::encode(body).map_err(|_| {
            protocol_fault(
                "command::body-unencodable",
                "selected CLI body cannot be represented by the current wire",
            )
        })?;
        kind.binding().body.decode(&encoded).map_err(|_| {
            protocol_fault(
                "command::body-incomplete",
                "selected CLI body does not match the command registry binding",
            )
        })?;
        let body = wire::decode(&encoded).map_err(|_| {
            protocol_fault(
                "command::body-unrepresentable",
                "validated command body did not retain its wire representation",
            )
        })?;
        let mapped = Self {
            id: new_uuid(),
            kind,
            expected,
            body,
        };
        let preflight = mapped.envelope(new_uuid());
        preflight
            .witnessed()
            .map_err(|error| protocol_fault("command::witness-law", &error.to_string()))?;
        Ok(mapped)
    }

    fn envelope(&self, epoch: CanonicalUuid) -> Command<serde_json::Value> {
        Command {
            v: wire::WireVersion::CURRENT,
            id: self.id,
            epoch,
            kind: self.kind,
            expected: self.expected.clone(),
            body: self.body.clone(),
        }
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
        let mut file = open_discovery(path)?;
        let opened = file.metadata().map_err(discovery_unavailable)?;
        require_secure(&opened, DiscoveryKind::File)?;
        if FileIdentity::from_metadata(&opened) != file_identity {
            return Err(discovery_replaced());
        }
        let mut bytes = Vec::new();
        file.by_ref()
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
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    options.open(path).map_err(discovery_unavailable)
}

fn request(
    contract: &Contract,
    mapped: MappedCommand,
    discovery: DiscoveryRead,
) -> Outcome<CommandAnswer> {
    let capability = match ambient_lane_capability() {
        Ok(Some(capability)) => capability,
        Ok(None) => discovery.value.capability,
        Err(outcome) => return outcome,
    };
    let authority = match endpoint_authority(&discovery.value.endpoint) {
        Ok(authority) => authority,
        Err(outcome) => return outcome,
    };
    let address = match authority.parse::<SocketAddr>() {
        Ok(address) if address.ip().is_loopback() => address,
        Ok(_) => {
            return protocol_fault(
                "canvas::endpoint-not-loopback",
                "canvas discovery does not name a loopback socket",
            );
        }
        Err(_) => {
            return protocol_fault(
                "canvas::endpoint-invalid",
                "canvas discovery does not name one socket authority",
            );
        }
    };
    if let Err(outcome) = discovery.verify_current() {
        return outcome;
    }
    let stream = match TcpStream::connect_timeout(&address, SOCKET_TIMEOUT) {
        Ok(stream) => stream,
        Err(error) => return environment_fault("canvas::connect-failed", &error.to_string()),
    };
    if let Err(error) = stream.set_read_timeout(Some(SOCKET_TIMEOUT)) {
        return environment_fault("canvas::timeout-failed", &error.to_string());
    }
    if let Err(error) = stream.set_write_timeout(Some(SOCKET_TIMEOUT)) {
        return environment_fault("canvas::timeout-failed", &error.to_string());
    }
    if let Err(outcome) = discovery.verify_current() {
        return outcome;
    }
    let mut request = match format!("ws://{authority}/socket").into_client_request() {
        Ok(request) => request,
        Err(_) => {
            return protocol_fault(
                "canvas::request-invalid",
                "canvas endpoint cannot form the current socket request",
            );
        }
    };
    let origin = match discovery
        .value
        .endpoint
        .parse::<tungstenite::http::HeaderValue>()
    {
        Ok(origin) => origin,
        Err(_) => {
            return protocol_fault(
                "canvas::origin-invalid",
                "canvas endpoint cannot form the current socket origin",
            );
        }
    };
    request.headers_mut().insert(ORIGIN, origin);
    let (mut socket, _) = match tungstenite::client(request, stream) {
        Ok(opened) => opened,
        Err(error) => return environment_fault("canvas::opening-failed", &error.to_string()),
    };
    if let Err(outcome) = discovery.verify_current() {
        return outcome;
    }
    let hello = wire::Hello {
        wire: wire::WireVersion::CURRENT,
        capability,
        epoch: discovery.value.epoch,
    };
    if let Err(outcome) = send(&mut socket, &hello) {
        return outcome;
    }
    let opening = match receive(&mut socket) {
        Ok(opening) => opening,
        Err(outcome) => return outcome,
    };
    let accepted = match decode_opening(&opening, &discovery.value, capability) {
        Ok(accepted) => accepted,
        Err(outcome) => return outcome,
    };
    if !principal_matches(contract.principal, &accepted.actor) {
        return protocol_fault(
            "command::principal-mismatch",
            "resident capability resolved to a principal outside the selected CLI contract",
        );
    }
    if let Err(outcome) = discovery.verify_current() {
        return outcome;
    }
    let command = mapped.envelope(discovery.value.epoch);
    if let Err(outcome) = send(&mut socket, &command) {
        return outcome;
    }
    let response = match receive(&mut socket) {
        Ok(response) => response,
        Err(outcome) => return outcome,
    };
    decode_response(
        &response,
        contract,
        mapped.id,
        mapped.kind,
        discovery.value.epoch,
        capability,
    )
}

fn ambient_lane_capability() -> ClientResult<Option<CanonicalUuid>> {
    let Some(value) = crate::bootstrap_environment("SPICE_LANE") else {
        // env-policy: allow
        return Ok(None);
    };
    let Some(value) = value.to_str() else {
        return Err(protocol_fault(
            "hello::lane-capability-invalid",
            "SPICE_LANE is not UTF-8",
        ));
    };
    value.parse().map(Some).map_err(|_| {
        protocol_fault(
            "hello::lane-capability-invalid",
            "SPICE_LANE is not one canonical capability UUID",
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

fn decode_opening(
    bytes: &[u8],
    discovery: &canvas::Discovery,
    capability: CanonicalUuid,
) -> ClientResult<wire::Accepted> {
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
        if discloses_capability(&refusal, capability) {
            return Err(capability_disclosed());
        }
        return Err(Outcome::Refusal(refusal));
    }
    if let Ok(fault) = wire::decode::<Fault>(bytes) {
        if discloses_capability(&fault, capability) {
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

fn send<T: Serialize>(socket: &mut WebSocket<TcpStream>, value: &T) -> ClientResult<()> {
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

fn receive(socket: &mut WebSocket<TcpStream>) -> ClientResult<Vec<u8>> {
    loop {
        match socket
            .read()
            .map_err(|error| environment_fault("canvas::receive-failed", &error.to_string()))?
        {
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

fn new_uuid() -> CanonicalUuid {
    CanonicalUuid::new(uuid::Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use wire::intent::{StateAnswer, UpChange};
    use wire::{ResidentState, Seq, SeqRange};

    use super::*;
    use crate::cli::{ExitOutcome, JournalAccess, ResultClass};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    const COMMAND_CONTRACT: Contract = Contract {
        path: &["fixture", "command"],
        syntax: "fixture command",
        principal: Principal::Operator,
        journal: JournalAccess::Command,
        reply_position: ReplyPositionLaw::Committed,
        facts: &[],
        guards: &[Guard::Resident, Guard::OperatorCapability],
        tools: &[],
        result: ResultClass::Reply,
        outcomes: ExitOutcome::ALL,
        battery: "command-client::fixture",
    };

    struct Fixture {
        root: PathBuf,
        canvas: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "spice-command-client-{name}-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("fixture runtime directory");
            let canvas = root.join("canvas");
            Self { root, canvas }
        }

        fn publish(&self) -> canvas::Discovery {
            let discovery = canvas::Discovery {
                endpoint: "http://127.0.0.1:48123".to_owned(),
                capability: uuid(1),
                epoch: uuid(2),
            };
            discovery
                .publish(&self.canvas)
                .expect("fixture discovery publishes");
            discovery
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).expect("fixture cleanup");
        }
    }

    fn uuid(last: u8) -> CanonicalUuid {
        format!("00000000-0000-4000-8000-{last:012x}")
            .parse()
            .expect("canonical fixture UUID")
    }

    fn mapped() -> MappedCommand {
        MappedCommand::new(
            &COMMAND_CONTRACT,
            &["fixture", "command"],
            CommandKind::UpChange,
            vec![Witness::Transition {
                from: ResidentState::Seated,
                to: ResidentState::Dark,
            }],
            &UpChange {
                state: ResidentState::Dark,
            },
        )
        .expect("fixture command maps")
    }

    fn response(
        id: CanonicalUuid,
        epoch: CanonicalUuid,
        position: ReplyPosition,
        body: serde_json::Value,
    ) -> Vec<u8> {
        wire::encode(&Response {
            v: wire::WireVersion::CURRENT,
            id,
            epoch,
            outcome: Outcome::Reply(Reply { position, body }),
        })
        .expect("fixture response encodes")
    }

    fn committed() -> ReplyPosition {
        ReplyPosition::Committed(SeqRange {
            first: Seq::new(1).expect("fixture position"),
            last: Seq::new(2).expect("fixture position"),
        })
    }

    fn state_body() -> serde_json::Value {
        serde_json::to_value(StateAnswer {
            from: ResidentState::Seated,
            to: ResidentState::Dark,
        })
        .expect("fixture answer encodes")
    }

    #[test]
    fn mapping_rejects_the_wrong_contract_and_incomplete_body_before_discovery() {
        let wrong_path = MappedCommand::selected(&COMMAND_CONTRACT, SelectedCommand::Down);
        let Err(Outcome::Fault(fault)) = wrong_path else {
            panic!("wrong path must fail mapping");
        };
        assert_eq!(fault.reason, "command::mapping-unsupported");

        let incomplete = MappedCommand::new(
            &COMMAND_CONTRACT,
            &["fixture", "command"],
            CommandKind::UpChange,
            vec![Witness::Transition {
                from: ResidentState::Seated,
                to: ResidentState::Dark,
            }],
            &serde_json::json!({}),
        );
        let Err(Outcome::Fault(fault)) = incomplete else {
            panic!("incomplete body must fail mapping");
        };
        assert_eq!(fault.reason, "command::body-incomplete");
    }

    #[test]
    fn mapping_enforces_the_wire_witness_law() {
        let missing = MappedCommand::new(
            &COMMAND_CONTRACT,
            &["fixture", "command"],
            CommandKind::UpChange,
            Vec::new(),
            &UpChange {
                state: ResidentState::Dark,
            },
        );
        let Err(Outcome::Fault(fault)) = missing else {
            panic!("missing witness must fail mapping");
        };
        assert_eq!(fault.reason, "command::witness-law");
    }

    #[test]
    fn discovery_replacement_is_detected_by_file_identity() {
        let fixture = Fixture::new("replacement");
        let original = fixture.publish();
        let read = DiscoveryRead::open(&fixture.canvas).expect("discovery opens");
        let replacement = canvas::Discovery {
            capability: uuid(3),
            epoch: uuid(4),
            ..original
        };
        replacement
            .publish(&fixture.canvas)
            .expect("replacement publishes");

        let Err(Outcome::Fault(fault)) = read.verify_current() else {
            panic!("replacement must invalidate the captured discovery");
        };
        assert_eq!(fault.reason, "canvas::discovery-replaced");
    }

    #[cfg(unix)]
    #[test]
    fn discovery_symlinks_and_public_modes_are_refused() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let fixture = Fixture::new("insecure");
        let discovery = fixture.publish();
        fs::set_permissions(&fixture.canvas, fs::Permissions::from_mode(0o644))
            .expect("fixture mode changes");
        let Err(Outcome::Fault(fault)) = DiscoveryRead::open(&fixture.canvas) else {
            panic!("public discovery must be refused");
        };
        assert_eq!(fault.reason, "canvas::discovery-insecure");

        fs::remove_file(&fixture.canvas).expect("public fixture removes");
        let target = fixture.root.join("target");
        discovery.publish(&target).expect("target publishes");
        symlink(&target, &fixture.canvas).expect("fixture symlink");
        let Err(Outcome::Fault(fault)) = DiscoveryRead::open(&fixture.canvas) else {
            panic!("symlink discovery must be refused");
        };
        assert_eq!(fault.reason, "canvas::discovery-insecure");
    }

    #[test]
    fn response_binds_identity_epoch_position_and_answer_shape() {
        let mapped = mapped();
        let epoch = uuid(5);
        let bytes = response(mapped.id, epoch, committed(), state_body());
        let outcome = decode_response(
            &bytes,
            &COMMAND_CONTRACT,
            mapped.id,
            mapped.kind,
            epoch,
            uuid(6),
        );
        assert!(matches!(
            outcome,
            Outcome::Reply(Reply {
                position: ReplyPosition::Committed(_),
                body: CommandAnswer::State(StateAnswer {
                    to: ResidentState::Dark,
                    ..
                })
            })
        ));

        let replay = decode_response(
            &bytes,
            &COMMAND_CONTRACT,
            uuid(7),
            mapped.kind,
            epoch,
            uuid(6),
        );
        let Outcome::Fault(fault) = replay else {
            panic!("a replayed response cannot answer a different command");
        };
        assert_eq!(fault.reason, "command::answer-mismatch");

        let snapshot = response(
            mapped.id,
            epoch,
            ReplyPosition::Snapshot(Seq::new(1).expect("fixture position")),
            state_body(),
        );
        let Outcome::Fault(fault) = decode_response(
            &snapshot,
            &COMMAND_CONTRACT,
            mapped.id,
            mapped.kind,
            epoch,
            uuid(6),
        ) else {
            panic!("snapshot cannot answer a mutating contract");
        };
        assert_eq!(fault.reason, "command::position-mismatch");

        let malformed = response(
            mapped.id,
            epoch,
            committed(),
            serde_json::json!({ "from": "seated" }),
        );
        let Outcome::Fault(fault) = decode_response(
            &malformed,
            &COMMAND_CONTRACT,
            mapped.id,
            mapped.kind,
            epoch,
            uuid(6),
        ) else {
            panic!("wrong answer shape must fault");
        };
        assert_eq!(fault.reason, "command::answer-binding");
    }

    #[test]
    fn remote_outcomes_cannot_echo_the_discovery_capability() {
        let capability = uuid(8);
        let discovery = canvas::Discovery {
            endpoint: "http://127.0.0.1:48123".to_owned(),
            capability,
            epoch: uuid(9),
        };
        let refusal = Refusal {
            reason: "hello::fixture".to_owned(),
            witness: None,
            expected: None,
            observed: Some(capability.to_string()),
            remedy: "use current discovery".to_owned(),
        };
        let opening = wire::encode(&refusal).expect("fixture refusal encodes");
        let Err(Outcome::Fault(fault)) = decode_opening(&opening, &discovery, capability) else {
            panic!("opening capability echo must fault");
        };
        assert_eq!(fault.reason, "canvas::capability-disclosed");
        assert!(!fault.detail.contains(&capability.to_string()));

        let mapped = mapped();
        let response = wire::encode(&Response {
            v: wire::WireVersion::CURRENT,
            id: mapped.id,
            epoch: discovery.epoch,
            outcome: Outcome::<serde_json::Value>::Refusal(refusal),
        })
        .expect("fixture response encodes");
        let Outcome::Fault(fault) = decode_response(
            &response,
            &COMMAND_CONTRACT,
            mapped.id,
            mapped.kind,
            discovery.epoch,
            capability,
        ) else {
            panic!("response capability echo must fault");
        };
        assert_eq!(fault.reason, "canvas::capability-disclosed");
        assert!(!fault.detail.contains(&capability.to_string()));
    }
}
