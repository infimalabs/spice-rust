//! Process coverage for the resident's transcript-backed steering path.

use std::fs;
use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};

use bus::{Bus, CommitOutcome};
use journal::{FactCause, JournalWrite};
use serde::de::DeserializeOwned;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::header::ORIGIN;
use tungstenite::{Message, WebSocket};
use wire::{Actor, Instant, Outcome, Producer};

#[path = "support/fixture_root.rs"]
mod fixture_root;
#[path = "support/readiness.rs"]
mod readiness;
#[path = "support/staged_app.rs"]
mod staged_app;

use readiness::Readiness;

const SESSION_ID: &str = "018f7777-7777-7444-8888-999999999999";
const SESSION_RECORD: &str = r#"{"id":"spice-state","type":"response","command":"get_state","success":true,"data":{"sessionId":"018f7777-7777-7444-8888-999999999999"}}"#;

struct Fixture {
    root: PathBuf,
    runtime: PathBuf,
    cargo_home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = fixture_root::claim(&std::env::temp_dir(), "steering-process");
        let runtime = root.join(".git/.spice-rs");
        let cargo_home = root.join("fixture-cargo-home");
        gix::init(&root).expect("initialize fixture repository in process");
        fs::create_dir(&cargo_home).expect("fixture bootstrap Cargo home");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"steering-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("fixture Cargo manifest");
        fs::write(
            root.join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"steering-fixture\"\nversion = \"0.0.0\"\n",
        )
        .expect("fixture Cargo lockfile");
        let skill = root.join(".git/.spice-install/agents/skills/spice/SKILL.md");
        fs::create_dir_all(skill.parent().expect("fixture skill parent"))
            .expect("fixture skill parent creates");
        fs::write(&skill, ::std::include_bytes!("../agent/SKILL.md"))
            .expect("fixture installs the appliance skill");
        staged_app::seed(&root);
        let vendor = root.join("fake-omp");
        fs::write(
            &vendor,
            concat!(
                "#!/bin/sh\n",
                "trap 'exit 0' TERM INT\n",
                "session=018f7777-7777-7444-8888-999999999999\n",
                "while [ \"$#\" -gt 1 ]; do\n",
                "  if [ \"$1\" = --resume ]; then session=$2; fi\n",
                "  shift\n",
                "done\n",
                "printf '%s\\n' '{\"type\":\"ready\",\"protocolVersion\":1}'\n",
                "printf '{\"id\":\"spice-state\",\"type\":\"response\",\"command\":\"get_state\",",
                "\"success\":true,\"data\":{\"sessionId\":\"%s\"}}\\n' \"$session\"\n",
                "while [ -d .git ]; do sleep 1; done\n",
            ),
        )
        .expect("fixture vendor writes");
        #[cfg(unix)]
        fs::set_permissions(&vendor, fs::Permissions::from_mode(0o700))
            .expect("fixture vendor becomes executable");
        fs::write(
            root.join("spice-rust.toml"),
            format!(
                concat!(
                    "[lane]\nlease = \"20s\"\n\n",
                    "[tasks]\nstems = [\"taste\"]\n\n",
                    "[policy]\ntaste_mining_threshold = 3\n\n",
                    "[vendor.omp]\nbinary = {:?}\n",
                ),
                vendor.display().to_string()
            ),
        )
        .expect("fixture vendor configuration writes");
        Self {
            root,
            runtime,
            cargo_home,
        }
    }

    fn journal(&self) -> PathBuf {
        self.runtime.join("data/spicejournal.sqlite3")
    }

    fn canvas(&self) -> PathBuf {
        self.runtime.join("canvas")
    }

    fn discovery(&self) -> canvas::Discovery {
        wire::decode(&fs::read(self.canvas()).expect("operator discovery reads"))
            .expect("operator discovery decodes")
    }

    fn tape(&self, lane: wire::CanonicalUuid) -> PathBuf {
        self.runtime.join("tape").join(lane.to_string()).join("omp")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        make_fixture_writable(&self.root);
        fs::remove_dir_all(&self.root).expect("fixture cleanup");
    }
}

fn make_fixture_writable(path: &std::path::Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    if metadata.is_dir() {
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .expect("fixture directory becomes writable");
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                make_fixture_writable(&entry.path());
            }
        }
    } else {
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .expect("fixture file becomes writable");
    }
}

struct RunningResident {
    child: Option<Child>,
}

