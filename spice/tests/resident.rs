use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
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

#[path = "support/staged_app.rs"]
mod staged_app;

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
        let runtime = root.join(".git/.spice-rs");
        fs::create_dir_all(root.join(".git")).expect("fixture git directory");
        let fixture = Self {
            cleanup: root.clone(),
            root,
            runtime,
        };
        fixture.install_app();
        fixture
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
        let fixture = Self {
            root,
            runtime: common.join(".spice-rs"),
            cleanup,
        };
        fixture.install_app();
        fixture
    }

    fn install_app(&self) {
        staged_app::seed(&self.root);
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

/// The ignore rule every fixture repository owes its generated app store.
///
/// A resident refuses to start without that stage, so every fixture root holds
/// one whether or not the case under test is about the app. It is built output
/// rather than repository content, exactly as in a real worktree, and a fixture
/// that does not ignore it reports itself dirty and fails cases that assert a
/// clean tree for unrelated reasons. Derived from the constant the stage is
/// written from so the two cannot come to name different directories.
fn app_stage_ignore() -> String {
    format!("/{}/\n", canvas::APP_STAGE)
}

/// Assert a freshly seeded fixture repository sees nothing but what it committed.
///
/// The stage [`Fixture`] installs lands inside the worktree, so a fixture whose
/// ignore policy does not cover it is dirty from birth. Cases that refuse on a
/// dirty worktree then fail far from the cause, naming a git refusal in a case
/// about lanes or vendors; asserting it where the repository is built says which
/// ignore policy is short instead.
fn assert_fixture_repository_clean(root: &Path) {
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .expect("fixture status runs");
    assert!(
        status.status.success(),
        "fixture status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&status.stdout),
        "",
        "the fixture repository is dirty before its case begins: its ignore \
         policy does not cover everything the fixture writes"
    );
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
        Self::spawn_with_form(fixture, executable, path, true)
    }

    fn spawn_text(fixture: &Fixture) -> Self {
        Self::spawn_with_form(fixture, Path::new(env!("CARGO_BIN_EXE_spice")), None, false)
    }

    fn spawn_text_at(fixture: &Fixture, listen: SocketAddr) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        command
            .args([
                "up",
                "--host",
                &listen.ip().to_string(),
                "--port",
                &listen.port().to_string(),
            ])
            .current_dir(&fixture.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = command
            .spawn()
            .expect("foreground resident launches at an explicit address");
        Self { child: Some(child) }
    }

    fn spawn_text_authenticated(fixture: &Fixture, token: &str) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_spice"))
            .args([
                "up",
                "--host",
                "0.0.0.0",
                "--port",
                "0",
                "--allow-insecure-bind",
                "--auth-token",
                token,
            ])
            .current_dir(&fixture.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("authenticated foreground resident launches");
        Self { child: Some(child) }
    }

    fn spawn_with_form(
        fixture: &Fixture,
        executable: &Path,
        path: Option<&Path>,
        json: bool,
    ) -> Self {
        let mut command = Command::new(executable);
        // The default port belongs to the operator's one resident, and these
        // fixtures are neither one of those nor alone on the machine: every
        // battery here spawns its own, alongside whatever the lanes sharing
        // this host are running. Each asks the kernel for a free port instead,
        // and reads the one it got back out of the published discovery.
        if json {
            command.arg("--json");
        }
        command.args(["up", "--port", "0"]);
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

    fn read_stdout_line(&mut self) -> String {
        let child = self.child.as_mut().expect("resident child");
        let stdout = child.stdout.take().expect("resident stdout is piped");
        let (sender, receiver) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            let mut line = String::new();
            let read = stdout.read_line(&mut line);
            let _ = sender.send((read, line, stdout.into_inner()));
        });
        let received = receiver.recv_timeout(CLIENT_DEADLINE_BOUND);
        if received.is_err() {
            let _ = child.kill();
        }
        let (read, line, stdout) = received.expect("resident reports its endpoint while alive");
        reader.join().expect("resident stdout reader joins");
        child.stdout = Some(stdout);
        assert!(read.expect("resident stdout is readable") > 0);
        line
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
    /// `Success` in every byte on the wire, but the discovery record stays
    /// published after the reply. Nothing the client reads distinguishes it
    /// from `Success`; the difference is only visible on disk, which is what
    /// makes it the fixture for a resident that answered and then failed to
    /// withdraw.
    RetainedDiscovery,
    /// Answer and withdraw discovery, but keep the runtime lock past the
    /// client's release-confirmation bound.
    ReleaseStall,
    Replay,
    EchoCapability,
    ResidentLoss,
    ReadTimeout,
    TotalDeadlineDrip,
    ControlFrames,
    WriteTimeout,
    OpeningStall,
}

