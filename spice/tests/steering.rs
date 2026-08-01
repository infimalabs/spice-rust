use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::time::{Duration, Instant as Monotonic};

use bus::{Bus, CommitOutcome};
use journal::{FactCause, JournalWrite};
use notify::{RecursiveMode, Watcher};
use wire::{Actor, FactKind, Instant, Outcome, Producer};

const REFUSAL_EXIT: i32 = 2;
const READINESS_DEADLINE: Duration = Duration::from_secs(10);

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    runtime: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-steering-process-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let runtime = root.join(".git/.spice-rust");
        fs::create_dir_all(root.join(".git")).expect("fixture git directory");
        Self { root, runtime }
    }

    fn journal(&self) -> PathBuf {
        self.runtime.join("data/spicejournal.sqlite3")
    }

    fn canvas(&self) -> PathBuf {
        self.runtime.join("canvas")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("fixture cleanup");
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
        while !fixture.canvas().is_file() {
            assert!(
                self.child
                    .as_mut()
                    .expect("resident child")
                    .try_wait()
                    .expect("resident status")
                    .is_none(),
                "foreground resident exited before readiness"
            );
            let remaining = deadline.saturating_duration_since(Monotonic::now());
            match receiver.recv_timeout(remaining) {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => panic!("readiness notification failed: {error}"),
                Err(error) => panic!("resident readiness failed: {error}"),
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

#[derive(Clone, Copy)]
struct Lanes {
    target: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    other: wire::CanonicalUuid,
}

fn invoke(fixture: &Fixture, arguments: &[&str], json: bool) -> Output {
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

fn text_directive(output: &Output) -> wire::intent::DirectiveAnswer {
    assert!(
        output.status.success(),
        "command failed: {:?}",
        output.stderr
    );
    wire::decode(&output.stdout).expect("text reply carries the directive body")
}

fn text_inbox(output: &Output) -> wire::intent::InboxAnswer {
    assert!(output.status.success(), "inbox failed: {:?}", output.stderr);
    wire::decode(&output.stdout).expect("text reply carries the inbox body")
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
    let mut next = current.body().clone();
    next.lanes.push(wire::LaneResidency {
        lane,
        occupant: Some(occupant),
        lineage: vec![occupant],
        capabilities: vec![capability],
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
                capability,
                worktree: fixture.root.display().to_string(),
            },
            FactCause::Existing(current.position()),
        )
        .expect("lane fixture fact");
    let started = write
        .fact(
            wire::body::LaneStarted {
                lane,
                occupant,
                epoch,
            },
            FactCause::Pending(seated),
        )
        .expect("occupant fixture fact");
    write.residency_head(started, move |_| next);
    match bus.commit(write).expect("fixture commit reaches writer") {
        CommitOutcome::Committed(_) => {}
        outcome => panic!("fixture write did not commit: {outcome:?}"),
    }
    bus.shutdown().expect("fixture bus stops");
}

fn acknowledgment_paths(fixture: &Fixture, lanes: Lanes) {
    let lane = lanes.target.to_string();
    let sent = invoke(
        fixture,
        &["steer", &lane, "bind the exact command family"],
        true,
    );
    let Outcome::Reply(reply): Outcome<wire::intent::DirectiveAnswer> = json_outcome(&sent) else {
        panic!("JSON steering send must reply: {:?}", sent.stderr);
    };
    let sent = reply.body;
    assert_eq!(sent.disposition, wire::Disposition::Live);
    let inbox = text_inbox(&invoke(fixture, &["inbox", &lane], false));
    assert_eq!(inbox.lanes.len(), 1);
    assert_eq!(inbox.lanes[0].occupant, Some(lanes.occupant));
    assert_eq!(inbox.lanes[0].pending[0].directive, sent.directive);
    assert_eq!(inbox.lanes[0].pending[0].key, sent.key);

    let other = lanes.other.to_string();
    let wrong = invoke_as_lane(fixture, &other, &["ack", &sent.key], true);
    assert_refusal(&wrong, "steer::key-not-live");
    let acknowledged = text_directive(&invoke_as_lane(fixture, &lane, &["ack", &sent.key], false));
    assert_eq!(acknowledged.directive, sent.directive);
    assert_eq!(acknowledged.disposition, wire::Disposition::Acked);
    let duplicate = invoke_as_lane(fixture, &lane, &["ack", &sent.key], true);
    assert_refusal(&duplicate, "steer::key-not-live");
}

fn decline_path(fixture: &Fixture, lane: wire::CanonicalUuid) {
    let lane = lane.to_string();
    let sent = text_directive(&invoke(
        fixture,
        &["steer", &lane, "decline this direction"],
        false,
    ));
    let declined = invoke_as_lane(fixture, &lane, &["ack", &sent.key, "--decline"], true);
    let Outcome::Reply(reply): Outcome<wire::intent::DirectiveAnswer> = json_outcome(&declined)
    else {
        panic!("JSON decline must reply: {:?}", declined.stderr);
    };
    assert_eq!(reply.body.disposition, wire::Disposition::Declined);
}

fn cancellation_race(fixture: &Fixture, lane: wire::CanonicalUuid) -> wire::Disposition {
    let lane = lane.to_string();
    let sent = text_directive(&invoke(
        fixture,
        &["steer", &lane, "retire exactly once under contention"],
        false,
    ));
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
    assert!(matches!(
        loser.reason.as_str(),
        "steer::directive-retired" | "steer::directive-moved" | "steer::key-not-live"
    ));
    disposition
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
    let refused = invoke_as_lane(fixture, &lane, &["ack", &sent.key], true);
    assert_refusal(&refused, "steer::key-not-live");
}

fn rendering_failure_path(fixture: &Fixture, lane: wire::CanonicalUuid) {
    let lane = lane.to_string();
    let inbox_root = fixture.runtime.join("inbox");
    fs::remove_dir_all(&inbox_root).expect("fixture removes disposable inbox renderings");
    fs::write(&inbox_root, b"blocked").expect("fixture blocks inbox publication");
    let sent = text_directive(&invoke(
        fixture,
        &["steer", &lane, "remain authoritative without a rendering"],
        false,
    ));
    let inbox = text_inbox(&invoke(fixture, &["inbox", &lane], false));
    assert!(
        inbox.lanes[0]
            .pending
            .iter()
            .any(|pending| pending.directive == sent.directive)
    );
    fs::remove_file(&inbox_root).expect("fixture unblocks inbox publication");
    let canceled = text_directive(&invoke(
        fixture,
        &["steer", "cancel", &sent.directive.to_string()],
        false,
    ));
    assert_eq!(canceled.disposition, wire::Disposition::Canceled);
}

fn restart_path(fixture: &Fixture, resident: RunningResident, lane: wire::CanonicalUuid) {
    let lane = lane.to_string();
    let unread = text_directive(&invoke(
        fixture,
        &["steer", &lane, "survive the resident restart"],
        false,
    ));
    let rendering = fixture.runtime.join("inbox").join(&lane);
    assert!(
        fs::read_to_string(&rendering)
            .unwrap()
            .contains(&unread.key)
    );
    stop(fixture, resident);

    let mut restarted = RunningResident::spawn(fixture);
    restarted.assert_ready(fixture);
    let inbox = invoke(fixture, &["inbox", &lane], true);
    let Outcome::Reply(reply): Outcome<wire::intent::InboxAnswer> = json_outcome(&inbox) else {
        panic!("restarted inbox must reply: {:?}", inbox.stderr);
    };
    assert_eq!(reply.body.lanes[0].pending[0].key, unread.key);
    assert!(
        fs::read_to_string(&rendering)
            .unwrap()
            .contains(&unread.key)
    );
    let canceled = invoke(
        fixture,
        &["steer", "cancel", &unread.directive.to_string()],
        true,
    );
    let Outcome::Reply(_): Outcome<wire::intent::DirectiveAnswer> = json_outcome(&canceled) else {
        panic!("restarted cancellation must reply: {:?}", canceled.stderr);
    };
    stop(fixture, restarted);
}

fn assert_refusal(output: &Output, reason: &str) {
    assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal): Outcome<wire::intent::DirectiveAnswer> = json_outcome(output)
    else {
        panic!("command must refuse: {:?}", output.stderr);
    };
    assert_eq!(refusal.reason, reason);
}

fn verify_authority(fixture: &Fixture, lane: wire::CanonicalUuid, race: wire::Disposition) {
    let journal = journal::Journal::open(fixture.journal()).expect("steering journal opens");
    journal.verify().expect("steering authority verifies");
    let snapshot = journal.fact_snapshot().expect("steering facts read");
    let count = |kind| {
        snapshot
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == kind)
            .count()
    };
    assert_eq!(count(FactKind::SteerSent), 6);
    assert_eq!(
        count(FactKind::SteerAcked),
        1 + usize::from(race == wire::Disposition::Acked)
    );
    assert_eq!(count(FactKind::SteerDeclined), 1);
    assert_eq!(
        count(FactKind::SteerCanceled),
        3 + usize::from(race == wire::Disposition::Canceled)
    );
    assert!(snapshot.facts().iter().all(|fact| {
        !matches!(
            fact.fact.kind(),
            FactKind::SteerAcked | FactKind::SteerDeclined
        ) || matches!(fact.actor, Actor::Lane(named) if named == lane)
    }));
}

fn uuid(value: &str) -> wire::CanonicalUuid {
    value.parse().expect("canonical fixture UUID")
}

fn instant(value: &str) -> Instant {
    value.parse().expect("canonical fixture instant")
}

#[test]
fn steering_commands_preserve_principals_races_and_restart_state() {
    let (fixture, resident, lanes) = setup();
    acknowledgment_paths(&fixture, lanes);
    decline_path(&fixture, lanes.target);
    let race = cancellation_race(&fixture, lanes.target);
    assert!(matches!(
        race,
        wire::Disposition::Canceled | wire::Disposition::Acked
    ));
    cancellation_path(&fixture, lanes.target);
    rendering_failure_path(&fixture, lanes.target);
    restart_path(&fixture, resident, lanes.target);
    verify_authority(&fixture, lanes.target, race);
}
