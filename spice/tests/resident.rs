use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant as Monotonic};

use bus::{Bus, CommitOutcome};
use journal::{FactCause, JournalWrite};
use rusqlite::{Connection, params};
use sill::{Acquisition, LockFile, LockName};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::header::ORIGIN;
use tungstenite::{Message, WebSocket};
use wire::{
    Actor, ClaimDisposition, ClaimHead, FactKind, Instant, LeaseHead, Outcome, Producer,
    ReplyPosition, TaskHead, WorkflowHead,
};

const FAULT_EXIT: i32 = 1;
const REFUSAL_EXIT: i32 = 2;
/// The longest a client command may take, and the longest a fixture socket
/// waits while one is in flight. This is not the bound on waiting for a state
/// to arrive: that one is sized from what announcing an arrival costs and
/// lives with the wait itself, in [`readiness::ARRIVAL_HORIZON`].
const CLIENT_DEADLINE_BOUND: Duration = Duration::from_secs(10);
const LANE_DEADLINE_BOUND: Duration = Duration::from_secs(20);
/// The hang guard around a supervised vendor effect reaching its tape.
///
/// The wait itself is event-driven; this covers the full vendor, capture,
/// decode, and persistence pipeline rather than resident discovery.
const LANE_EFFECT_HORIZON: Duration = Duration::from_secs(45);
const ADVERSARIAL_PEER_LIFE: Duration = Duration::from_secs(7);

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    runtime: PathBuf,
    cleanup: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-resident-process-{name}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let runtime = root.join(".git/.spice");
        fs::create_dir_all(root.join(".git")).expect("fixture git directory");
        Self {
            cleanup: root.clone(),
            root,
            runtime,
        }
    }

    fn linked(name: &str) -> Self {
        let cleanup = std::env::temp_dir().join(format!(
            "spice-resident-linked-{name}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let root = cleanup.join("worktree");
        let common = cleanup.join("common.git");
        let git = common.join("worktrees/linked");
        fs::create_dir_all(&root).expect("linked worktree directory");
        fs::create_dir_all(&git).expect("linked Git directory");
        fs::write(root.join(".git"), format!("gitdir: {}\n", git.display()))
            .expect("linked Git pointer");
        fs::write(git.join("commondir"), "../..\n").expect("linked common pointer");
        Self {
            root,
            runtime: common.join(".spice"),
            cleanup,
        }
    }

    fn journal(&self) -> PathBuf {
        self.runtime.join("data/spicejournal.sqlite3")
    }

    fn resident(&self) -> PathBuf {
        self.runtime.join("resident")
    }

    fn canvas(&self) -> PathBuf {
        self.runtime.join("canvas")
    }

    fn discovery(&self) -> canvas::Discovery {
        let bytes = fs::read(self.canvas()).expect("current discovery reads");
        wire::decode(&bytes).expect("current discovery decodes")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.cleanup).expect("fixture cleanup");
    }
}

struct RunningResident {
    child: Option<Child>,
}

impl RunningResident {
    fn spawn(fixture: &Fixture) -> Self {
        Self::spawn_with(fixture, Path::new(env!("CARGO_BIN_EXE_spice")))
    }

    fn spawn_with(fixture: &Fixture, executable: &Path) -> Self {
        Self::spawn_with_environment(fixture, executable, None)
    }

    fn spawn_with_path(fixture: &Fixture, executable: &Path, path: &Path) -> Self {
        Self::spawn_with_environment(fixture, executable, Some(path))
    }

