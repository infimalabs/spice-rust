use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message as ClientMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use super::*;

type ClientSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const ASSERTION_DEADLINE: Duration = Duration::from_secs(2);

fn uuid(text: &str) -> wire::CanonicalUuid {
    text.parse().expect("canonical fixture UUID")
}

fn epoch() -> wire::CanonicalUuid {
    uuid("018f82d4-6f59-7d06-92e3-bce9131163c3")
}

fn operator() -> wire::CanonicalUuid {
    uuid("018f82d4-6f59-7d06-92e3-bce9131163c4")
}

fn identity(last: u8) -> wire::CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02x}")
        .parse()
        .expect("canonical fixture identity")
}

fn seq(value: u64) -> wire::Seq {
    wire::Seq::new(value).expect("non-zero fixture position")
}

fn stream_write(note: &str, trust: bool) -> journal::JournalWrite {
    let mut write = journal::JournalWrite::new(
        wire::Actor::Operator,
        "task:STREAM-a1".parse().expect("origin"),
        "2030-04-12T18:44:09.271Z".parse().expect("instant"),
        wire::Producer::CommandDoor,
    );
    write
        .fact(
            wire::body::TaskNoted {
                task: "STREAM-a1".parse().expect("task handle"),
                note: note.to_owned(),
            },
            journal::FactCause::Root,
        )
        .expect("note fact builds");
    if trust {
        let digest = wire::Digest::compute(wire::Domain::ConfigExecutableSurface, b"surface");
        let fact = write
            .fact(
                wire::body::TrustPended {
                    surface: "config".to_owned(),
                    digest: digest.clone(),
                    previous: None,
                },
                journal::FactCause::Root,
            )
            .expect("trust fact builds");
        write.trust_head(fact, move |_| wire::TrustHead {
            digest,
            state: wire::TrustState::Pended,
        });
    }
    write
}

fn commit_stream(bus: &bus::Bus, note: &str, trust: bool) -> bus::JournalCommit {
    match bus.commit(stream_write(note, trust)).expect("bus answers") {
        bus::CommitOutcome::Committed(commit) => commit,
        other => panic!("stream fixture commit failed: {other:?}"),
    }
}

async fn stream_frame(socket: &mut ClientSocket) -> wire::StreamFrame {
    wire::decode(&next_frame(socket).await).expect("current stream frame")
}

async fn websocket(address: SocketAddr) -> ClientSocket {
    let mut request = format!("ws://{address}/socket")
        .into_client_request()
        .expect("socket request");
    request.headers_mut().insert(
        tokio_tungstenite::tungstenite::http::header::ORIGIN,
        format!("http://{address}").parse().expect("origin"),
    );
    connect_async(request).await.expect("socket opens").0
}

async fn authenticate(
    address: SocketAddr,
    capability: wire::CanonicalUuid,
    session_epoch: wire::CanonicalUuid,
) -> (ClientSocket, wire::Accepted) {
    let mut socket = websocket(address).await;
    send_client(
        &mut socket,
        &wire::Hello {
            wire: wire::WireVersion::CURRENT,
            capability,
            epoch: session_epoch,
        },
    )
    .await;
    let accepted = wire::decode(&next_frame(&mut socket).await).expect("accepted frame");
    (socket, accepted)
}

async fn send_change(
    socket: &mut ClientSocket,
    id: wire::CanonicalUuid,
    command_epoch: wire::CanonicalUuid,
    state: wire::ResidentState,
) {
    send_client(
        socket,
        &wire::Command {
            v: wire::WireVersion::CURRENT,
            id,
            epoch: command_epoch,
            kind: wire::CommandKind::UpChange,
            expected: vec![wire::Witness::Transition {
                from: wire::ResidentState::Seated,
                to: state,
            }],
            body: wire::intent::UpChange { state },
        },
    )
    .await;
}

async fn send_client<T: serde::Serialize>(socket: &mut ClientSocket, value: &T) {
    let bytes = wire::encode(value).expect("client frame encodes");
    socket
        .send(ClientMessage::Binary(bytes.into()))
        .await
        .expect("client frame sends");
}

