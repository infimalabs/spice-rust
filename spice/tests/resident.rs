use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant as Monotonic};

use bus::{Bus, CommitOutcome};
use journal::{FactCause, JournalWrite};
use notify::{RecursiveMode, Watcher};
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
const READINESS_DEADLINE: Duration = Duration::from_secs(10);
const CLIENT_DEADLINE_BOUND: Duration = Duration::from_secs(5);
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
        let runtime = root.join(".git/.spice-rust");
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
            runtime: common.join(".spice-rust"),
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
        let child = Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(["--json", "up"])
            .current_dir(&fixture.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("foreground resident launches");
        Self { child: Some(child) }
    }

    fn assert_ready(&mut self, fixture: &Fixture) {
        let (sender, receiver) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })
        .expect("readiness watcher starts");
        watcher
            .watch(
                fixture
                    .runtime
                    .parent()
                    .expect("runtime has Git-common parent"),
                RecursiveMode::Recursive,
            )
            .expect("readiness watcher arms before publication");
        let deadline = Monotonic::now() + READINESS_DEADLINE;
        loop {
            assert!(
                self.child
                    .as_mut()
                    .expect("resident child")
                    .try_wait()
                    .expect("resident status")
                    .is_none(),
                "foreground resident exited before readiness"
            );
            if fixture.canvas().is_file() {
                return;
            }
            let remaining = deadline.saturating_duration_since(Monotonic::now());
            match receiver.recv_timeout(remaining) {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => panic!("readiness notification failed: {error}"),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    panic!("resident did not publish discovery before {READINESS_DEADLINE:?}")
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("readiness watcher disconnected")
                }
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
        .set_read_timeout(Some(READINESS_DEADLINE))
        .expect("drip resident read timeout");
    stream
        .set_write_timeout(Some(READINESS_DEADLINE))
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
        .set_read_timeout(Some(READINESS_DEADLINE))
        .expect("fake resident read timeout");
    stream
        .set_write_timeout(Some(READINESS_DEADLINE))
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
        .set_read_timeout(Some(READINESS_DEADLINE))
        .expect("fixture read timeout");
    stream
        .set_write_timeout(Some(READINESS_DEADLINE))
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
        occupant: None,
        lineage: Vec::new(),
        capabilities: vec![capability],
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

#[test]
fn resident_lock_precedes_journal_open_and_names_the_holder() {
    let fixture = Fixture::new("lock-first");
    fs::create_dir_all(fixture.journal().parent().expect("journal parent"))
        .expect("data directory");
    fs::write(fixture.journal(), b"not sqlite").expect("invalid journal fixture");
    let _holder = hold(&fixture.resident(), "pid=4242;epoch=fixture-epoch\n");

    let output = invoke(&fixture, "up");

    assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) = outcome(&output) else {
        panic!("a contended lock must refuse");
    };
    assert_eq!(refusal.reason, "resident::already-up");
    assert_eq!(
        refusal.observed.as_deref(),
        Some("pid=4242;epoch=fixture-epoch")
    );
}

#[test]
fn failed_migration_removes_discovery_and_releases_the_lock() {
    let fixture = Fixture::new("migration-cleanup");
    fs::create_dir_all(fixture.journal().parent().expect("journal parent"))
        .expect("data directory");
    fs::write(fixture.journal(), b"not sqlite").expect("invalid journal fixture");
    fs::write(fixture.canvas(), b"stale discovery").expect("stale discovery fixture");

    let output = invoke(&fixture, "up");

    assert_eq!(output.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&output) else {
        panic!("an invalid journal must fault");
    };
    assert_eq!(fault.reason, "journal::open-failed");
    assert!(!fixture.canvas().exists(), "stale discovery was removed");
    let possession = hold(&fixture.resident(), "replacement holder\n");
    drop(possession);
}

