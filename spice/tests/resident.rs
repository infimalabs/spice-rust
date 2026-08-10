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

fn unique_fixture_root(family: &str, name: &str) -> PathBuf {
    loop {
        let root = std::env::temp_dir().join(format!(
            "spice-resident-{family}-{name}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&root) {
            Ok(()) => return root,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => panic!("fixture root {} cannot be claimed: {error}", root.display()),
        }
    }
}

struct Fixture {
    root: PathBuf,
    runtime: PathBuf,
    cleanup: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = unique_fixture_root("process", name);
        let runtime = root.join(".git/.spice-rs");
        fs::create_dir(root.join(".git")).expect("fixture git directory");
        let fixture = Self {
            cleanup: root.clone(),
            root,
            runtime,
        };
        fixture.install_app();
        fixture
    }

    fn linked(name: &str) -> Self {
        let cleanup = unique_fixture_root("linked", name);
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
        &wire::Refusal::bare(wire::RefusalDiagnostic::new(
            "hello::fixture".to_owned(),
            None,
            None,
            Some(capability.to_string()),
        )),
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
        Outcome::Refusal(ref refusal)
            if refusal.reason() == "command::authority"
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

include!("resident/support_tail.rs");
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