async fn next_frame(socket: &mut ClientSocket) -> Vec<u8> {
    let message = timeout(ASSERTION_DEADLINE, socket.next())
        .await
        .expect("socket frame arrives before deadline")
        .expect("socket remains open")
        .expect("socket frame is valid");
    match message {
        ClientMessage::Binary(bytes) => bytes.to_vec(),
        ClientMessage::Text(text) => text.as_bytes().to_vec(),
        other => panic!("expected a data frame, got {other:?}"),
    }
}

async fn response(socket: &mut ClientSocket) -> wire::Response<serde_json::Value> {
    wire::decode(&next_frame(socket).await).expect("response frame")
}

async fn assert_socket_terminated(socket: &mut ClientSocket) {
    let result = timeout(ASSERTION_DEADLINE, socket.next())
        .await
        .expect("socket terminates before deadline");
    assert!(
        matches!(
            result,
            None | Some(Err(_)) | Some(Ok(ClientMessage::Close(_)))
        ),
        "socket remained active: {result:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn authenticated_socket_streams_one_bus_snapshot_updates_resume_and_typed_shutdown() {
    let bus = bus::Bus::start(journal::Journal::in_memory().expect("authority opens"))
        .expect("bus starts");
    commit_stream(&bus, "initial", true);
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, _control) = Server::bind_streaming(epoch(), capabilities, bus.reader(), 8)
        .await
        .expect("streaming server binds");
    let address = server.local_addr().expect("address");
    let (shutdown, stopped) = oneshot::channel();
    let server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));
    let (mut socket, _) = authenticate(address, operator(), epoch()).await;
    let subscription = identity(0xd0);
    send_client(
        &mut socket,
        &wire::StreamRequest::Subscribe {
            v: wire::WireVersion::CURRENT,
            id: subscription,
            epoch: epoch(),
            subjects: vec![wire::ViewSubject::Facts, wire::ViewSubject::Trust],
            after: None,
        },
    )
    .await;
    let initial_facts = stream_frame(&mut socket).await;
    let initial_trust = stream_frame(&mut socket).await;
    let wire::StreamFrame::Facts { packed, .. } = &initial_facts else {
        panic!("expected initial facts: {initial_facts:?}");
    };
    let horizon = packed.horizon;
    assert_eq!(packed.rows.len(), 2);
    assert!(matches!(
        initial_trust,
        wire::StreamFrame::Trust { ref packed, .. }
            if packed.horizon == horizon && packed.rows.len() == 1
    ));
    let returned = [
        wire::encode(&initial_facts).expect("facts encode"),
        wire::encode(&initial_trust).expect("trust encodes"),
    ]
    .concat();
    assert!(!String::from_utf8_lossy(&returned).contains(&operator().to_string()));

    let update = commit_stream(&bus, "later", false);
    let live = stream_frame(&mut socket).await;
    assert!(matches!(
        live,
        wire::StreamFrame::Fact { ref fact, id, .. }
            if id == subscription && fact.seq.get() == update.last()
    ));
    // Trust is a replacement view and follows the same broadcast barrier.
    assert!(matches!(
        stream_frame(&mut socket).await,
        wire::StreamFrame::Trust { .. }
    ));
    socket.close(None).await.expect("first socket closes");

    let (mut resumed, _) = authenticate(address, operator(), epoch()).await;
    send_client(
        &mut resumed,
        &wire::StreamRequest::Subscribe {
            v: wire::WireVersion::CURRENT,
            id: identity(0xd1),
            epoch: epoch(),
            subjects: vec![wire::ViewSubject::Facts],
            after: Some(wire::StreamCursor {
                epoch: epoch(),
                seq: seq(update.last()),
            }),
        },
    )
    .await;
    assert!(matches!(
        stream_frame(&mut resumed).await,
        wire::StreamFrame::Facts { packed, .. }
            if packed.rows.is_empty() && packed.horizon == seq(update.last())
    ));

    shutdown.send(()).expect("shutdown signals");
    assert!(matches!(
        stream_frame(&mut resumed).await,
        wire::StreamFrame::Closing { after, .. }
            if after.seq == seq(update.last()) && after.epoch == epoch()
    ));
    assert_socket_terminated(&mut resumed).await;
    server_task
        .await
        .expect("server task joins")
        .expect("server stops");
    bus.shutdown().expect("bus shuts down");
}

