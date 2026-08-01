//! Axum loopback binding, web-origin checks, and socket capability admission.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use ::axum::Router;
use ::axum::body::Body;
use ::axum::extract::State;
use ::axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use ::axum::http::header::{CONTENT_TYPE, HOST, ORIGIN};
use ::axum::http::{HeaderMap, HeaderValue, StatusCode};
use ::axum::response::{IntoResponse, Response};
use ::axum::routing::get;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

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
    admission: Mutex<Admission>,
    commands: mpsc::Sender<ControlRequest>,
    closed: watch::Sender<bool>,
    connections: CancellationToken,
    #[cfg(test)]
    admission_observed: watch::Sender<Option<AdmissionObservation>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Admission {
    Open,
    Dark,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EnqueueError {
    NotAdmitting,
    ControlClosed,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AdmissionObservation {
    Ordering(wire::CanonicalUuid),
    Queued(wire::CanonicalUuid),
}

impl SurfaceState {
    async fn opens_session(&self) -> bool {
        *self.admission.lock().await == Admission::Open
    }

    async fn enqueue(
        &self,
        command: wire::AdmittedCommand,
    ) -> Result<PendingResponse, EnqueueError> {
        let dark = is_dark(&command);
        let mut admission = self.admission.lock().await;
        if *admission != Admission::Open {
            return Err(EnqueueError::NotAdmitting);
        }
        #[cfg(test)]
        self.admission_observed
            .send_replace(Some(AdmissionObservation::Ordering(command.id())));
        let permit = self
            .commands
            .reserve()
            .await
            .map_err(|_| EnqueueError::ControlClosed)?;
        let (answer, returned) = oneshot::channel();
        let (sent, response_sent) = oneshot::channel();
        if dark {
            *admission = Admission::Dark;
        }
        #[cfg(test)]
        let id = command.id();
        permit.send(ControlRequest {
            command,
            answer,
            response_sent,
        });
        #[cfg(test)]
        self.admission_observed
            .send_replace(Some(AdmissionObservation::Queued(id)));
        Ok(PendingResponse { returned, sent })
    }

    async fn reopen_after_dark_refusal(&self) {
        let mut admission = self.admission.lock().await;
        if *admission == Admission::Dark {
            *admission = Admission::Open;
        }
    }

    async fn close(&self) {
        *self.admission.lock().await = Admission::Closed;
        self.closed.send_replace(true);
        self.connections.cancel();
    }
}

#[derive(Debug)]
struct ClosingListener {
    listener: tokio::net::TcpListener,
    connections: CancellationToken,
}

impl ::axum::serve::Listener for ClosingListener {
    type Io = ClosingStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (stream, address) = ::axum::serve::Listener::accept(&mut self.listener).await;
        (
            ClosingStream {
                stream,
                cancelled: Box::pin(self.connections.clone().cancelled_owned()),
            },
            address,
        )
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

#[derive(Debug)]
struct ClosingStream {
    stream: TcpStream,
    cancelled: Pin<Box<WaitForCancellationFutureOwned>>,
}

impl AsyncRead for ClosingStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.cancelled.as_mut().poll(context).is_ready() {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.stream).poll_read(context, buffer)
    }
}

impl AsyncWrite for ClosingStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.cancelled.as_mut().poll(context).is_ready() {
            return Poll::Ready(Err(connection_closed()));
        }
        Pin::new(&mut self.stream).poll_write(context, buffer)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.cancelled.as_mut().poll(context).is_ready() {
            return Poll::Ready(Err(connection_closed()));
        }
        Pin::new(&mut self.stream).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(context)
    }
}

fn connection_closed() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "canvas connection closed with the resident",
    )
}

/// One command received from an authenticated operator or lane session.
#[derive(Debug)]
pub struct ControlRequest {
    command: wire::AdmittedCommand,
    answer: oneshot::Sender<wire::Outcome<wire::intent::CommandAnswer>>,
    response_sent: oneshot::Receiver<()>,
}

#[derive(Debug)]
struct PendingResponse {
    returned: oneshot::Receiver<wire::Outcome<wire::intent::CommandAnswer>>,
    sent: oneshot::Sender<()>,
}

/// Completion signal for the command response write attempt.
#[derive(Debug)]
pub struct ResponseReceipt {
    response_sent: oneshot::Receiver<()>,
}

impl ResponseReceipt {
    /// Wait until the authenticated session attempted its correlated response.
    pub async fn wait(self) {
        let _ = self.response_sent.await;
    }
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
    ) -> Result<ResponseReceipt, wire::Outcome<wire::intent::CommandAnswer>> {
        self.answer.send(outcome).map(|()| ResponseReceipt {
            response_sent: self.response_sent,
        })
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
}

