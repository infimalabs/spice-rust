//! Process coverage for the resident's transcript-backed steering path.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

use bus::{Bus, CommitOutcome};
use journal::{FactCause, JournalWrite};
use wire::{Actor, FactKind, Instant, Outcome, Producer};

#[path = "support/readiness.rs"]
mod readiness;
#[path = "support/staged_app.rs"]
mod staged_app;

use readiness::Readiness;

const REFUSAL_EXIT: i32 = 2;
const SESSION_ID: &str = "018f7777-7777-7444-8888-999999999999";
const SESSION_RECORD: &str =
    r#"{"type":"system","subtype":"init","session_id":"018f7777-7777-7444-8888-999999999999"}"#;

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
        let runtime = root.join(".git/.spice");
        fs::create_dir_all(root.join(".git")).expect("fixture git directory");
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
            root.join("spice.toml"),
            format!(
                "[lane]\nlease = \"1s\"\n\n[vendor.claude]\nbinary = {:?}\n",
                vendor.display().to_string()
            ),
        )
        .expect("fixture vendor configuration writes");
        Self { root, runtime }
    }

    fn journal(&self) -> PathBuf {
        self.runtime.join("data/spicejournal.sqlite3")
    }

    fn canvas(&self) -> PathBuf {
        self.runtime.join("canvas")
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
        fs::remove_dir_all(&self.root).expect("fixture cleanup");
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
            .args(["--json", "up", "--port", "0"])
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
    let column = text.find(token).expect("reply names its token") + 1;
    wire::body::TranscriptLocus {
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
        inbox.lanes[0].pending[0].preview.as_deref(),
        Some("bind the exact command family")
    );

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
            loser.reason.as_str(),
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

fn restart_path(
    fixture: &Fixture,
    resident: RunningResident,
    lane: wire::CanonicalUuid,
    historical_key: &str,
) {
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
    let duplicate = invoke_as_lane(fixture, &lane, &["ack", historical_key], true);
    assert_refusal(&duplicate, "steer::ack-missed");
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

fn assert_text_refusal(output: &Output, reason: &str) {
    assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
    assert!(output.stdout.is_empty());
    let text = std::str::from_utf8(&output.stderr).expect("text refusal is UTF-8");
    assert!(text.starts_with(&format!("spice: {reason};")), "{text:?}");
}

fn verify_authority(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    race: wire::Disposition,
    expected: &[ExpectedRecognition],
) {
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
    assert_eq!(count(FactKind::SteerSent), 7);
    assert_eq!(
        count(FactKind::SteerAcked),
        1 + usize::from(race == wire::Disposition::Acked)
    );
    assert_eq!(count(FactKind::SteerDeclined), 1);
    assert_eq!(
        count(FactKind::SteerCanceled),
        4 + usize::from(race == wire::Disposition::Canceled)
    );
    assert!(snapshot.facts().iter().all(|fact| {
        !matches!(
            fact.fact.kind(),
            FactKind::SteerAcked | FactKind::SteerDeclined
        ) || matches!(fact.actor, Actor::Lane(named) if named == lane)
    }));
    let mut observed = Vec::new();
    for envelope in snapshot.facts() {
        match envelope.fact.kind() {
            FactKind::SteerAcked => {
                let body: wire::body::SteerAcked = fact_body(envelope);
                observed.push(ExpectedRecognition {
                    directive: body.directive,
                    key: body.key,
                    epoch: body.epoch,
                    disposition: wire::Disposition::Acked,
                    locus: body.locus,
                });
            }
            FactKind::SteerDeclined => {
                let body: wire::body::SteerDeclined = fact_body(envelope);
                observed.push(ExpectedRecognition {
                    directive: body.directive,
                    key: body.key,
                    epoch: body.epoch,
                    disposition: wire::Disposition::Declined,
                    locus: body.locus,
                });
            }
            _ => {}
        }
    }
    assert_eq!(observed.len(), expected.len());
    for expected in expected {
        assert!(observed.iter().any(|observed| {
            observed.directive == expected.directive
                && observed.key == expected.key
                && observed.epoch == expected.epoch
                && observed.disposition == expected.disposition
                && observed.locus == expected.locus
        }));
        assert_transcript_locus(fixture, lane, expected.locus, &expected.key);
    }
    let missed = snapshot
        .facts()
        .iter()
        .filter(|fact| fact.fact.kind() == FactKind::SteerMissed)
        .map(fact_body::<wire::body::SteerMissed>)
        .collect::<Vec<_>>();
    assert!(
        missed.len() >= 2,
        "duplicate and historical ACKs were not recorded"
    );
    let initial_epoch = expected.first().expect("successful recognition").epoch;
    assert!(missed.iter().all(|body| {
        body.lane == lane
            && body.miss == wire::body::AcknowledgmentMiss::Historical
            && body.epoch == initial_epoch
    }));
    let unique = missed
        .iter()
        .map(|body| (&body.key, body.locus))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        unique.len(),
        missed.len(),
        "a missed recognition was duplicated"
    );
    for body in &missed {
        assert_transcript_locus(fixture, lane, body.locus, &body.key);
    }
}

fn fact_body<T: serde::de::DeserializeOwned>(envelope: &wire::Envelope<wire::AnyFact>) -> T {
    let bytes = wire::encode(envelope.fact.body()).expect("fact body encodes");
    wire::decode(&bytes).expect("registered fact body decodes")
}

fn assert_transcript_locus(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    locus: wire::body::TranscriptLocus,
    token: &str,
) {
    let tape = tape::Tape::open_existing(fixture.tape(lane)).expect("lane tape reopens");
    let bytes = tape
        .read(tape::Locus::new(locus.member, locus.offset, locus.length))
        .expect("persisted transcript locus resolves");
    let decoded = listen::decode(
        listen::Dialect::Claude,
        &bytes,
        tape::Locus::new(locus.member, locus.offset, locus.length),
    )
    .expect("persisted transcript locus is exact");
    let event = decoded
        .events()
        .get(usize::try_from(locus.event).expect("event index fits"))
        .expect("persisted transcript event exists");
    let listen::Event::Prose(prose) = event else {
        panic!("persisted transcript event is not assistant prose");
    };
    let line = prose
        .text
        .lines()
        .nth(usize::try_from(locus.line - 1).expect("line index fits"))
        .expect("persisted transcript line exists");
    let column = usize::try_from(locus.column - 1).expect("column index fits");
    assert_eq!(line.get(column..column + token.len()), Some(token));
}

fn uuid(value: &str) -> wire::CanonicalUuid {
    value.parse().expect("canonical fixture UUID")
}

fn instant(value: &str) -> Instant {
    value.parse().expect("canonical fixture instant")
}

#[test]
fn steering_commands_preserve_principals_races_and_restart_state() {
    let (fixture, mut resident, lanes) = setup();
    let (acknowledged, historical_key) = acknowledgment_paths(&fixture, &mut resident, lanes);
    let declined = decline_path(&fixture, lanes.target);
    let (race, raced) = cancellation_race(&fixture, lanes.target);
    assert!(matches!(
        race,
        wire::Disposition::Canceled | wire::Disposition::Acked
    ));
    cancellation_path(&fixture, lanes.target);
    direct_input_cannot_ack(&fixture, lanes.target);
    rendering_failure_path(&fixture, lanes.target);
    restart_path(&fixture, resident, lanes.target, &historical_key);
    let mut expected = vec![acknowledged, declined];
    expected.extend(raced);
    verify_authority(&fixture, lanes.target, race, &expected);
}