#[tokio::test(flavor = "current_thread")]
async fn capability_replacement_faults_the_live_stream_without_disclosure() {
    let bus = bus::Bus::start(journal::Journal::in_memory().expect("authority opens"))
        .expect("bus starts");
    commit_stream(&bus, "initial", false);
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, _control) = Server::bind_streaming(epoch(), capabilities.clone(), bus.reader(), 8)
        .await
        .expect("streaming server binds");
    let address = server.local_addr().expect("address");
    let server_task = tokio::spawn(server.up());
    let (mut socket, _) = authenticate(address, operator(), epoch()).await;
    send_client(
        &mut socket,
        &wire::StreamRequest::Subscribe {
            v: wire::WireVersion::CURRENT,
            id: identity(0xd0),
            epoch: epoch(),
            subjects: vec![wire::ViewSubject::Facts],
            after: None,
        },
    )
    .await;
    let _ = stream_frame(&mut socket).await;

    assert!(capabilities.remove(operator(), &wire::Actor::Operator));
    commit_stream(&bus, "replacement boundary", false);
    let frame = stream_frame(&mut socket).await;
    assert!(matches!(
        frame,
        wire::StreamFrame::Fault { ref fault, .. }
            if fault.reason == "stream::capability-replaced"
    ));
    assert!(
        !String::from_utf8_lossy(&wire::encode(&frame).expect("fault encodes"))
            .contains(&operator().to_string())
    );
    assert_socket_terminated(&mut socket).await;
    server_task.abort();
    let _ = server_task.await;
    bus.shutdown().expect("bus shuts down");
}

#[tokio::test(flavor = "current_thread")]
async fn blocked_command_delivery_turns_broadcast_lag_into_resumable_overflow() {
    let bus = bus::Bus::start(journal::Journal::in_memory().expect("authority opens"))
        .expect("bus starts");
    commit_stream(&bus, "initial", false);
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, mut control) = Server::bind_streaming(epoch(), capabilities, bus.reader(), 8)
        .await
        .expect("streaming server binds");
    let address = server.local_addr().expect("address");
    let server_task = tokio::spawn(server.up());
    let (mut socket, _) = authenticate(address, operator(), epoch()).await;
    let subscription = identity(0xd0);
    send_client(
        &mut socket,
        &wire::StreamRequest::Subscribe {
            v: wire::WireVersion::CURRENT,
            id: subscription,
            epoch: epoch(),
            subjects: vec![wire::ViewSubject::Facts],
            after: None,
        },
    )
    .await;
    let initial = stream_frame(&mut socket).await;
    let wire::StreamFrame::Facts { packed, .. } = initial else {
        panic!("expected initial facts");
    };

    send_change(
        &mut socket,
        identity(0xd2),
        epoch(),
        wire::ResidentState::Seated,
    )
    .await;
    let request = control.recv().await.expect("command enters control queue");
    for index in 0..70 {
        commit_stream(&bus, &format!("queued {index}"), false);
    }
    request
        .respond(wire::Outcome::Refusal(wire::Refusal {
            reason: "fixture::released".to_owned(),
            witness: None,
            expected: None,
            observed: None,
            remedy: "continue the fixture".to_owned(),
        }))
        .expect("response returns");
    let _ = response(&mut socket).await;
    let overflow = stream_frame(&mut socket).await;
    assert!(matches!(
        overflow,
        wire::StreamFrame::Overflow { id, after, dropped, .. }
            if id == subscription && after.seq == packed.horizon && dropped > 0
    ));
    assert_socket_terminated(&mut socket).await;
    server_task.abort();
    let _ = server_task.await;
    bus.shutdown().expect("bus shuts down");
}

