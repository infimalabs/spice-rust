//! Resident process cases sharing the integration fixture.
//!
//! Keeping cases below the fixture root makes the process boundary reusable
//! without letting one integration-test source exceed the repository limit.

use super::*;

/// Longer than the roughly one-event-per-second signature that prompted this
/// trace. The socket and journal are both event-driven, so a quiet interval
/// spends this bound only in the assertion that nothing arrived.
const QUIET_INTERVAL: Duration = Duration::from_millis(1_250);

fn send_process_stream(
    socket: &mut WebSocket<TcpStream>,
    request: &wire::StreamRequest,
    label: &str,
) {
    socket
        .send(Message::Binary(
            wire::encode(request)
                .unwrap_or_else(|error| panic!("{label} encodes: {error}"))
                .into(),
        ))
        .unwrap_or_else(|error| panic!("{label} sends: {error}"));
}

fn read_process_stream(
    socket: &mut WebSocket<TcpStream>,
    label: &str,
) -> (wire::StreamFrame, Vec<u8>) {
    let bytes = socket
        .read()
        .unwrap_or_else(|error| panic!("{label} arrives: {error}"))
        .into_data()
        .to_vec();
    let frame = wire::decode(&bytes).unwrap_or_else(|error| panic!("{label} decodes: {error}"));
    (frame, bytes)
}

fn resident_facts(fixture: &Fixture) -> journal::FactSnapshot {
    journal::Journal::open(fixture.journal())
        .expect("resident journal opens for quietness evidence")
        .fact_snapshot()
        .expect("resident facts read for quietness evidence")
}

