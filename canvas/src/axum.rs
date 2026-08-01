//! Axum loopback binding, web-origin checks, and socket capability admission.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ::axum::Router;
use ::axum::body::Body;
use ::axum::extract::State;
use ::axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use ::axum::http::header::{CONTENT_TYPE, HOST, ORIGIN};
use ::axum::http::{HeaderMap, HeaderValue, StatusCode};
use ::axum::response::{IntoResponse, Response};
use ::axum::routing::get;
use tokio::sync::{mpsc, oneshot, watch};

use crate::{Discovery, EMBEDDED_APP};

/// Closed capability-to-principal map for one resident generation.
#[derive(Clone, Default)]
pub struct Capabilities {
    principals: BTreeMap<wire::CanonicalUuid, wire::Actor>,
}

impl fmt::Debug for Capabilities {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Capabilities")
            .field("principals", &self.principals.len())
            .finish()
    }
}

impl Capabilities {
    /// Register an operator or lane capability exactly once.
    pub fn insert(
        &mut self,
        capability: wire::CanonicalUuid,
        actor: wire::Actor,
    ) -> Result<(), CapabilityError> {
        if matches!(actor, wire::Actor::Spice) {
            return Err(CapabilityError::ResidentIsNotInteractive);
        }
        match self.principals.entry(capability) {
            Entry::Vacant(entry) => {
                entry.insert(actor);
                Ok(())
            }
            Entry::Occupied(_) => Err(CapabilityError::Duplicate),
        }
    }

    /// Admit a current hello and derive its principal server-side.
    pub fn admit(
        &self,
        hello: &wire::Hello,
        epoch: wire::CanonicalUuid,
    ) -> Result<wire::Accepted, OpeningError> {
        hello.admit(epoch).map_err(|rejected| match rejected {
            wire::Rejected::Wire(refusal) => OpeningError::Refused(refusal),
            wire::Rejected::Epoch(fault) => OpeningError::Fault(fault),
        })?;
        let actor = self
            .principals
            .get(&hello.capability)
            .cloned()
            .ok_or_else(|| {
                OpeningError::Refused(wire::Refusal {
                    reason: "hello::capability-unknown".to_owned(),
                    witness: None,
                    expected: None,
                    observed: None,
                    remedy:
                        "read the current owner-only canvas discovery file and open a new session"
                            .to_owned(),
                })
            })?;
        Ok(wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor,
            epoch,
        })
    }

    fn is_operator(&self, capability: wire::CanonicalUuid) -> bool {
        self.principals
            .get(&capability)
            .is_some_and(|actor| matches!(actor, wire::Actor::Operator))
    }
}

/// Capability registration was structurally invalid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityError {
    Duplicate,
    ResidentIsNotInteractive,
    NotOperator,
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Duplicate => "canvas capability is already registered",
            Self::ResidentIsNotInteractive => "resident actor cannot own an interactive capability",
            Self::NotOperator => "canvas discovery requires an operator capability",
        })
    }
}

impl Error for CapabilityError {}

/// Opening failed before a capability-scoped session existed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpeningError {
    Refused(wire::Refusal),
    Fault(wire::Fault),
}

impl fmt::Display for OpeningError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(refusal) => {
                write!(formatter, "socket opening refused: {}", refusal.reason)
            }
            Self::Fault(fault) => write!(formatter, "socket opening faulted: {}", fault.reason),
        }
    }
}

impl Error for OpeningError {}

#[derive(Debug)]
struct SurfaceState {
    authority: String,
    origin: String,
    epoch: wire::CanonicalUuid,
    capabilities: Capabilities,
    accepting: AtomicBool,
    commands: mpsc::Sender<ControlRequest>,
    closed: watch::Sender<bool>,
}

/// One command received from an authenticated operator or lane session.
#[derive(Debug)]
pub struct ControlRequest {
    command: wire::AdmittedCommand,
    answer: oneshot::Sender<wire::Outcome<wire::intent::CommandAnswer>>,
}