    fn spawn_with_environment(fixture: &Fixture, executable: &Path, path: Option<&Path>) -> Self {
        let mut command = Command::new(executable);
        command.args(["--json", "up"]);
        if let Some(path) = path {
            command.env("PATH", path);
        }
        let child = command
            .current_dir(&fixture.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("foreground resident launches");
        Self { child: Some(child) }
    }

    fn assert_ready(&mut self, fixture: &Fixture) {
        fs::create_dir_all(&fixture.runtime).expect("runtime directory");
        match readiness::wait_until_ready(
            self.child.as_mut().expect("resident child"),
            &fixture.runtime,
            readiness::ARRIVAL_HORIZON,
            "resident discovery",
            || fixture.canvas().is_file(),
        ) {
            Readiness::Ready => {}
            Readiness::Exited => {
                let output = self
                    .child
                    .take()
                    .expect("resident child")
                    .wait_with_output()
                    .expect("collect early resident exit");
                panic!(
                    "foreground resident exited before readiness: stdout={} stderr={}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                );
            }
        }
    }

    fn wait(mut self) -> Output {
        self.child
            .take()
            .expect("resident child")
            .wait_with_output()
            .expect("foreground resident exits")
    }
}

impl Drop for RunningResident {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn invoke(fixture: &Fixture, command: &str) -> Output {
    invoke_form(fixture, command, true)
}

fn invoke_form(fixture: &Fixture, command: &str, json: bool) -> Output {
    invoke_args(fixture, &[command], json)
}

fn invoke_args(fixture: &Fixture, arguments: &[&str], json: bool) -> Output {
    let mut process = Command::new(env!("CARGO_BIN_EXE_spice"));
    if json {
        process.arg("--json");
    }
    process
        .args(arguments)
        .current_dir(&fixture.root)
        .output()
        .expect("spice launches")
}

fn outcome(output: &Output) -> Outcome<()> {
    let bytes = if output.status.success() {
        &output.stdout
    } else {
        &output.stderr
    };
    wire::decode(bytes).expect("process speaks one outcome")
}

fn value_outcome(output: &Output) -> Outcome<serde_json::Value> {
    let bytes = if output.status.success() {
        &output.stdout
    } else {
        &output.stderr
    };
    wire::decode(bytes).expect("process speaks one typed value outcome")
}

fn journal_cursor(fixture: &Fixture) -> wire::Seq {
    journal::JournalReader::open(fixture.journal())
        .expect("fixture journal reads")
        .cursor()
        .expect("fixture cursor reads")
        .expect("running resident has a cursor")
}

fn trust_pended(fixture: &Fixture) -> Vec<wire::body::TrustPended> {
    journal::JournalReader::open(fixture.journal())
        .expect("fixture journal reads")
        .fact_snapshot()
        .expect("fixture facts read")
        .facts()
        .iter()
        .filter(|envelope| envelope.fact.kind() == FactKind::TrustPended)
        .map(|envelope| {
            let bytes = wire::encode(&envelope.fact).expect("opaque trust fact encodes");
            wire::decode::<wire::Fact<wire::body::TrustPended>>(&bytes)
                .expect("validated trust fact decodes to its registered body")
                .body
        })
        .collect()
}

#[derive(Clone, Copy)]
enum FakeBehavior {
    Success,
    Replay,
    EchoCapability,
    ResidentLoss,
    ReadTimeout,
    TotalDeadlineDrip,
    ControlFrames,
    WriteTimeout,
    OpeningStall,
}

fn fake_down(fixture: &Fixture, behavior: FakeBehavior, json: bool) -> (Output, Duration) {
    fs::create_dir_all(&fixture.runtime).expect("fixture runtime directory");
    let listener = TcpListener::bind("127.0.0.1:0").expect("fake resident binds");
    let discovery = canvas::Discovery {
        endpoint: format!(
            "http://{}",
            listener.local_addr().expect("fake resident address")
        ),
        capability: uuid("00000000-0000-4000-8000-000000000031"),
        epoch: uuid("00000000-0000-4000-8000-000000000032"),
    };
    discovery
        .publish(fixture.canvas())
        .expect("fake discovery publishes");
    let canvas = fixture.canvas();
    let server_discovery = discovery.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let server = std::thread::spawn(move || {
        run_fake_resident(listener, server_discovery, canvas, behavior, server_stop);
    });
    let started = Monotonic::now();
    let output = invoke_form(fixture, "down", json);
    let elapsed = started.elapsed();
    stop.store(true, Ordering::Release);
    server.join().expect("fake resident completes");
    (output, elapsed)
}

fn run_fake_resident(
    listener: TcpListener,
    discovery: canvas::Discovery,
    canvas: PathBuf,
    behavior: FakeBehavior,
    stop: Arc<AtomicBool>,
) {
    if matches!(behavior, FakeBehavior::TotalDeadlineDrip) {
        run_deadline_drip(listener, &discovery, &canvas, stop);
        return;
    }
    if matches!(behavior, FakeBehavior::OpeningStall) {
        let (stream, _) = listener.accept().expect("fake resident accepts");
        hold_adversarial_peer(&stop);
        drop(stream);
        return;
    }
    let mut socket = accept_fake(listener);
    assert_fake_hello(&mut socket, &discovery);
    if matches!(behavior, FakeBehavior::EchoCapability) {
        send_capability_echo(&mut socket, discovery.capability);
        return;
    }
    send_server(
        &mut socket,
        &wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch: discovery.epoch,
        },
    );
    if matches!(behavior, FakeBehavior::ResidentLoss) {
        socket
            .get_mut()
            .shutdown(Shutdown::Both)
            .expect("fake resident socket shuts down");
        return;
    }
    let command = read_fake_dark_command(&mut socket, discovery.epoch);
    if matches!(behavior, FakeBehavior::ReadTimeout) {
        hold_adversarial_peer(&stop);
        return;
    }
    if matches!(behavior, FakeBehavior::ControlFrames) {
        send_control_frames(&mut socket, &stop);
        return;
    }
    if matches!(behavior, FakeBehavior::WriteTimeout) {
        saturate_control_writes(&mut socket, &stop);
        return;
    }
    if matches!(behavior, FakeBehavior::Success) {
        fs::remove_file(canvas).expect("fake resident withdraws discovery");
    }
    send_dark_response(&mut socket, &discovery, &command, behavior);
}

struct DripStream {
    stream: TcpStream,
    stop: Arc<AtomicBool>,
}

impl Read for DripStream {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.stream.read(bytes)
    }
}

impl Write for DripStream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        for chunk in bytes.chunks(8) {
            if self.stop.load(Ordering::Acquire) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "fixture client completed",
                ));
            }
            std::thread::sleep(Duration::from_millis(75));
            self.stream.write_all(chunk)?;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

