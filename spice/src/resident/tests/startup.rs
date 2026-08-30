//! Resident startup, access, path, and lock regressions split from the root suite.

use super::*;

fn listen_for(argv: &[&str]) -> CommandResult<SocketAddr> {
    let matches = spice::cli::command()
        .try_get_matches_from(argv)
        .expect("declared up operands parse");
    requested_listen(spice::cli::selected_matches(&matches))
}

fn listen_refusal_for(argv: &[&str]) -> String {
    match listen_for(argv) {
        Ok(listen) => panic!("{argv:?} must not bind {listen}"),
        Err(Outcome::Refusal(refusal)) => refusal.reason().to_owned(),
        Err(other) => panic!("{argv:?} must refuse rather than fault: {other:?}"),
    }
}

fn access_for(argv: &[&str]) -> CommandResult<()> {
    let matches = spice::cli::command()
        .try_get_matches_from(argv)
        .expect("declared up operands parse");
    let leaf = spice::cli::selected_matches(&matches);
    let listen = requested_listen(leaf)?;
    requested_access(leaf, listen).map(|_| ())
}

fn access_refusal(argv: &[&str]) -> Refusal {
    match access_for(argv) {
        Ok(()) => panic!("{argv:?} must not admit access"),
        Err(Outcome::Refusal(refusal)) => refusal,
        Err(other) => panic!("{argv:?} must refuse rather than fault: {other:?}"),
    }
}

fn access_refusal_for(argv: &[&str]) -> String {
    access_refusal(argv).reason().to_owned()
}

struct NoopCaptureReporter;

impl lane::capture::CaptureReporter for NoopCaptureReporter {
    fn record(
        &self,
        _record: lane::capture::CaptureRecord,
    ) -> Result<(), lane::capture::CaptureReportError> {
        Ok(())
    }

    fn failed(&self, _failure: lane::capture::CaptureFailure) {}

    fn exited(&self, _exit: lane::capture::CaptureExit) {}
}

fn stage_canvas_app(root: &std::path::Path) {
    let identity = wire::Digest::compute(wire::Domain::DistArtifact, b"resident-startup-app");
    let stage = root.join(canvas::APP_STAGE);
    let generation = stage
        .join(canvas::APP_STAGE_GENERATIONS)
        .join(identity.as_str());
    fs::create_dir_all(&generation).expect("Canvas generation directory");
    for asset in canvas::APP.assets() {
        fs::write(
            generation.join(asset.file()),
            format!("fixture {}", asset.file()),
        )
        .expect("Canvas fixture asset writes");
    }
    fs::write(
        stage.join(canvas::APP_STAGE_CURRENT),
        format!("{identity}\n"),
    )
    .expect("Canvas generation publishes");
}

fn admitted_lane_seat(
    epoch: CanonicalUuid,
    body: &wire::intent::LaneSeat,
) -> wire::AdmittedCommand {
    Command {
        v: wire::WireVersion::CURRENT,
        id: test_uuid(70),
        epoch,
        kind: CommandKind::LaneSeat,
        expected: Vec::new(),
        body: serde_json::to_value(body).expect("lane seat command body encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch,
    })
    .expect("operator lane seat command admits")
}

fn send_socket<T: serde::Serialize>(
    socket: &mut tungstenite::WebSocket<std::net::TcpStream>,
    value: &T,
) {
    socket
        .send(tungstenite::Message::Binary(
            wire::encode(value).expect("fixture frame encodes").into(),
        ))
        .expect("fixture frame sends");
}

