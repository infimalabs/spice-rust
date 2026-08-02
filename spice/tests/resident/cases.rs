//! Resident process cases sharing the integration fixture.
//!
//! Keeping cases below the fixture root makes the process boundary reusable
//! without letting one integration-test source exceed the repository limit.

use super::*;

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

fn open_process_stream(
    discovery: &canvas::Discovery,
    lane_capability: wire::CanonicalUuid,
) -> (
    WebSocket<TcpStream>,
    wire::CanonicalUuid,
    wire::Seq,
    [String; 2],
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
    assert!(packed.rows.iter().any(|fact| {
        fact.kind() == FactKind::LaneSeated && !fact.body().contains_key("capability")
    }));
    assert!(matches!(
        initial_trust,
        wire::StreamFrame::Trust { ref packed, .. }
            if packed.horizon == initial_horizon && packed.rows.len() == 1
    ));
    let capability_text = [
        discovery.capability.to_string(),
        lane_capability.to_string(),
    ];
    for credential in &capability_text {
        assert!(!String::from_utf8_lossy(&facts_bytes).contains(credential));
        assert!(!String::from_utf8_lossy(&trust_bytes).contains(credential));
    }
    (socket, subscription, initial_horizon, capability_text)
}

fn mutate_and_close_process_stream(
    fixture: &Fixture,
    discovery: &canvas::Discovery,
    socket: &mut WebSocket<TcpStream>,
    subscription: wire::CanonicalUuid,
    initial_horizon: wire::Seq,
) -> wire::Seq {
    let granted = invoke_args(fixture, &["task", "add", "Drive the canvas stream"], true);
    assert!(
        granted.status.success(),
        "task mutation failed: {}",
        String::from_utf8_lossy(&granted.stderr)
    );
    let (live, _) = read_process_stream(socket, "live fact");
    let wire::StreamFrame::Fact { fact, id, .. } = live else {
        panic!("Bus commit must produce one live fact");
    };
    assert_eq!(id, subscription);
    assert!(fact.seq() > initial_horizon);
    let live_cursor = fact.seq();
    let (trust_update, _) = read_process_stream(socket, "trust update");
    assert!(matches!(
        trust_update,
        wire::StreamFrame::Trust { packed, .. }
            if packed.horizon == live_cursor
                && !packed.rows.is_empty()
                && packed.rows.iter().all(|row| row.head.state == wire::TrustState::Granted)
    ));

    let thread_id = uuid("018f82d4-6f59-7d06-92e3-bce9131163d1");
    send_process_stream(
        socket,
        &wire::StreamRequest::Thread {
            v: wire::WireVersion::CURRENT,
            id: thread_id,
            epoch: discovery.epoch,
            cursor: wire::StreamCursor {
                epoch: discovery.epoch,
                seq: live_cursor,
            },
        },
        "thread request",
    );
    let (thread, _) = read_process_stream(socket, "thread");
    assert!(matches!(
        thread,
        wire::StreamFrame::Thread { id, ref packed, .. }
            if id == thread_id && packed.rows.first().is_some_and(|row| row.seq() == live_cursor)
    ));
    socket.close(None).expect("first stream closes");
    live_cursor
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
) {
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

    let lane = uuid("00000000-0000-4000-8000-000000000041");
    let lane_capability = uuid("00000000-0000-4000-8000-000000000042");
    seed_lane(&fixture, lane, lane_capability);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let discovery = fixture.discovery();
    let (mut socket, subscription, initial_horizon, capability_text) =
        open_process_stream(&discovery, lane_capability);
    let live_cursor = mutate_and_close_process_stream(
        &fixture,
        &discovery,
        &mut socket,
        subscription,
        initial_horizon,
    );
    let mut resumed = resume_process_stream(&discovery, live_cursor);
    assert_stale_process_stream(&discovery);
    finish_process_stream(
        &fixture,
        &discovery,
        &mut resumed,
        live_cursor,
        resident,
        &capability_text,
    );
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

#[cfg(unix)]
#[test]
fn resident_startup_refuses_linked_policy_components_without_trust_effects() {
    use std::os::unix::fs::symlink;

    for linked in ["tracked-file", "local-parent"] {
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
                symlink(&target, fixture.root.join("spice.toml"))
                    .expect("link tracked policy file");
            }
            "local-parent" => {
                let target = fixture.root.join("outside-local");
                fs::create_dir(&target).expect("create local target directory");
                fs::write(target.join("config.toml"), "# linked local policy\n")
                    .expect("write local target");
                symlink(&target, fixture.root.join(".git/.spice"))
                    .expect("link local policy parent");
            }
            _ => unreachable!("fixture names the closed linked cases"),
        }

        let output = invoke(&fixture, "up");
        assert_eq!(output.status.code(), Some(REFUSAL_EXIT), "{linked}");
        let Outcome::Refusal(refusal) = outcome(&output) else {
            panic!("{linked} startup did not return a typed refusal");
        };
        assert_eq!(refusal.reason, "resident::configuration-refused");
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
        fixture.root.join("spice.toml"),
        "# tracked executable policy v1\n",
    )
    .expect("tracked policy writes");
    let local = fixture.root.join(".git/.spice/config.toml");
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
        fixture.root.join("spice.toml"),
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
        assert_eq!(refusal.reason, "trust::digest-not-current");
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
    assert_eq!(refusal.reason, "trust::standing-moved");
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
