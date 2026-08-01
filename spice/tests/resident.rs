use std::fs;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant as Monotonic};

use bus::{Bus, CommitOutcome};
use journal::{FactCause, JournalWrite};
use notify::{RecursiveMode, Watcher};
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
    Command::new(env!("CARGO_BIN_EXE_spice"))
        .args(["--json", command])
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
    assert_eq!((range.first.get(), range.last.get()), (3, 8));

    let up = resident.wait();
    assert!(up.status.success(), "up failed: {:?}", up.stderr);
    let Outcome::Reply(reply) = outcome(&up) else {
        panic!("foreground up must reply after shutdown");
    };
    let ReplyPosition::Committed(range) = reply.position else {
        panic!("up commits startup");
    };
    assert_eq!((range.first.get(), range.last.get()), (1, 2));
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