#[test]
fn command_client_process_renders_one_success_across_reconnects() {
    let fixture = Fixture::new("client-success");

    let (text, text_elapsed) = fake_down(&fixture, FakeBehavior::Success, false);
    assert!(text.status.success(), "text down failed: {:?}", text.stderr);
    assert_eq!(text.stdout, b"null\n");
    assert!(text.stderr.is_empty());
    assert!(text_elapsed < READINESS_DEADLINE);

    let (json, json_elapsed) = fake_down(&fixture, FakeBehavior::Success, true);
    assert!(json.status.success(), "JSON down failed: {:?}", json.stderr);
    let Outcome::Reply(reply) = outcome(&json) else {
        panic!("JSON down must carry the same successful outcome");
    };
    assert!(matches!(reply.position, ReplyPosition::Committed(_)));
    assert!(json_elapsed < READINESS_DEADLINE);
}

#[test]
fn command_client_process_fences_replay_loss_and_capability_echo() {
    let fixture = Fixture::new("client-fencing");
    let capability = uuid("00000000-0000-4000-8000-000000000031").to_string();

    let (replay, _) = fake_down(&fixture, FakeBehavior::Replay, true);
    assert_eq!(replay.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&replay) else {
        panic!("replayed response must fault");
    };
    assert_eq!(fault.reason, "command::answer-mismatch");

    let (lost, elapsed) = fake_down(&fixture, FakeBehavior::ResidentLoss, true);
    assert_eq!(lost.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&lost) else {
        panic!("resident loss must fault");
    };
    assert!(matches!(
        fault.reason.as_str(),
        "canvas::send-failed" | "canvas::receive-failed" | "canvas::closed-without-answer"
    ));
    assert!(elapsed < READINESS_DEADLINE);

    let (echo, _) = fake_down(&fixture, FakeBehavior::EchoCapability, true);
    assert_eq!(echo.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&echo) else {
        panic!("capability echo must fault");
    };
    assert_eq!(fault.reason, "canvas::capability-disclosed");
    assert!(
        !echo
            .stdout
            .windows(capability.len())
            .any(|w| w == capability.as_bytes())
    );
    assert!(
        !echo
            .stderr
            .windows(capability.len())
            .any(|w| w == capability.as_bytes())
    );
}

#[test]
fn command_client_process_bounds_an_unanswered_request() {
    let fixture = Fixture::new("client-timeout");
    let (output, elapsed) = fake_down(&fixture, FakeBehavior::ReadTimeout, true);

    assert_eq!(output.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&output) else {
        panic!("unanswered request must fault");
    };
    assert_eq!(fault.reason, "canvas::receive-failed", "{fault:?}");
    assert!(
        elapsed < CLIENT_DEADLINE_BOUND,
        "read deadline was not bounded: {elapsed:?}"
    );
}

#[test]
fn command_client_process_shares_one_deadline_across_dripped_socket_phases() {
    let fixture = Fixture::new("client-total-deadline");
    let (output, elapsed) = fake_down(&fixture, FakeBehavior::TotalDeadlineDrip, true);

    assert_eq!(output.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&output) else {
        panic!("a response dripped beyond the total deadline must fault");
    };
    assert_eq!(fault.reason, "canvas::receive-failed", "{fault:?}");
    assert!(
        fixture.runtime.join("deadline-response").is_file(),
        "the peer must reach command response after dripping opening and Hello"
    );
    assert!(
        elapsed < CLIENT_DEADLINE_BOUND,
        "the shared socket deadline was renewed: {elapsed:?}"
    );
}

#[test]
fn command_client_process_bounds_unlimited_control_frames_and_control_writes() {
    let fixture = Fixture::new("client-control-deadline");
    let (output, elapsed) = fake_down(&fixture, FakeBehavior::ControlFrames, true);

    assert_eq!(output.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&output) else {
        panic!("unlimited control frames must fault at the total deadline");
    };
    assert_eq!(fault.reason, "canvas::receive-failed");
    assert!(
        elapsed < CLIENT_DEADLINE_BOUND,
        "control frames renewed the receive deadline: {elapsed:?}"
    );

    let (output, elapsed) = fake_down(&fixture, FakeBehavior::WriteTimeout, true);
    assert_eq!(output.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&output) else {
        panic!("a saturated automatic control write must fault");
    };
    assert_eq!(fault.reason, "canvas::send-failed");
    assert!(
        elapsed < CLIENT_DEADLINE_BOUND,
        "control write deadline was not bounded: {elapsed:?}"
    );
}