async fn wait_for_admission(
    observed: &mut watch::Receiver<Option<AdmissionObservation>>,
    expected: AdmissionObservation,
) {
    timeout(
        ASSERTION_DEADLINE,
        observed.wait_for(|observation| *observation == Some(expected)),
    )
    .await
    .expect("admission observation arrives before deadline")
    .expect("admission observer remains open");
}

#[tokio::test(flavor = "current_thread")]
async fn server_binds_ipv4_loopback_and_publishes_that_endpoint() {
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, _control) = Server::bind(epoch(), capabilities).await.expect("binds");
    let address = server.local_addr().expect("local address");
    let discovery = server.discovery(operator()).expect("operator discovery");

    assert!(is_loopback(address));
    assert_eq!(discovery.endpoint, format!("http://{address}"));
    assert_eq!(discovery.epoch, epoch());
}

async fn request(address: SocketAddr, host: &str, path: &str) -> Vec<u8> {
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("connect to canvas");
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .expect("write request");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read response");
    response
}

#[tokio::test(flavor = "current_thread")]
async fn live_server_serves_exact_embedded_bytes_and_refuses_another_host() {
    let (server, _control) = Server::bind(epoch(), Capabilities::default())
        .await
        .expect("binds");
    let address = server.local_addr().expect("local address");
    let task = tokio::spawn(server.up());

    let admitted = request(address, &address.to_string(), "/").await;
    let split = admitted
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("response carries headers")
        + 4;
    assert!(admitted.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert_eq!(&admitted[split..], EMBEDDED_APP.bytes());

    let script = request(address, &address.to_string(), "/app.module").await;
    assert!(script.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(
        script
            .windows(b"content-type: text/javascript; charset=utf-8".len())
            .any(|window| window == b"content-type: text/javascript; charset=utf-8")
    );

    let missing = request(address, &address.to_string(), "/missing").await;
    assert!(missing.starts_with(b"HTTP/1.1 404 Not Found\r\n"));

    let refused = request(address, "attacker.example", "/").await;
    assert!(refused.starts_with(b"HTTP/1.1 403 Forbidden\r\n"));

    task.abort();
    assert!(task.await.expect_err("aborted server task").is_cancelled());
}

#[tokio::test(flavor = "current_thread")]
async fn bootstrap_names_the_exact_operator_generation_and_is_not_cached() {
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, _control) = Server::bind(epoch(), capabilities).await.expect("binds");
    let address = server.local_addr().expect("local address");
    let task = tokio::spawn(server.up());

    let response = request(address, &address.to_string(), "/bootstrap").await;
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("response carries headers")
        + 4;
    assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(
        response[..split]
            .windows(b"cache-control: no-store".len())
            .any(|window| window == b"cache-control: no-store")
    );
    let discovery: Discovery = wire::decode(&response[split..]).expect("bootstrap decodes");
    assert_eq!(discovery.endpoint, format!("http://{address}"));
    assert_eq!(discovery.capability, operator());
    assert_eq!(discovery.epoch, epoch());

    task.abort();
    assert!(task.await.expect_err("aborted server task").is_cancelled());
}

#[tokio::test(flavor = "current_thread")]
async fn authenticated_session_routes_multiple_typed_commands_and_correlates_answers() {
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, mut control) = Server::bind(epoch(), capabilities).await.expect("binds");
    let address = server.local_addr().expect("local address");
    let (shutdown, stopped) = oneshot::channel();
    let server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));
    let client = tokio::task::spawn_blocking(move || command_client(address));
    answer_commands(&mut control).await;
    client.await.expect("client finishes");
    let _ = shutdown.send(());
    server_task
        .await
        .expect("server task")
        .expect("server stops");
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_closes_partial_upgrade_pre_hello_and_active_sockets() {
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, _control) = Server::bind(epoch(), capabilities).await.expect("binds");
    let state = Arc::clone(&server.state);
    let address = server.local_addr().expect("local address");
    let (shutdown, stopped) = oneshot::channel();
    let server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));

    let mut partial = tokio::net::TcpStream::connect(address)
        .await
        .expect("partial upgrade connects");
    partial
        .write_all(format!("GET /socket HTTP/1.1\r\nHost: {address}\r\n").as_bytes())
        .await
        .expect("partial upgrade writes");
    let mut pre_hello = websocket(address).await;
    let (mut active, accepted) = authenticate(address, operator(), epoch()).await;
    assert_eq!(accepted.actor, wire::Actor::Operator);

    shutdown.send(()).expect("shutdown requests");
    timeout(ASSERTION_DEADLINE, server_task)
        .await
        .expect("graceful shutdown cannot wait on accepted sockets")
        .expect("server task")
        .expect("server stops");
    assert!(*state.closed.subscribe().borrow());
    assert!(state.connections.is_cancelled());

    let mut remaining = Vec::new();
    timeout(ASSERTION_DEADLINE, partial.read_to_end(&mut remaining))
        .await
        .expect("partial upgrade closes")
        .expect("partial upgrade reads to closure");
    assert_socket_terminated(&mut pre_hello).await;
    assert_socket_terminated(&mut active).await;
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_forces_transport_closed_when_a_session_cannot_finish_draining() {
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, _control) = Server::bind(epoch(), capabilities).await.expect("binds");
    let state = Arc::clone(&server.state);
    let session = SessionGuard::enter(&state);

    timeout(ASSERTION_DEADLINE, state.close())
        .await
        .expect("session drain has a finite grace");
    assert!(state.connections.is_cancelled());
    assert_eq!(state.sessions.load(Ordering::Acquire), 1);

    drop(session);
    assert_eq!(state.sessions.load(Ordering::Acquire), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn operator_lane_replacement_and_reconnect_cross_the_live_door() {
    let lane = identity(0xd0);
    let lane_capability = identity(0xd1);
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    capabilities
        .insert(lane_capability, wire::Actor::Lane(lane))
        .expect("lane capability");
    let (server, mut control) = Server::bind(epoch(), capabilities).await.expect("binds");
    let address = server.local_addr().expect("local address");
    let (shutdown, stopped) = oneshot::channel();
    let server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));

    let (mut lane_socket, accepted) = authenticate(address, lane_capability, epoch()).await;
    assert_eq!(accepted.actor, wire::Actor::Lane(lane));
    let lane_request = identity(0xd2);
    send_change(
        &mut lane_socket,
        lane_request,
        epoch(),
        wire::ResidentState::Dark,
    )
    .await;
    let lane_response = response(&mut lane_socket).await;
    assert_eq!(
        (lane_response.id, lane_response.epoch),
        (lane_request, epoch())
    );
    assert!(matches!(
        lane_response.outcome,
        wire::Outcome::Refusal(wire::Refusal { ref reason, .. })
            if reason == "command::authority"
    ));
    assert!(control.receiver.try_recv().is_err());

    let (mut operator_socket, accepted) = authenticate(address, operator(), epoch()).await;
    assert_eq!(accepted.actor, wire::Actor::Operator);
    let operator_request = identity(0xd3);
    send_change(
        &mut operator_socket,
        operator_request,
        epoch(),
        wire::ResidentState::Seated,
    )
    .await;
    let request = control.recv().await.expect("operator request enters queue");
    assert_eq!(request.command().id(), operator_request);
    request
        .respond(state_reply(wire::ResidentState::Seated))
        .expect("operator awaits response");
    let operator_response = response(&mut operator_socket).await;
    assert_eq!(operator_response.id, operator_request);

    shutdown.send(()).expect("first generation stops");
    timeout(ASSERTION_DEADLINE, server_task)
        .await
        .expect("first generation stops before deadline")
        .expect("server task")
        .expect("server stops");

    let replacement_epoch = identity(0xd4);
    let replacement = Capabilities::default();
    replacement
        .insert(operator(), wire::Actor::Lane(lane))
        .expect("replacement generation capability");
    let (server, _control) = Server::bind(replacement_epoch, replacement)
        .await
        .expect("replacement binds");
    let address = server.local_addr().expect("replacement address");
    let (shutdown, stopped) = oneshot::channel();
    let server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));

    let mut stale = websocket(address).await;
    send_client(
        &mut stale,
        &wire::Hello {
            wire: wire::WireVersion::CURRENT,
            capability: operator(),
            epoch: epoch(),
        },
    )
    .await;
    let stale_fault: wire::Fault =
        wire::decode(&next_frame(&mut stale).await).expect("stale fault");
    assert_eq!(stale_fault.reason, "hello::epoch-mismatch");

    let mut removed = websocket(address).await;
    send_client(
        &mut removed,
        &wire::Hello {
            wire: wire::WireVersion::CURRENT,
            capability: lane_capability,
            epoch: replacement_epoch,
        },
    )
    .await;
    let removed_refusal: wire::Refusal =
        wire::decode(&next_frame(&mut removed).await).expect("removed capability refusal");
    assert_eq!(removed_refusal.reason, "hello::capability-unknown");

    let (mut current, accepted) = authenticate(address, operator(), replacement_epoch).await;
    assert_eq!(accepted.actor, wire::Actor::Lane(lane));
    shutdown.send(()).expect("replacement stops");
    timeout(ASSERTION_DEADLINE, server_task)
        .await
        .expect("replacement stops before deadline")
        .expect("server task")
        .expect("server stops");
    assert_socket_terminated(&mut current).await;
}