impl ControlRequest {
    /// Command admitted before this request reached resident business logic.
    #[must_use]
    pub const fn command(&self) -> &wire::AdmittedCommand {
        &self.command
    }

    /// Return the resident's terminal outcome to the authenticated session.
    pub fn respond(
        self,
        outcome: wire::Outcome<wire::intent::CommandAnswer>,
    ) -> Result<(), wire::Outcome<wire::intent::CommandAnswer>> {
        self.answer.send(outcome)
    }
}

/// Bounded receiver for authenticated resident command requests.
#[derive(Debug)]
pub struct Control {
    receiver: mpsc::Receiver<ControlRequest>,
}

impl Control {
    /// Wait for the next authenticated operator or lane request.
    pub async fn recv(&mut self) -> Option<ControlRequest> {
        self.receiver.recv().await
    }

    /// Stop admitting senders while preserving requests already in the queue.
    pub fn close(&mut self) {
        self.receiver.close();
    }

    /// Take one already-admitted request without waiting.
    pub fn try_recv(&mut self) -> Option<ControlRequest> {
        self.receiver.try_recv().ok()
    }
}

/// One loopback-only listener and its generation-scoped router.
#[derive(Debug)]
pub struct Server {
    listener: tokio::net::TcpListener,
    state: Arc<SurfaceState>,
}

impl Server {
    /// Bind an ephemeral IPv4 loopback port.
    pub async fn bind(
        epoch: wire::CanonicalUuid,
        capabilities: Capabilities,
    ) -> Result<(Self, Control), std::io::Error> {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let authority = address.to_string();
        let origin = format!("http://{authority}");
        let (commands, receiver) = mpsc::channel(64);
        let (closed, _) = watch::channel(false);
        Ok((
            Self {
                listener,
                state: Arc::new(SurfaceState {
                    authority,
                    origin,
                    epoch,
                    capabilities,
                    accepting: AtomicBool::new(true),
                    commands,
                    closed,
                }),
            },
            Control { receiver },
        ))
    }

    /// Bound address, always IPv4 loopback.
    pub fn local_addr(&self) -> Result<SocketAddr, std::io::Error> {
        self.listener.local_addr()
    }

    /// Discovery document for the registered operator capability.
    pub fn discovery(&self, capability: wire::CanonicalUuid) -> Result<Discovery, CapabilityError> {
        if !self.state.capabilities.is_operator(capability) {
            return Err(CapabilityError::NotOperator);
        }
        Ok(Discovery {
            endpoint: self.state.origin.clone(),
            capability,
            epoch: self.state.epoch,
        })
    }

    /// Serve the embedded app and current-wire socket until the task is cancelled.
    pub async fn up(self) -> Result<(), std::io::Error> {
        self.up_until(std::future::pending()).await
    }

    /// Serve until shutdown is requested, then finish admitted connections.
    pub async fn up_until<F>(self, shutdown: F) -> Result<(), std::io::Error>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let state = self.state;
        let closing = Arc::clone(&state);
        let router = Router::new()
            .route("/socket", get(socket))
            .fallback(app)
            .with_state(state);
        ::axum::serve(self.listener, router)
            .with_graceful_shutdown(async move {
                shutdown.await;
                closing.accepting.store(false, Ordering::Release);
                let _ = closing.closed.send(true);
            })
            .await
    }
}

async fn app(State(state): State<Arc<SurfaceState>>, headers: HeaderMap) -> Response {
    if admit_headers(&state, &headers, false).is_err() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = Response::new(Body::from(EMBEDDED_APP.bytes()));
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static(EMBEDDED_APP.content_type()),
    );
    response
}