#[test]
fn command_client_process_refuses_untrusted_discovery_before_connecting() {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;

    let fixture = Fixture::new("client-discovery");
    let missing = invoke(&fixture, "down");
    assert_eq!(missing.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&missing) else {
        panic!("missing discovery must fault");
    };
    assert_eq!(fault.reason, "canvas::discovery-unavailable");

    fs::create_dir_all(&fixture.runtime).expect("fixture runtime directory");
    fs::write(fixture.canvas(), b"not current discovery").expect("malformed discovery writes");
    #[cfg(unix)]
    fs::set_permissions(fixture.canvas(), fs::Permissions::from_mode(0o600))
        .expect("malformed discovery is owner-only");
    let malformed = invoke(&fixture, "down");
    assert_eq!(malformed.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&malformed) else {
        panic!("malformed discovery must fault");
    };
    assert_eq!(fault.reason, "canvas::discovery-malformed");

    let capability = uuid("00000000-0000-4000-8000-000000000041");
    let mut discovery = canvas::Discovery {
        endpoint: "http://192.0.2.1:48123".to_owned(),
        capability,
        epoch: uuid("00000000-0000-4000-8000-000000000042"),
    };
    discovery
        .publish(fixture.canvas())
        .expect("non-loopback discovery publishes");
    let non_loopback = invoke(&fixture, "down");
    assert_eq!(non_loopback.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&non_loopback) else {
        panic!("non-loopback discovery must fault");
    };
    assert_eq!(fault.reason, "canvas::endpoint-not-loopback");

    #[cfg(unix)]
    {
        fs::set_permissions(fixture.canvas(), fs::Permissions::from_mode(0o644))
            .expect("fixture discovery becomes public");
        let insecure = invoke(&fixture, "down");
        assert_eq!(insecure.status.code(), Some(FAULT_EXIT));
        let Outcome::Fault(fault) = outcome(&insecure) else {
            panic!("public discovery must fault");
        };
        assert_eq!(fault.reason, "canvas::discovery-insecure");
    }

    let listener = TcpListener::bind("127.0.0.1:0").expect("unused endpoint binds");
    discovery.endpoint = format!(
        "http://{}",
        listener.local_addr().expect("unused endpoint address")
    );
    drop(listener);
    discovery
        .publish(fixture.canvas())
        .expect("unavailable discovery publishes");
    let started = Monotonic::now();
    let unavailable = invoke(&fixture, "down");
    assert!(started.elapsed() < Duration::from_secs(4));
    assert_eq!(unavailable.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&unavailable) else {
        panic!("unavailable endpoint must fault");
    };
    assert_eq!(fault.reason, "canvas::connect-failed");
    let capability = capability.to_string();
    assert!(
        !unavailable
            .stderr
            .windows(capability.len())
            .any(|w| w == capability.as_bytes())
    );
}

#[test]
fn unfinalized_rust_schema_refuses_startup_with_its_exact_state() {
    let fixture = Fixture::new("schema-refusal");
    fs::create_dir_all(fixture.journal().parent().expect("journal parent"))
        .expect("data directory");
    let connection = Connection::open(fixture.journal()).expect("open schema fixture");
    connection
        .execute_batch(concat!(
            "CREATE TABLE journal_schema(singleton INTEGER PRIMARY KEY, ",
            "version INTEGER NOT NULL, state TEXT NOT NULL, ",
            "from_version INTEGER, to_version INTEGER, plan_digest TEXT);",
        ))
        .expect("create schema identity");
    connection
        .execute(
            concat!(
                "INSERT INTO journal_schema(singleton, version, state, ",
                "from_version, to_version) VALUES (1, ?1, ?2, ?3, ?4)"
            ),
            params![1_u32, "prepared", 0_u32, 1_u32],
        )
        .expect("insert prepared schema identity");
    connection
        .pragma_update(None, "user_version", 1_u32)
        .expect("set schema pragma");
    drop(connection);

    let output = invoke(&fixture, "up");

    assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) = outcome(&output) else {
        panic!("an unfinalized Rust journal must refuse");
    };
    assert_eq!(refusal.reason, "journal::schema-refused");
    assert!(
        refusal
            .observed
            .as_deref()
            .is_some_and(|observed| observed.contains("prepared and unfinalized")),
        "refusal omitted the exact schema state: {refusal:?}",
    );
}