impl RunningResident {
    fn spawn(fixture: &Fixture) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_spice"))
            // A fixture resident asks the kernel for a free port: the fixed default
            // belongs to the operator's one resident, and concurrent fixtures on a
            // shared machine cannot all have it.
            .args(["--json", "up", "--foreground", "--port", "0"])
            .current_dir(&fixture.root)
            .arg(spice::install::TEST_CANVAS_STAGE_ARGUMENT)
            .env(
                spice::cargo_isolation::BOOTSTRAP_HOME_VARIABLE,
                &fixture.cargo_home,
            )
            .env(
                spice::cargo_isolation::NATIVE_CARGO_VARIABLE,
                fixture_native_tool("cargo"),
            )
            .env(
                spice::cargo_isolation::NATIVE_RUSTC_VARIABLE,
                fixture_native_tool("rustc"),
            )
            .env(
                spice::cargo_isolation::NATIVE_RUSTDOC_VARIABLE,
                fixture_native_tool("rustdoc"),
            )
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
            Readiness::Exited => self.assert_running(),
        }
    }

    fn assert_running(&mut self) {
        if self
            .child
            .as_mut()
            .expect("resident child")
            .try_wait()
            .expect("resident status")
            .is_none()
        {
            return;
        }
        let output = self
            .child
            .take()
            .expect("exited resident child")
            .wait_with_output()
            .expect("exited resident output");
        panic!(
            "foreground resident exited: stdout={:?} stderr={:?}",
            output.stdout, output.stderr
        );
    }

    fn wait(mut self) -> Output {
        self.child
            .take()
            .expect("resident child")
            .wait_with_output()
            .expect("foreground resident exits")
    }
}

fn fixture_native_tool(name: &str) -> PathBuf {
    let tool = PathBuf::from(env!("CARGO"))
        .parent()
        .expect("Cargo test executable has one toolchain directory")
        .join(name);
    assert!(tool.is_file(), "native fixture tool exists: {tool:?}");
    tool
}

