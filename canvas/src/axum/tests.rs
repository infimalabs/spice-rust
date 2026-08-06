use std::net::Ipv6Addr;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message as ClientMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use super::*;
use crate::APP;

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

fn agent_message(author: u8, offset: u64, text: &str) -> wire::AgentMessage {
    wire::AgentMessage {
        lane: identity(0xe5),
        author: identity(author),
        locus: wire::MessageLocus {
            member: 3,
            offset,
            length: 24,
            event: 0,
        },
        activity: wire::AgentActivityKind::Prose,
        text: text.to_owned(),
    }
}

fn commit_lane_stream(
    bus: &bus::Bus,
    lane: wire::CanonicalUuid,
    capability: wire::CanonicalUuid,
) -> bus::JournalCommit {
    let mut write = journal::JournalWrite::new(
        wire::Actor::Spice,
        format!("resident:{}", epoch()).parse().expect("origin"),
        "2030-04-12T18:44:09.272Z".parse().expect("instant"),
        wire::Producer::ResidentRecognition,
    );
    write
        .fact(
            wire::body::LaneSeated {
                lane,
                lineage_id: lane,
                capability,
                name: None,
                worktree: "/tmp/canvas-stream-lane".to_owned(),
                life: "drive".to_owned(),
                vendor: "claude".parse().expect("vendor"),
            },
            journal::FactCause::Root,
        )
        .expect("lane fact builds");
    match bus.commit(write).expect("bus answers") {
        bus::CommitOutcome::Committed(commit) => commit,
        other => panic!("lane stream fixture failed: {other:?}"),
    }
}

async fn stream_frame(socket: &mut ClientSocket) -> wire::StreamFrame {
    stream_frame_bytes(socket).await.0
}

async fn stream_frame_bytes(socket: &mut ClientSocket) -> (wire::StreamFrame, Vec<u8>) {
    let bytes = next_frame(socket).await;
    let frame = wire::decode(&bytes).expect("current stream frame");
    (frame, bytes)
}

fn speech() -> crate::SpeechConfig {
    crate::SpeechConfig::default()
}

const FIXTURE_GENERATION: &str = "3333333333333333333333333333333333333333333333333333333333333333";

fn seed_app_generation(root: &std::path::Path, label: &str) -> std::path::PathBuf {
    let stage = root.join(crate::APP_STAGE);
    let generation = stage
        .join(crate::APP_STAGE_GENERATIONS)
        .join(FIXTURE_GENERATION);
    std::fs::create_dir_all(&generation).expect("fixture app generation");
    for asset in APP.assets() {
        let bytes = if asset.path() == "/" {
            format!(
                "{label} {}{INLINE_MODULE_OPEN}\nconsole.log({label:?});\n{INLINE_MODULE_CLOSE}",
                asset.file()
            )
        } else {
            format!("{label} {}", asset.file())
        };
        std::fs::write(generation.join(asset.file()), bytes).expect("fixture app asset");
    }
    std::fs::write(
        stage.join(crate::APP_STAGE_CURRENT),
        format!("{FIXTURE_GENERATION}\n"),
    )
    .expect("fixture app pointer");
    generation
}

/// One complete lane-built app shared by tests that do not inspect its bytes.
fn app() -> crate::AppSource {
    static ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let root = ROOT.get_or_init(|| {
        let root =
            std::env::temp_dir().join(format!("spice-canvas-test-app-{}", std::process::id()));
        seed_app_generation(&root, "test app");
        root
    });
    crate::AppSource::resolve(root).expect("fixture app resolves")
}