/// One loopback-only listener and its generation-scoped router.
#[derive(Debug)]
pub struct Server {
    listener: ClosingListener,
    state: Arc<SurfaceState>,
}

impl Server {
    /// Bind an ephemeral IPv4 loopback port.
    pub async fn bind(
        epoch: wire::CanonicalUuid,
        capabilities: Capabilities,
    ) -> Result<(Self, Control), std::io::Error> {
        Self::bind_with_capacity(epoch, capabilities, 64).await
    }

    async fn bind_with_capacity(
        epoch: wire::CanonicalUuid,
        capabilities: Capabilities,
        capacity: usize,
    ) -> Result<(Self, Control), std::io::Error> {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let authority = address.to_string();
        let origin = format!("http://{authority}");
        let (commands, receiver) = mpsc::channel(capacity);
        let (closed, _) = watch::channel(false);
        let connections = CancellationToken::new();
        #[cfg(test)]
        let (admission_observed, _) = watch::channel(None);
        Ok((
            Self {
                listener: ClosingListener {
                    listener,
                    connections: connections.clone(),
                },
                state: Arc::new(SurfaceState {
                    authority,
                    origin,
                    epoch,
                    capabilities,
                    admission: Mutex::new(Admission::Open),
                    commands,
                    closed,
                    connections,
                    #[cfg(test)]
                    admission_observed,
                }),
            },
            Control { receiver },
        ))
    }

    /// Bound address, always IPv4 loopback.
    pub fn local_addr(&self) -> Result<SocketAddr, std::io::Error> {
        self.listener.listener.local_addr()
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
                closing.close().await;
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
    if !state.opens_session().await {
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
    let mut closed = state.closed.subscribe();
    let Some(accepted) = open_session(&mut socket, &state, &mut closed).await else {
        return;
    };
    send_encoded(&mut socket, &accepted).await;
    while let Some(bytes) = receive_command(&mut socket, &mut closed).await {
        if matches!(
            route_command(&mut socket, &state, &accepted, &bytes).await,
            SessionFlow::Close
        ) {
            return;
        }
    }
}

async fn open_session(
    socket: &mut WebSocket,
    state: &SurfaceState,
    closed: &mut watch::Receiver<bool>,
) -> Option<wire::Accepted> {
    let message = receive_message(socket, closed).await?;
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
        let message = receive_message(socket, closed).await?;
        match message {
            Message::Binary(bytes) => return Some(bytes.to_vec()),
            Message::Text(text) => return Some(text.as_bytes().to_vec()),
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => return None,
        }
    }
}

async fn receive_message(
    socket: &mut WebSocket,
    closed: &mut watch::Receiver<bool>,
) -> Option<Message> {
    loop {
        if *closed.borrow() {
            return None;
        }
        let message = tokio::select! {
            changed = closed.changed() => {
                changed.ok()?;
                continue;
            }
            message = socket.recv() => message,
        };
        let Some(Ok(message)) = message else {
            return None;
        };
        if *closed.borrow() {
            return None;
        }
        return Some(message);
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
    let response = command.clone();
    let pending = match state.enqueue(command).await {
        Ok(pending) => pending,
        Err(EnqueueError::NotAdmitting) => {
            send_encoded(socket, &response.response(closing_refusal())).await;
            return SessionFlow::Continue;
        }
        Err(EnqueueError::ControlClosed) => {
            send_encoded(socket, &response.response(closing_refusal())).await;
            return SessionFlow::Close;
        }
    };
    let Ok(outcome) = pending.returned.await else {
        return SessionFlow::Close;
    };
    let reached_dark = dark && matches!(outcome, wire::Outcome::Reply(_));
    send_encoded(socket, &response.response(outcome)).await;
    let _ = pending.sent.send(());
    if dark && !reached_dark {
        state.reopen_after_dark_refusal().await;
    }
    if reached_dark {
        SessionFlow::Close
    } else {
        SessionFlow::Continue
    }
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

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_closes_partial_upgrade_pre_hello_and_active_sockets() {
        let mut capabilities = Capabilities::default();
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
    async fn operator_lane_replacement_and_reconnect_cross_the_live_door() {
        let lane = identity(0xd0);
        let lane_capability = identity(0xd1);
        let mut capabilities = Capabilities::default();
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
        let mut replacement = Capabilities::default();
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
        let mut capabilities = Capabilities::default();
        capabilities
            .insert(operator(), wire::Actor::Operator)
            .expect("operator capability");
        let (server, mut control) = Server::bind_with_capacity(epoch(), capabilities, 1)
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
}