fn assert_process_stream_silent(socket: &mut WebSocket<TcpStream>) {
    socket
        .get_mut()
        .set_read_timeout(Some(QUIET_INTERVAL))
        .expect("quiet socket timeout sets");
    let result = socket.read();
    assert!(
        matches!(
            result,
            Err(tungstenite::Error::Io(ref error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                )
        ),
        "a quiet Canvas connection produced an unsolicited frame: {result:?}"
    );
}

fn open_process_stream(
    discovery: &canvas::Discovery,
) -> (
    WebSocket<TcpStream>,
    wire::CanonicalUuid,
    wire::Seq,
    Vec<u8>,
) {
    let (mut socket, accepted) = authenticate(discovery, discovery.capability);
    assert_eq!(accepted.actor, Actor::Operator);
    let subscription = uuid("018f82d4-6f59-7d06-92e3-bce9131163d0");
    send_process_stream(
        &mut socket,
        &wire::StreamRequest::Subscribe {
            v: wire::WireVersion::CURRENT,
            id: subscription,
            epoch: discovery.epoch,
            subjects: vec![wire::ViewSubject::Facts, wire::ViewSubject::Trust],
            after: None,
        },
        "subscription",
    );
    let (initial_facts, facts_bytes) = read_process_stream(&mut socket, "initial facts");
    let (initial_trust, trust_bytes) = read_process_stream(&mut socket, "initial trust");
    let wire::StreamFrame::Facts { packed, .. } = initial_facts else {
        panic!("resident must begin with facts");
    };
    let initial_horizon = packed.horizon;
    assert!(packed.overflow.is_none(), "fixture remains one packed page");
    assert!(matches!(
        initial_trust,
        wire::StreamFrame::Trust { ref packed, .. }
            if packed.horizon == initial_horizon && packed.rows.len() == 1
    ));
    let initial_bytes = [facts_bytes, trust_bytes].concat();
    assert!(!String::from_utf8_lossy(&initial_bytes).contains(&discovery.capability.to_string()));
    (socket, subscription, initial_horizon, initial_bytes)
}

fn seat_and_close_process_stream(
    fixture: &Fixture,
    discovery: &canvas::Discovery,
    socket: &mut WebSocket<TcpStream>,
    subscription: wire::CanonicalUuid,
    initial_horizon: wire::Seq,
) -> (wire::Seq, [String; 2], wire::CanonicalUuid) {
    let seated = invoke_args(
        fixture,
        &["lane", "add", "--life", "Drive", "--vendor", "claude"],
        true,
    );
    assert!(
        seated.status.success(),
        "lane seating failed: {}",
        String::from_utf8_lossy(&seated.stderr)
    );

    let journal = journal::Journal::open(fixture.journal()).expect("lane journal opens");
    let residency = journal
        .read(|read| read.residency_head())
        .expect("lane residency reads")
        .expect("lane residency exists")
        .into_body();
    let lane = residency.lanes.first().expect("resident seated one lane");
    let lane_id = lane.lane;
    let lane_capability = lane.capabilities[0];
    let capability_text = [
        discovery.capability.to_string(),
        lane_capability.to_string(),
    ];

    let mut saw_seated = false;
    let mut saw_declared = false;
    let mut started_at = None;
    let mut live_cursor = None;
    for _ in 0..16 {
        let (frame, bytes) = read_process_stream(socket, "live lane frame");
        for credential in &capability_text {
            assert!(!String::from_utf8_lossy(&bytes).contains(credential));
        }
        match frame {
            wire::StreamFrame::Fact { fact, id, .. } => {
                assert_eq!(id, subscription);
                assert!(fact.seq() > initial_horizon);
                match fact.kind() {
                    FactKind::LaneSeated => {
                        saw_seated = true;
                        assert!(!fact.body().contains_key("capability"));
                    }
                    FactKind::LaneSessionDeclared => saw_declared = true,
                    FactKind::LaneStarted => started_at = Some(fact.seq()),
                    _ => {}
                }
            }
            wire::StreamFrame::Trust { packed, .. }
                if saw_seated && saw_declared && started_at.is_some() =>
            {
                assert!(packed.rows.iter().all(|row| {
                    row.head.state == wire::TrustState::Granted
                        || row.head.state == wire::TrustState::Pended
                }));
                live_cursor = Some(packed.horizon);
                break;
            }
            wire::StreamFrame::Trust { .. } => {}
            other => panic!("unexpected live lane frame: {other:?}"),
        }
    }
    let live_cursor = live_cursor.expect("lane lifecycle reached its stream horizon");

    let thread_id = uuid("018f82d4-6f59-7d06-92e3-bce9131163d1");
    send_process_stream(
        socket,
        &wire::StreamRequest::Thread {
            v: wire::WireVersion::CURRENT,
            id: thread_id,
            epoch: discovery.epoch,
            cursor: wire::StreamCursor {
                epoch: discovery.epoch,
                seq: started_at.expect("lane start arrived"),
            },
        },
        "thread request",
    );
    let (thread, thread_bytes) = read_process_stream(socket, "thread");
    for credential in &capability_text {
        assert!(!String::from_utf8_lossy(&thread_bytes).contains(credential));
    }
    let wire::StreamFrame::Thread { id, packed, .. } = thread else {
        panic!("lane causal thread must be returned");
    };
    assert_eq!(id, thread_id);
    assert!(
        packed
            .rows
            .iter()
            .any(|fact| fact.kind() == FactKind::LaneSeated)
    );
    socket.close(None).expect("first stream closes");
    (live_cursor, capability_text, lane_id)
}

fn resume_process_stream(
    discovery: &canvas::Discovery,
    live_cursor: wire::Seq,
) -> WebSocket<TcpStream> {
    let (mut resumed, _) = authenticate(discovery, discovery.capability);
    send_process_stream(
        &mut resumed,
        &wire::StreamRequest::Subscribe {
            v: wire::WireVersion::CURRENT,
            id: uuid("018f82d4-6f59-7d06-92e3-bce9131163d2"),
            epoch: discovery.epoch,
            subjects: vec![wire::ViewSubject::Facts],
            after: Some(wire::StreamCursor {
                epoch: discovery.epoch,
                seq: live_cursor,
            }),
        },
        "resume",
    );
    let (resumed_frame, _) = read_process_stream(&mut resumed, "resume");
    assert!(matches!(
        resumed_frame,
        wire::StreamFrame::Facts { packed, .. }
            if packed.rows.is_empty() && packed.horizon == live_cursor
    ));
    resumed
}

fn assert_stale_process_stream(discovery: &canvas::Discovery) {
    let (mut stale, _) = authenticate(discovery, discovery.capability);
    let stale_epoch = uuid("018f82d4-6f59-7d06-92e3-bce9131163d9");
    send_process_stream(
        &mut stale,
        &wire::StreamRequest::Subscribe {
            v: wire::WireVersion::CURRENT,
            id: uuid("018f82d4-6f59-7d06-92e3-bce9131163d3"),
            epoch: stale_epoch,
            subjects: vec![wire::ViewSubject::Facts],
            after: None,
        },
        "stale request",
    );
    let (stale_frame, _) = read_process_stream(&mut stale, "stale fault");
    assert!(matches!(
        stale_frame,
        wire::StreamFrame::Fault { ref fault, .. }
            if fault.reason == "stream::epoch-mismatch"
    ));
    stale.close(None).expect("stale socket closes");
}

fn finish_process_stream(
    fixture: &Fixture,
    discovery: &canvas::Discovery,
    resumed: &mut WebSocket<TcpStream>,
    live_cursor: wire::Seq,
    resident: RunningResident,
    capability_text: &[String; 2],
    lane: wire::CanonicalUuid,
) {
    let drained = invoke_args(fixture, &["lane", "drain", &lane.to_string()], true);
    assert!(
        drained.status.success(),
        "lane drain failed: {:?}",
        drained.stderr
    );
    let down = invoke(fixture, "down");
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    let mut last = live_cursor;
    let mut closed_at = None;
    for _ in 0..16 {
        let (frame, _) = read_process_stream(resumed, "shutdown stream frame");
        match frame {
            wire::StreamFrame::Fact { fact, .. } => {
                assert!(fact.seq() > last, "shutdown facts stay monotone");
                last = fact.seq();
            }
            wire::StreamFrame::Closing { after, .. } => {
                assert_eq!(after.epoch, discovery.epoch);
                closed_at = Some(after.seq);
                break;
            }
            other => panic!("unexpected shutdown stream frame: {other:?}"),
        }
    }
    assert_eq!(closed_at, Some(last));
    assert!(
        last > live_cursor,
        "shutdown cursor includes lifecycle facts"
    );
    let up = resident.wait();
    assert!(up.status.success(), "resident failed: {:?}", up.stderr);
    for credential in capability_text {
        assert!(!String::from_utf8_lossy(&up.stdout).contains(credential));
        assert!(!String::from_utf8_lossy(&up.stderr).contains(credential));
    }
}

#[test]
fn resident_process_streams_snapshot_bus_update_thread_resume_stale_and_shutdown_boundaries() {
    let fixture = Fixture::new("canvas-stream");
    super::lane_process::configure_fake_vendor(&fixture);
    let mut initial = RunningResident::spawn(&fixture);
    initial.assert_ready(&fixture);
    let digest = trust_pended(&fixture)
        .into_iter()
        .next()
        .expect("startup trust pends")
        .digest;
    let granted = invoke_args(&fixture, &["trust", "grant", digest.as_str()], true);
    assert!(
        granted.status.success(),
        "initial trust grant failed: {}",
        String::from_utf8_lossy(&granted.stderr)
    );
    let down = invoke(&fixture, "down");
    assert!(
        down.status.success(),
        "initial down failed: {:?}",
        down.stderr
    );
    assert!(initial.wait().status.success(), "initial resident failed");

    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let discovery = fixture.discovery();
    let (mut socket, subscription, initial_horizon, initial_bytes) =
        open_process_stream(&discovery);
    let (live_cursor, capability_text, lane) = seat_and_close_process_stream(
        &fixture,
        &discovery,
        &mut socket,
        subscription,
        initial_horizon,
    );
    for credential in &capability_text {
        assert!(!String::from_utf8_lossy(&initial_bytes).contains(credential));
    }
    let mut resumed = resume_process_stream(&discovery, live_cursor);
    assert_stale_process_stream(&discovery);
    finish_process_stream(
        &fixture,
        &discovery,
        &mut resumed,
        live_cursor,
        resident,
        &capability_text,
        lane,
    );
}

/// An idle resident is inert whether or not Canvas has an admitted subscriber.
///
/// The first interval has no socket at all. The second crosses the real
/// authenticated WebSocket and subscribes to the durable fact stream. A Ping is
/// then the one external event: it produces exactly one Pong, no durable fact,
/// and no refresh or follow-on frame over the measured interval. This pins the
/// liveness request to the transport boundary instead of letting it feed the
/// journal broadcast back into itself.
#[test]
fn resident_and_connected_canvas_stay_quiet_and_one_probe_does_not_amplify() {
    let fixture = Fixture::new("canvas-quietness");
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    let disconnected = resident_facts(&fixture);
    std::thread::sleep(QUIET_INTERVAL);
    assert_eq!(
        resident_facts(&fixture),
        disconnected,
        "a disconnected idle resident appended durable facts"
    );

    let discovery = fixture.discovery();
    let (mut socket, _subscription, _, _) = open_process_stream(&discovery);
    let connected = resident_facts(&fixture);
    let probe = uuid("018f82d4-6f59-7d06-92e3-bce9131163f0");
    send_process_stream(
        &mut socket,
        &wire::StreamRequest::Ping {
            v: wire::WireVersion::CURRENT,
            id: probe,
            epoch: discovery.epoch,
        },
        "quietness probe",
    );
    assert!(matches!(
        read_process_stream(&mut socket, "quietness Pong").0,
        wire::StreamFrame::Pong { id, epoch, .. }
            if id == probe && epoch == discovery.epoch
    ));
    assert_process_stream_silent(&mut socket);
    assert_eq!(
        resident_facts(&fixture),
        connected,
        "a connected Canvas probe appended durable facts"
    );

    drop(socket);
    let down = invoke(&fixture, "down");
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
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
    assert_eq!(refusal.reason(), "resident::already-up");
    assert_eq!(
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed),
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
    assert!(text_elapsed < CLIENT_DEADLINE_BOUND);

    let (json, json_elapsed) = fake_down(&fixture, FakeBehavior::Success, true);
    assert!(json.status.success(), "JSON down failed: {:?}", json.stderr);
    let Outcome::Reply(reply) = outcome(&json) else {
        panic!("JSON down must carry the same successful outcome");
    };
    assert!(matches!(reply.position, ReplyPosition::Committed(_)));
    assert!(json_elapsed < CLIENT_DEADLINE_BOUND);
}

/// A resident that answers `down` and leaves its record published is refused.
///
/// The reply is the same on both halves, so the successful one is what proves
/// the fault is about the record rather than about the exchange: the client
/// waits for the lock either way, and only the surviving discovery file
/// separates them. This is the release side of the same invariant a reader
/// enforces from the outside — a published record must mean a running
/// publisher — checked here where the resident is the one at fault.
#[test]
fn down_refuses_a_resident_that_released_without_withdrawing_discovery() {
    let fixture = Fixture::new("client-release-incomplete");

    let (withdrawn, _) = fake_down(&fixture, FakeBehavior::Success, true);
    assert!(
        withdrawn.status.success(),
        "a withdrawn record must complete down: {:?}",
        withdrawn.stderr
    );
    assert!(
        !fixture.canvas().exists(),
        "the successful half must leave no record behind"
    );

    let (retained, elapsed) = fake_down(&fixture, FakeBehavior::RetainedDiscovery, true);
    assert_eq!(retained.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&retained) else {
        panic!("a record surviving release must fault");
    };
    assert_eq!(fault.reason, "canvas::release-incomplete");
    assert!(
        fixture.canvas().exists(),
        "the fault must name a record that really is still published"
    );
    assert!(elapsed < CLIENT_DEADLINE_BOUND);
}

/// A resident reply cannot leave `down` waiting forever for the runtime lock.
///
/// Both halves withdraw discovery and send the same successful response. The
/// first releases its lock normally and proves this path can complete; the
/// second keeps the lock beyond the client bound and must return the deadline
/// fault rather than waiting for the fixture's longer adversarial lifetime.
#[test]
fn down_bounds_release_confirmation_after_the_resident_answers() {
    let fixture = Fixture::new("client-release-deadline");

    let (released, _) = fake_down(&fixture, FakeBehavior::Success, true);
    assert!(
        released.status.success(),
        "a normally released lock must complete down: {:?}",
        released.stderr
    );

    let (held, elapsed) = fake_down(&fixture, FakeBehavior::ReleaseStall, true);
    assert_eq!(held.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&held) else {
        panic!("a held release lock must fault at the client bound");
    };
    assert_eq!(fault.reason, "canvas::deadline-elapsed");
    let expected_bound = if cfg!(feature = "test-short-horizons") {
        "3s client bound"
    } else {
        "20s client bound"
    };
    assert!(fault.detail.contains(expected_bound), "{fault:?}");
    assert!(
        !fixture.canvas().exists(),
        "the deadline fault must remain distinct from release-incomplete"
    );
    assert!(
        elapsed < CLIENT_DEADLINE_BOUND,
        "release confirmation exceeded its client bound: {elapsed:?}"
    );
}

#[test]
fn authenticated_board_snapshot_survives_a_resident_restart() {
    let fixture = Fixture::new("board-restart");
    let mut first = RunningResident::spawn(&fixture);
    first.assert_ready(&fixture);

    let initial = invoke_args(&fixture, &["board"], true);
    assert!(
        initial.status.success(),
        "board failed: {:?}",
        initial.stderr
    );
    let Outcome::Reply(initial) = value_outcome(&initial) else {
        panic!("board must return one typed reply");
    };
    assert!(matches!(initial.position, ReplyPosition::Snapshot(_)));

    assert!(invoke(&fixture, "down").status.success());
    assert!(first.wait().status.success());
    let mut restarted = RunningResident::spawn(&fixture);
    restarted.assert_ready(&fixture);
    let after_restart = invoke_args(&fixture, &["board"], true);
    assert!(
        after_restart.status.success(),
        "restarted board failed: {:?}",
        after_restart.stderr
    );
    let Outcome::Reply(after_restart) = value_outcome(&after_restart) else {
        panic!("restarted board must return one typed reply");
    };
    assert!(matches!(after_restart.position, ReplyPosition::Snapshot(_)));
    assert_eq!(after_restart.body["schema"], initial.body["schema"]);
    assert_eq!(after_restart.body["principal"], initial.body["principal"]);
    assert_eq!(after_restart.body["tasks"], initial.body["tasks"]);
    assert_ne!(
        after_restart.body["residency"]["body"]["epoch"],
        initial.body["residency"]["body"]["epoch"],
        "restart must publish a new resident generation"
    );
    assert!(
        after_restart.body["cursor"].as_u64() > initial.body["cursor"].as_u64(),
        "restart authority must advance while the empty board remains stable"
    );

    assert!(invoke(&fixture, "down").status.success());
    assert!(restarted.wait().status.success());
}

const FLEET_FACTS: u64 = 8_501;
const FLEET_BYTES: usize = 14_700_000;
const FLEET_HANDSHAKE_BOUND: Duration = Duration::from_secs(3);
const FLEET_STREAM_COMMITS: usize = 12;

fn create_fleet_task(fixture: &Fixture) -> String {
    let mut initial = RunningResident::spawn(&fixture);
    initial.assert_ready(&fixture);
    let added = invoke_args(
        &fixture,
        &[
            "task",
            "oops",
            "Fleet snapshot witness",
            "--origin",
            "fixture:fleet-snapshot",
        ],
        true,
    );
    assert!(
        added.status.success(),
        "task oops failed: {:?}",
        added.stderr
    );
    let Outcome::Reply(added) = value_outcome(&added) else {
        panic!("task oops must return one typed reply");
    };
    let task = added.body["task"]
        .as_str()
        .expect("task add answer names its handle")
        .to_owned();
    let down = invoke(&fixture, "down");
    assert!(
        down.status.success(),
        "initial down failed: {:?}",
        down.stderr
    );
    assert!(initial.wait().status.success(), "initial resident failed");

    task
}

fn fill_fleet_history(fixture: &Fixture, task: &str) -> usize {
    let mut journal = journal::Journal::open(fixture.journal()).expect("fleet journal opens");
    let current = journal
        .read(|read| read.cursor())
        .expect("fleet cursor reads")
        .expect("seeded resident has durable facts");
    assert!(
        current.get() < FLEET_FACTS,
        "fixture already exceeds fleet target"
    );
    let stamp = task
        .split_once('-')
        .expect("task handle has a project stem")
        .1
        .parse::<wire::TaskStamp>()
        .expect("task handle carries its inception stamp");
    let mut write = JournalWrite::new(
        Actor::Operator,
        "task:PERFORM-a1".parse().expect("fleet fixture origin"),
        Instant::new(jiff::Timestamp::now()).expect("fleet fixture instant"),
        Producer::CommandDoor,
    );
    let padding = "x".repeat(1_800);
    let mut cause = FactCause::Root;
    for index in current.get()..FLEET_FACTS {
        let pending = write
            .fact(
                wire::body::TaskNoted {
                    task: stamp.clone(),
                    note: format!("fleet-{index:04}-{padding}"),
                },
                cause,
            )
            .expect("fleet history fact builds");
        cause = FactCause::Pending(pending);
    }
    let applied = journal.apply(write).expect("fleet history commits");
    assert_eq!(applied.last().get(), FLEET_FACTS);
    let snapshot = journal.fact_snapshot().expect("fleet history reads");
    let encoded_bytes = snapshot
        .facts()
        .iter()
        .map(|fact| wire::encode(fact).expect("fleet fact re-encodes").len())
        .sum::<usize>();
    assert_eq!(snapshot.facts().len(), FLEET_FACTS as usize);
    assert!(
        encoded_bytes >= FLEET_BYTES,
        "fleet fixture measured only {encoded_bytes} encoded bytes"
    );
    drop(snapshot);
    drop(journal);

    encoded_bytes
}

fn measure_fleet_client(fixture: &Fixture, task: &str, encoded_bytes: usize) {
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let completed = AtomicU64::new(0);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for index in 0..FLEET_STREAM_COMMITS {
                let note = format!("sustained commit {index}");
                let output = invoke_args(&fixture, &["task", "note", &task, &note], true);
                assert!(
                    output.status.success(),
                    "sustained commit {index} failed: {:?}",
                    output.stderr
                );
                completed.fetch_add(1, Ordering::Release);
            }
        });
        while completed.load(Ordering::Acquire) == 0 {
            std::thread::yield_now();
        }
        let started = Monotonic::now();
        let board = invoke_args(&fixture, &["board"], true);
        let elapsed = started.elapsed();
        assert!(board.status.success(), "board failed: {:?}", board.stderr);
        eprintln!(
            "fleet snapshot measurement: facts={FLEET_FACTS} encoded_bytes={encoded_bytes} \
             concurrent_commits={FLEET_STREAM_COMMITS} new_client={elapsed:?}"
        );
        assert!(
            elapsed < FLEET_HANDSHAKE_BOUND,
            "new client board handshake exceeded the 3 second bound under sustained commits: {elapsed:?}"
        );
    });
    assert_eq!(
        completed.load(Ordering::Acquire),
        FLEET_STREAM_COMMITS as u64
    );
    assert!(
        journal_cursor(&fixture).get() >= FLEET_FACTS + FLEET_STREAM_COMMITS as u64,
        "every sustained commit advances the fleet journal"
    );

    let down = invoke(&fixture, "down");
    assert!(
        down.status.success(),
        "final down failed: {:?}",
        down.stderr
    );
    assert!(resident.wait().status.success(), "final resident failed");
}