fn run_deadline_drip(
    listener: TcpListener,
    discovery: &canvas::Discovery,
    canvas: &Path,
    stop: Arc<AtomicBool>,
) {
    let (stream, _) = listener.accept().expect("drip resident accepts");
    stream
        .set_read_timeout(Some(CLIENT_DEADLINE_BOUND))
        .expect("drip resident read timeout");
    stream
        .set_write_timeout(Some(CLIENT_DEADLINE_BOUND))
        .expect("drip resident write timeout");
    let stream = DripStream { stream, stop };
    let Ok(mut socket) = tungstenite::accept(stream) else {
        return;
    };
    assert_fake_hello(&mut socket, discovery);
    if send_server_result(
        &mut socket,
        &wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch: discovery.epoch,
        },
    )
    .is_err()
    {
        return;
    }
    let command = read_fake_dark_command(&mut socket, discovery.epoch);
    fs::write(canvas.with_file_name("deadline-response"), b"response")
        .expect("drip resident reaches the command response");
    let _ = send_server_result(
        &mut socket,
        &dark_response(discovery, &command, FakeBehavior::Success),
    );
}

fn send_control_frames(socket: &mut WebSocket<TcpStream>, stop: &AtomicBool) {
    let deadline = Monotonic::now() + ADVERSARIAL_PEER_LIFE;
    while !stop.load(Ordering::Acquire) && Monotonic::now() < deadline {
        if socket.send(Message::Pong(vec![0x51].into())).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn saturate_control_writes(socket: &mut WebSocket<TcpStream>, stop: &AtomicBool) {
    socket2::SockRef::from(socket.get_ref())
        .set_recv_buffer_size(1_024)
        .expect("fake resident receive buffer shrinks");
    socket
        .get_mut()
        .set_write_timeout(Some(Duration::from_millis(100)))
        .expect("fake resident flood write timeout");
    let deadline = Monotonic::now() + ADVERSARIAL_PEER_LIFE;
    while !stop.load(Ordering::Acquire) && Monotonic::now() < deadline {
        if socket.send(Message::Ping(vec![0x57; 125].into())).is_err() {
            hold_adversarial_peer_until(stop, deadline);
            return;
        }
    }
}

fn hold_adversarial_peer(stop: &AtomicBool) {
    hold_adversarial_peer_until(stop, Monotonic::now() + ADVERSARIAL_PEER_LIFE);
}

fn hold_adversarial_peer_until(stop: &AtomicBool, deadline: Monotonic) {
    while !stop.load(Ordering::Acquire) && Monotonic::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn accept_fake(listener: TcpListener) -> WebSocket<TcpStream> {
    let (stream, _) = listener.accept().expect("fake resident accepts");
    stream
        .set_read_timeout(Some(CLIENT_DEADLINE_BOUND))
        .expect("fake resident read timeout");
    stream
        .set_write_timeout(Some(CLIENT_DEADLINE_BOUND))
        .expect("fake resident write timeout");
    tungstenite::accept(stream).expect("fake WebSocket accepts")
}

fn assert_fake_hello<S: Read + Write>(socket: &mut WebSocket<S>, discovery: &canvas::Discovery) {
    let opening = socket
        .read()
        .expect("fake resident reads Hello")
        .into_data();
    let hello: wire::Hello = wire::decode(&opening).expect("client Hello decodes");
    assert_eq!(hello.wire, wire::WireVersion::CURRENT);
    assert_eq!(hello.capability, discovery.capability);
    assert_eq!(hello.epoch, discovery.epoch);
}

fn send_capability_echo<S: Read + Write>(
    socket: &mut WebSocket<S>,
    capability: wire::CanonicalUuid,
) {
    send_server(
        socket,
        &wire::Refusal {
            reason: "hello::fixture".to_owned(),
            witness: None,
            expected: None,
            observed: Some(capability.to_string()),
            remedy: "read current discovery".to_owned(),
        },
    );
}

fn read_fake_dark_command<S: Read + Write>(
    socket: &mut WebSocket<S>,
    epoch: wire::CanonicalUuid,
) -> wire::Command<serde_json::Value> {
    let frame = socket
        .read()
        .expect("fake resident reads one command")
        .into_data();
    let command: wire::Command<serde_json::Value> =
        wire::decode(&frame).expect("client command decodes");
    assert_eq!(command.v, wire::WireVersion::CURRENT);
    assert_eq!(command.epoch, epoch);
    assert_eq!(command.kind, wire::CommandKind::UpChange);
    assert_eq!(
        command.expected,
        [wire::Witness::Transition {
            from: wire::ResidentState::Seated,
            to: wire::ResidentState::Dark,
        }]
    );
    let body = command
        .kind
        .binding()
        .body
        .decode(&wire::encode(&command.body).expect("command body encodes"))
        .expect("command body follows its binding");
    assert!(matches!(
        body,
        wire::intent::Intent::UpChange(wire::intent::UpChange {
            state: wire::ResidentState::Dark
        })
    ));
    command
}

fn dark_response(
    discovery: &canvas::Discovery,
    command: &wire::Command<serde_json::Value>,
    behavior: FakeBehavior,
) -> wire::Response<wire::intent::StateAnswer> {
    let id = if matches!(behavior, FakeBehavior::Replay) {
        uuid("00000000-0000-4000-8000-000000000033")
    } else {
        command.id
    };
    wire::Response {
        v: wire::WireVersion::CURRENT,
        id,
        epoch: discovery.epoch,
        outcome: Outcome::Reply(wire::Reply {
            position: ReplyPosition::Committed(wire::SeqRange {
                first: wire::Seq::new(1).expect("fixture position"),
                last: wire::Seq::new(2).expect("fixture position"),
            }),
            body: wire::intent::StateAnswer {
                from: wire::ResidentState::Seated,
                to: wire::ResidentState::Dark,
            },
        }),
    }
}

fn send_dark_response<S: Read + Write>(
    socket: &mut WebSocket<S>,
    discovery: &canvas::Discovery,
    command: &wire::Command<serde_json::Value>,
    behavior: FakeBehavior,
) {
    send_server(socket, &dark_response(discovery, command, behavior));
}

fn send_server<S: Read + Write, T: serde::Serialize>(socket: &mut WebSocket<S>, value: &T) {
    send_server_result(socket, value).expect("fake response sends");
}

fn send_server_result<S: Read + Write, T: serde::Serialize>(
    socket: &mut WebSocket<S>,
    value: &T,
) -> tungstenite::Result<()> {
    socket.send(Message::Binary(
        wire::encode(value).expect("fake response encodes").into(),
    ))
}

fn open_socket(discovery: &canvas::Discovery) -> WebSocket<TcpStream> {
    let authority = discovery
        .endpoint
        .strip_prefix("http://")
        .expect("fixture endpoint is HTTP");
    let address: SocketAddr = authority.parse().expect("fixture endpoint address");
    let stream = TcpStream::connect(address).expect("fixture socket connects");
    stream
        .set_read_timeout(Some(CLIENT_DEADLINE_BOUND))
        .expect("fixture read timeout");
    stream
        .set_write_timeout(Some(CLIENT_DEADLINE_BOUND))
        .expect("fixture write timeout");
    let mut request = format!("ws://{authority}/socket")
        .into_client_request()
        .expect("fixture socket request");
    request.headers_mut().insert(
        ORIGIN,
        discovery.endpoint.parse().expect("fixture socket origin"),
    );
    tungstenite::client(request, stream)
        .expect("fixture WebSocket opens")
        .0
}

fn hello(
    discovery: &canvas::Discovery,
    capability: wire::CanonicalUuid,
) -> (WebSocket<TcpStream>, Vec<u8>) {
    let mut socket = open_socket(discovery);
    socket
        .send(Message::Binary(
            wire::encode(&wire::Hello {
                wire: wire::WireVersion::CURRENT,
                capability,
                epoch: discovery.epoch,
            })
            .expect("fixture Hello encodes")
            .into(),
        ))
        .expect("fixture Hello sends");
    let frame = socket.read().expect("fixture opening reply").into_data();
    (socket, frame.to_vec())
}

fn authenticate(
    discovery: &canvas::Discovery,
    capability: wire::CanonicalUuid,
) -> (WebSocket<TcpStream>, wire::Accepted) {
    let (socket, frame) = hello(discovery, capability);
    let accepted = wire::decode(&frame).expect("fixture session accepted");
    (socket, accepted)
}

fn assert_lane_session(
    discovery: &canvas::Discovery,
    lane: wire::CanonicalUuid,
    capability: wire::CanonicalUuid,
    request_id: wire::CanonicalUuid,
) {
    let (mut socket, accepted) = authenticate(discovery, capability);
    assert_eq!(accepted.actor, Actor::Lane(lane));
    let command = wire::Command {
        v: wire::WireVersion::CURRENT,
        id: request_id,
        epoch: discovery.epoch,
        kind: wire::CommandKind::UpChange,
        expected: vec![wire::Witness::Transition {
            from: wire::ResidentState::Seated,
            to: wire::ResidentState::Dark,
        }],
        body: wire::intent::UpChange {
            state: wire::ResidentState::Dark,
        },
    };
    socket
        .send(Message::Binary(
            wire::encode(&command).expect("lane command encodes").into(),
        ))
        .expect("lane command sends");
    let frame = socket.read().expect("lane response arrives").into_data();
    let response: wire::Response<serde_json::Value> =
        wire::decode(&frame).expect("lane response decodes");
    assert_eq!((response.id, response.epoch), (request_id, discovery.epoch));
    assert!(matches!(
        response.outcome,
        Outcome::Refusal(wire::Refusal { ref reason, .. })
            if reason == "command::authority"
    ));
}

fn hold(path: &Path, identity: &str) -> sill::Possession {
    fs::create_dir_all(path.parent().expect("lock parent")).expect("lock directory");
    let lock = LockFile::new(
        LockName::new("resident").expect("lock name"),
        path.to_path_buf(),
    );
    let Acquisition::Acquired(possession) = lock.try_acquire().expect("lock acquisition") else {
        panic!("fixture lock unexpectedly contended");
    };
    let file = possession.file();
    file.set_len(0).expect("holder truncates");
    let mut writer = file;
    writer
        .write_all(identity.as_bytes())
        .expect("holder writes");
    writer.sync_data().expect("holder persists");
    possession
}

fn seed_active_claim(fixture: &Fixture) {
    let journal = journal::Journal::open(fixture.journal()).expect("journal opens for fixture");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let task: wire::TaskHandle = "RESIDEN-active".parse().expect("task handle");
    let task_uuid = uuid("00000000-0000-4000-8000-000000000010");
    let lane = uuid("00000000-0000-4000-8000-000000000011");
    let occupant = uuid("00000000-0000-4000-8000-000000000012");
    let epoch = uuid("00000000-0000-4000-8000-000000000013");
    let phase: wire::Phase = "todo".parse().expect("phase");
    let origin: wire::Origin = "task:RESIDEN-active".parse().expect("origin");
    let start = instant("2030-04-12T18:44:09.271Z");
    let expiry = instant("2030-04-12T18:45:09.271Z");
    let maximum = instant("2030-04-12T18:46:09.271Z");

    let mut add = JournalWrite::new(
        Actor::Operator,
        origin.clone(),
        start,
        Producer::CommandDoor,
    );
    let added = add
        .fact(
            wire::body::TaskAdded {
                task: task.clone(),
                uuid: task_uuid,
                title: "Preserve an active claim".to_owned(),
                description: None,
                acceptance: Vec::new(),
                project: "resident".to_owned(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: vec![phase.clone()],
                phase: phase.clone(),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("task fact");
    let task_for_head = task.clone();
    let origin_for_head = origin.clone();
    add.task_head(added, move |seq| TaskHead {
        handle: task_for_head,
        uuid: task_uuid,
        title: "Preserve an active claim".to_owned(),
        description: None,
        acceptance: Vec::new(),
        project: "resident".to_owned(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: origin_for_head,
        revision: seq,
    });
    let phase_for_head = phase.clone();
    add.workflow_head(task.clone(), added, move |_| WorkflowHead {
        flow: vec![phase_for_head.clone()],
        phase: phase_for_head,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    });
    let added = committed(&bus, add);

    let actor = Actor::Lane(lane);
    let mut claim = JournalWrite::new(actor.clone(), origin, start, Producer::CommandDoor);
    let won = claim
        .fact(
            wire::body::ClaimWon {
                task: task.clone(),
                actor: actor.clone(),
                lane,
                occupant,
                epoch,
                until: expiry,
                git: None,
            },
            FactCause::Existing(
                wire::Seq::new(added.last()).expect("task commit has a non-zero position"),
            ),
        )
        .expect("claim fact");
    claim.claim_head(task.clone(), won, move |seq| ClaimHead {
        actor,
        fact: seq,
        lane,
        occupant,
        epoch,
        disposition: ClaimDisposition::Active,
    });
    claim.lease_head(task, won, move |_| LeaseHead {
        start,
        renewal: start,
        maximum,
        expiry,
        epoch,
    });
    committed(&bus, claim);
    bus.shutdown().expect("fixture bus stops");
}

fn seed_lane(fixture: &Fixture, lane: wire::CanonicalUuid, capability: wire::CanonicalUuid) {
    let journal = journal::Journal::open(fixture.journal()).expect("lane fixture journal opens");
    let bus = Bus::start(journal).expect("lane fixture bus starts");
    let current = bus
        .residency_head()
        .expect("lane fixture head reads")
        .expect("lane fixture head exists");
    let epoch = current.body().epoch;
    let mut next = current.body().clone();
    next.lanes.push(wire::LaneResidency {
        lane,
        worktree: fixture.root.display().to_string(),
        life: "drive".to_owned(),
        vendor: "claude".parse().expect("fixture vendor"),
        occupant: None,
        lineage: Vec::new(),
        capabilities: vec![capability],
        session: None,
        disposition: "vacant".to_owned(),
    });
    next.lanes.sort_by_key(|row| row.lane);
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("lane fixture origin");
    let mut write = JournalWrite::new(
        Actor::Spice,
        origin,
        instant("2030-04-12T18:47:09.271Z"),
        Producer::ResidentRecognition,
    );
    let seated = write
        .fact(
            wire::body::LaneSeated {
                lane,
                capability,
                worktree: fixture.root.display().to_string(),
                life: "drive".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
            },
            FactCause::Existing(current.position()),
        )
        .expect("lane fixture fact");
    write.residency_head(seated, move |_| next);
    committed(&bus, write);
    bus.shutdown().expect("lane fixture bus stops");
}

fn committed(bus: &Bus, write: JournalWrite) -> bus::JournalCommit {
    match bus.commit(write).expect("fixture commit reaches writer") {
        CommitOutcome::Committed(committed) => committed,
        outcome => panic!("fixture write did not commit: {outcome:?}"),
    }
}

fn uuid(value: &str) -> wire::CanonicalUuid {
    value.parse().expect("canonical fixture UUID")
}

fn instant(value: &str) -> Instant {
    value.parse().expect("canonical fixture instant")
}

/// Block until `observe` sees the effect of `trigger` under `path`.
///
/// The native observer arms before `trigger` runs and `observe` is read once
/// immediately afterwards, so an effect that lands during setup is still seen.
/// After that read, only a direct kernel notification can cause another
/// observation. A trigger that already ran passes `|| {}` and relies on the
/// post-arm read.
///
/// Panics report the waiting call site rather than this shared helper.
#[track_caller]
fn wait_for_event<T>(
    path: &Path,
    trigger: impl FnOnce(),
    mut observe: impl FnMut() -> Option<T>,
) -> T {
    let (sender, receiver) = mpsc::channel::<std::io::Result<()>>();
    let observers = effect_observers(path);
    trigger();
    if let Some(observed) = observe() {
        return observed;
    }
    wait_for_effect_change(observers, sender.clone());
    let deadline = Monotonic::now() + LANE_EFFECT_HORIZON;
    let mut change_signals = 0_u64;
    loop {
        let remaining = deadline.saturating_duration_since(Monotonic::now());
        match receiver.recv_timeout(remaining) {
            Ok(Ok(())) => {
                change_signals += 1;
                let observers = effect_observers(path);
                if let Some(observed) = observe() {
                    return observed;
                }
                wait_for_effect_change(observers, sender.clone());
            }
            Ok(Err(error)) => panic!("effect observer failed: {error}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!(
                    "effect was not observed before {:?}: change_signals={change_signals}",
                    LANE_EFFECT_HORIZON,
                )
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("effect observer disconnected")
            }
        }
    }
}

fn effect_observers(path: &Path) -> Vec<readiness::filesystem_event::DirectObserver> {
    let root = if path.is_dir() {
        path
    } else {
        path.parent().expect("effect path has a parent")
    };
    let mut watched = vec![root.to_path_buf()];
    if path.is_file() {
        watched.push(path.to_path_buf());
    } else {
        watched.extend(
            fs::read_dir(root)
                .expect("effect directory reads")
                .map(|entry| entry.expect("effect directory entry reads").path())
                .filter(|entry| entry.is_file()),
        );
    }
    watched.sort();
    watched.dedup();
    watched
        .iter()
        .map(|watched| readiness::filesystem_event::DirectObserver::arm(watched, "effect"))
        .collect()
}

fn wait_for_effect_change(
    observers: Vec<readiness::filesystem_event::DirectObserver>,
    sender: mpsc::Sender<std::io::Result<()>>,
) {
    for observer in observers {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let _ = sender.send(observer.wait());
        });
    }
}

/// Block until the fixture vendor records its stop, or fail as soon as the
/// vendor is gone without recording one.
///
/// The stop file is a proxy for the event these tests want, which is the vendor
/// ending. A vendor whose TERM trap outlives its termination grace is killed
/// before the trap writes, and the file it would have written can never appear.
/// Waiting for it would only delay the failure and then report a timeout, which
/// describes the observation rather than what happened to the vendor.
///
/// The operator command returns only after the resident has terminated and
/// reaped the vendor, so in that case the first observation already sees a
/// vendor that is gone and reports it.
///
/// Panics report the waiting call site rather than this shared helper.
#[track_caller]
fn wait_for_vendor_stop(watch: &Path, pid_file: &Path, stopped: &Path) -> String {
    let observed = wait_for_event(
        watch,
        || {},
        || {
            if let Some(stop) = recorded_stop(stopped) {
                return Some(Ok(stop));
            }
            let pid = recorded_pid(pid_file)?;
            if process_alive(pid) {
                return None;
            }
            // The vendor is gone. One more read closes the window where it
            // wrote its stop between these two observations.
            Some(recorded_stop(stopped).ok_or(pid))
        },
    );
    match observed {
        Ok(stop) => stop,
        Err(pid) => panic!(
            "vendor {pid} ended without recording a stop in {}; \
             its TERM trap did not complete within the termination grace",
            stopped.display()
        ),
    }
}

/// The stop a fixture vendor finished recording, if it recorded one.
///
/// The trap redirect creates the file before the write lands, so an empty file
/// is a vendor still writing rather than a vendor that stopped.
fn recorded_stop(stopped: &Path) -> Option<String> {
    fs::read_to_string(stopped)
        .ok()
        .filter(|text| !text.is_empty())
}

/// The identity a fixture vendor recorded for itself, if it got that far.
fn recorded_pid(pid_file: &Path) -> Option<u32> {
    fs::read_to_string(pid_file).ok()?.trim().parse().ok()
}

#[cfg(target_os = "linux")]
fn process_alive(pid: u32) -> bool {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat
            .rsplit_once(") ")
            .and_then(|(_, fields)| fields.as_bytes().first().copied())
            .is_none_or(|state| !matches!(state, b'Z' | b'X' | b'x')),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => process_identity_exists(pid),
    }
}

#[cfg(not(target_os = "linux"))]
fn process_alive(pid: u32) -> bool {
    process_identity_exists(pid)
}

fn process_identity_exists(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .expect("process probe launches")
        .status
        .success()
}

/// Process cases share the transport and lifecycle fixtures defined above.
#[path = "support/readiness.rs"]
mod readiness;

use readiness::Readiness;

#[path = "resident/cases.rs"]
mod cases;

#[path = "resident/event_wait.rs"]
mod event_wait;

#[path = "resident/lane_process.rs"]
mod lane_process;

#[path = "resident/lane_continuity.rs"]
mod lane_continuity;

#[path = "resident/steering_authority.rs"]
mod steering_authority;

#[path = "resident/vendor_process.rs"]
mod vendor_process;