fn connect_operator_socket(
    address: SocketAddr,
    operator: CanonicalUuid,
    epoch: CanonicalUuid,
) -> tungstenite::WebSocket<std::net::TcpStream> {
    use tungstenite::client::IntoClientRequest;
    use tungstenite::http::header::ORIGIN;

    let stream = std::net::TcpStream::connect(address).expect("client connects");
    let mut request = format!("ws://{address}/socket")
        .into_client_request()
        .expect("socket request builds");
    request.headers_mut().insert(
        ORIGIN,
        format!("http://{address}").parse().expect("origin builds"),
    );
    let (mut socket, _) = tungstenite::client(request, stream).expect("socket opens");
    send_socket(
        &mut socket,
        &wire::Hello {
            wire: wire::WireVersion::CURRENT,
            capability: operator,
            epoch,
        },
    );
    let accepted = socket.read().expect("accepted frame").into_data();
    let accepted: wire::Accepted = wire::decode(&accepted).expect("accepted decodes");
    assert_eq!(accepted.actor, Actor::Operator);
    socket
}

fn send_lane_read(
    socket: &mut tungstenite::WebSocket<std::net::TcpStream>,
    command_id: CanonicalUuid,
    epoch: CanonicalUuid,
) {
    send_socket(
        socket,
        &Command {
            v: wire::WireVersion::CURRENT,
            id: command_id,
            epoch,
            kind: CommandKind::LaneRead,
            expected: Vec::new(),
            body: serde_json::to_value(wire::intent::LaneRead {})
                .expect("lane read command encodes"),
        },
    );
}

fn send_steer_send(
    socket: &mut tungstenite::WebSocket<std::net::TcpStream>,
    command_id: CanonicalUuid,
    epoch: CanonicalUuid,
    fanout: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    message: String,
) {
    let body = wire::intent::SteerSend {
        fanout,
        lane,
        message,
        attachments: Vec::new(),
    };
    send_socket(
        socket,
        &Command {
            v: wire::WireVersion::CURRENT,
            id: command_id,
            epoch,
            kind: CommandKind::SteerSend,
            expected: vec![Witness::Residency {
                lane,
                occupant,
                epoch,
            }],
            body: serde_json::to_value(body).expect("steering send command encodes"),
        },
    );
}

fn seed_socket_fanout_lanes(
    bus: &Bus,
    epoch: CanonicalUuid,
    lineage: CanonicalUuid,
    root: &std::path::Path,
) -> [(CanonicalUuid, CanonicalUuid); 3] {
    let lanes = [
        (test_uuid(94), test_uuid(95)),
        (test_uuid(96), test_uuid(97)),
        (test_uuid(98), test_uuid(99)),
    ];
    for (lane, occupant) in lanes {
        seed_lane_in_lineage(bus, epoch, lane, lineage, occupant, root);
    }
    lanes
}

fn assert_lane_read_response(
    socket: &mut tungstenite::WebSocket<std::net::TcpStream>,
    command_id: CanonicalUuid,
) {
    let response = socket.read().expect("response frame").into_data();
    let response: wire::Response<serde_json::Value> =
        wire::decode(&response).expect("response decodes");
    assert_eq!(response.id, command_id);
    assert!(matches!(response.outcome, Outcome::Reply(_)));
}

fn assert_steer_send_response(
    socket: &mut tungstenite::WebSocket<std::net::TcpStream>,
    command_id: CanonicalUuid,
) {
    let response = socket.read().expect("response frame").into_data();
    let response: wire::Response<serde_json::Value> =
        wire::decode(&response).expect("response decodes");
    assert_eq!(response.id, command_id);
    let Outcome::Reply(reply) = response.outcome else {
        panic!("steering send must reply: {response:?}");
    };
    assert_eq!(
        reply.body.get("disposition"),
        Some(&serde_json::json!("live")),
        "directive remains live after socket dispatch"
    );
    assert_eq!(
        reply.body.pointer("/delivery/standing"),
        Some(&serde_json::json!("published")),
        "socket dispatch publishes the selected lane inbox before replying"
    );
}