#[test]
fn app_policy_hashes_the_exact_inline_loader_bytes() {
    let policy =
        app_content_security_policy(b"<!doctype html><script type=\"module\">alert(1)</script>")
            .expect("one inline module loader produces a policy");

    assert!(
        policy
            .to_str()
            .expect("policy is ASCII")
            .contains("'sha256-bhHHL3z2vDgxUt0W3dWQOrprscmda2Y5pLsLg4GF+pI='")
    );
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

async fn assert_initial_stream(
    socket: &mut ClientSocket,
    subscription: wire::CanonicalUuid,
    lane_capability: wire::CanonicalUuid,
) {
    send_client(
        socket,
        &wire::StreamRequest::Subscribe {
            v: wire::WireVersion::CURRENT,
            id: subscription,
            epoch: epoch(),
            subjects: vec![wire::ViewSubject::Facts, wire::ViewSubject::Trust],
            after: None,
        },
    )
    .await;
    let (facts, fact_bytes) = stream_frame_bytes(socket).await;
    let (trust, trust_bytes) = stream_frame_bytes(socket).await;
    let wire::StreamFrame::Facts { packed, .. } = &facts else {
        panic!("expected initial facts: {facts:?}");
    };
    let horizon = packed.horizon;
    assert_eq!(packed.rows.len(), 3);
    assert!(
        packed
            .rows
            .iter()
            .any(|fact| fact.kind() == wire::FactKind::LaneSeated)
    );
    assert!(matches!(
        trust,
        wire::StreamFrame::Trust { ref packed, .. }
            if packed.horizon == horizon && packed.rows.len() == 1
    ));
    let returned = [fact_bytes, trust_bytes].concat();
    assert!(!String::from_utf8_lossy(&returned).contains(&operator().to_string()));
    assert!(!String::from_utf8_lossy(&returned).contains(&lane_capability.to_string()));
}

async fn assert_live_stream(
    socket: &mut ClientSocket,
    bus: &bus::Bus,
    subscription: wire::CanonicalUuid,
) -> u64 {
    let update = commit_stream(bus, "later", false);
    let live = stream_frame(socket).await;
    assert!(matches!(
        live,
        wire::StreamFrame::Fact { ref fact, id, .. }
            if id == subscription && fact.seq().get() == update.last()
    ));
    assert!(matches!(
        stream_frame(socket).await,
        wire::StreamFrame::Trust { .. }
    ));
    let lane_capability = identity(0xe6);
    let lane = commit_lane_stream(bus, identity(0xe5), lane_capability);
    let (lane_frame, lane_bytes) = stream_frame_bytes(socket).await;
    assert!(matches!(
        lane_frame,
        wire::StreamFrame::Fact { ref fact, .. }
            if fact.seq().get() == lane.last() && fact.kind() == wire::FactKind::LaneSeated
    ));
    assert!(!String::from_utf8_lossy(&lane_bytes).contains(&lane_capability.to_string()));
    assert!(matches!(
        stream_frame(socket).await,
        wire::StreamFrame::Trust { .. }
    ));
    send_client(
        socket,
        &wire::StreamRequest::Thread {
            v: wire::WireVersion::CURRENT,
            id: identity(0xd2),
            epoch: epoch(),
            cursor: wire::StreamCursor {
                epoch: epoch(),
                seq: seq(lane.last()),
            },
        },
    )
    .await;
    let (thread, bytes) = stream_frame_bytes(socket).await;
    assert!(matches!(
        thread,
        wire::StreamFrame::Thread { ref packed, .. }
            if packed.rows.first().is_some_and(|fact| {
                fact.seq().get() == lane.last() && fact.kind() == wire::FactKind::LaneSeated
            })
    ));
    assert!(!String::from_utf8_lossy(&bytes).contains(&lane_capability.to_string()));
    lane.last()
}

#[tokio::test(flavor = "current_thread")]
async fn authenticated_socket_streams_one_bus_snapshot_updates_resume_and_typed_shutdown() {
    let bus = bus::Bus::start(journal::Journal::in_memory().expect("authority opens"))
        .expect("bus starts");
    commit_stream(&bus, "initial", true);
    let initial_lane_capability = identity(0xe8);
    commit_lane_stream(&bus, identity(0xe7), initial_lane_capability);
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, _control) = Server::bind_streaming(
        Binding::new(LOOPBACK_EPHEMERAL, Access::local()),
        epoch(),
        capabilities,
        app(),
        speech(),
        bus.reader(),
        8,
    )
    .await
    .expect("streaming server binds");
    let address = server.local_addr().expect("address");
    let (shutdown, stopped) = oneshot::channel();
    let server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));
    let (mut socket, _) = authenticate(address, operator(), epoch()).await;
    let subscription = identity(0xd0);
    assert_initial_stream(&mut socket, subscription, initial_lane_capability).await;
    let live_cursor = assert_live_stream(&mut socket, &bus, subscription).await;
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
                seq: seq(live_cursor),
            }),
        },
    )
    .await;
    assert!(matches!(
        stream_frame(&mut resumed).await,
        wire::StreamFrame::Facts { packed, .. }
            if packed.rows.is_empty() && packed.horizon == seq(live_cursor)
    ));

    shutdown.send(()).expect("shutdown signals");
    assert!(matches!(
        stream_frame(&mut resumed).await,
        wire::StreamFrame::Closing { after, .. }
            if after.seq == seq(live_cursor) && after.epoch == epoch()
    ));
    assert_socket_terminated(&mut resumed).await;
    server_task
        .await
        .expect("server task joins")
        .expect("server stops");
    bus.shutdown().expect("bus shuts down");
}

