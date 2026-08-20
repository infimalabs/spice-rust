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
    // Every refusal below is about the discovery being untrustworthy, so a
    // publisher holds the runtime lock throughout and claims the generation
    // published below: without a holder the client would refuse because nobody
    // is serving, and without the claim it would refuse because the resident
    // serving is not the one that published. Either way this case would stop
    // testing what it names.
    let epoch = uuid("00000000-0000-4000-8000-000000000042");
    let publisher = fake_publisher(&fixture, epoch);
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
        endpoint: "http://canvas.example:48123".to_owned(),
        capability,
        epoch,
    };
    discovery
        .publish(fixture.canvas())
        .expect("unresolved discovery publishes");
    // Which interface the resident bound is the operator's decision and no
    // longer this client's, but the endpoint still has to be a socket it can
    // dial rather than a name some resolver answers for.
    let unresolved = invoke(&fixture, "down");
    assert_eq!(unresolved.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&unresolved) else {
        panic!("named-host discovery must fault");
    };
    assert_eq!(fault.reason, "canvas::endpoint-invalid");

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

    // The same unanswered endpoint, one variable changed: the publisher is
    // gone. `connect-failed` above is a report about a socket and says nothing
    // about who owns it, which is why a foreign server holding the port earns
    // a connection rather than a refusal. Losing the lock is what makes this
    // record unusable, and the client says so instead of dialling to find out.
    drop(publisher);
    let orphaned = invoke(&fixture, "down");
    assert_eq!(orphaned.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&orphaned) else {
        panic!("discovery outliving its publisher must fault");
    };
    assert_eq!(fault.reason, "canvas::publisher-gone");
    assert!(
        !orphaned
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
    assert_eq!(refusal.reason(), "journal::schema-refused");
    assert!(
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed)
            .is_some_and(|observed| observed.contains("prepared and unfinalized")),
        "refusal omitted the exact schema state: {refusal:?}",
    );
}

#[cfg(unix)]
#[test]
fn resident_startup_refuses_linked_policy_components_without_trust_effects() {
    use std::os::unix::fs::symlink;

    for linked in ["tracked-file", "local-file"] {
        let fixture = Fixture::new(linked);
        // Seed authority first so startup refusal can be proved byte-for-byte inert.
        fs::create_dir_all(fixture.journal().parent().expect("journal data parent"))
            .expect("create journal data parent");
        drop(journal::Journal::open(fixture.journal()).expect("baseline journal opens"));
        let before = fs::read(fixture.journal()).expect("baseline journal bytes");
        match linked {
            "tracked-file" => {
                let target = fixture.root.join("outside-tracked.toml");
                fs::write(&target, "# linked tracked policy\n").expect("write tracked target");
                symlink(&target, fixture.root.join("spice-rust.toml"))
                    .expect("link tracked policy file");
            }
            "local-file" => {
                let target = fixture.root.join("outside-local");
                fs::write(&target, "# linked local policy\n").expect("write local target");
                symlink(&target, fixture.root.join(".git/.spice-rs/config.toml"))
                    .expect("link local policy file");
            }
            _ => unreachable!("fixture names the closed linked cases"),
        }

        let output = invoke(&fixture, "up");
        assert_eq!(output.status.code(), Some(REFUSAL_EXIT), "{linked}");
        let Outcome::Refusal(refusal) = outcome(&output) else {
            panic!("{linked} startup did not return a typed refusal");
        };
        assert_eq!(refusal.reason(), "cli::mount-registry-refused");
        assert_eq!(
            fs::read(fixture.journal()).expect("journal bytes after refusal"),
            before,
            "{linked} refusal changed authority"
        );
        let snapshot = journal::JournalReader::open(fixture.journal())
            .expect("refused authority opens")
            .fact_snapshot()
            .expect("refused authority reads");
        assert!(
            snapshot.facts().iter().all(|fact| !matches!(
                fact.fact.kind(),
                FactKind::TrustPended | FactKind::UpSeated
            ))
        );
    }
}

#[test]
fn operator_trust_process_binds_policy_bytes_across_reconnect_and_restart() {
    let fixture = trust_fixture();
    let mut first = RunningResident::spawn(&fixture);
    first.assert_ready(&fixture);
    let first_digest = assert_initial_trust_pending(&fixture);
    grant_current_trust(&fixture, &first_digest);
    let restarted = restart_with_same_trust(&fixture, first, &first_digest);
    let (changed, changed_digest) = start_changed_trust(&fixture, restarted, &first_digest);
    assert_noncurrent_trust_refusals(&fixture, &first_digest);
    grant_and_revoke_changed_trust(&fixture, &changed_digest);
    assert!(invoke(&fixture, "down").status.success());
    assert!(changed.wait().status.success());
    journal::JournalReader::open(fixture.journal())
        .expect("final authority opens")
        .verify()
        .expect("final trust authority verifies");
}