fn assert_socket_fanout_published(
    bus: &Bus,
    paths: &RuntimePaths,
    fanout: CanonicalUuid,
    lanes: [(CanonicalUuid, CanonicalUuid); 3],
) {
    let snapshot = bus.fact_snapshot().expect("fanout snapshot reads");
    let sent = snapshot
        .facts()
        .iter()
        .filter(|fact| fact.fact.kind() == wire::FactKind::SteerSent)
        .collect::<Vec<_>>();
    assert_eq!(sent.len(), 3);
    assert!(
        sent.iter()
            .all(|fact| fact.fact.body().get("fanout") == Some(&serde_json::json!(fanout))),
        "one socket-submitted fanout stays attached to every target-specific directive"
    );
    for (lane, _) in lanes {
        let inbox = fs::read_to_string(steer::inbox::path(&paths.inbox, lane))
            .expect("selected lane inbox is published");
        assert!(inbox.contains("BROADCAST: socket delivery to lane"));
        let fold = steer::fold::read(&paths.data, lane)
            .expect("fold progress reads")
            .expect("fold progress was published");
        assert!(matches!(
            fold.delivery(),
            wire::intent::DeliveryStanding::Published { .. }
        ));
    }
}

struct SocketFanoutDispatch<'a> {
    bus: &'a Bus,
    repository: &'a Repository,
    paths: &'a RuntimePaths,
    config: &'a runtime::SurfaceConfig,
    capabilities: &'a canvas::axum::Capabilities,
    discovery: &'a canvas::Discovery,
    epoch: CanonicalUuid,
}

impl SocketFanoutDispatch<'_> {
    async fn run(
        self,
        control: &mut canvas::axum::Control,
        lanes: [(CanonicalUuid, CanonicalUuid); 3],
    ) {
        let reads =
            read_materializations::CurrentReads::start(self.bus, self.paths, &self.config.consent)
                .expect("read materializations start");
        let reporter = Arc::new(NoopCaptureReporter);
        let command_surface = command_surface::Surface::new(
            self.bus,
            self.repository,
            self.paths,
            self.epoch,
            command_surface::Runtimes::new(
                lane_surface::Runtime::new(self.capabilities, self.discovery, reporter.clone()),
                lane_recovery::Runtime::new(reporter),
            ),
            self.config,
            &reads,
        );
        let mut seats = Seats::new();
        let mut lifecycle = lifecycle::Runtime::new(&self.config.lane);
        for (index, _) in lanes.iter().enumerate() {
            let request = tokio::time::timeout(Duration::from_secs(3), control.recv())
                .await
                .expect("control request arrives before socket dispatch stalls")
                .expect("control receives command");
            assert_eq!(
                request.command().id(),
                test_uuid(84 + u8::try_from(index).expect("fixture index fits"))
            );
            assert!(
                !runtime::dispatch_control(
                    &command_surface,
                    &mut seats,
                    &mut lifecycle,
                    control,
                    Some(request),
                )
                .await
                .expect("socket steering dispatch completes")
                .reached_dark,
                "steering send must not request resident shutdown"
            );
        }
    }
}

async fn stop_canvas_fixture(
    shutdown: tokio::sync::oneshot::Sender<()>,
    server_task: tokio::task::JoinHandle<Result<(), std::io::Error>>,
) {
    shutdown.send(()).expect("shutdown signals");
    server_task
        .await
        .expect("server task joins")
        .expect("server stops");
}

/// The surface listens exactly where the operator requested or at one stable default.
///
/// Both halves are load-bearing. An operator reaching this surface from another
/// machine has to be able to say which address it answers on, and an operator
/// who says nothing has to get the same URL back after a restart: an ephemeral
/// port would hand them a new address every time, which no bookmark, open tab,
/// or written-down instruction survives.
#[test]
fn the_listen_address_is_what_was_asked_for_or_one_stable_default() {
    assert_eq!(listen_for(&["spice", "up"]), Ok(DEFAULT_LISTEN));
    assert_eq!(DEFAULT_LISTEN.port(), 8767);
    assert_ne!(
        DEFAULT_LISTEN.port(),
        0,
        "an ephemeral default port changes the URL on every restart"
    );

    assert_eq!(
        listen_for(&["spice", "up", "--host", "0.0.0.0"]),
        Ok(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            DEFAULT_LISTEN.port()
        )),
        "a named host keeps the default port"
    );
    assert_eq!(
        listen_for(&["spice", "up", "--port", "9000"]),
        Ok(SocketAddr::new(DEFAULT_LISTEN.ip(), 9000)),
        "a named port keeps the default host"
    );
    assert_eq!(
        listen_for(&["spice", "up", "--host", "10.0.0.5", "--port", "9000"]),
        Ok("10.0.0.5:9000".parse().expect("fixture address")),
        "both named is bound verbatim"
    );
}

