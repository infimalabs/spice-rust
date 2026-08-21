//! Process coverage for the resident's transcript-backed steering path.

use std::fs;
use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Barrier};

use bus::{Bus, CommitOutcome};
use journal::{FactCause, JournalWrite};
use serde::de::DeserializeOwned;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::header::ORIGIN;
use tungstenite::{Message, WebSocket};
use wire::{Actor, FactKind, Instant, Outcome, Producer};

#[path = "support/fixture_root.rs"]
mod fixture_root;
#[path = "support/readiness.rs"]
mod readiness;
#[path = "support/staged_app.rs"]
mod staged_app;

use readiness::Readiness;

const FAULT_EXIT: i32 = 1;
const REFUSAL_EXIT: i32 = 2;
const SESSION_ID: &str = "018f7777-7777-7444-8888-999999999999";
const SESSION_RECORD: &str =
    r#"{"type":"system","subtype":"init","session_id":"018f7777-7777-7444-8888-999999999999"}"#;

struct Fixture {
    root: PathBuf,
    runtime: PathBuf,
    cargo_home: PathBuf,
}

struct InboxPermissionBlock {
    path: PathBuf,
    original: Option<fs::Permissions>,
}

impl InboxPermissionBlock {
    fn new(path: PathBuf) -> Self {
        let original = fs::metadata(&path)
            .expect("fixture inbox metadata")
            .permissions();
        let mut blocked = original.clone();
        blocked.set_readonly(true);
        fs::set_permissions(&path, blocked)
            .expect("fixture blocks inbox publication without changing its admitted shape");
        Self {
            path,
            original: Some(original),
        }
    }

    fn restore(mut self) {
        let original = self.original.take().expect("permission block is armed");
        fs::set_permissions(&self.path, original)
            .expect("fixture restores inbox publication permissions");
    }
}

impl Drop for InboxPermissionBlock {
    fn drop(&mut self) {
        if let Some(original) = self.original.take() {
            let _ = fs::set_permissions(&self.path, original);
        }
    }
}