#[tokio::test(flavor = "current_thread")]
async fn saturated_queue_orders_concurrent_work_before_dark_and_refuses_later_work() {
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, mut control) = Server::bind_with_capacity(epoch(), capabilities, None, 1)
        .await
        .expect("binds");
    let state = Arc::clone(&server.state);
    let mut observed = state.admission_observed.subscribe();
    let address = server.local_addr().expect("local address");
    let (shutdown, stopped) = oneshot::channel();
    let server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));
    let (mut first, _) = authenticate(address, operator(), epoch()).await;
    let (mut blocked, _) = authenticate(address, operator(), epoch()).await;
    let (mut dark, _) = authenticate(address, operator(), epoch()).await;

    let first_id = identity(0xe0);
    send_change(&mut first, first_id, epoch(), wire::ResidentState::Seated).await;
    wait_for_admission(&mut observed, AdmissionObservation::Queued(first_id)).await;

    let blocked_id = identity(0xe1);
    send_change(
        &mut blocked,
        blocked_id,
        epoch(),
        wire::ResidentState::Seated,
    )
    .await;
    wait_for_admission(&mut observed, AdmissionObservation::Ordering(blocked_id)).await;

    let dark_id = identity(0xe2);
    send_change(&mut dark, dark_id, epoch(), wire::ResidentState::Dark).await;

    let request = control.recv().await.expect("first request");
    assert_eq!(request.command().id(), first_id);
    request
        .respond(state_reply(wire::ResidentState::Seated))
        .expect("first client waits");
    assert_eq!(response(&mut first).await.id, first_id);

    let request = control.recv().await.expect("blocked request");
    assert_eq!(request.command().id(), blocked_id);
    request
        .respond(state_reply(wire::ResidentState::Seated))
        .expect("blocked client waits");
    assert_eq!(response(&mut blocked).await.id, blocked_id);

    wait_for_admission(&mut observed, AdmissionObservation::Queued(dark_id)).await;
    assert_eq!(*state.admission.lock().await, Admission::Dark);

    let refused_id = identity(0xe3);
    send_change(&mut first, refused_id, epoch(), wire::ResidentState::Seated).await;
    let refused = response(&mut first).await;
    assert_eq!((refused.id, refused.epoch), (refused_id, epoch()));
    assert!(matches!(
        refused.outcome,
        wire::Outcome::Refusal(wire::Refusal { ref reason, .. })
            if reason == "resident::not-admitting"
    ));

    let request = control.recv().await.expect("Dark request");
    assert_eq!(request.command().id(), dark_id);
    request
        .respond(state_reply(wire::ResidentState::Dark))
        .expect("Dark client waits");
    assert_eq!(response(&mut dark).await.id, dark_id);
    assert!(control.receiver.try_recv().is_err());

    shutdown.send(()).expect("server stops");
    timeout(ASSERTION_DEADLINE, server_task)
        .await
        .expect("server stops before deadline")
        .expect("server task")
        .expect("server stops");
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_interrupts_a_queue_reservation_blocked_by_saturation() {
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, _control) = Server::bind_with_capacity(epoch(), capabilities, None, 1)
        .await
        .expect("binds");
    let state = Arc::clone(&server.state);
    let mut observed = state.admission_observed.subscribe();
    let address = server.local_addr().expect("local address");
    let (shutdown, stopped) = oneshot::channel();
    let server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));
    let (mut queued, _) = authenticate(address, operator(), epoch()).await;
    let (mut blocked, _) = authenticate(address, operator(), epoch()).await;

    let queued_id = identity(0xe4);
    send_change(&mut queued, queued_id, epoch(), wire::ResidentState::Seated).await;
    wait_for_admission(&mut observed, AdmissionObservation::Queued(queued_id)).await;

    let blocked_id = identity(0xe5);
    send_change(
        &mut blocked,
        blocked_id,
        epoch(),
        wire::ResidentState::Seated,
    )
    .await;
    wait_for_admission(&mut observed, AdmissionObservation::Ordering(blocked_id)).await;

    shutdown.send(()).expect("shutdown requests");
    timeout(ASSERTION_DEADLINE, server_task)
        .await
        .expect("saturated shutdown stops before deadline")
        .expect("server task")
        .expect("server stops");
    assert_socket_terminated(&mut queued).await;
    assert_socket_terminated(&mut blocked).await;
}