/// An address this machine cannot listen on is named rather than approximated.
///
/// Resolving a host name here would bind whatever the resolver answered with,
/// which is not what the operator asked for and not something they could check
/// afterwards, so a name is refused instead of guessed at.
#[test]
fn an_unbindable_listen_request_is_refused_by_name() {
    assert_eq!(
        listen_refusal_for(&["spice", "up", "--host", "canvas.example"]),
        "resident::host-invalid"
    );
    assert_eq!(
        listen_refusal_for(&["spice", "up", "--port", "70000"]),
        "resident::port-invalid"
    );
    assert_eq!(
        listen_refusal_for(&["spice", "up", "--port", "http"]),
        "resident::port-invalid"
    );
}

#[test]
fn remote_listening_requires_explicit_consent_and_validates_an_optional_token() {
    assert_eq!(
        access_refusal_for(&["spice", "up", "--host", "0.0.0.0"]),
        "resident::insecure-bind-consent-required"
    );
    assert!(
        access_for(&["spice", "up", "--host", "0.0.0.0", "--allow-insecure-bind"]).is_ok(),
        "explicit insecure consent admits a network-protected remote bind"
    );
    let invalid = access_refusal(&[
        "spice",
        "up",
        "--host",
        "0.0.0.0",
        "--allow-insecure-bind",
        "--auth-token",
        "too-short-token",
    ]);
    assert_eq!(invalid.reason(), "resident::auth-token-invalid");
    assert_eq!(
        invalid
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed),
        None,
        "a refused credential is never echoed"
    );
    assert!(
        access_for(&[
            "spice",
            "up",
            "--host",
            "0.0.0.0",
            "--allow-insecure-bind",
            "--auth-token",
            "correct-horse-battery-staple",
        ])
        .is_ok(),
        "explicit consent and a caller-held token admit the remote bind"
    );
    assert!(
        access_for(&["spice", "up"]).is_ok(),
        "the stable loopback default remains frictionless"
    );
}

/// `::1` is loopback here and at the listener, because one predicate says so.
///
/// This gate and the canvas one both ask [`std::net::IpAddr::is_loopback`]. A
/// narrowing on either side would split one decision in two: this side would
/// admit `--host ::1` with no token, and the listener would refuse the address
/// it had just admitted, telling the operator a machine-local bind is remote.
///
/// The wildcard cases sit beside the loopback one deliberately. An `is_ok` on
/// `::1` alone passes just as well against a gate that admits everything, and
/// `::` is the address that a rule reading only `Ipv4Addr` would wave through.
#[test]
fn ipv6_loopback_needs_no_consent_while_its_wildcard_needs_explicit_consent() {
    assert!(
        access_for(&["spice", "up", "--host", "::1"]).is_ok(),
        "IPv6 loopback is reachable from this machine alone"
    );

    assert_eq!(
        access_refusal_for(&["spice", "up", "--host", "::"]),
        "resident::insecure-bind-consent-required"
    );
    assert!(
        access_for(&["spice", "up", "--host", "::", "--allow-insecure-bind"]).is_ok(),
        "explicit insecure consent admits the IPv6 wildcard"
    );
}

