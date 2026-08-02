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

fn speech() -> crate::SpeechConfig {
    crate::SpeechConfig::default()
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
    let (server, _control) = Server::bind(epoch(), capabilities, speech())
        .await
        .expect("binds");
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

async fn speech_request(address: SocketAddr, host: &str, origin: &str, body: &[u8]) -> Vec<u8> {
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("connect to canvas");
    let head = format!(
        concat!(
            "POST /speech HTTP/1.1\r\n",
            "Host: {host}\r\n",
            "Origin: {origin}\r\n",
            "Content-Type: application/json\r\n",
            "Content-Length: {}\r\n",
            "Connection: close\r\n\r\n",
        ),
        body.len(),
        host = host,
        origin = origin,
    );
    stream.write_all(head.as_bytes()).await.expect("write head");
    stream.write_all(body).await.expect("write body");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read response");
    response
}

fn response_body(response: &[u8]) -> &[u8] {
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("response carries headers")
        + 4;
    &response[split..]
}

fn external_speech(command: &[&str]) -> crate::SpeechConfig {
    crate::SpeechConfig::new(
        crate::SpeechBackend::External,
        command.iter().map(ToString::to_string).collect(),
        crate::speech::DEFAULT_VOICE.to_owned(),
        "audio/test".to_owned(),
        175,
    )
    .expect("external speech fixture")
}

#[tokio::test(flavor = "current_thread")]
async fn speech_endpoint_executes_only_the_server_owned_argv_contract() {
    let (server, _control) = Server::bind(
        epoch(),
        Capabilities::default(),
        external_speech(&["/bin/cat"]),
    )
    .await
    .expect("binds");
    let address = server.local_addr().expect("local address");
    let task = tokio::spawn(server.up());
    let body = wire::encode(&wire::SpeechRequest {
        text: "spoken exactly; $(not a shell)".to_owned(),
    })
    .expect("request encodes");
    let response = speech_request(
        address,
        &address.to_string(),
        &format!("http://{address}"),
        &body,
    )
    .await;

    assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(
        response
            .windows(b"content-type: audio/test".len())
            .any(|window| window == b"content-type: audio/test")
    );
    assert_eq!(response_body(&response), b"spoken exactly; $(not a shell)");

    task.abort();
    assert!(task.await.expect_err("aborted server task").is_cancelled());
}

#[tokio::test(flavor = "current_thread")]
async fn speech_endpoint_bounds_disabled_malformed_and_unavailable_backends() {
    for speech in [
        crate::SpeechConfig::new(
            crate::SpeechBackend::Disabled,
            Vec::new(),
            crate::speech::DEFAULT_VOICE.to_owned(),
            "audio/wav".to_owned(),
            175,
        )
        .expect("disabled speech"),
        crate::SpeechConfig::default(),
        external_speech(&[]),
        external_speech(&["/definitely/missing/spice-speech"]),
    ] {
        let (server, _control) = Server::bind(epoch(), Capabilities::default(), speech)
            .await
            .expect("binds");
        let address = server.local_addr().expect("local address");
        let task = tokio::spawn(server.up());
        let body = wire::encode(&wire::SpeechRequest {
            text: "bounded failure".to_owned(),
        })
        .expect("request encodes");
        let response = speech_request(
            address,
            &address.to_string(),
            &format!("http://{address}"),
            &body,
        )
        .await;
        assert!(response.starts_with(b"HTTP/1.1 503 Service Unavailable\r\n"));
        assert_eq!(
            response_body(&response),
            b"external speech backend is unavailable"
        );
        task.abort();
        assert!(task.await.expect_err("aborted server task").is_cancelled());
    }

    let (server, _control) = Server::bind(
        epoch(),
        Capabilities::default(),
        external_speech(&["/bin/cat"]),
    )
    .await
    .expect("binds");
    let address = server.local_addr().expect("local address");
    let task = tokio::spawn(server.up());
    let malformed = speech_request(
        address,
        &address.to_string(),
        &format!("http://{address}"),
        br#"{"text":"valid","command":["/bin/cat"]}"#,
    )
    .await;
    assert!(malformed.starts_with(b"HTTP/1.1 422 Unprocessable Entity\r\n"));
    let cross_origin = speech_request(
        address,
        &address.to_string(),
        "https://attacker.example",
        br#"{"text":"valid"}"#,
    )
    .await;
    assert!(cross_origin.starts_with(b"HTTP/1.1 403 Forbidden\r\n"));

    task.abort();
    assert!(task.await.expect_err("aborted server task").is_cancelled());
}

#[tokio::test(flavor = "current_thread")]
async fn live_server_serves_exact_embedded_bytes_and_refuses_another_host() {
    let (server, _control) = Server::bind(epoch(), Capabilities::default(), speech())
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
    let (server, _control) = Server::bind(epoch(), capabilities, speech())
        .await
        .expect("binds");
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
    let discovery: wire::CanvasBootstrap =
        wire::decode(&response[split..]).expect("bootstrap decodes");
    assert_eq!(discovery.endpoint, format!("http://{address}"));
    assert_eq!(discovery.capability, operator());
    assert_eq!(discovery.epoch, epoch());
    assert_eq!(
        discovery.speech,
        wire::SpeechPlayback::System {
            voice: "Zoe (Premium)".to_owned(),
            words_per_minute: 175,
        }
    );

    task.abort();
    assert!(task.await.expect_err("aborted server task").is_cancelled());
}

#[tokio::test(flavor = "current_thread")]
async fn authenticated_session_routes_multiple_typed_commands_and_correlates_answers() {
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, mut control) = Server::bind(epoch(), capabilities, speech())
        .await
        .expect("binds");
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
    let (server, _control) = Server::bind(epoch(), capabilities, speech())
        .await
        .expect("binds");
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
    let (server, mut control) = Server::bind(epoch(), capabilities, speech())
        .await
        .expect("binds");
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
    let (server, _control) = Server::bind(replacement_epoch, replacement, speech())
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
    let (server, mut control) = Server::bind_with_capacity(epoch(), capabilities, speech(), 1)
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
    let (server, _control) = Server::bind_with_capacity(epoch(), capabilities, speech(), 1)
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
        speech: speech(),
        speech_slot: Semaphore::new(1),
        admission: Mutex::new(Admission::Open),
        commands: mpsc::channel(1).0,
        closed: watch::channel(false).0,
        connections: CancellationToken::new(),
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