fn trust_fixture() -> Fixture {
    let fixture = Fixture::new("trust-lifecycle");
    fs::write(
        fixture.root.join("spice-rust.toml"),
        "# tracked executable policy v1\n",
    )
    .expect("tracked policy writes");
    let local = fixture.root.join(".git/.spice-rs/config.toml");
    fs::create_dir_all(local.parent().expect("local policy parent"))
        .expect("local policy directory");
    fs::write(&local, "# local executable policy v1\n").expect("local policy writes");
    fixture
}

fn assert_initial_trust_pending(fixture: &Fixture) -> String {
    let pends = trust_pended(fixture);
    assert_eq!(pends.len(), 1, "startup pends exactly once");
    assert_eq!(pends[0].surface, "configuration");
    assert_eq!(pends[0].previous, None);
    let report = invoke_args(fixture, &["trust"], false);
    assert!(
        report.status.success(),
        "text trust failed: {:?}",
        report.stderr
    );
    assert!(report.stderr.is_empty());
    let body: serde_json::Value =
        serde_json::from_slice(&report.stdout).expect("text trust body is canonical JSON");
    assert_eq!(body["standing"], "pended");
    assert_eq!(body["provenance"].as_array().map(Vec::len), Some(2));
    assert_eq!(body["provenance"][0]["source"], "tracked");
    assert_eq!(body["provenance"][1]["source"], "local");
    assert_eq!(body["provenance"][0]["present"], true);
    assert_eq!(body["provenance"][1]["present"], true);
    let digest = body["digest"]
        .as_str()
        .expect("trust report names its current digest")
        .to_owned();
    assert_eq!(digest, pends[0].digest.as_str());
    digest
}

fn grant_current_trust(fixture: &Fixture, digest: &str) {
    let granted = invoke_args(fixture, &["trust", "grant", digest], true);
    assert!(
        granted.status.success(),
        "JSON grant failed: {:?}",
        granted.stderr
    );
    let Outcome::Reply(reply) = value_outcome(&granted) else {
        panic!("grant must return one typed reply");
    };
    assert!(matches!(reply.position, ReplyPosition::Committed(_)));
    assert_eq!(reply.body["digest"], digest);
    assert_eq!(reply.body["standing"], "granted");
    let before = journal_cursor(fixture);
    let duplicate = invoke_args(fixture, &["trust", "grant", digest], false);
    assert_eq!(duplicate.status.code(), Some(REFUSAL_EXIT));
    assert_eq!(
        duplicate.stderr,
        concat!(
            "spice: trust::standing-moved; re-read the current trust report ",
            "before issuing another ruling\n"
        )
        .as_bytes()
    );
    assert_eq!(journal_cursor(fixture), before);
    let report = invoke_args(fixture, &["trust"], true);
    let Outcome::Reply(reply) = value_outcome(&report) else {
        panic!("JSON trust must return one typed reply");
    };
    assert!(matches!(reply.position, ReplyPosition::Snapshot(_)));
    assert_eq!(reply.body["standing"], "granted");
}

fn restart_with_same_trust(
    fixture: &Fixture,
    resident: RunningResident,
    digest: &str,
) -> RunningResident {
    assert!(invoke(fixture, "down").status.success());
    assert!(resident.wait().status.success());
    let pends_before = trust_pended(fixture);
    let mut restarted = RunningResident::spawn(fixture);
    restarted.assert_ready(fixture);
    let report = invoke_args(fixture, &["trust"], true);
    let Outcome::Reply(reply) = value_outcome(&report) else {
        panic!("restarted trust must return one typed reply");
    };
    assert_eq!(reply.body["digest"], digest);
    assert_eq!(reply.body["standing"], "granted");
    assert_eq!(
        trust_pended(fixture),
        pends_before,
        "restart cannot duplicate an existing digest pend"
    );
    restarted
}

fn start_changed_trust(
    fixture: &Fixture,
    resident: RunningResident,
    previous: &str,
) -> (RunningResident, String) {
    assert!(invoke(fixture, "down").status.success());
    assert!(resident.wait().status.success());
    fs::write(
        fixture.root.join("spice-rust.toml"),
        "# tracked executable policy v2\n",
    )
    .expect("tracked policy changes");
    let mut changed = RunningResident::spawn(fixture);
    changed.assert_ready(fixture);
    let pends = trust_pended(fixture);
    assert_eq!(pends.len(), 2, "changed bytes append one new pend");
    assert_eq!(
        pends[1].previous.as_ref().map(wire::Digest::as_str),
        Some(previous)
    );
    let report = invoke_args(fixture, &["trust"], true);
    let Outcome::Reply(reply) = value_outcome(&report) else {
        panic!("changed trust must return one typed reply");
    };
    assert_eq!(reply.body["standing"], "pended");
    let digest = reply.body["digest"]
        .as_str()
        .expect("changed report names current digest")
        .to_owned();
    assert_ne!(digest, previous);
    (changed, digest)
}