#[test]
fn runtime_paths_follow_the_repository_common_directory() {
    let root = fixture("main");
    let nested = root.join("a/b");
    fs::create_dir_all(root.join(".git")).expect("git directory");
    fs::create_dir_all(&nested).expect("nested directory");

    let paths = RuntimePaths::discover(&nested).expect("paths resolve");
    let common = fs::canonicalize(root.join(".git")).expect("canonical common directory");

    assert_eq!(paths.runtime, common.join(".spice-rs"));
    assert_eq!(
        paths.journal,
        common.join(".spice-rs/data/spicejournal.sqlite3")
    );
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[tokio::test(flavor = "current_thread")]
async fn control_dispatch_waits_until_the_socket_response_is_attempted() {
    let (root, paths) = resident_fixture("control-response-receipt");
    stage_canvas_app(&root);
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let StartupSurface { config, trust } =
        load_startup_surface(&repository).expect("startup surface loads");
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let epoch = test_uuid(81);
    commit_startup(&bus, &repository, &trust, epoch, None).expect("resident seats");
    let operator = test_uuid(82);
    let capabilities = canvas::axum::Capabilities::default();
    capabilities
        .insert(operator, Actor::Operator)
        .expect("operator capability registers");
    let app = canvas::AppSource::resolve(&root).expect("fixture app resolves");
    let (server, mut control) = canvas::axum::Server::bind(
        epoch,
        capabilities.clone(),
        app,
        canvas::SpeechConfig::default(),
    )
    .await
    .expect("Canvas server binds");
    let address = server.local_addr().expect("server publishes address");
    let discovery = server
        .discovery(operator)
        .expect("server discovery describes operator access");
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));
    let (sent_command, command_sent) = tokio::sync::oneshot::channel();
    let (read_requested, read_response) = std::sync::mpsc::sync_channel(0);
    let (answered, response_read) = tokio::sync::oneshot::channel();
    let command_id = test_uuid(83);
    let client = tokio::task::spawn_blocking(move || {
        let mut socket = connect_operator_socket(address, operator, epoch);
        send_lane_read(&mut socket, command_id, epoch);
        let _ = sent_command.send(());
        read_response.recv().expect("test releases response read");
        assert_lane_read_response(&mut socket, command_id);
        let _ = answered.send(());
    });
    command_sent.await.expect("client submits command");
    let request = control.recv().await.expect("control receives command");
    assert_eq!(request.command().id(), command_id);
    {
        let reads = read_materializations::CurrentReads::start(&bus, &paths, &config.consent)
            .expect("read materializations start");
        let reporter = Arc::new(NoopCaptureReporter);
        let command_surface = command_surface::Surface::new(
            &bus,
            &repository,
            &paths,
            epoch,
            command_surface::Runtimes::new(
                lane_surface::Runtime::new(&capabilities, &discovery, reporter.clone()),
                lane_recovery::Runtime::new(reporter),
            ),
            &config,
            &reads,
        );
        let mut seats = Seats::new();
        let mut lifecycle = lifecycle::Runtime::new(&config.lane);
        let mut dispatch = std::pin::pin!(runtime::dispatch_control(
            &command_surface,
            &mut seats,
            &mut lifecycle,
            &mut control,
            Some(request),
        ));
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        assert!(
            matches!(
                std::future::Future::poll(dispatch.as_mut(), &mut context),
                std::task::Poll::Pending
            ),
            "dispatch must wait for the socket response receipt before returning"
        );
        read_requested
            .send(())
            .expect("client response read is released");
        let dispatched = dispatch.await.expect("lane read keeps resident seated");
        assert!(
            !dispatched.reached_dark,
            "lane read must not request resident shutdown"
        );
    }
    response_read.await.expect("client observed response");
    shutdown.send(()).expect("shutdown signals");
    server_task
        .await
        .expect("server task joins")
        .expect("server stops");
    client.await.expect("client thread joins");
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[tokio::test(flavor = "current_thread")]
async fn socket_dispatch_publishes_three_lane_fanout_before_any_restart_or_ack() {
    let (root, paths) = resident_fixture("socket-three-lane-fanout");
    stage_canvas_app(&root);
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let StartupSurface { config, trust } =
        load_startup_surface(&repository).expect("startup surface loads");
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let (epoch, lineage, fanout) = (test_uuid(91), test_uuid(92), test_uuid(93));
    commit_startup(&bus, &repository, &trust, epoch, None).expect("resident seats");
    let lanes = seed_socket_fanout_lanes(&bus, epoch, lineage, &root);
    let operator = test_uuid(89);
    let capabilities = canvas::axum::Capabilities::default();
    capabilities
        .insert(operator, Actor::Operator)
        .expect("operator capability registers");
    let app = canvas::AppSource::resolve(&root).expect("fixture app resolves");
    let (server, mut control) = canvas::axum::Server::bind(
        epoch,
        capabilities.clone(),
        app,
        canvas::SpeechConfig::default(),
    )
    .await
    .expect("Canvas server binds");
    let address = server.local_addr().expect("server publishes address");
    let discovery = server
        .discovery(operator)
        .expect("server discovery describes operator access");
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));
    let (commands_sent, commands_ready) = tokio::sync::oneshot::channel();
    let (responses_read, responses_done) = tokio::sync::oneshot::channel();
    let client = tokio::task::spawn_blocking(move || {
        let mut socket = connect_operator_socket(address, operator, epoch);
        for (index, (lane, occupant)) in lanes.into_iter().enumerate() {
            send_steer_send(
                &mut socket,
                test_uuid(84 + u8::try_from(index).expect("fixture index fits")),
                epoch,
                fanout,
                lane,
                occupant,
                format!("BROADCAST: socket delivery to lane {index}"),
            );
        }
        let _ = commands_sent.send(());
        for index in 0..lanes.len() {
            assert_steer_send_response(
                &mut socket,
                test_uuid(84 + u8::try_from(index).expect("fixture index fits")),
            );
        }
        let _ = responses_read.send(());
    });
    commands_ready.await.expect("client submits commands");
    SocketFanoutDispatch {
        bus: &bus,
        repository: &repository,
        paths: &paths,
        config: &config,
        capabilities: &capabilities,
        discovery: &discovery,
        epoch,
    }
    .run(&mut control, lanes)
    .await;
    tokio::time::timeout(Duration::from_secs(3), responses_done)
        .await
        .expect("socket responses are read before client stalls")
        .expect("client observes every response");
    assert_socket_fanout_published(&bus, &paths, fanout, lanes);
    stop_canvas_fixture(shutdown, server_task).await;
    client.await.expect("client thread joins");
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