impl Fixture {
    fn new() -> Self {
        let root = fixture_root::claim(&std::env::temp_dir(), "steering-process");
        let runtime = root.join(".git/.spice-rs");
        let cargo_home = root.join("fixture-cargo-home");
        fs::create_dir(root.join(".git")).expect("fixture git directory");
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
        fs::write(&skill, include_bytes!("../agent/SKILL.md"))
            .expect("fixture installs the appliance skill");
        staged_app::seed(&root);
        let vendor = root.join("fake-claude");
        fs::write(
            &vendor,
            concat!(
                "#!/bin/sh\n",
                "trap 'exit 0' TERM INT\n",
                "printf '%s\\n' '",
                "{\"type\":\"system\",\"subtype\":\"init\",",
                "\"session_id\":\"018f7777-7777-7444-8888-999999999999\"}'\n",
                "while :; do sleep 1; done\n",
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
                    "[lane]\nlease = \"1s\"\n\n",
                    "[tasks]\nstems = [\"taste\"]\n\n",
                    "[policy]\ntaste_mining_threshold = 3\n\n",
                    "[vendor.claude]\nbinary = {:?}\n",
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
        self.runtime
            .join("tape")
            .join(lane.to_string())
            .join("claude")
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
            .env(
                spice::cargo_isolation::BOOTSTRAP_HOME_VARIABLE,
                &fixture.cargo_home,
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

#[derive(Clone)]
struct ExpectedRecognition {
    directive: wire::CanonicalUuid,
    key: String,
    epoch: wire::CanonicalUuid,
    disposition: wire::Disposition,
    locus: wire::body::TranscriptLocus,
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

fn invoke_as_lane(fixture: &Fixture, lane: &str, arguments: &[&str], json: bool) -> Output {
    let mut process = Command::new(env!("CARGO_BIN_EXE_spice"));
    if json {
        process.arg("--json");
    }
    process
        .args(arguments)
        .env(steer::hook::LANE, lane)
        .current_dir(&fixture.root)
        .output()
        .expect("lane spice launches")
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

#[track_caller]
fn text_inbox(output: &Output) -> wire::intent::InboxAnswer {
    assert!(output.status.success(), "inbox failed: {:?}", output.stderr);
    wire::decode(&output.stdout).expect("text reply carries the inbox body")
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
                occupant,
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
        vendor: "claude".parse().expect("fixture vendor"),
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
        lineage_id: lane,
        name: None,
        worktree: worktree.clone(),
        life: "drive".to_owned(),
        vendor: "claude".parse().expect("fixture vendor"),
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
                lineage_id: lane,
                capability,
                name: None,
                worktree,
                life: "drive".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
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

fn record_reply(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    text: &str,
    token: &str,
) -> wire::body::TranscriptLocus {
    let record = serde_json::json!({
        "type": "assistant",
        "message": {
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "text", "text": text }],
            "usage": {
                "input_tokens": 7,
                "cache_read_input_tokens": 5,
                "cache_creation_input_tokens": 3,
                "output_tokens": 2
            }
        }
    });
    let mut bytes = wire::encode(&record).expect("assistant transcript record encodes");
    bytes.push(b'\n');
    let tape = tape::Tape::open(fixture.tape(lane)).expect("lane tape opens");
    let locus = tape.tee(&bytes).expect("assistant record reaches tape");
    let source = tape.source_id().expect("assistant tape source reads");
    let column = text.find(token).expect("reply names its token") + 1;
    wire::body::TranscriptLocus {
        source: Some(source.as_str().parse().expect("tape source is canonical")),
        member: locus.member,
        offset: locus.offset,
        length: locus.length,
        event: 0,
        line: 1,
        column: u32::try_from(column).expect("fixture column fits"),
    }
}

fn lane_epoch(fixture: &Fixture, lane: wire::CanonicalUuid) -> wire::CanonicalUuid {
    let lane = lane.to_string();
    text_inbox(&invoke(fixture, &["inbox", &lane], false)).epoch
}

fn acknowledgment_paths(
    fixture: &Fixture,
    resident: &mut RunningResident,
    lanes: Lanes,
) -> (ExpectedRecognition, String) {
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
    let inbox = text_inbox(&invoke(fixture, &["inbox", &lane], false));
    assert_eq!(inbox.lanes.len(), 1);
    assert_eq!(
        inbox.lanes[0].occupant,
        Some(current_occupant(fixture, lanes.target))
    );
    assert_eq!(inbox.lanes[0].pending[0].directive, sent.directive);
    assert_eq!(inbox.lanes[0].pending[0].key, sent.key);
    assert_eq!(inbox.lanes[0].pending[0].text, direction);
    assert_eq!(
        inbox.lanes[0].pending[0].authored,
        directive_authored(fixture, sent.directive)
    );
    assert!(inbox.lanes[0].pending[0].authored < inbox.lanes[0].pending[0].expires);
    assert_eq!(
        inbox.lanes[0].pending[0].preview.as_deref(),
        Some("bind the exact command family")
    );

    let codex = invoke_as_lane(fixture, &lane, &["hook", "codex"], false);
    assert!(
        codex.status.success(),
        "hook inbox failed: {:?}",
        codex.stderr
    );
    assert!(codex.stderr.is_empty(), "hook inbox spoke on stderr");
    let hook: serde_json::Value =
        wire::decode(&codex.stdout).expect("hook inbox speaks one Codex hook document");
    assert_eq!(hook["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    let additional_context = hook["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("hook context is text");
    assert!(additional_context.contains(&sent.key));
    assert!(additional_context.contains("stage=published"));
    assert!(additional_context.contains(direction));
    assert!(additional_context.contains("ack <key>"));
    hook_harness_paths(fixture, &lane, &codex.stdout);

    let other = lanes.other.to_string();
    record_reply(
        fixture,
        lanes.other,
        &format!("ACK {}: wrong lane", sent.key),
        &sent.key,
    );
    let wrong = invoke_as_lane(fixture, &other, &["ack", &sent.key], true);
    assert_refusal(&wrong, "steer::lane-scope");
    let locus = record_reply(
        fixture,
        lanes.target,
        &format!("ACK {}: applied", sent.key),
        &sent.key,
    );
    let acknowledged = text_directive(&invoke_as_lane(fixture, &lane, &["ack", &sent.key], false));
    assert_eq!(acknowledged.directive, sent.directive);
    assert_eq!(acknowledged.disposition, wire::Disposition::Acked);
    let empty_hook = invoke_as_lane(fixture, &lane, &["hook", "codex"], false);
    assert!(
        empty_hook.status.success(),
        "empty hook inbox failed: {:?}",
        empty_hook.stderr
    );
    assert!(empty_hook.stdout.is_empty());
    assert!(empty_hook.stderr.is_empty());
    let duplicate = invoke_as_lane(fixture, &lane, &["ack", &sent.key], true);
    assert_refusal(&duplicate, "steer::ack-missed");
    let duplicate_again = invoke_as_lane(fixture, &lane, &["ack", &sent.key], true);
    assert_refusal(&duplicate_again, "steer::ack-missed");
    (
        ExpectedRecognition {
            directive: sent.directive,
            key: sent.key.clone(),
            epoch: inbox.epoch,
            disposition: wire::Disposition::Acked,
            locus,
        },
        sent.key,
    )
}

/// Both admitted harnesses, and one that is not, over the same pending inbox.
///
/// The two admitted spellings take byte-identical documents today, so the only
/// thing the operand decides is whether the command runs at all. Pinning the
/// equality is what will notice the day one harness needs its own shape, and
/// pinning the refusal is what keeps an unrecognised harness from being handed
/// a document written for someone else.
fn hook_harness_paths(fixture: &Fixture, lane: &str, codex: &[u8]) {
    let claude = invoke_as_lane(fixture, lane, &["hook", "claude"], false);
    assert!(
        claude.status.success(),
        "claude hook inbox failed: {:?}",
        claude.stderr
    );
    assert!(
        claude.stderr.is_empty(),
        "claude hook inbox spoke on stderr"
    );
    assert_eq!(
        claude.stdout, codex,
        "the admitted harnesses no longer take one document"
    );

    let unsupported = invoke_as_lane(fixture, lane, &["hook", "nushell"], false);
    assert_eq!(unsupported.status.code(), Some(FAULT_EXIT));
    assert!(
        unsupported.stdout.is_empty(),
        "an unadmitted harness still took a document"
    );
    let text = std::str::from_utf8(&unsupported.stderr).expect("text fault is UTF-8");
    assert!(text.contains("unsupported hook harness"), "{text:?}");
    assert!(text.contains("nushell"), "{text:?}");
}

fn decline_path(fixture: &Fixture, lane: wire::CanonicalUuid) -> ExpectedRecognition {
    let lane = lane.to_string();
    let sent = text_directive(&invoke(
        fixture,
        &["steer", &lane, "decline this direction"],
        false,
    ));
    let epoch = lane_epoch(fixture, lane.parse().expect("lane UUID"));
    let locus = record_reply(
        fixture,
        lane.parse().expect("lane UUID"),
        &format!("NACK {}: cannot apply", sent.key),
        &sent.key,
    );
    let mismatched = invoke_as_lane(fixture, &lane, &["ack", &sent.key], true);
    assert_refusal(&mismatched, "steer::transcript-disposition-mismatch");
    let declined = invoke_as_lane(fixture, &lane, &["ack", &sent.key, "--decline"], true);
    let Outcome::Reply(reply): Outcome<wire::intent::DirectiveAnswer> = json_outcome(&declined)
    else {
        panic!("JSON decline must reply: {:?}", declined.stderr);
    };
    assert_eq!(reply.body.disposition, wire::Disposition::Declined);
    ExpectedRecognition {
        directive: sent.directive,
        key: sent.key,
        epoch,
        disposition: wire::Disposition::Declined,
        locus,
    }
}

fn cancellation_race(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
) -> (wire::Disposition, Option<ExpectedRecognition>) {
    let lane = lane.to_string();
    let sent = text_directive(&invoke(
        fixture,
        &["steer", &lane, "retire exactly once under contention"],
        false,
    ));
    let epoch = lane_epoch(fixture, lane.parse().expect("lane UUID"));
    let locus = record_reply(
        fixture,
        lane.parse().expect("lane UUID"),
        &format!("ACK {}: raced", sent.key),
        &sent.key,
    );
    let directive = sent.directive.to_string();
    let barrier = Arc::new(Barrier::new(3));
    let (cancel, ack) = std::thread::scope(|scope| {
        let cancel_barrier = Arc::clone(&barrier);
        let cancel_fixture = fixture;
        let cancel_directive = &directive;
        let cancel = scope.spawn(move || {
            cancel_barrier.wait();
            invoke(cancel_fixture, &["steer", "cancel", cancel_directive], true)
        });
        let ack_barrier = Arc::clone(&barrier);
        let ack_fixture = fixture;
        let ack_lane = &lane;
        let ack_key = &sent.key;
        let ack = scope.spawn(move || {
            ack_barrier.wait();
            invoke_as_lane(ack_fixture, ack_lane, &["ack", ack_key], true)
        });
        barrier.wait();
        (cancel.join().unwrap(), ack.join().unwrap())
    });
    let cancel: Outcome<wire::intent::DirectiveAnswer> = json_outcome(&cancel);
    let ack: Outcome<wire::intent::DirectiveAnswer> = json_outcome(&ack);
    let (disposition, loser) = match (&cancel, &ack) {
        (Outcome::Reply(reply), Outcome::Refusal(refusal)) => (reply.body.disposition, refusal),
        (Outcome::Refusal(refusal), Outcome::Reply(reply)) => (reply.body.disposition, refusal),
        outcomes => panic!("race must have one winner and one refusal: {outcomes:?}"),
    };
    assert!(
        matches!(
            loser.reason(),
            "steer::directive-retired"
                | "steer::directive-moved"
                | "steer::witness-moved"
                | "steer::ack-missed"
        ),
        "unexpected race refusal: {loser:?}"
    );
    let recognition = (disposition == wire::Disposition::Acked).then_some(ExpectedRecognition {
        directive: sent.directive,
        key: sent.key.clone(),
        epoch,
        disposition,
        locus,
    });
    (disposition, recognition)
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
    record_reply(
        fixture,
        lane.parse().expect("lane UUID"),
        &format!("ACK {}: too late", sent.key),
        &sent.key,
    );
    let refused = invoke_as_lane(fixture, &lane, &["ack", &sent.key], true);
    assert_refusal(&refused, "steer::ack-missed");
}

fn direct_input_cannot_ack(fixture: &Fixture, lane: wire::CanonicalUuid) {
    let lane = lane.to_string();
    let sent = text_directive(&invoke(
        fixture,
        &["steer", &lane, "require durable transcript evidence"],
        false,
    ));
    let refused = invoke_as_lane(fixture, &lane, &["ack", &sent.key], false);
    assert_text_refusal(&refused, "steer::transcript-evidence-absent");
    let inbox = text_inbox(&invoke(fixture, &["inbox", &lane], false));
    assert!(
        inbox.lanes[0]
            .pending
            .iter()
            .any(|pending| pending.directive == sent.directive),
        "direct ack input retired a directive without transcript evidence"
    );
    let canceled = text_directive(&invoke(
        fixture,
        &["steer", "cancel", &sent.directive.to_string()],
        false,
    ));
    assert_eq!(canceled.disposition, wire::Disposition::Canceled);
}

#[path = "steering/flows.rs"]
mod flows;

use flows::{assert_refusal, assert_text_refusal, directive_authored, instant, uuid};