fn assert_noncurrent_trust_refusals(fixture: &Fixture, stale: &str) {
    let unknown =
        wire::Digest::compute(wire::Domain::ConfigExecutableSurface, b"unknown").to_string();
    let cross = wire::Digest::compute(wire::Domain::DistArtifact, b"cross-domain").to_string();
    for (label, digest) in [
        ("stale", stale),
        ("unknown", unknown.as_str()),
        ("cross-domain", cross.as_str()),
    ] {
        let before = journal_cursor(fixture);
        let refused = invoke_args(fixture, &["trust", "grant", digest], true);
        assert_eq!(
            refused.status.code(),
            Some(REFUSAL_EXIT),
            "{label} digest did not refuse"
        );
        let Outcome::Refusal(refusal) = value_outcome(&refused) else {
            panic!("{label} digest must return a typed refusal");
        };
        assert_eq!(refusal.reason(), "trust::digest-not-current");
        assert_eq!(journal_cursor(fixture), before, "{label} changed authority");
    }
}

fn grant_and_revoke_changed_trust(fixture: &Fixture, digest: &str) {
    let grant = invoke_args(fixture, &["trust", "grant", digest], false);
    assert!(
        grant.status.success(),
        "text grant failed: {:?}",
        grant.stderr
    );
    let grant_body: serde_json::Value =
        serde_json::from_slice(&grant.stdout).expect("text grant body is JSON");
    assert_eq!(grant_body["standing"], "granted");
    let revoked = invoke_args(fixture, &["trust", "revoke", digest], true);
    assert!(
        revoked.status.success(),
        "JSON revoke failed: {:?}",
        revoked.stderr
    );
    let Outcome::Reply(reply) = value_outcome(&revoked) else {
        panic!("revoke must return one typed reply");
    };
    assert_eq!(reply.body["standing"], "revoked");
    let before = journal_cursor(fixture);
    let duplicate = invoke_args(fixture, &["trust", "revoke", digest], true);
    assert_eq!(duplicate.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) = value_outcome(&duplicate) else {
        panic!("duplicate revoke must return one typed refusal");
    };
    assert_eq!(refusal.reason(), "trust::standing-moved");
    assert_eq!(journal_cursor(fixture), before);
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
    let observed = refusal
        .diagnostic()
        .and_then(wire::RefusalDiagnostic::observed)
        .expect("holder identity");
    assert!(
        observed.contains("pid="),
        "holder names its process: {observed}"
    );
    assert!(
        observed.contains(&format!("epoch={}", discovery.epoch)),
        "holder names its generation: {observed}"
    );

    // A generation the holder does not claim is refused where it is read, not
    // where it is presented. The live resident would reject this Hello as
    // `hello::epoch-mismatch` and still does — `canvas/src/axum/tests/
    // lifecycle.rs` proves that against the server itself — but the client no
    // longer travels far enough to be told, because reaching that answer means
    // handing a capability to whoever is listening first. The holder line
    // asserted above is what makes the earlier answer possible: it names the
    // generation this record fails to match.
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
    assert_eq!(fault.reason, "canvas::publisher-replaced");
    assert!(
        !rejected
            .stderr
            .windows(discovery.capability.to_string().len())
            .any(|window| window == discovery.capability.to_string().as_bytes())
    );

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
    assert_eq!(refusal.reason(), "hello::capability-unknown");
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
    assert_eq!(refusal.reason(), "hello::capability-unknown");
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

/// The resident resolves exactly one lane-built generation from its worktree;
/// an incomplete generation stops it before the endpoint is published.
///
/// This is the half of the lane app loop that lives outside `canvas`: that
/// the source is resolved at all, and against this worktree rather than some
/// other directory. A resident that skipped the resolve would reach readiness
/// here, and one that looked elsewhere would never see this stage.
#[test]
fn resident_process_refuses_an_app_stage_that_cannot_supply_the_whole_app() {
    let fixture = Fixture::new("app-source");
    let stage = fixture.root.join(canvas::APP_STAGE);
    fs::remove_dir_all(&stage).expect("remove complete fixture app");
    let identity = "5555555555555555555555555555555555555555555555555555555555555555";
    let generation = stage.join(canvas::APP_STAGE_GENERATIONS).join(identity);
    fs::create_dir_all(&generation).expect("fixture generation");
    fs::write(
        generation.join("canvas-app.html"),
        b"locally built canvas-app.html",
    )
    .expect("fixture asset");
    fs::write(
        stage.join(canvas::APP_STAGE_CURRENT),
        format!("{identity}\n"),
    )
    .expect("fixture pointer");

    let mut resident = RunningResident::spawn(&fixture);
    // Bounded rather than a bare wait: a resident that never resolved this
    // stage stays up forever, and that has to read as a failure here rather
    // than as a battery that hangs.
    fs::create_dir_all(&fixture.runtime).expect("runtime directory");
    let ending = readiness::wait_until_ready(
        resident.child.as_mut().expect("resident child"),
        &fixture.runtime,
        readiness::ARRIVAL_HORIZON,
        "resident refusal of a partial lane app stage",
        || false,
    );
    assert!(
        matches!(ending, Readiness::Exited),
        "the resident served on with a stage it cannot serve the whole app from"
    );
    let up = resident.wait();

    assert_eq!(up.status.code(), Some(FAULT_EXIT));
    let refusal = outcome(&up);
    let Outcome::Fault(fault) = &refusal else {
        panic!("a partial app stage answered {refusal:?}");
    };
    let detail = &fault.detail;
    assert_eq!(fault.reason, "canvas::app-source-invalid");
    for missing in [
        "app.module",
        "style.css",
        "mosaic.css",
        "responsive.css",
        "app_bg.wasm",
    ] {
        assert!(
            detail.contains(missing),
            "the refusal does not name {missing}: {detail}"
        );
    }
    assert!(
        detail.contains("cargo xtask app"),
        "the refusal does not say how to build the stage: {detail}"
    );
    assert!(
        !fixture.canvas().is_file(),
        "the endpoint was published for an app the resident cannot serve"
    );
}

#[test]
fn resident_text_startup_names_the_exact_lane_app_source() {
    let lane = Fixture::new("app-announcement");
    let stage = lane.root.join(canvas::APP_STAGE);
    let mut resident = RunningResident::spawn_text(&lane);
    resident.assert_ready(&lane);
    let published = lane.discovery().endpoint;
    let announced_endpoint = resident.read_stdout_line();
    assert_eq!(
        announced_endpoint,
        format!("canvas endpoint: {published}\n"),
        "the bound endpoint must reach stdout before shutdown"
    );
    let down = invoke(&lane, "down");
    assert!(down.status.success(), "lane resident down failed");
    let output = resident.wait();
    assert!(output.status.success(), "lane resident failed");
    let stage = fs::canonicalize(&stage).expect("lane app stage canonicalizes");
    let announced_source = String::from_utf8(output.stderr).expect("lane source is UTF-8");
    assert_eq!(
        announced_source,
        format!("canvas app source: lane app at {}\n", stage.display())
    );
    assert_eq!(output.stdout, b"null\n");
    // And that published endpoint names the port the kernel handed out. Both
    // sides of the endpoint equality are the resident's own, so on its own it
    // would still hold if the resident published the address it asked for:
    // this fixture starts with `--port 0`, which is unreachable.
    let port = published
        .strip_prefix("http://127.0.0.1:")
        .unwrap_or_else(|| panic!("the resident publishes a loopback endpoint: {published:?}"));
    assert!(
        port.parse::<u16>().is_ok_and(|port| port != 0),
        "the announced endpoint must name the bound port, not the requested one: {port:?}"
    );
}

#[test]
fn resident_text_startup_reports_an_explicit_bound_endpoint_on_stdout() {
    let lane = Fixture::new("explicit-endpoint-announcement");
    let reservation = TcpListener::bind(("127.0.0.1", 0)).expect("reserve an explicit port");
    let listen = reservation.local_addr().expect("reserved address");
    drop(reservation);

    let mut resident = RunningResident::spawn_text_at(&lane, listen);
    resident.assert_ready(&lane);
    let published = lane.discovery().endpoint;
    assert_eq!(published, format!("http://{listen}"));
    assert_eq!(
        resident.read_stdout_line(),
        format!("canvas endpoint: {published}\n")
    );

    let down = invoke(&lane, "down");
    assert!(down.status.success(), "lane resident down failed");
    assert!(resident.wait().status.success(), "lane resident failed");
}

#[test]
fn resident_authenticated_startup_announces_a_fragment_bearing_opening() {
    const TOKEN: &str = "correct-horse-battery-staple";

    let lane = Fixture::new("authenticated-endpoint-announcement");
    let mut resident = RunningResident::spawn_text_authenticated(&lane, TOKEN);
    resident.assert_ready(&lane);
    let published = lane.discovery().endpoint;
    assert_eq!(
        resident.read_stdout_line(),
        format!("canvas endpoint: {published}#{TOKEN}\n")
    );

    let down = invoke(&lane, "down");
    assert!(down.status.success(), "authenticated resident down failed");
    assert!(resident.wait().status.success(), "resident failed");
}