/// A holder replacement never creates an empty observation for a contender.
///
/// The callback is the exact instant after the writer has prepared its one
/// landing write and before that write occurs. The second acquisition therefore
/// exercises the resident's real contended-refusal path while the replacement
/// is in flight. Restoring a truncation before that callback makes this fixture
/// observe `holder metadata unavailable` instead of the seeded holder.
#[test]
fn contended_resident_names_the_holder_while_its_record_is_rewritten() {
    let root = fixture("holder-rewrite");
    fs::create_dir_all(root.join(".git")).expect("git directory");
    let paths = RuntimePaths::discover(&root).expect("runtime paths resolve");
    fs::create_dir_all(&paths.runtime).expect("runtime directory");
    let lock = LockFile::new(
        LockName::new("resident").expect("resident lock name"),
        &paths.resident,
    );
    let Acquisition::Acquired(possession) = lock.try_acquire().expect("holder acquires") else {
        panic!("fresh resident lock unexpectedly contended");
    };
    let seeded = "pid=4242;epoch=seeded-holder";
    possession
        .rewrite_record(seeded.as_bytes())
        .expect("seeded holder lands");

    let mut observed = None;
    possession
        .rewrite_record_with(b"pid=4242;epoch=replacement", || {
            let Err(Outcome::Refusal(refusal)) = ResidentLock::acquire(&paths) else {
                panic!("the resident contender did not refuse");
            };
            assert_eq!(refusal.reason(), "resident::already-up");
            assert!(matches!(refusal, wire::Refusal::Bare { .. }));
            observed = refusal
                .diagnostic()
                .and_then(wire::RefusalDiagnostic::observed)
                .map(str::to_owned);
            Ok(())
        })
        .expect("replacement holder lands");

    assert_eq!(observed.as_deref(), Some(seeded));
    assert_eq!(
        fs::read_to_string(&paths.resident)
            .expect("replacement holder reads")
            .trim(),
        "pid=4242;epoch=replacement"
    );
    drop(possession);
    fs::remove_dir_all(root).expect("fixture cleanup");
}