#[test]
fn operator_trust_process_binds_policy_bytes_across_reconnect_and_restart() {
    let fixture = Fixture::new("trust-lifecycle");
    fs::write(
        fixture.root.join("spice.toml"),
        "# tracked executable policy v1\n",
    )
    .expect("tracked policy writes");
    let local = fixture.root.join(".git/.spice/config.toml");
    fs::create_dir_all(local.parent().expect("local policy parent"))
        .expect("local policy directory");
    fs::write(&local, "# local executable policy v1\n").expect("local policy writes");

    let mut first = RunningResident::spawn(&fixture);
    first.assert_ready(&fixture);
    let initial_pends = trust_pended(&fixture);
    assert_eq!(initial_pends.len(), 1, "startup pends exactly once");
    assert_eq!(initial_pends[0].surface, "configuration");
    assert_eq!(initial_pends[0].previous, None);

    let text_report = invoke_args(&fixture, &["trust"], false);
    assert!(
        text_report.status.success(),
        "text trust failed: {:?}",
        text_report.stderr
    );
    assert!(text_report.stderr.is_empty());
    let text_body: serde_json::Value =
        serde_json::from_slice(&text_report.stdout).expect("text trust body is canonical JSON");
    assert_eq!(text_body["standing"], "pended");
    assert_eq!(text_body["provenance"].as_array().map(Vec::len), Some(2));
    assert_eq!(text_body["provenance"][0]["source"], "tracked");
    assert_eq!(text_body["provenance"][1]["source"], "local");
    assert_eq!(text_body["provenance"][0]["present"], true);
    assert_eq!(text_body["provenance"][1]["present"], true);
    let first_digest = text_body["digest"]
        .as_str()
        .expect("trust report names its current digest")
        .to_owned();
    assert_eq!(first_digest, initial_pends[0].digest.as_str());

    let granted = invoke_args(&fixture, &["trust", "grant", &first_digest], true);
    assert!(
        granted.status.success(),
        "JSON grant failed: {:?}",
        granted.stderr
    );
    let Outcome::Reply(granted_reply) = value_outcome(&granted) else {
        panic!("grant must return one typed reply");
    };
    assert!(matches!(
        granted_reply.position,
        ReplyPosition::Committed(_)
    ));
    assert_eq!(granted_reply.body["digest"], first_digest);
    assert_eq!(granted_reply.body["standing"], "granted");

    let before_duplicate = journal_cursor(&fixture);
    let duplicate = invoke_args(&fixture, &["trust", "grant", &first_digest], false);
    assert_eq!(duplicate.status.code(), Some(REFUSAL_EXIT));
    assert_eq!(
        duplicate.stderr,
        concat!(
            "spice: trust::standing-moved; re-read the current trust report ",
            "before issuing another ruling\n"
        )
        .as_bytes()
    );
    assert_eq!(journal_cursor(&fixture), before_duplicate);

    let json_report = invoke_args(&fixture, &["trust"], true);
    assert!(json_report.status.success());
    let Outcome::Reply(report_reply) = value_outcome(&json_report) else {
        panic!("JSON trust must return one typed reply");
    };
    assert!(matches!(report_reply.position, ReplyPosition::Snapshot(_)));
    assert_eq!(report_reply.body["standing"], "granted");

    assert!(invoke(&fixture, "down").status.success());
    assert!(first.wait().status.success());
    let pends_before_restart = trust_pended(&fixture);
    let mut restarted = RunningResident::spawn(&fixture);
    restarted.assert_ready(&fixture);
    let after_restart = invoke_args(&fixture, &["trust"], true);
    let Outcome::Reply(after_restart) = value_outcome(&after_restart) else {
        panic!("restarted trust must return one typed reply");
    };
    assert_eq!(after_restart.body["digest"], first_digest);
    assert_eq!(after_restart.body["standing"], "granted");
    assert_eq!(
        trust_pended(&fixture),
        pends_before_restart,
        "restart cannot duplicate an existing digest pend"
    );

    assert!(invoke(&fixture, "down").status.success());
    assert!(restarted.wait().status.success());
    fs::write(
        fixture.root.join("spice.toml"),
        "# tracked executable policy v2\n",
    )
    .expect("tracked policy changes");
    let mut changed = RunningResident::spawn(&fixture);
    changed.assert_ready(&fixture);
    let pends = trust_pended(&fixture);
    assert_eq!(pends.len(), 2, "changed bytes append one new pend");
    assert_eq!(
        pends[1].previous.as_ref().map(wire::Digest::as_str),
        Some(first_digest.as_str())
    );
    let changed_report = invoke_args(&fixture, &["trust"], true);
    let Outcome::Reply(changed_report) = value_outcome(&changed_report) else {
        panic!("changed trust must return one typed reply");
    };
    assert_eq!(changed_report.body["standing"], "pended");
    let changed_digest = changed_report.body["digest"]
        .as_str()
        .expect("changed report names current digest")
        .to_owned();
    assert_ne!(changed_digest, first_digest);

    let unknown =
        wire::Digest::compute(wire::Domain::ConfigExecutableSurface, b"unknown").to_string();
    let cross = wire::Digest::compute(wire::Domain::DistArtifact, b"cross-domain").to_string();
    for (label, digest) in [
        ("stale", first_digest.as_str()),
        ("unknown", unknown.as_str()),
        ("cross-domain", cross.as_str()),
    ] {
        let before = journal_cursor(&fixture);
        let refused = invoke_args(&fixture, &["trust", "grant", digest], true);
        assert_eq!(
            refused.status.code(),
            Some(REFUSAL_EXIT),
            "{label} digest did not refuse"
        );
        let Outcome::Refusal(refusal) = value_outcome(&refused) else {
            panic!("{label} digest must return a typed refusal");
        };
        assert_eq!(refusal.reason, "trust::digest-not-current");
        assert_eq!(
            journal_cursor(&fixture),
            before,
            "{label} changed authority"
        );
    }

    let grant_changed = invoke_args(&fixture, &["trust", "grant", &changed_digest], false);
    assert!(
        grant_changed.status.success(),
        "text grant failed: {:?}",
        grant_changed.stderr
    );
    let grant_body: serde_json::Value =
        serde_json::from_slice(&grant_changed.stdout).expect("text grant body is JSON");
    assert_eq!(grant_body["standing"], "granted");

    let revoked = invoke_args(&fixture, &["trust", "revoke", &changed_digest], true);
    assert!(
        revoked.status.success(),
        "JSON revoke failed: {:?}",
        revoked.stderr
    );
    let Outcome::Reply(revoked_reply) = value_outcome(&revoked) else {
        panic!("revoke must return one typed reply");
    };
    assert_eq!(revoked_reply.body["standing"], "revoked");

    let before_duplicate_revoke = journal_cursor(&fixture);
    let duplicate_revoke = invoke_args(&fixture, &["trust", "revoke", &changed_digest], true);
    assert_eq!(duplicate_revoke.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) = value_outcome(&duplicate_revoke) else {
        panic!("duplicate revoke must return one typed refusal");
    };
    assert_eq!(refusal.reason, "trust::standing-moved");
    assert_eq!(journal_cursor(&fixture), before_duplicate_revoke);

    assert!(invoke(&fixture, "down").status.success());
    assert!(changed.wait().status.success());
    journal::JournalReader::open(fixture.journal())
        .expect("final authority opens")
        .verify()
        .expect("final trust authority verifies");
}