#[test]
fn fleet_sized_commit_stream_keeps_new_client_handshakes_inside_three_seconds() {
    let fixture = Fixture::new("fleet-snapshot-latency");
    let task = create_fleet_task(&fixture);
    let encoded_bytes = fill_fleet_history(&fixture, &task);
    measure_fleet_client(&fixture, &task, encoded_bytes);
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
    assert!(elapsed < CLIENT_DEADLINE_BOUND);

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
    assert_eq!(fault.reason, "canvas::deadline-elapsed", "{fault:?}");
    assert!(
        elapsed < CLIENT_DEADLINE_BOUND,
        "read deadline was not bounded: {elapsed:?}"
    );
}

/// A resident that has not answered the upgrade is not a peer that failed.
///
/// Accepting the socket costs the resident nothing, so under load it accepts
/// long before it answers, and the bound elapses here first. The handshake
/// spells that as an interrupted would-block, which is the exact shape the
/// elapsed bound exists to keep apart from a transport that broke.
#[test]
fn command_client_process_names_the_bound_a_stalled_opening_spent() {
    let fixture = Fixture::new("client-opening-stall");
    let (output, elapsed) = fake_down(&fixture, FakeBehavior::OpeningStall, true);

    assert_eq!(output.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&output) else {
        panic!("a stalled opening must fault");
    };
    assert_eq!(fault.reason, "canvas::deadline-elapsed", "{fault:?}");
    assert!(
        elapsed < CLIENT_DEADLINE_BOUND,
        "the opening deadline was not bounded: {elapsed:?}"
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
    assert_eq!(fault.reason, "canvas::deadline-elapsed", "{fault:?}");
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
    assert_eq!(fault.reason, "canvas::deadline-elapsed");
    assert!(
        elapsed < CLIENT_DEADLINE_BOUND,
        "control frames renewed the receive deadline: {elapsed:?}"
    );

    let (output, elapsed) = fake_down(&fixture, FakeBehavior::WriteTimeout, true);
    assert_eq!(output.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&output) else {
        panic!("a saturated automatic control write must fault");
    };
    assert_eq!(fault.reason, "canvas::deadline-elapsed");
    assert!(
        elapsed < CLIENT_DEADLINE_BOUND,
        "control write deadline was not bounded: {elapsed:?}"
    );
}

#[path = "cases/authority.rs"]
mod authority;