fn command_client(address: SocketAddr) {
    use tungstenite::client::IntoClientRequest;
    use tungstenite::http::header::ORIGIN;

    let stream = std::net::TcpStream::connect(address).expect("connects");
    let mut request = format!("ws://{address}/socket")
        .into_client_request()
        .expect("request");
    request
        .headers_mut()
        .insert(ORIGIN, format!("http://{address}").parse().expect("origin"));
    let (mut socket, _) = tungstenite::client(request, stream).expect("opens");
    let hello = wire::Hello {
        wire: wire::WireVersion::CURRENT,
        capability: operator(),
        epoch: epoch(),
    };
    socket
        .send(tungstenite::Message::Binary(
            wire::encode(&hello).expect("hello encodes").into(),
        ))
        .expect("hello sends");
    let opening = socket.read().expect("acceptance").into_data();
    let accepted: wire::Accepted = wire::decode(&opening).expect("accepted frame");
    assert_eq!(accepted.actor, wire::Actor::Operator);

    for (id, to, succeeds) in command_cases() {
        let command = wire::Command {
            v: wire::WireVersion::CURRENT,
            id,
            epoch: epoch(),
            kind: wire::CommandKind::UpChange,
            expected: vec![wire::Witness::Transition {
                from: wire::ResidentState::Seated,
                to,
            }],
            body: wire::intent::UpChange { state: to },
        };
        socket
            .send(tungstenite::Message::Binary(
                wire::encode(&command).expect("command encodes").into(),
            ))
            .expect("command sends");
        let frame = socket.read().expect("response").into_data();
        let response: wire::Response<serde_json::Value> =
            wire::decode(&frame).expect("response decodes");
        assert_eq!((response.id, response.epoch), (id, epoch()));
        assert_eq!(
            matches!(response.outcome, wire::Outcome::Reply(_)),
            succeeds
        );
    }
}