#[test]
fn authenticated_down_rejects_a_stale_epoch_then_reaches_verified_dark() {
    let fixture = Fixture::new("lifecycle");
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let discovery_bytes = fs::read(fixture.canvas()).expect("current discovery");
    let discovery: canvas::Discovery = wire::decode(&discovery_bytes).expect("discovery decodes");

    let contender = invoke(&fixture, "up");
    assert_eq!(contender.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) = outcome(&contender) else {
        panic!("second resident must refuse");
    };
    let observed = refusal.observed.expect("holder identity");
    assert!(
        observed.contains("pid="),
        "holder names its process: {observed}"
    );
    assert!(
        observed.contains(&format!("epoch={}", discovery.epoch)),
        "holder names its generation: {observed}"
    );

    let mut stale = discovery.clone();
    stale.epoch = "00000000-0000-4000-8000-000000000001"
        .parse()
        .expect("stale epoch");
    stale
        .publish(fixture.canvas())
        .expect("stale discovery publishes");
    let rejected = invoke(&fixture, "down");
    assert_eq!(rejected.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&rejected) else {
        panic!("stale epoch must fault");
    };
    assert_eq!(fault.reason, "hello::epoch-mismatch");

    discovery
        .publish(fixture.canvas())
        .expect("current discovery restores");
    let down = invoke(&fixture, "down");
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    let Outcome::Reply(reply) = outcome(&down) else {
        panic!("current operator down must reply");
    };
    let ReplyPosition::Committed(range) = reply.position else {
        panic!("down mutates the lifecycle");
    };
    assert_eq!((range.first.get(), range.last.get()), (4, 9));

    let up = resident.wait();
    assert!(up.status.success(), "up failed: {:?}", up.stderr);
    let Outcome::Reply(reply) = outcome(&up) else {
        panic!("foreground up must reply after shutdown");
    };
    let ReplyPosition::Committed(range) = reply.position else {
        panic!("up commits startup");
    };
    assert_eq!((range.first.get(), range.last.get()), (1, 3));
    assert!(!fixture.canvas().exists(), "dark removes discovery");
    assert_eq!(fs::read(fixture.resident()).expect("holder file"), b"");

    let journal = journal::Journal::open(fixture.journal()).expect("journal reopens");
    journal.verify().expect("final authority verifies");
    let kinds: Vec<_> = journal
        .fact_snapshot()
        .expect("fact snapshot")
        .facts()
        .iter()
        .map(|envelope| envelope.fact.kind())
        .collect();
    assert_eq!(
        kinds,
        [
            FactKind::JournalMigrated,
            FactKind::TrustPended,
            FactKind::UpSeated,
            FactKind::UpQuiescing,
            FactKind::UpDraining,
            FactKind::UpTerminating,
            FactKind::UpKilling,
            FactKind::UpVerifying,
            FactKind::UpDark,
        ]
    );
}