async fn socket(
    State(state): State<Arc<SurfaceState>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if admit_headers(&state, &headers, true).is_err() {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !state.accepting.load(Ordering::Acquire) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    upgrade
        .on_upgrade(move |socket| socket_session(socket, state))
        .into_response()
}

fn admit_headers(
    state: &SurfaceState,
    headers: &HeaderMap,
    origin_required: bool,
) -> Result<(), ()> {
    let host = headers.get(HOST).and_then(|value| value.to_str().ok());
    if host != Some(state.authority.as_str()) {
        return Err(());
    }
    let origin = headers.get(ORIGIN).and_then(|value| value.to_str().ok());
    if origin_required && origin != Some(state.origin.as_str()) {
        return Err(());
    }
    if origin.is_some_and(|origin| origin != state.origin) {
        return Err(());
    }
    Ok(())
}

async fn socket_session(mut socket: WebSocket, state: Arc<SurfaceState>) {
    let Some(accepted) = open_session(&mut socket, &state).await else {
        return;
    };
    send_encoded(&mut socket, &accepted).await;
    let mut closed = state.closed.subscribe();
    while let Some(bytes) = receive_command(&mut socket, &mut closed).await {
        if matches!(
            route_command(&mut socket, &state, &accepted, &bytes).await,
            SessionFlow::Close
        ) {
            return;
        }
    }
}

async fn open_session(socket: &mut WebSocket, state: &SurfaceState) -> Option<wire::Accepted> {
    let Some(Ok(message)) = socket.recv().await else {
        return None;
    };
    let bytes = match message {
        Message::Binary(bytes) => bytes.to_vec(),
        Message::Text(text) => text.as_bytes().to_vec(),
        Message::Close(_) | Message::Ping(_) | Message::Pong(_) => return None,
    };
    match wire::decode::<wire::Hello>(&bytes) {
        Ok(hello) => match state.capabilities.admit(&hello, state.epoch) {
            Ok(accepted) => Some(accepted),
            Err(OpeningError::Refused(refusal)) => {
                send_encoded(socket, &refusal).await;
                None
            }
            Err(OpeningError::Fault(fault)) => {
                send_encoded(socket, &fault).await;
                None
            }
        },
        Err(error) => {
            send_encoded(
                socket,
                &wire::Fault {
                    class: wire::FaultClass::Protocol,
                    reason: "hello::undecodable".to_owned(),
                    detail: error.to_string(),
                },
            )
            .await;
            None
        }
    }
}

async fn receive_command(
    socket: &mut WebSocket,
    closed: &mut watch::Receiver<bool>,
) -> Option<Vec<u8>> {
    loop {
        let message = tokio::select! {
            changed = closed.changed() => {
                if changed.is_ok() && *closed.borrow() {
                    return None;
                }
                continue;
            }
            message = socket.recv() => message,
        };
        let Some(Ok(message)) = message else {
            return None;
        };
        match message {
            Message::Binary(bytes) => return Some(bytes.to_vec()),
            Message::Text(text) => return Some(text.as_bytes().to_vec()),
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => return None,
        }
    }
}

#[derive(Clone, Copy)]
enum SessionFlow {
    Continue,
    Close,
}

async fn route_command(
    socket: &mut WebSocket,
    state: &SurfaceState,
    accepted: &wire::Accepted,
    bytes: &[u8],
) -> SessionFlow {
    let crossing = match wire::decode::<wire::Command<serde_json::Value>>(bytes) {
        Ok(command) => command,
        Err(error) => {
            send_encoded(socket, &undecodable_command(error)).await;
            return SessionFlow::Continue;
        }
    };
    let id = crossing.id;
    let command = match crossing.admit(accepted) {
        Ok(command) => command,
        Err(rejection) => {
            send_encoded(
                socket,
                &wire::Response {
                    v: wire::WireVersion::CURRENT,
                    id,
                    epoch: state.epoch,
                    outcome: rejection.outcome(),
                },
            )
            .await;
            return SessionFlow::Continue;
        }
    };
    let dark = is_dark(&command);
    if !begin_request(state, &command, dark, socket).await {
        return SessionFlow::Continue;
    }
    let response = command.clone();
    let (answer, returned) = oneshot::channel();
    if state
        .commands
        .send(ControlRequest { command, answer })
        .await
        .is_err()
    {
        send_encoded(socket, &response.response(closing_refusal())).await;
        return SessionFlow::Close;
    }
    let Ok(outcome) = returned.await else {
        return SessionFlow::Close;
    };
    let reached_dark = dark && matches!(outcome, wire::Outcome::Reply(_));
    send_encoded(socket, &response.response(outcome)).await;
    if dark && !reached_dark {
        state.accepting.store(true, Ordering::Release);
    }
    if reached_dark {
        SessionFlow::Close
    } else {
        SessionFlow::Continue
    }
}

async fn begin_request(
    state: &SurfaceState,
    command: &wire::AdmittedCommand,
    dark: bool,
    socket: &mut WebSocket,
) -> bool {
    let admitted = if dark {
        state.accepting.swap(false, Ordering::AcqRel)
    } else {
        state.accepting.load(Ordering::Acquire)
    };
    if !admitted {
        send_encoded(socket, &command.response(closing_refusal())).await;
    }
    admitted
}

fn undecodable_command(error: serde_json::Error) -> wire::Fault {
    wire::Fault {
        class: wire::FaultClass::Protocol,
        reason: "command::undecodable".to_owned(),
        detail: error.to_string(),
    }
}

fn is_dark(command: &wire::AdmittedCommand) -> bool {
    matches!(
        command.body(),
        wire::intent::Intent::UpChange(wire::intent::UpChange {
            state: wire::ResidentState::Dark
        })
    )
}

fn closing_refusal() -> wire::Outcome<wire::intent::CommandAnswer> {
    wire::Outcome::Refusal(wire::Refusal {
        reason: "resident::not-admitting".to_owned(),
        witness: Some("resident-state".to_owned()),
        expected: Some("seated".to_owned()),
        observed: Some("quiescing".to_owned()),
        remedy: "wait for shutdown to finish and reconnect to the next resident epoch".to_owned(),
    })
}

async fn send_encoded<T: serde::Serialize>(socket: &mut WebSocket, value: &T) {
    if let Ok(bytes) = wire::encode(value) {
        let _ = socket.send(Message::Binary(bytes.into())).await;
    }
}

/// True only for the one address family this surface may bind.
#[must_use]
pub const fn is_loopback(address: SocketAddr) -> bool {
    matches!(address.ip(), IpAddr::V4(ip) if ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    fn uuid(text: &str) -> wire::CanonicalUuid {
        text.parse().expect("canonical fixture UUID")
    }

    fn epoch() -> wire::CanonicalUuid {
        uuid("018f82d4-6f59-7d06-92e3-bce9131163c3")
    }

    fn operator() -> wire::CanonicalUuid {
        uuid("018f82d4-6f59-7d06-92e3-bce9131163c4")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_binds_ipv4_loopback_and_publishes_that_endpoint() {
        let mut capabilities = Capabilities::default();
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

    async fn request(address: SocketAddr, host: &str) -> Vec<u8> {
        let mut stream = tokio::net::TcpStream::connect(address)
            .await
            .expect("connect to canvas");
        stream
            .write_all(
                format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
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

        let admitted = request(address, &address.to_string()).await;
        let split = admitted
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("response carries headers")
            + 4;
        assert!(admitted.starts_with(b"HTTP/1.1 200 OK\r\n"));
        assert_eq!(&admitted[split..], EMBEDDED_APP.bytes());

        let refused = request(address, "attacker.example").await;
        assert!(refused.starts_with(b"HTTP/1.1 403 Forbidden\r\n"));

        task.abort();
        assert!(task.await.expect_err("aborted server task").is_cancelled());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn authenticated_session_routes_multiple_typed_commands_and_correlates_answers() {
        let mut capabilities = Capabilities::default();
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
        let mut capabilities = Capabilities::default();
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
            accepting: AtomicBool::new(true),
            commands: mpsc::channel(1).0,
            closed: watch::channel(false).0,
        };
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("attacker.example"));
        headers.insert(ORIGIN, HeaderValue::from_static("https://attacker.example"));
        assert!(admit_headers(&state, &headers, true).is_err());

        headers.insert(HOST, HeaderValue::from_static("127.0.0.1:48123"));
        headers.insert(ORIGIN, HeaderValue::from_static("http://127.0.0.1:48123"));
        assert_eq!(admit_headers(&state, &headers, true), Ok(()));
    }
}