impl Drop for RunningResident {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[derive(Clone, Copy)]
struct Lanes {
    target: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    other: wire::CanonicalUuid,
}

fn invoke(fixture: &Fixture, arguments: &[&str], json: bool) -> Output {
    let mut process = Command::new(env!("CARGO_BIN_EXE_spice"));
    process.env_remove(steer::hook::LANE);
    if json {
        process.arg("--json");
    }
    process
        .args(arguments)
        .current_dir(&fixture.root)
        .output()
        .expect("spice launches")
}

fn json_outcome<B: serde::de::DeserializeOwned>(output: &Output) -> Outcome<B> {
    let bytes = if output.status.success() {
        &output.stdout
    } else {
        &output.stderr
    };
    wire::decode(bytes).expect("process speaks one command outcome")
}

#[track_caller]
fn text_directive(output: &Output) -> wire::intent::DirectiveAnswer {
    assert!(
        output.status.success(),
        "command failed: {:?}",
        output.stderr
    );
    wire::decode(&output.stdout).expect("text reply carries the directive body")
}

/// The pending detail rows of one lane's `spice --json inbox` page.
#[track_caller]
fn inbox(fixture: &Fixture, lane: &str) -> (serde_json::Value, Vec<serde_json::Value>) {
    let output = invoke(fixture, &["inbox", lane], true);
    let Outcome::Reply(reply) = json_outcome::<serde_json::Value>(&output) else {
        panic!("inbox failed: {:?}", output.stderr);
    };
    let rows = reply.body["pending"]["rows"]
        .as_array()
        .expect("inbox detail rows")
        .clone();
    (reply.body, rows)
}

fn holds(rows: &[serde_json::Value], directive: wire::CanonicalUuid) -> bool {
    rows.iter()
        .any(|row| row["directive"] == directive.to_string())
}

fn operator_socket(fixture: &Fixture) -> (WebSocket<TcpStream>, canvas::Discovery) {
    let discovery = fixture.discovery();
    let authority = discovery
        .endpoint
        .strip_prefix("http://")
        .expect("fixture endpoint is HTTP");
    let stream = TcpStream::connect(authority).expect("operator socket connects");
    let mut request = format!("ws://{authority}/socket")
        .into_client_request()
        .expect("operator socket request");
    request
        .headers_mut()
        .insert(ORIGIN, discovery.endpoint.parse().expect("operator origin"));
    let (mut socket, _) = tungstenite::client(request, stream).expect("operator socket opens");
    socket_send(
        &mut socket,
        &wire::Hello {
            wire: wire::WireVersion::CURRENT,
            capability: discovery.capability,
            epoch: discovery.epoch,
        },
    );
    let accepted: wire::Accepted = socket_receive(&mut socket);
    assert_eq!(accepted.actor, Actor::Operator);
    assert_eq!(accepted.epoch, discovery.epoch);
    (socket, discovery)
}

fn socket_send<T: serde::Serialize>(socket: &mut WebSocket<TcpStream>, value: &T) {
    socket
        .send(Message::Binary(
            wire::encode(value).expect("socket value encodes").into(),
        ))
        .expect("socket value sends");
}

fn socket_receive<T: DeserializeOwned>(socket: &mut WebSocket<TcpStream>) -> T {
    loop {
        match socket.read().expect("socket value arrives") {
            Message::Binary(bytes) => {
                return wire::decode(&bytes).expect("binary socket value decodes");
            }
            Message::Text(text) => {
                return wire::decode(text.as_bytes()).expect("text socket value decodes");
            }
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            Message::Close(_) => panic!("resident closed before its answer"),
        }
    }
}

fn send_operator_direction(
    socket: &mut WebSocket<TcpStream>,
    discovery: &canvas::Discovery,
    directive: wire::CanonicalUuid,
    fanout: wire::CanonicalUuid,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    message: &str,
) -> wire::intent::DirectiveAnswer {
    socket_send(
        socket,
        &wire::Command {
            v: wire::WireVersion::CURRENT,
            id: directive,
            epoch: discovery.epoch,
            kind: wire::CommandKind::SteerSend,
            expected: vec![wire::Witness::Residency {
                lane,
                occupant: Some(occupant),
                epoch: discovery.epoch,
            }],
            body: wire::intent::SteerSend {
                fanout,
                lane,
                message: message.to_owned(),
                attachments: Vec::new(),
            },
        },
    );
    let response: wire::Response<wire::intent::DirectiveAnswer> = socket_receive(socket);
    assert_eq!((response.id, response.epoch), (directive, discovery.epoch));
    let Outcome::Reply(reply) = response.outcome else {
        panic!("operator steering did not reply")
    };
    reply.body
}

fn setup() -> (Fixture, RunningResident, Lanes) {
    let fixture = Fixture::new();
    let mut initial = RunningResident::spawn(&fixture);
    initial.assert_ready(&fixture);
    stop(&fixture, initial);

    let lanes = Lanes {
        target: uuid("00000000-0000-4000-8000-000000000061"),
        occupant: uuid("00000000-0000-4000-8000-000000000063"),
        other: uuid("00000000-0000-4000-8000-000000000064"),
    };
    seed_occupied_lane(
        &fixture,
        lanes.target,
        uuid("00000000-0000-4000-8000-000000000062"),
        lanes.occupant,
    );
    seed_occupied_lane(
        &fixture,
        lanes.other,
        uuid("00000000-0000-4000-8000-000000000065"),
        uuid("00000000-0000-4000-8000-000000000066"),
    );
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    // Since 40baef75 a restart no longer resurrects a durably occupied lane
    // whose supervisor left no trace: the seeded rows degrade to vacant and an
    // operator wakes them explicitly, which is what the fixture does here.
    wake_lane(&fixture, lanes.target);
    wake_lane(&fixture, lanes.other);
    assert!(
        fixture
            .runtime
            .join("canvas-lanes")
            .join(lanes.target.to_string())
            .is_file(),
        "resident did not publish the lane capability document"
    );
    (fixture, resident, lanes)
}

fn wake_lane(fixture: &Fixture, lane: wire::CanonicalUuid) {
    let woken = invoke(fixture, &["lane", "wake", &lane.to_string()], true);
    assert!(
        woken.status.success(),
        "lane wake failed: stdout={} stderr={}",
        String::from_utf8_lossy(&woken.stdout),
        String::from_utf8_lossy(&woken.stderr),
    );
}

fn current_occupant(fixture: &Fixture, lane: wire::CanonicalUuid) -> wire::CanonicalUuid {
    journal::Journal::open(fixture.journal())
        .expect("steering journal opens")
        .read(|read| read.residency_head())
        .expect("lane residency reads")
        .expect("lane residency exists")
        .into_body()
        .lanes
        .into_iter()
        .find(|row| row.lane == lane)
        .and_then(|row| row.occupant)
        .expect("restored lane has a current occupant")
}

fn stop(fixture: &Fixture, resident: RunningResident) {
    let down = invoke(fixture, &["down"], true);
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

fn seed_occupied_lane(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    capability: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
) {
    let journal = journal::Journal::open(fixture.journal()).expect("lane fixture journal opens");
    let bus = Bus::start(journal).expect("lane fixture bus starts");
    let current = bus
        .residency_head()
        .expect("lane fixture head reads")
        .expect("lane fixture head exists");
    let epoch = current.body().epoch;
    let worktree = fs::canonicalize(&fixture.root)
        .expect("lane fixture worktree canonicalizes")
        .display()
        .to_string();
    let mut declaration = SESSION_RECORD.as_bytes().to_vec();
    declaration.push(b'\n');
    let locus = tape::Tape::open(fixture.tape(lane))
        .expect("lane fixture tape opens")
        .tee(&declaration)
        .expect("session declaration reaches tape");
    let session = wire::LaneSession {
        lane,
        occupant,
        worktree: worktree.clone(),
        vendor: "omp".parse().expect("fixture vendor"),
        launch: Some(wire::body::LaneLaunchIdentity::new("sonnet", "high")),
        epoch,
        stream: "stdout".to_owned(),
        identity: SESSION_ID.to_owned(),
        locus: wire::TapeLocus {
            member: locus.member,
            offset: locus.offset,
            length: locus.length,
        },
    };
    let mut next = current.body().clone();
    next.lanes.push(wire::LaneResidency {
        lane,
        team: lane,
        name: None,
        worktree: worktree.clone(),
        life: "drive".to_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        model: None,
        effort: None,
        occupant: Some(occupant),
        lineage: vec![occupant],
        capabilities: vec![capability],
        session: Some(session.clone()),
        disposition: "occupied".to_owned(),
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
                team: lane,
                capability,
                name: None,
                worktree,
                life: "drive".to_owned(),
                vendor: "omp".parse().expect("fixture vendor"),
                model: None,
                effort: None,
            },
            FactCause::Existing(current.position()),
        )
        .expect("lane fixture fact");
    let declared = write
        .fact(
            wire::body::LaneSessionDeclared {
                lane: session.lane,
                occupant: session.occupant,
                worktree: session.worktree,
                vendor: session.vendor,
                launch: session.launch,
                epoch: session.epoch,
                stream: session.stream,
                source: None,
                identity: session.identity,
                member: session.locus.member,
                offset: session.locus.offset,
                length: session.locus.length,
            },
            FactCause::Pending(seated),
        )
        .expect("session fixture fact");
    let started = write
        .fact(
            wire::body::LaneStarted {
                lane,
                occupant,
                epoch,
                group: 123,
                fresh_session: None,
            },
            FactCause::Pending(declared),
        )
        .expect("occupant fixture fact");
    write.residency_head(started, move |_| next);
    match bus.commit(write).expect("fixture commit reaches writer") {
        CommitOutcome::Committed(_) => {}
        outcome => panic!("fixture write did not commit: {outcome:?}"),
    }
    bus.shutdown().expect("fixture bus stops");
}

fn publication_path(fixture: &Fixture, resident: &mut RunningResident, lanes: Lanes) {
    let lane = lanes.target.to_string();
    let direction = "> > earlier context\n> quoted context\n\nbind the exact command family";
    let sent = invoke(fixture, &["steer", &lane, direction], true);
    if !sent.status.success() {
        resident.assert_running();
    }
    let Outcome::Reply(reply): Outcome<wire::intent::DirectiveAnswer> = json_outcome(&sent) else {
        panic!("JSON steering send must reply: {:?}", sent.stderr);
    };
    let sent = reply.body;
    assert_eq!(sent.disposition, wire::Disposition::Live);
    let current_key = sent
        .key
        .parse::<steer::Key>()
        .expect("current steering key");
    assert_eq!(current_key.as_str().len(), steer::Key::WIDTH);
    assert!(current_key.minted_micros().is_some());
    let (inbox, pending) = inbox(fixture, &lane);
    assert_eq!(inbox["lanes"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        inbox["lanes"][0]["occupant"],
        current_occupant(fixture, lanes.target).to_string()
    );
    assert_eq!(inbox["lanes"][0]["keys"], serde_json::json!([sent.key]));
    assert_eq!(pending[0]["directive"], sent.directive.to_string());
    assert_eq!(pending[0]["key"], sent.key);
    assert_eq!(pending[0]["text"]["text"], direction);
    assert_eq!(
        pending[0]["authored"],
        directive_authored(fixture, sent.directive).to_string()
    );
    assert!(pending[0]["authored"].as_str() < pending[0]["expires"].as_str());
    assert_eq!(
        pending[0]["preview"]["text"],
        "bind the exact command family"
    );
    let canceled = text_directive(&invoke(
        fixture,
        &["steer", "cancel", &sent.directive.to_string()],
        false,
    ));
    assert_eq!(canceled.disposition, wire::Disposition::Canceled);
}

fn cancellation_path(fixture: &Fixture, lane: wire::CanonicalUuid) {
    let lane = lane.to_string();
    let sent = text_directive(&invoke(
        fixture,
        &["steer", &lane, "cancel this direction"],
        false,
    ));
    let canceled = text_directive(&invoke(
        fixture,
        &["steer", "cancel", &sent.directive.to_string()],
        false,
    ));
    assert_eq!(canceled.disposition, wire::Disposition::Canceled);
    let (_, pending) = inbox(fixture, &lane);
    assert!(
        !holds(&pending, sent.directive),
        "a canceled directive stayed pending"
    );
}

#[path = "steering/flows.rs"]
mod flows;

use flows::{directive_authored, instant, uuid};