#[test]
fn shutdown_preserves_an_unfinished_claim_before_releasing_authority() {
    let fixture = Fixture::new("claim-preservation");

    let mut initial = RunningResident::spawn(&fixture);
    initial.assert_ready(&fixture);
    let down = invoke(&fixture, "down");
    assert!(
        down.status.success(),
        "initial down failed: {:?}",
        down.stderr
    );
    assert!(initial.wait().status.success(), "initial resident failed");
    seed_active_claim(&fixture);

    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let down = invoke(&fixture, "down");
    assert!(
        down.status.success(),
        "claim down failed: {:?}",
        down.stderr
    );
    assert!(resident.wait().status.success(), "claim resident failed");

    let journal = journal::Journal::open(fixture.journal()).expect("journal reopens");
    journal.verify().expect("claim-bearing authority verifies");
    let kinds: Vec<_> = journal
        .fact_snapshot()
        .expect("fact snapshot")
        .facts()
        .iter()
        .map(|envelope| envelope.fact.kind())
        .collect();
    let claim = kinds
        .iter()
        .position(|kind| *kind == FactKind::ClaimWon)
        .expect("active claim remains durable");
    assert_eq!(
        &kinds[claim..],
        [
            FactKind::ClaimWon,
            FactKind::UpSeated,
            FactKind::UpQuiescing,
            FactKind::UpDraining,
            FactKind::UpTerminating,
            FactKind::UpKilling,
            FactKind::UpVerifying,
            FactKind::UpDark,
        ],
        "shutdown appended no claim disposition transition"
    );
}