/// A liveness observation cannot impersonate the resident it is checking.
///
/// The probe hook freezes the shared observation lock after it lands. Only
/// then does startup make its exclusive attempt; its contention hook releases
/// the probe, proving the two operations overlapped at the kernel lock rather
/// than merely running in nearby threads. Startup must retry that observation,
/// while the same availability path must still distinguish its resulting live
/// exclusive holder from the available lock after release.
#[cfg(unix)]
#[test]
fn resident_start_survives_an_overlapping_liveness_probe() {
    let root = fixture("startup-probe-overlap");
    fs::create_dir_all(root.join(".git")).expect("git directory");
    let paths = RuntimePaths::discover(&root).expect("runtime paths resolve");
    fs::create_dir_all(&paths.runtime).expect("runtime directory");
    fs::write(&paths.resident, b"stale holder record\n").expect("existing resident lock file");
    let lock = LockFile::new(
        LockName::new("resident").expect("resident lock name"),
        &paths.resident,
    );
    let (probe_held, probe_landed) = mpsc::sync_channel(0);
    let (release_probe, released) = mpsc::sync_channel(0);

    std::thread::scope(|scope| {
        let observed_lock = lock.clone();
        let observer = scope.spawn(move || {
            observed_lock
                .availability_with(|| {
                    probe_held.send(()).expect("announce held probe");
                    released.recv().expect("startup releases probe");
                })
                .expect("probe answers")
        });
        probe_landed.recv().expect("probe lands before startup");

        let mut release_probe = Some(release_probe);
        let resident = ResidentLock::acquire_with(&paths, || {
            if let Some(release) = release_probe.take() {
                release.send(()).expect("release overlapping probe");
            }
        })
        .expect("probe-width contention cannot become already-up");

        assert_eq!(
            observer.join().expect("probe thread"),
            sill::Availability::Available
        );
        assert_eq!(
            lock.availability().expect("live resident probe"),
            sill::Availability::Held
        );
        drop(resident);
        assert_eq!(
            lock.availability().expect("released resident probe"),
            sill::Availability::Available
        );
    });

    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn stale_surface_cleanup_is_idempotent_and_leaves_inbox_authority() {
    let root = fixture("stale-surfaces");
    fs::create_dir_all(root.join(".git")).expect("git directory");
    let paths = RuntimePaths::discover(&root).expect("paths resolve");
    fs::create_dir_all(&paths.lane_canvas).expect("lane discovery directory");
    fs::write(&paths.canvas, b"stale operator discovery").expect("operator discovery");
    fs::write(paths.lane_canvas.join("lane"), b"stale lane discovery").expect("lane discovery");
    fs::create_dir_all(&paths.inbox).expect("inbox directory");
    fs::write(paths.inbox.join("lane"), b"durable pending direction").expect("lane inbox");

    paths.clear_stale_surfaces().expect("stale surfaces clear");
    paths
        .clear_stale_surfaces()
        .expect("repeated stale surface cleanup is ordinary");

    assert!(!paths.canvas.exists());
    assert!(!paths.lane_canvas.exists());
    assert_eq!(
        fs::read(paths.inbox.join("lane")).expect("inbox remains"),
        b"durable pending direction"
    );
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn linked_worktrees_share_the_git_common_directory() {
    let root = fixture("linked");
    let worktree = root.join("worktree");
    let common = root.join("common.git");
    let git_dir = common.join("worktrees/linked");
    fs::create_dir_all(&worktree).expect("worktree directory");
    fs::create_dir_all(&git_dir).expect("git directory");
    fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", git_dir.display()),
    )
    .expect("git marker");
    fs::write(git_dir.join("commondir"), "../..\n").expect("common marker");

    let paths = RuntimePaths::discover(&worktree).expect("paths resolve");
    let common = fs::canonicalize(&common).expect("canonical common directory");

    assert_eq!(paths.runtime, common.join(".spice-rs"));
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn startup_surface_captures_configured_hidden_project_policy_once() {
    let root = fixture("hidden-project-policy");
    fs::create_dir_all(root.join(".git")).expect("git directory");
    fs::write(
        root.join("spice-rust.toml"),
        "[tasks]\nstems = ['board']\nhidden_stems = ['sandbox']\n",
    )
    .expect("tracked task metadata");
    let repository = Repository::discover(&root).expect("fixture repository resolves");

    let startup = load_startup_surface(&repository).expect("startup policy resolves");
    assert_eq!(
        startup.config.projects.hidden_stems(),
        ["oops", "maxim_proposal", "sandbox"]
    );
    assert_eq!(
        startup
            .config
            .projects
            .admit(".sandbox.triage_1")
            .expect("configured hidden descendant")
            .visibility()
            .hidden_stem(),
        Some("sandbox")
    );
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn lane_add_seats_a_vacant_lane_without_starting_an_occupant() {
    let (root, paths) = resident_fixture("lane-add-vacant");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let surface =
        crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let epoch = test_uuid(71);
    commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
    let capabilities = canvas::axum::Capabilities::default();
    let discovery = canvas::Discovery {
        endpoint: "http://127.0.0.1:8767".to_owned(),
        capability: test_uuid(72),
        epoch,
    };
    let runtime =
        lane_surface::Runtime::new(&capabilities, &discovery, Arc::new(NoopCaptureReporter));
    let mut seats = Seats::new();
    let lane = test_uuid(73);
    let body = wire::intent::LaneSeat {
        topology_revision: 0,
        lane,
        lineage_id: lane,
        name: Some("new lane".to_owned()),
        worktree: repository.worktree_root().display().to_string(),
        life: "drive".to_owned(),
        vendor: "claude".parse().expect("fixture vendor"),
        resume: None,
    };
    let command = admitted_lane_seat(epoch, &body);

    let answer = lane_surface::seat(&bus, &paths, epoch, &mut seats, &runtime, &command, &body);

    let Outcome::Reply(reply) = answer else {
        panic!("lane add must commit: {answer:?}");
    };
    let CommandAnswer::LaneChange(change) = reply.body else {
        panic!("lane add returned a non-lane-change answer");
    };
    assert_eq!(
        (change.from.as_str(), change.to.as_str()),
        ("absent", "vacant")
    );
    assert_eq!(change.git, None);
    assert!(seats.is_empty(), "lane add must not supervise a process");
    let snapshot = bus.fact_snapshot().expect("facts read");
    assert!(
        snapshot
            .facts()
            .iter()
            .any(|fact| fact.fact.kind() == wire::FactKind::LaneSeated),
        "lane add still records the durable lane seat"
    );
    assert!(
        !snapshot
            .facts()
            .iter()
            .any(|fact| fact.fact.kind() == wire::FactKind::LaneSessionDeclared),
        "lane add must not declare a vendor session"
    );
    assert!(
        !snapshot
            .facts()
            .iter()
            .any(|fact| fact.fact.kind() == wire::FactKind::LaneStarted),
        "lane add must not start an occupant"
    );
    let row = bus
        .residency_head()
        .expect("residency reads")
        .expect("residency exists")
        .into_body()
        .lanes
        .into_iter()
        .find(|row| row.lane == lane)
        .expect("lane add publishes one row");
    assert_eq!(row.occupant, None);
    assert_eq!(row.session, None);
    assert_eq!(row.lineage, Vec::<CanonicalUuid>::new());
    assert_eq!(row.disposition, "vacant");
    assert_eq!(row.capabilities.len(), 1);
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}