fn command_cases() -> [(wire::CanonicalUuid, wire::ResidentState, bool); 3] {
    [
        (
            uuid("018f82d4-6f59-7d06-92e3-bce9131163c6"),
            wire::ResidentState::Dark,
            false,
        ),
        (
            uuid("018f82d4-6f59-7d06-92e3-bce9131163c7"),
            wire::ResidentState::Seated,
            true,
        ),
        (
            uuid("018f82d4-6f59-7d06-92e3-bce9131163c8"),
            wire::ResidentState::Dark,
            true,
        ),
    ]
}

async fn answer_commands(control: &mut Control) {
    for (index, (_, to, _)) in command_cases().into_iter().enumerate() {
        let request = control.recv().await.expect("resident request");
        assert_eq!(request.command().actor(), &wire::Actor::Operator);
        assert!(matches!(
            request.command().body(),
            wire::intent::Intent::UpChange(wire::intent::UpChange { state }) if *state == to
        ));
        let outcome = if index == 0 {
            transition_refusal()
        } else {
            state_reply(to)
        };
        request.respond(outcome).expect("client waits for answer");
    }
}

fn transition_refusal() -> wire::Outcome<wire::intent::CommandAnswer> {
    wire::Outcome::Refusal(wire::Refusal {
        reason: "resident::transition-moved".to_owned(),
        witness: Some("transition".to_owned()),
        expected: Some("seated->dark".to_owned()),
        observed: Some("quiescing->dark".to_owned()),
        remedy: "refresh the transition".to_owned(),
    })
}

