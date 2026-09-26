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

mod identity;
mod message_history;

use identity::{TestApp, app_version, bind_test_server};

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
        "task:1kKRRsDP".parse().expect("origin"),
        "2030-04-12T18:44:09.271Z".parse().expect("instant"),
        wire::Producer::CommandDoor,
    );
    write
        .fact(
            wire::body::TaskNoted {
                task: "1kKRRsDP".parse().expect("task stamp"),
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
                    worktree: None,
                    previous: None,
                },
                journal::FactCause::Root,
            )
            .expect("trust fact builds");
        write.trust_head(fact, move |_| wire::TrustHead {
            digest,
            state: wire::TrustState::Pended,
            worktree: None,
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
        authored: "2030-04-12T18:44:09.271Z".parse().expect("fixture instant"),
        authored_synthetic: false,
        observed: None,
        activity: wire::AgentActivityKind::Prose,
        final_answer: false,
        text: text.to_owned(),
        classifications: Vec::new(),
        automatic_speech: None,
        unavailable_tool_terminals: Vec::new(),
        boundary: None,
        media: Vec::new(),
    }
}

#[test]
fn canvas_message_ingress_refuses_invalid_media_without_retaining_or_broadcasting_it() {
    let bus = bus::Bus::start(journal::Journal::in_memory().expect("authority opens"))
        .expect("bus starts");
    let source = crate::stream::Source::new(bus.reader(), epoch(), 8);
    let publisher = source.message_publisher();
    let mut updates = source.subscribe_messages();
    let mut invalid = agent_message(0xe6, 10, "forged capture");
    invalid.media.push(wire::Media {
        name: "capture.txt".to_owned(),
        content_type: "text/plain".to_owned(),
        data_url: "data:text/plain;base64,aGVsbG8=".to_owned(),
    });

    assert!(matches!(
        publisher.publish(invalid),
        Err(crate::stream::MessagePublishError::InvalidMedia(
            wire::MediaError::UnsupportedType(found)
        )) if found == "text/plain"
    ));
    assert!(matches!(
        updates.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    drop(source);
    bus.shutdown().expect("bus stops");
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
                team: lane,
                capability,
                name: None,
                worktree: "/tmp/canvas-stream-lane".to_owned(),
                life: "drive".to_owned(),
                vendor: "claude".parse().expect("vendor"),
                model: None,
                effort: None,
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

fn generation(app: &TestApp, capabilities: Capabilities) -> Generation {
    Generation::new(epoch(), capabilities, app.version(), app.source(), speech())
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
                "{label} {}{INLINE_THEME_OPEN}\ntheme();\n{INLINE_MODULE_CLOSE}{INLINE_MODULE_OPEN}\nconsole.log({label:?});\n{INLINE_MODULE_CLOSE}",
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

#[test]
fn app_fixture_directory_does_not_outlive_its_owner() {
    let root = {
        let fixture = TestApp::new();
        let root = fixture.root().to_path_buf();
        assert!(
            root.is_dir(),
            "fixture directory exists while its owner lives"
        );
        root
    };
    assert!(
        !root.exists(),
        "fixture directory is removed when its owner drops: {}",
        root.display()
    );
}

#[test]
fn app_policy_hashes_the_exact_inline_theme_and_loader_bytes() {
    let policy = app_content_security_policy(
        b"<!doctype html><script data-canvas-theme>theme()</script><script type=\"module\">alert(1)</script>",
    )
    .expect("the theme resolver and module loader produce one policy");

    let policy = policy.to_str().expect("policy is ASCII");
    assert!(policy.contains("'sha256-Rfr3GgHZFiXm+TmNr+li+0sZ4J9FGllo8jsXcJF5XZ8='"));
    assert!(policy.contains("'sha256-bhHHL3z2vDgxUt0W3dWQOrprscmda2Y5pLsLg4GF+pI='"));
}

#[test]
fn app_policy_refuses_any_script_outside_the_two_hashed_entries() {
    let error = app_content_security_policy(
        b"<script data-canvas-theme>theme()</script><script src=\"/other.js\"></script><script type=\"module\">alert(1)</script>",
    )
    .expect_err("an unexpected script element is refused");

    assert_eq!(
        error,
        "canvas app unavailable: canvas-app.html carried unexpected script elements"
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

async fn send_recall(
    socket: &mut ClientSocket,
    id: wire::CanonicalUuid,
    lane: wire::CanonicalUuid,
) {
    send_client(
        socket,
        &wire::Command {
            v: wire::WireVersion::CURRENT,
            id,
            epoch: epoch(),
            kind: wire::CommandKind::LaneRecall,
            expected: Vec::new(),
            body: wire::intent::LaneRecall { lane: Some(lane) },
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
    assert!(packed.rows.is_empty());
    assert!(packed.overflow.is_none());
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
async fn authenticated_socket_streams_bounded_bus_pages_updates_resume_and_typed_shutdown() {
    let app = TestApp::new();
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
        generation(&app, capabilities),
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

/// A probe is answered on a socket that carries no subscription at all.
///
/// The client sends this when nothing has arrived for a while, which on a quiet
/// board is the normal state rather than a broken one. So the answer cannot
/// depend on there being a subscription to answer from, and it has to name the
/// probe that asked: a fixed identity would let one stale answer satisfy every
/// later probe, which is exactly the half-open socket the probe exists to catch.
#[tokio::test(flavor = "current_thread")]
async fn a_liveness_probe_is_answered_per_probe_and_still_admits_its_generation() {
    let app = TestApp::new();
    let bus = bus::Bus::start(journal::Journal::in_memory().expect("authority opens"))
        .expect("bus starts");
    commit_stream(&bus, "probe horizon", false);
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, _control) = Server::bind_streaming(
        Binding::new(LOOPBACK_EPHEMERAL, Access::local()),
        generation(&app, capabilities),
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

    for probe in [identity(0xf1), identity(0xf2)] {
        send_client(
            &mut socket,
            &wire::StreamRequest::Ping {
                v: wire::WireVersion::CURRENT,
                id: probe,
                epoch: epoch(),
            },
        )
        .await;
        assert!(
            matches!(
                stream_frame(&mut socket).await,
                wire::StreamFrame::Pong { id, epoch: answered, .. }
                    if id == probe && answered == epoch()
            ),
            "each probe is answered by its own identity"
        );
    }

    // A probe addressed to a replaced generation is told so. Answering it with
    // a Pong would report a healthy socket to a client whose whole view is
    // stale, which is the one case where staying connected is the wrong move.
    send_client(
        &mut socket,
        &wire::StreamRequest::Ping {
            v: wire::WireVersion::CURRENT,
            id: identity(0xf3),
            epoch: identity(0xf9),
        },
    )
    .await;
    assert!(matches!(
        stream_frame(&mut socket).await,
        wire::StreamFrame::Fault { fault, .. } if fault.reason == "stream::epoch-mismatch"
    ));

    shutdown.send(()).expect("shutdown signals");
    server_task
        .await
        .expect("server task joins")
        .expect("server stops");
    bus.shutdown().expect("bus shuts down");
}

/// One held command cannot own the authenticated socket's receive loop.
///
/// The first `ControlRequest` is the event-driven barrier: retaining it keeps
/// its response oneshot open without sleeping or substituting another route.
/// The probe and sibling recall cross the same real WebSocket after that
/// barrier. Answering the sibling first proves response correlation is by
/// command identity rather than request position; a final probe is a sentinel
/// proving neither response was duplicated before the next known frame.
#[tokio::test(flavor = "current_thread")]
async fn a_held_recall_does_not_block_a_probe_or_sibling_recall_on_one_socket() {
    let app = TestApp::new();
    let bus = bus::Bus::start(journal::Journal::in_memory().expect("authority opens"))
        .expect("bus starts");
    commit_stream(&bus, "concurrent command horizon", false);
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, mut control) = Server::bind_streaming(
        Binding::new(LOOPBACK_EPHEMERAL, Access::local()),
        generation(&app, capabilities),
        bus.reader(),
        2,
    )
    .await
    .expect("streaming server binds");
    let address = server.local_addr().expect("address");
    let (shutdown, stopped) = oneshot::channel();
    let server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));
    let (mut socket, _) = authenticate(address, operator(), epoch()).await;

    let lane_a = identity(0xa1);
    let lane_b = identity(0xb1);
    let recall_a = identity(0xa2);
    let recall_b = identity(0xb2);
    let probe = identity(0xc1);
    send_recall(&mut socket, recall_a, lane_a).await;
    let first = control.recv().await.expect("first recall enters Control");
    assert!(matches!(
        first.command().body(),
        wire::intent::Intent::LaneRecall(wire::intent::LaneRecall { lane: Some(lane) })
            if *lane == lane_a
    ));

    send_client(
        &mut socket,
        &wire::StreamRequest::Ping {
            v: wire::WireVersion::CURRENT,
            id: probe,
            epoch: epoch(),
        },
    )
    .await;
    send_recall(&mut socket, recall_b, lane_b).await;
    let second = timeout(ASSERTION_DEADLINE, control.recv())
        .await
        .expect("sibling recall enters Control while the first response is held")
        .expect("Control remains open");
    assert!(matches!(
        second.command().body(),
        wire::intent::Intent::LaneRecall(wire::intent::LaneRecall { lane: Some(lane) })
            if *lane == lane_b
    ));
    assert!(matches!(
        stream_frame(&mut socket).await,
        wire::StreamFrame::Pong { id, .. } if id == probe
    ));

    let second_sent = second
        .respond(wire::Outcome::Refusal(wire::Refusal::bare(
            wire::RefusalDiagnostic::new("fixture::lane-b".to_owned(), None, None, None),
        )))
        .expect("sibling response receiver remains open");
    let answered_b = response(&mut socket).await;
    assert_eq!(answered_b.id, recall_b);
    second_sent.wait().await;

    let first_sent = first
        .respond(wire::Outcome::Refusal(wire::Refusal::bare(
            wire::RefusalDiagnostic::new("fixture::lane-a".to_owned(), None, None, None),
        )))
        .expect("held response receiver remains open");
    let answered_a = response(&mut socket).await;
    assert_eq!(answered_a.id, recall_a);
    first_sent.wait().await;

    let sentinel = identity(0xc2);
    send_client(
        &mut socket,
        &wire::StreamRequest::Ping {
            v: wire::WireVersion::CURRENT,
            id: sentinel,
            epoch: epoch(),
        },
    )
    .await;
    assert!(matches!(
        stream_frame(&mut socket).await,
        wire::StreamFrame::Pong { id, .. } if id == sentinel
    ));

    shutdown.send(()).expect("shutdown signals");
    server_task
        .await
        .expect("server task joins")
        .expect("server stops");
    assert_socket_terminated(&mut socket).await;
    bus.shutdown().expect("bus shuts down");
}

#[tokio::test(flavor = "current_thread")]
async fn authenticated_socket_reads_and_pushes_agent_messages_with_each_author() {
    let app = TestApp::new();
    let bus = bus::Bus::start(journal::Journal::in_memory().expect("authority opens"))
        .expect("bus starts");
    commit_stream(&bus, "message horizon", false);
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, _control) = Server::bind_streaming(
        Binding::new(LOOPBACK_EPHEMERAL, Access::local()),
        generation(&app, capabilities),
        bus.reader(),
        8,
    )
    .await
    .expect("streaming server binds");
    let publisher = server
        .message_publisher()
        .expect("streaming server publishes messages");
    let first = agent_message(0xe6, 10, "first author");
    let mut second = agent_message(0xe7, 40, "compaction boundary");
    second.activity = wire::AgentActivityKind::Compaction;
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

/// An inbound transport that always has another message waiting.
///
/// The session only ever asks whether a transport message is available, so a
/// source that answers immediately is a faithful stand-in for a peer that keeps
/// the socket busy, and it is ready on every poll rather than on a race.
struct ReadyInbound;

impl super::socket::Inbound for ReadyInbound {
    async fn next_message(
        &mut self,
    ) -> Option<Result<::axum::extract::ws::Message, ::axum::Error>> {
        Some(Ok(::axum::extract::ws::Message::Close(None)))
    }
}

/// Every external source a session can serve, each one ready before any is read.
fn every_source_ready() -> (
    bus::Bus,
    tokio::sync::broadcast::Sender<wire::AgentMessage>,
    Option<bus::BroadcastSubscriber>,
    Option<tokio::sync::broadcast::Receiver<wire::AgentMessage>>,
) {
    let bus = bus::Bus::start(journal::Journal::in_memory().expect("authority opens"))
        .expect("bus starts");
    let broadcasts = bus.reader().subscribe();
    for index in 0..8 {
        commit_stream(&bus, &format!("ready commit {index}"), false);
    }
    let (publisher, transcript) = tokio::sync::broadcast::channel(16);
    for offset in 0..8 {
        publisher
            .send(agent_message(0xe9, offset, "ready message"))
            .expect("the retained receiver holds the channel open");
    }
    (bus, publisher, Some(broadcasts), Some(transcript))
}

/// Each rotation step serves its own source while all five stay ready.
///
/// Every source is ready before the first call and none is exhausted by the
/// six, so the lead is the only thing left that can decide which event comes
/// back. One fixed order answers `Journal` every time; a rotation that fails to
/// cycle answers `Journal` after the first step; and the fifth call is what
/// proves the cycle closes rather than parking on its last source. One lead is
/// carried across all six calls, so the advance a session depends on is the
/// same advance under test.
///
/// Readiness is established by the assertions themselves rather than by timing.
/// Were the journal or the transcript source empty, the very step that must
/// yield it would return a later source instead and fail there, so this cannot
/// decay into a test that passes because nothing was ever ready.
#[tokio::test(flavor = "current_thread")]
async fn each_rotation_step_serves_its_own_source_while_all_five_stay_ready() {
    use super::socket::{
        CommandCompletion, Event, Lead, PendingCommands, SessionReadiness, SessionSignals,
        next_event,
    };

    let (bus, _publisher, mut broadcasts, mut messages) = every_source_ready();
    let (_closed, mut closed) = tokio::sync::watch::channel(false);
    let mut inbound = ReadyInbound;
    let mut lead = Lead::Journal;
    let retired = CancellationToken::new();
    let mut commands = PendingCommands::new();
    for _ in 0..8 {
        commands.push(Box::pin(async { CommandCompletion::Closed }));
    }

    for expected in [
        "journal",
        "message",
        "inbound",
        "command",
        "continuation",
        "journal",
    ] {
        let event = next_event(
            &mut lead,
            &mut inbound,
            SessionSignals::new(&mut closed, &retired),
            &mut broadcasts,
            &mut messages,
            &mut commands,
            SessionReadiness::live(true),
        )
        .await;
        let served = match event {
            Event::Closed(_) => "closed",
            Event::Retired => "retired",
            Event::Journal(notice) => {
                notice.expect("the journal source stayed ready");
                "journal"
            }
            Event::Message(notice) => {
                notice.expect("the transcript source stayed ready");
                "message"
            }
            Event::Inbound(_) => "inbound",
            Event::Command(completed) => {
                assert!(matches!(completed, Some(CommandCompletion::Closed)));
                "command"
            }
            Event::Continuation => "continuation",
        };
        assert_eq!(
            served, expected,
            "this rotation step must serve its own source while the others are ready"
        );
    }

    bus.shutdown().expect("bus shuts down");
}

/// Global close outranks retirement and every work source.
///
/// Fairness is bought by rotating the work sources, and the risk of that is
/// trading a starvation bug for a close-ordering one. Shutdown is re-signalled
/// before each step because a watch marks its value seen once observed, so each
/// step is a fresh race between a ready close, capability retirement, and five
/// ready work sources.
#[tokio::test(flavor = "current_thread")]
async fn close_outranks_every_ready_work_source_on_every_lead() {
    use super::socket::{
        CommandCompletion, Event, Lead, PendingCommands, SessionReadiness, SessionSignals,
        next_event,
    };

    let (bus, _publisher, mut broadcasts, mut messages) = every_source_ready();
    let (shutdown, mut closed) = tokio::sync::watch::channel(false);
    let mut inbound = ReadyInbound;
    let mut lead = Lead::Journal;
    let retired = CancellationToken::new();
    retired.cancel();
    let mut commands = PendingCommands::new();
    for _ in 0..8 {
        commands.push(Box::pin(async { CommandCompletion::Closed }));
    }

    for _ in 0..5 {
        shutdown
            .send(true)
            .expect("the session holds the watch open");
        assert!(
            matches!(
                next_event(
                    &mut lead,
                    &mut inbound,
                    SessionSignals::new(&mut closed, &retired),
                    &mut broadcasts,
                    &mut messages,
                    &mut commands,
                    SessionReadiness::live(true),
                )
                .await,
                Event::Closed(Ok(()))
            ),
            "global close must lead even with retirement and every work source ready"
        );
    }

    bus.shutdown().expect("bus shuts down");
}

#[tokio::test(flavor = "current_thread")]
async fn capability_replacement_faults_the_live_stream_without_disclosure() {
    let app = TestApp::new();
    let bus = bus::Bus::start(journal::Journal::in_memory().expect("authority opens"))
        .expect("bus starts");
    commit_stream(&bus, "initial", false);
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, _control) = Server::bind_streaming(
        Binding::new(LOOPBACK_EPHEMERAL, Access::local()),
        generation(&app, capabilities.clone()),
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
    let app = TestApp::new();
    let bus = bus::Bus::start(journal::Journal::in_memory().expect("authority opens"))
        .expect("bus starts");
    commit_stream(&bus, "initial", false);
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, mut control) = Server::bind_streaming(
        Binding::new(LOOPBACK_EPHEMERAL, Access::local()),
        generation(&app, capabilities),
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
        .respond(wire::Outcome::Refusal(wire::Refusal::bare(
            wire::RefusalDiagnostic::new("fixture::released".to_owned(), None, None, None),
        )))
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
    request_with_headers(address, host, path, &[]).await
}

async fn request_with_headers(
    address: SocketAddr,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Vec<u8> {
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("connect to canvas");
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n");
    for (name, value) in headers {
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str("Connection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
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

fn response_header<'a>(response: &'a [u8], expected: &str) -> Option<&'a str> {
    let response = std::str::from_utf8(response).ok()?;
    let head = response.split_once("\r\n\r\n")?.0;
    head.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case(expected).then(|| value.trim())
    })
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
async fn speech_endpoint_uses_the_request_worktree_server_owned_argv_contract() {
    let app = TestApp::new();
    let generation = Generation::new(
        epoch(),
        Capabilities::default(),
        app.version(),
        app.source(),
        crate::SpeechConfig::default(),
    )
    .with_speech_resolver(|worktree| {
        (worktree == "/worktrees/speech")
            .then(|| external_speech(&["/bin/cat"]))
            .ok_or_else(|| "unknown worktree".to_owned())
    });
    let (server, _control) = Server::bind(generation).await.expect("binds");
    let address = server.local_addr().expect("local address");
    let task = tokio::spawn(server.up());
    let body = wire::encode(&wire::SpeechRequest {
        text: "spoken exactly; $(not a shell)".to_owned(),
        worktree: "/worktrees/speech".to_owned(),
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
    let app = TestApp::new();
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
        let (server, _control) = Server::bind(Generation::new(
            epoch(),
            Capabilities::default(),
            app.version(),
            app.source(),
            speech,
        ))
        .await
        .expect("binds");
        let address = server.local_addr().expect("local address");
        let task = tokio::spawn(server.up());
        let body = wire::encode(&wire::SpeechRequest {
            text: "bounded failure".to_owned(),
            worktree: "/worktrees/speech".to_owned(),
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

    let (server, _control) = Server::bind(Generation::new(
        epoch(),
        Capabilities::default(),
        app.version(),
        app.source(),
        external_speech(&["/bin/cat"]),
    ))
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
        br#"{"text":"valid","worktree":"/worktrees/speech"}"#,
    )
    .await;
    assert!(cross_origin.starts_with(b"HTTP/1.1 403 Forbidden\r\n"));

    task.abort();
    assert!(task.await.expect_err("aborted server task").is_cancelled());
}

#[tokio::test(flavor = "current_thread")]
async fn live_server_serves_the_lane_app_and_refuses_another_host() {
    let app = TestApp::new();
    let (server, _control) = Server::bind(generation(&app, Capabilities::default()))
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

/// One worktree holding a complete app stage for exactly as long as its owner.
struct AppWorktree {
    root: tempfile::TempDir,
    generation: std::path::PathBuf,
}

impl AppWorktree {
    fn root(&self) -> &std::path::Path {
        self.root.path()
    }

    fn generation(&self) -> &std::path::Path {
        &self.generation
    }
}

/// Build an owned worktree and return the generation containing its app stage.
///
/// The bytes name their own file so a route served from the wrong one is
/// visible in the assertion.
fn app_worktree() -> AppWorktree {
    let root = tempfile::Builder::new()
        .prefix("spice-canvas-app-")
        .tempdir()
        .expect("fixture temp directory");
    let generation = seed_app_generation(root.path(), "locally built");
    AppWorktree { root, generation }
}

#[test]
fn app_worktree_directory_is_removed_during_assertion_unwind() {
    let mut root = None;
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let fixture = app_worktree();
        root = Some(fixture.root().to_path_buf());
        assert!(
            fixture
                .root()
                .join("injected-missing-assertion-path")
                .is_file(),
            "injected fixture assertion failure"
        );
    }));

    assert!(failure.is_err(), "the injected fixture assertion fails");
    let root = root.expect("the failing assertion recorded its fixture root");
    assert!(
        !root.exists(),
        "fixture directory is removed while the assertion unwinds: {}",
        root.display()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_server_keeps_serving_an_admitted_generation_from_memory() {
    let fixture = app_worktree();
    let app = crate::AppSource::resolve(fixture.root()).expect("a complete stage resolves");
    let (server, _control) = Server::bind_with_capacity(
        Binding::new(LOOPBACK_EPHEMERAL, Access::local()),
        Generation::new(
            epoch(),
            Capabilities::default(),
            app_version(&app),
            app,
            speech(),
        ),
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

    // Once the immutable generation is admitted, request completion no longer
    // depends on reopening every file from disk.
    std::fs::remove_file(fixture.generation().join("canvas-app.html"))
        .expect("fixture removes one staged file");

    let cached = request(address, &address.to_string(), "/").await;
    assert!(cached.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(response_body(&cached).starts_with(b"locally built canvas-app.html"));

    task.abort();
    assert!(task.await.expect_err("aborted server task").is_cancelled());
}

mod app;
mod lifecycle;