#[test]
fn replaced_resident_generations_install_durable_lane_capabilities() {
    let fixture = Fixture::new("lane-capabilities");
    let mut initial = RunningResident::spawn(&fixture);
    initial.assert_ready(&fixture);
    let initial_discovery = fixture.discovery();
    let down = invoke(&fixture, "down");
    assert!(
        down.status.success(),
        "initial down failed: {:?}",
        down.stderr
    );
    assert!(initial.wait().status.success(), "initial resident failed");

    let lane = uuid("00000000-0000-4000-8000-000000000021");
    let lane_capability = uuid("00000000-0000-4000-8000-000000000022");
    seed_lane(&fixture, lane, lane_capability);

    let mut replacement = RunningResident::spawn(&fixture);
    replacement.assert_ready(&fixture);
    let replacement_discovery = fixture.discovery();
    assert_ne!(replacement_discovery.epoch, initial_discovery.epoch);
    assert_ne!(
        replacement_discovery.capability,
        initial_discovery.capability
    );
    let (_, frame) = hello(&replacement_discovery, initial_discovery.capability);
    let refusal: wire::Refusal = wire::decode(&frame).expect("replaced capability refuses");
    assert_eq!(refusal.reason, "hello::capability-unknown");
    let (_operator, accepted) =
        authenticate(&replacement_discovery, replacement_discovery.capability);
    assert_eq!(accepted.actor, Actor::Operator);
    assert_lane_session(
        &replacement_discovery,
        lane,
        lane_capability,
        uuid("00000000-0000-4000-8000-000000000023"),
    );
    let down = invoke(&fixture, "down");
    assert!(
        down.status.success(),
        "replacement down failed: {:?}",
        down.stderr
    );
    assert!(
        replacement.wait().status.success(),
        "replacement resident failed"
    );

    let mut reconnected = RunningResident::spawn(&fixture);
    reconnected.assert_ready(&fixture);
    let reconnected_discovery = fixture.discovery();
    let (_, frame) = hello(&reconnected_discovery, replacement_discovery.capability);
    let refusal: wire::Refusal = wire::decode(&frame).expect("prior capability refuses");
    assert_eq!(refusal.reason, "hello::capability-unknown");
    assert_lane_session(
        &reconnected_discovery,
        lane,
        lane_capability,
        uuid("00000000-0000-4000-8000-000000000024"),
    );
    let down = invoke(&fixture, "down");
    assert!(
        down.status.success(),
        "reconnected down failed: {:?}",
        down.stderr
    );
    assert!(
        reconnected.wait().status.success(),
        "reconnected resident failed"
    );

    let journal = journal::Journal::open(fixture.journal()).expect("final journal opens");
    let head = journal
        .read(|read| read.residency_head())
        .expect("final lane head reads")
        .expect("final lane head exists")
        .into_body();
    assert_eq!(head.lanes.len(), 1);
    assert_eq!(head.lanes[0].lane, lane);
    assert_eq!(head.lanes[0].capabilities, [lane_capability]);
}

#[test]
fn down_releases_before_reply_and_linked_worktrees_use_common_state() {
    let fixture = Fixture::linked("restart");
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    assert!(
        fixture.canvas().starts_with(&fixture.runtime),
        "linked discovery escaped Git-common state"
    );
    for _ in 0..3 {
        let down = invoke(&fixture, "down");
        assert!(down.status.success(), "down failed: {:?}", down.stderr);
        let previous = resident;
        resident = RunningResident::spawn(&fixture);
        resident.assert_ready(&fixture);
        assert!(
            previous.wait().status.success(),
            "replaced foreground resident failed"
        );
    }
    let down = invoke(&fixture, "down");
    assert!(
        down.status.success(),
        "final down failed: {:?}",
        down.stderr
    );
    assert!(
        resident.wait().status.success(),
        "foreground resident failed"
    );
}