fn state_reply(to: wire::ResidentState) -> wire::Outcome<wire::intent::CommandAnswer> {
    wire::Outcome::Reply(wire::Reply {
        position: wire::ReplyPosition::Direct {},
        body: wire::intent::CommandAnswer::State(wire::intent::StateAnswer {
            from: wire::ResidentState::Seated,
            to,
        }),
    })
}

#[test]
fn duplicate_capability_refuses_without_replacing_its_principal() {
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("first registration");
    let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163c5");

    assert_eq!(
        capabilities.insert(operator(), wire::Actor::Lane(lane)),
        Err(CapabilityError::Duplicate)
    );
    let accepted = capabilities
        .admit(
            &wire::Hello {
                wire: wire::WireVersion::CURRENT,
                capability: operator(),
                epoch: epoch(),
            },
            epoch(),
        )
        .expect("original registration survives");
    assert_eq!(accepted.actor, wire::Actor::Operator);
}

#[test]
fn unknown_capability_refuses_without_echoing_the_credential() {
    let capabilities = Capabilities::default();
    let unknown = operator();
    let hello = wire::Hello {
        wire: wire::WireVersion::CURRENT,
        capability: unknown,
        epoch: epoch(),
    };

    let refusal = capabilities
        .admit(&hello, epoch())
        .expect_err("unknown capability refuses");

    assert!(matches!(
        refusal,
        OpeningError::Refused(wire::Refusal { ref reason, .. })
            if reason == "hello::capability-unknown"
    ));
    assert!(!format!("{refusal:?}").contains(&unknown.to_string()));
}

#[test]
fn host_and_origin_must_name_the_published_endpoint() {
    let state = SurfaceState {
        authority: "127.0.0.1:48123".to_owned(),
        origin: "http://127.0.0.1:48123".to_owned(),
        epoch: epoch(),
        capabilities: Capabilities::default(),
        admission: Mutex::new(Admission::Open),
        commands: mpsc::channel(1).0,
        stream: None,
        closed: watch::channel(false).0,
        connections: CancellationToken::new(),
        sessions: AtomicUsize::new(0),
        sessions_closed: Notify::new(),
        admission_observed: watch::channel(None).0,
    };
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("attacker.example"));
    headers.insert(ORIGIN, HeaderValue::from_static("https://attacker.example"));
    assert!(admit_headers(&state, &headers, true).is_err());

    headers.insert(HOST, HeaderValue::from_static("127.0.0.1:48123"));
    headers.insert(ORIGIN, HeaderValue::from_static("http://127.0.0.1:48123"));
    assert_eq!(admit_headers(&state, &headers, true), Ok(()));
}