#[tokio::test(flavor = "current_thread")]
async fn authenticated_socket_reads_and_pushes_agent_messages_with_each_author() {
    let bus = bus::Bus::start(journal::Journal::in_memory().expect("authority opens"))
        .expect("bus starts");
    commit_stream(&bus, "message horizon", false);
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, _control) = Server::bind_streaming(
        Binding::new(LOOPBACK_EPHEMERAL, Access::local()),
        epoch(),
        capabilities,
        app(),
        speech(),
        bus.reader(),
        8,
    )
    .await
    .expect("streaming server binds");
    let publisher = server
        .message_publisher()
        .expect("streaming server publishes messages");
    let first = agent_message(0xe6, 10, "first author");
    let second = agent_message(0xe7, 40, "second author");
    assert!(publisher.publish(first.clone()).expect("first publishes"));
    assert!(publisher.publish(second.clone()).expect("second publishes"));
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
            subjects: vec![wire::ViewSubject::Messages],
            after: None,
        },
    )
    .await;
    let initial = stream_frame(&mut socket).await;
    assert!(matches!(
        initial,
        wire::StreamFrame::Messages { limit: 256, ref messages, .. }
            if messages == &[first, second]
    ));

    let third = agent_message(0xe8, 70, "third author");
    assert!(publisher.publish(third.clone()).expect("third publishes"));
    assert!(matches!(
        stream_frame(&mut socket).await,
        wire::StreamFrame::Message { id, message, .. }
            if id == subscription && message == third
    ));

    shutdown.send(()).expect("shutdown signals");
    assert!(matches!(
        stream_frame(&mut socket).await,
        wire::StreamFrame::Closing { .. }
    ));
    assert_socket_terminated(&mut socket).await;
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
    let (server, _control) = Server::bind_streaming(
        Binding::new(LOOPBACK_EPHEMERAL, Access::local()),
        epoch(),
        capabilities.clone(),
        app(),
        speech(),
        bus.reader(),
        8,
    )
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
    let (server, mut control) = Server::bind_streaming(
        Binding::new(LOOPBACK_EPHEMERAL, Access::local()),
        epoch(),
        capabilities,
        app(),
        speech(),
        bus.reader(),
        8,
    )
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

mod binding;

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