/// Hold the runtime lock the way the resident being faked would.
///
/// A fake resident that only binds a port is not yet a publisher a client may
/// dial: possession of this lock is what separates a record whose author is
/// still running from one it left behind, and the generation written into it
/// is what separates the author from whoever merely holds the lock now. A
/// fixture that serves the socket has to do both. The caller keeps the
/// possession for as long as the fake is meant to be alive.
fn fake_publisher(fixture: &Fixture, epoch: wire::CanonicalUuid) -> sill::Possession {
    let lock = sill::LockFile::new(
        sill::LockName::new("resident").expect("resident is a non-empty lock name"),
        fixture.resident(),
    );
    let sill::Acquisition::Acquired(publisher) = lock.try_acquire().expect("fixture resident lock")
    else {
        panic!("a fixture runtime with no resident cannot be contended");
    };
    publisher
        .rewrite_record(
            spice::resident_lock::holder_record(std::process::id(), Some(epoch)).as_bytes(),
        )
        .expect("fixture holder record writes");
    publisher
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
    let publisher = fake_publisher(fixture, discovery.epoch);
    discovery
        .publish(fixture.canvas())
        .expect("fake discovery publishes");
    let canvas = fixture.canvas();
    let server_discovery = discovery.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let server = std::thread::spawn(move || {
        run_fake_resident(
            listener,
            server_discovery,
            canvas,
            behavior,
            server_stop,
            publisher,
        );
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
    // The runtime lock, possessed for exactly as long as this fake serves and
    // released on every path out of it. It is acquired before the thread
    // starts, so a client can never observe the window between binding a port
    // and becoming a publisher. Releasing it here rather than in the caller is
    // what lets `down` finish: a successful shutdown blocks on acquiring this
    // lock to confirm the resident really went away, so a fixture that held it
    // past the reply would hang the client instead of the resident.
    _publisher: sill::Possession,
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
    if matches!(behavior, FakeBehavior::Success | FakeBehavior::ReleaseStall) {
        fs::remove_file(canvas).expect("fake resident withdraws discovery");
    }
    send_dark_response(&mut socket, &discovery, &command, behavior);
    if matches!(behavior, FakeBehavior::ReleaseStall) {
        hold_adversarial_peer(&stop);
    }
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
        queue_origin: seq,
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
                cell: Some(wire::ClaimCell {
                    project: "resident".to_owned(),
                    phase: phase.clone(),
                }),
                until: expiry,
                git: None,
                source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Existing(
                wire::Seq::new(added.last()).expect("task commit has a non-zero position"),
            ),
        )
        .expect("claim fact");
    claim.claim_head(task.clone(), won, move |seq| ClaimHead {
        actor,
        fact: seq,
        claimed: seq,
        cell: wire::ClaimCell {
            project: "resident".to_owned(),
            phase,
        },
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
        lineage_id: lane,
        name: None,
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
                lineage_id: lane,
                capability,
                name: None,
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

fn wait_for_lane_exit(fixture: &Fixture, lane: wire::CanonicalUuid) {
    wait_for_event(
        &fixture.runtime.join("data"),
        || {},
        || {
            let snapshot = journal::JournalReader::open(fixture.journal())
                .ok()?
                .fact_snapshot()
                .ok()?;
            snapshot
                .facts()
                .iter()
                .any(|fact| {
                    fact.fact.kind() == FactKind::LaneExited
                        && fact.fact.body().get("lane")
                            == Some(&serde_json::Value::String(lane.to_string()))
                })
                .then_some(())
        },
    );
}

fn effect_observers(path: &Path) -> Vec<readiness::filesystem_event::DirectObserver> {
    effect_observers_before_arm(path, |_| {})
}

fn effect_observers_before_arm(
    path: &Path,
    mut before_arm: impl FnMut(&Path),
) -> Vec<readiness::filesystem_event::DirectObserver> {
    let root = if path.is_dir() {
        path
    } else {
        path.parent().expect("effect path has a parent")
    };
    let mut watched = vec![(root.to_path_buf(), false)];
    if path.is_file() {
        watched.push((path.to_path_buf(), false));
    } else {
        watched.extend(
            fs::read_dir(root)
                .expect("effect directory reads")
                .map(|entry| entry.expect("effect directory entry reads").path())
                .filter(|entry| entry.is_file())
                .map(|entry| (entry, true)),
        );
    }
    watched.sort_by(|(left, _), (right, _)| left.cmp(right));
    watched.dedup_by(|(left, _), (right, _)| left == right);
    watched
        .into_iter()
        .filter_map(|(watched, incidental)| {
            before_arm(&watched);
            match readiness::filesystem_event::DirectObserver::try_arm(&watched) {
                Ok(observer) => Some(observer),
                Err(error) if incidental && error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => panic!("effect observer arms on {}: {error}", watched.display()),
            }
        })
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
/// A missing stop has three causes that the grace cannot separate on its own,
/// so the fixture witnesses two things the panic reads back: the trap records
/// its own entry first, and the vendor reports the traps it actually installed.
/// The handler lost the race, the machine never ran it, or TERM could never
/// reach it at all -- and the panic names exactly one of them. A fixture built
/// without [`vendor_prologue`] witnesses neither, and is told so rather than
/// being read as the vendor's fault. See [`entered_marker`] and
/// [`trap_report`].
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
        Err(pid) if entered_marker(stopped).exists() => panic!(
            "vendor {pid} entered its TERM trap and ended before recording a \
             stop in {}; the handler ran but did not finish inside the \
             termination grace",
            stopped.display()
        ),
        Err(pid) => match trap_report(stopped) {
            TrapReport::Installed => panic!(
                "vendor {pid} ended without entering its TERM trap, which it \
                 had installed and reported back; the machine never scheduled \
                 the handler inside the termination grace"
            ),
            TrapReport::Declined => panic!(
                "vendor {pid} ended without entering its TERM trap and never \
                 had one: TERM was already ignored when the fixture started, \
                 so its trap builtin could not take effect and the signal \
                 could not move it"
            ),
            TrapReport::Unreported => panic!(
                "vendor {pid} never reported its traps, so this fixture was \
                 not built from vendor_prologue and nothing here can say what \
                 the vendor did with TERM; instrument the fixture rather than \
                 reading this as a vendor fault"
            ),
        },
    }
}

/// The marker a fixture vendor writes as the first action of its TERM trap.
///
/// Derived from the stop path rather than passed alongside it so a fixture
/// cannot record one without the other. The two files split the single failure
/// this helper used to report: a handler that ran and lost the race to the
/// grace writes the marker and not the stop, while a handler that never ran
/// writes neither.
fn entered_marker(stopped: &Path) -> PathBuf {
    let mut marker = stopped.as_os_str().to_owned();
    marker.push("-entered");
    PathBuf::from(marker)
}

/// What a fixture vendor reported about its own TERM trap.
enum TrapReport {
    /// The vendor reported a TERM trap running its own entry marker.
    Installed,
    /// The vendor reported its traps and TERM was not among them, which a
    /// shell says exactly when TERM was already ignored as it started.
    Declined,
    /// The vendor reported nothing, so it was not built from
    /// [`vendor_prologue`] and its behaviour cannot be read at all.
    Unreported,
}

/// Read back whether the fixture vendor's TERM trap ever took effect.
///
/// A non-interactive shell cannot trap a signal that was already ignored when
/// it started: the trap builtin accepts the request and silently declines it.
/// That is the difference between a vendor TERM could not move and one the
/// machine never scheduled, and it is not observable from timing, so each
/// fixture reports its own installed traps and this reads the answer back.
///
/// Written before the pid file, so a recorded pid always implies a readable
/// readout. A readout that is missing anyway cannot mean the vendor declined
/// the trap; it means the fixture never had the prologue, which is why that is
/// a third answer here rather than folded into [`TrapReport::Declined`]. The
/// line must name TERM as well as the handler, because the same trap command
/// also covers INT, and a shell that declined only TERM would still print the
/// handler on the surviving INT line.
fn trap_report(stopped: &Path) -> TrapReport {
    let handler = entered_marker(stopped).display().to_string();
    let Ok(traps) = fs::read_to_string(trap_record(stopped)) else {
        return TrapReport::Unreported;
    };
    if traps
        .lines()
        .any(|trap| trap.contains("TERM") && trap.contains(&handler))
    {
        TrapReport::Installed
    } else {
        TrapReport::Declined
    }
}

/// The readout of its own traps that a fixture vendor writes at startup.
///
/// Derived from the stop path for the same reason [`entered_marker`] is: a
/// fixture cannot report one path while trapping another.
fn trap_record(stopped: &Path) -> PathBuf {
    let mut record = stopped.as_os_str().to_owned();
    record.push("-traps");
    PathBuf::from(record)
}

/// The opening lines every fixture vendor shares.
///
/// A TERM trap that records its own entry before recording the stop, the
/// vendor's readout of the traps it actually installed, and the pid the
/// resident is asked to terminate. That order is load-bearing:
/// [`wait_for_vendor_stop`] only diagnoses after reading a pid, so a recorded
/// pid implies a readout already on disk and the diagnosis never has to guess.
///
/// Built here rather than spelled out per fixture because every fixture that
/// hand-copied these lines had to be found and edited again each time the
/// diagnosis learned something new, and the one that gets missed is not a
/// visible gap but a confident wrong answer.
fn vendor_prologue(stopped: &Path, vendor_pid: &Path) -> String {
    format!(
        concat!(
            "#!/bin/sh\n",
            "trap 'printf entered > {}; printf stopped > {}; exit 0' TERM INT\n",
            "trap > {}\n",
            "printf '%s\\n' \"$$\" > {}\n",
        ),
        shell_word(&entered_marker(stopped).display().to_string()),
        shell_word(&stopped.display().to_string()),
        shell_word(&trap_record(stopped).display().to_string()),
        shell_word(&vendor_pid.display().to_string()),
    )
}

/// A path or word quoted for the fixture shell scripts written above.
fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
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

#[path = "resident/lane_resume.rs"]
mod lane_resume;

#[path = "resident/automatic_lifecycle.rs"]
mod automatic_lifecycle;

#[path = "resident/steering_authority.rs"]
mod steering_authority;

#[path = "resident/vendor_process.rs"]
mod vendor_process;