async fn remote_websocket(
    address: SocketAddr,
    host: &str,
    auth: Option<&str>,
) -> Result<ClientSocket, tokio_tungstenite::tungstenite::Error> {
    let path = auth.map_or_else(
        || "/socket".to_owned(),
        |auth| format!("/socket?auth={auth}"),
    );
    let mut request = format!("ws://{address}{path}")
        .into_client_request()
        .expect("remote socket request");
    request
        .headers_mut()
        .insert(HOST, host.parse().expect("remote Host"));
    request.headers_mut().insert(
        ORIGIN,
        format!("http://{host}").parse().expect("remote Origin"),
    );
    connect_async(request).await.map(|opened| opened.0)
}

async fn speech_request(address: SocketAddr, host: &str, origin: &str, body: &[u8]) -> Vec<u8> {
    speech_request_to("/speech", address, host, origin, body).await
}

async fn speech_request_to(
    path: &str,
    address: SocketAddr,
    host: &str,
    origin: &str,
    body: &[u8],
) -> Vec<u8> {
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("connect to canvas");
    let head = format!(
        concat!(
            "POST {path} HTTP/1.1\r\n",
            "Host: {host}\r\n",
            "Origin: {origin}\r\n",
            "Content-Type: application/json\r\n",
            "Content-Length: {}\r\n",
            "Connection: close\r\n\r\n",
        ),
        body.len(),
        path = path,
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
        app(),
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
        let (server, _control) = Server::bind(epoch(), Capabilities::default(), app(), speech)
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
        app(),
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
async fn live_server_serves_the_lane_app_and_refuses_another_host() {
    let (server, _control) = Server::bind(epoch(), Capabilities::default(), app(), speech())
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
    assert!(
        admitted[..split]
            .windows(b"img-src 'self' data:".len())
            .any(|window| window == b"img-src 'self' data:")
    );
    assert!(admitted[split..].starts_with(b"test app canvas-app.html"));

    let script = request(
        address,
        &address.to_string(),
        &format!(
            "{}/{FIXTURE_GENERATION}/app.module",
            crate::APP_GENERATION_ROUTE
        ),
    )
    .await;
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

/// A worktree holding a complete app stage, and that stage's path.
///
/// The bytes name their own file so a route served from the wrong one is
/// visible in the assertion.
fn app_worktree() -> (std::path::PathBuf, std::path::PathBuf) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);

    let root = std::env::temp_dir().join(format!(
        "spice-canvas-app-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let generation = seed_app_generation(&root, "locally built");
    (root, generation)
}

#[tokio::test(flavor = "current_thread")]
async fn live_server_serves_an_app_stage_and_refuses_when_it_goes_away() {
    let (root, stage) = app_worktree();
    let app = crate::AppSource::resolve(&root).expect("a complete stage resolves");
    let (server, _control) = Server::bind_with_capacity(
        Binding::new(LOOPBACK_EPHEMERAL, Access::local()),
        epoch(),
        Capabilities::default(),
        app,
        speech(),
        None,
        1,
    )
    .await
    .expect("binds");
    let address = server.local_addr().expect("local address");
    let task = tokio::spawn(server.up());

    let served = request(address, &address.to_string(), "/").await;
    let split = served
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("response carries headers")
        + 4;
    assert!(served.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(served[split..].starts_with(b"locally built canvas-app.html"));

    // A stage that stops answering mid-flight is reported as this endpoint's
    // own failure, not as the app the operator asked to stop serving.
    std::fs::remove_file(stage.join("canvas-app.html")).expect("fixture removes one staged file");

    let refused = request(address, &address.to_string(), "/").await;
    assert!(refused.starts_with(b"HTTP/1.1 503 Service Unavailable\r\n"));
    assert!(
        String::from_utf8_lossy(&refused).contains("canvas-app.html"),
        "the refusal does not name the file it could not read"
    );

    task.abort();
    assert!(task.await.expect_err("aborted server task").is_cancelled());
    std::fs::remove_dir_all(root).expect("fixture cleanup");
}

mod lifecycle;
