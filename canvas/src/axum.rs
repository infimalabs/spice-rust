//! Axum listener binding, web-origin checks, and socket capability admission.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ::axum::Router;
use ::axum::body::Body;
use ::axum::extract::DefaultBodyLimit;
use ::axum::extract::{Json, OriginalUri, RawQuery, State};
use ::axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, HOST, ORIGIN};
use ::axum::http::{HeaderMap, HeaderValue, StatusCode};
use ::axum::response::{IntoResponse, Response};
use ::axum::routing::{get, post};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, Notify, Semaphore, mpsc, oneshot, watch};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::Discovery;

mod listener;
mod socket;

use listener::ClosingListener;
use socket::{admit_headers, is_dark, reachable_as, socket};

/// The listener a caller that names no address gets: reachable from this
/// machine alone, on whatever port the kernel has free.
const LOOPBACK_EPHEMERAL: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
const SESSION_DRAIN_GRACE: Duration = Duration::from_secs(1);
const SPEECH_REQUEST_BYTES: usize = 64 * 1_024;
const SPEECH_TEXT_BYTES: usize = 32 * 1_024;
const SPEECH_AUDIO_BYTES: u64 = 16 * 1_024 * 1_024;
const SPEECH_DIAGNOSTIC_BYTES: u64 = 8 * 1_024;
const SPEECH_RUNTIME: Duration = Duration::from_secs(30);

/// Closed capability-to-principal map for one resident generation.
#[derive(Clone, Default)]
pub struct Capabilities {
    principals: Arc<RwLock<BTreeMap<wire::CanonicalUuid, wire::Actor>>>,
}

impl fmt::Debug for Capabilities {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Capabilities")
            .field(
                "principals",
                &self
                    .principals
                    .read()
                    .map_or(0, |principals| principals.len()),
            )
            .finish()
    }
}

impl Capabilities {
    /// Register an operator or lane capability exactly once.
    pub fn insert(
        &self,
        capability: wire::CanonicalUuid,
        actor: wire::Actor,
    ) -> Result<(), CapabilityError> {
        if matches!(actor, wire::Actor::Spice) {
            return Err(CapabilityError::ResidentIsNotInteractive);
        }
        let mut principals = self
            .principals
            .write()
            .map_err(|_| CapabilityError::Unavailable)?;
        match principals.entry(capability) {
            Entry::Vacant(entry) => {
                entry.insert(actor);
                Ok(())
            }
            Entry::Occupied(_) => Err(CapabilityError::Duplicate),
        }
    }

    /// Remove one exact capability mapping during a failed or retired seat.
    pub fn remove(&self, capability: wire::CanonicalUuid, actor: &wire::Actor) -> bool {
        let Ok(mut principals) = self.principals.write() else {
            return false;
        };
        if principals.get(&capability) != Some(actor) {
            return false;
        }
        principals.remove(&capability);
        true
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
            .read()
            .map_err(|_| {
                OpeningError::Fault(wire::Fault {
                    class: wire::FaultClass::Environment,
                    reason: "canvas::capability-unavailable".to_owned(),
                    detail: "resident capability registry is unavailable".to_owned(),
                })
            })?
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
        self.principals.read().ok().is_some_and(|principals| {
            principals
                .get(&capability)
                .is_some_and(|actor| matches!(actor, wire::Actor::Operator))
        })
    }

    fn matches(&self, capability: wire::CanonicalUuid, actor: &wire::Actor) -> bool {
        self.principals
            .read()
            .ok()
            .is_some_and(|principals| principals.get(&capability) == Some(actor))
    }

    fn contains(&self, capability: wire::CanonicalUuid) -> bool {
        self.principals
            .read()
            .ok()
            .is_some_and(|principals| principals.contains_key(&capability))
    }

    fn operator(&self) -> Option<wire::CanonicalUuid> {
        let principals = self.principals.read().ok()?;
        let mut operators = principals.iter().filter_map(|(capability, actor)| {
            matches!(actor, wire::Actor::Operator).then_some(*capability)
        });
        let operator = operators.next()?;
        operators.next().is_none().then_some(operator)
    }
}

/// Capability registration was structurally invalid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityError {
    Duplicate,
    ResidentIsNotInteractive,
    NotOperator,
    /// The shared registry could not be acquired.
    Unavailable,
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Duplicate => "canvas capability is already registered",
            Self::ResidentIsNotInteractive => "resident actor cannot own an interactive capability",
            Self::NotOperator => "canvas discovery requires an operator capability",
            Self::Unavailable => "canvas capability registry is unavailable",
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
    authority: Authority,
    access: Access,
    origin: String,
    epoch: wire::CanonicalUuid,
    capabilities: Capabilities,
    app: crate::AppSource,
    speech: crate::SpeechConfig,
    speech_slot: Semaphore,
    admission: Mutex<Admission>,
    commands: mpsc::Sender<ControlRequest>,
    stream: Option<crate::stream::Source>,
    closed: watch::Sender<bool>,
    connections: CancellationToken,
    sessions: AtomicUsize,
    sessions_closed: Notify,
    #[cfg(test)]
    admission_observed: watch::Sender<Option<AdmissionObservation>>,
}

/// Authentication required before a caller may learn or use operator authority.
#[derive(Clone, Default)]
pub struct Access {
    token: Option<Arc<str>>,
}

impl Access {
    /// Local-only surface whose network boundary is its authentication boundary.
    #[must_use]
    pub const fn local() -> Self {
        Self { token: None }
    }

    /// Remotely reachable surface guarded by one caller-held token.
    #[must_use]
    pub fn authenticated(token: String) -> Self {
        Self {
            token: Some(Arc::from(token)),
        }
    }

    fn is_authenticated(&self) -> bool {
        self.token.as_deref().is_some_and(|token| !token.is_empty())
    }

    fn admits_token(&self, query: Option<&str>) -> bool {
        self.token.as_deref().is_none_or(|expected| {
            query_secret(query).is_some_and(|got| {
                constant_time_eq::constant_time_eq(got.as_bytes(), expected.as_bytes())
            })
        })
    }

    fn admits_socket(&self, query: Option<&str>, capabilities: &Capabilities) -> bool {
        self.token.is_none()
            || query_secret(query).is_some_and(|got| {
                self.token.as_deref().is_some_and(|expected| {
                    constant_time_eq::constant_time_eq(got.as_bytes(), expected.as_bytes())
                }) || got
                    .parse::<wire::CanonicalUuid>()
                    .is_ok_and(|capability| capabilities.contains(capability))
            })
    }
}

impl fmt::Debug for Access {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Access")
            .field("authenticated", &self.token.is_some())
            .finish()
    }
}

/// Listener address and the authentication boundary guarding it.
#[derive(Clone, Debug)]
pub struct Binding {
    requested: SocketAddr,
    access: Access,
}

impl Binding {
    /// Pair a caller-owned listener address with its access policy.
    #[must_use]
    pub const fn new(requested: SocketAddr, access: Access) -> Self {
        Self { requested, access }
    }
}

fn query_secret(query: Option<&str>) -> Option<&str> {
    query?
        .strip_prefix("auth=")
        .filter(|secret| !secret.is_empty() && !secret.contains('&'))
}

/// Which `Host` headers a bound listener answers to.
#[derive(Debug, Eq, PartialEq)]
enum Authority {
    /// The one `host:port` a specific bind can be reached at.
    Only(String),
    /// Any host at this port, because a wildcard bind has no single name.
    AnyHostAt(u16),
}

impl Authority {
    /// Whether a `Host` header names this surface.
    ///
    /// A specific bind knows its own name and accepts nothing else. A wildcard
    /// bind answers on every address the machine has, including ones added
    /// after it started, so the port is the only part of the name it can check;
    /// the same-origin check that follows admission is what keeps another site
    /// from driving this one through a browser that reached it.
    fn admits(&self, host: &str) -> bool {
        match self {
            Self::Only(authority) => host == authority,
            Self::AnyHostAt(port) => host.rsplit_once(':').is_some_and(|(named, listening)| {
                !named.is_empty() && listening.parse::<u16>() == Ok(*port)
            }),
        }
    }
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
        let mut closed = self.closed.subscribe();
        let permit = tokio::select! {
            biased;
            changed = closed.changed() => {
                let _ = changed;
                return Err(EnqueueError::NotAdmitting);
            },
            _ = self.connections.cancelled() => return Err(EnqueueError::NotAdmitting),
            permit = self.commands.reserve() => {
                permit.map_err(|_| EnqueueError::ControlClosed)?
            }
        };
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
        self.closed.send_replace(true);
        *self.admission.lock().await = Admission::Closed;
        let drain = async {
            loop {
                let notified = self.sessions_closed.notified();
                if self.sessions.load(Ordering::Acquire) == 0 {
                    break;
                }
                notified.await;
            }
        };
        let _ = tokio::time::timeout(SESSION_DRAIN_GRACE, drain).await;
        self.connections.cancel();
    }
}

struct SessionGuard {
    state: Arc<SurfaceState>,
}

impl SessionGuard {
    fn enter(state: &Arc<SurfaceState>) -> Self {
        state.sessions.fetch_add(1, Ordering::AcqRel);
        Self {
            state: Arc::clone(state),
        }
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if self.state.sessions.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.state.sessions_closed.notify_waiters();
        }
    }
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

/// One listener and its generation-scoped router.
#[derive(Debug)]
pub struct Server {
    listener: ClosingListener,
    state: Arc<SurfaceState>,
}

impl Server {
    /// Bind an ephemeral IPv4 loopback port with one resolved lane app.
    pub async fn bind(
        epoch: wire::CanonicalUuid,
        capabilities: Capabilities,
        app: crate::AppSource,
        speech: crate::SpeechConfig,
    ) -> Result<(Self, Control), std::io::Error> {
        Self::bind_with_capacity(
            Binding::new(LOOPBACK_EPHEMERAL, Access::local()),
            epoch,
            capabilities,
            app,
            speech,
            None,
            64,
        )
        .await
    }

    /// Bind the command socket and Bus-backed canvas stream to one generation.
    ///
    /// `binding` carries the address and access policy the caller owns: an
    /// operator reaching this surface from another machine needs a routable one
    /// and a port that survives a restart, so neither can be decided here.
    pub async fn bind_streaming(
        binding: Binding,
        epoch: wire::CanonicalUuid,
        capabilities: Capabilities,
        app: crate::AppSource,
        speech: crate::SpeechConfig,
        bus: bus::BusReader,
        rows: usize,
    ) -> Result<(Self, Control), std::io::Error> {
        Self::bind_with_capacity(
            binding,
            epoch,
            capabilities,
            app,
            speech,
            Some(crate::stream::Source::new(bus, epoch, rows)),
            64,
        )
        .await
    }

    async fn bind_with_capacity(
        binding: Binding,
        epoch: wire::CanonicalUuid,
        capabilities: Capabilities,
        app: crate::AppSource,
        speech: crate::SpeechConfig,
        stream: Option<crate::stream::Source>,
        capacity: usize,
    ) -> Result<(Self, Control), std::io::Error> {
        let Binding { requested, access } = binding;
        if !is_loopback(requested) && !access.is_authenticated() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "a non-loopback canvas listener requires authenticated access",
            ));
        }
        let listener = tokio::net::TcpListener::bind(requested).await?;
        let (authority, origin) = reachable_as(listener.local_addr()?);
        let (commands, receiver) = mpsc::channel(capacity);
        let (closed, _) = watch::channel(false);
        let connections = CancellationToken::new();
        #[cfg(test)]
        let (admission_observed, _) = watch::channel(None);
        Ok((
            Self {
                listener: ClosingListener::new(listener, connections.clone()),
                state: Arc::new(SurfaceState {
                    authority,
                    access,
                    origin,
                    epoch,
                    capabilities,
                    app,
                    speech,
                    speech_slot: Semaphore::new(1),
                    admission: Mutex::new(Admission::Open),
                    commands,
                    stream,
                    closed,
                    connections,
                    sessions: AtomicUsize::new(0),
                    sessions_closed: Notify::new(),
                    #[cfg(test)]
                    admission_observed,
                }),
            },
            Control { receiver },
        ))
    }

    /// Bound address, which is what was asked for with the port resolved.
    pub fn local_addr(&self) -> Result<SocketAddr, std::io::Error> {
        self.listener.bound_address()
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

    /// Serve the lane-built app and current-wire socket until the task is cancelled.
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
            .route("/bootstrap", get(bootstrap))
            .route("/speech", post(speech))
            .layer(DefaultBodyLimit::max(SPEECH_REQUEST_BYTES))
            .fallback(app)
            .with_state(state);
        let connections = closing.connections.clone();
        let served = ::axum::serve(self.listener, router)
            .with_graceful_shutdown(async move {
                shutdown.await;
                closing.close().await;
            })
            .await;
        connections.cancel();
        served
    }
}

async fn app(
    State(state): State<Arc<SurfaceState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    if admit_headers(&state, &headers, false).is_err() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let asset = match state.app.served(uri.path()) {
        Ok(Some(asset)) => asset,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        // The source was whole when this generation started, so a read that
        // fails now is the local stage being rebuilt or removed underneath the
        // resident, which is a served answer nobody should cache.
        Err(error) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("canvas app unavailable: {error}"),
            )
                .into_response();
        }
    };
    let content_type = asset.content_type();
    let digest = asset.digest();
    let mut response = Response::new(Body::from(asset.into_bytes()));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        ETAG,
        HeaderValue::from_str(&format!("\"{digest}\""))
            .expect("a canonical digest is an admitted ETag"),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    install_browser_headers(&mut response);
    response
}

async fn bootstrap(
    State(state): State<Arc<SurfaceState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if !state.access.admits_token(query.as_deref()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(host) = admit_headers(&state, &headers, false) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let Some(capability) = state.capabilities.operator() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    // The socket this names has to be one the asking client can reach, which
    // the published endpoint is not when a remote browser is on the other end
    // of a wildcard bind. It reached this surface somehow; that is the address.
    let discovery = wire::CanvasBootstrap {
        endpoint: format!("http://{host}"),
        capability,
        epoch: state.epoch,
        speech: state.speech.playback(),
    };
    let Ok(bytes) = wire::encode(&discovery) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    install_browser_headers(&mut response);
    response
}

async fn speech(
    State(state): State<Arc<SurfaceState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    Json(request): Json<wire::SpeechRequest>,
) -> Response {
    if !state.access.admits_token(query.as_deref()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if admit_headers(&state, &headers, true).is_err() {
        return StatusCode::FORBIDDEN.into_response();
    }
    if request.text.is_empty() || request.text.len() > SPEECH_TEXT_BYTES {
        return speech_error(
            StatusCode::BAD_REQUEST,
            "speech text must be nonempty and at most 32768 bytes",
        );
    }
    if state.speech.backend() != crate::SpeechBackend::External || state.speech.command().is_none()
    {
        return speech_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "external speech backend is unavailable",
        );
    }
    let Ok(_permit) = state.speech_slot.try_acquire() else {
        return speech_error(StatusCode::TOO_MANY_REQUESTS, "speech renderer is busy");
    };
    let bytes = match render_external_speech(&state.speech, request.text.as_bytes()).await {
        Ok(bytes) => bytes,
        Err(error) => return speech_error(error.status(), error.message()),
    };
    let Ok(content_type) = HeaderValue::from_str(state.speech.content_type()) else {
        return speech_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "speech media type became unavailable",
        );
    };
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(CONTENT_TYPE, content_type);
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    install_browser_headers(&mut response);
    response
}

async fn render_external_speech(
    speech: &crate::SpeechConfig,
    text: &[u8],
) -> Result<Vec<u8>, SpeechRenderError> {
    let argv = speech
        .resolved_command()
        .ok_or(SpeechRenderError::Unavailable)?;
    let mut command = tokio::process::Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| SpeechRenderError::Unavailable)?;
    let mut stdin = child.stdin.take().ok_or(SpeechRenderError::Io)?;
    let stdout = child.stdout.take().ok_or(SpeechRenderError::Io)?;
    let stderr = child.stderr.take().ok_or(SpeechRenderError::Io)?;
    let rendered = timeout(SPEECH_RUNTIME, async {
        let write = async {
            stdin.write_all(text).await?;
            stdin.shutdown().await?;
            drop(stdin);
            Ok::<_, std::io::Error>(())
        };
        let read_audio = async {
            let mut bytes = Vec::new();
            stdout
                .take(SPEECH_AUDIO_BYTES + 1)
                .read_to_end(&mut bytes)
                .await?;
            Ok::<_, std::io::Error>(bytes)
        };
        let read_diagnostic = async {
            let mut bytes = Vec::new();
            stderr
                .take(SPEECH_DIAGNOSTIC_BYTES + 1)
                .read_to_end(&mut bytes)
                .await?;
            Ok::<_, std::io::Error>(bytes)
        };
        let ((), audio, _diagnostic, status) =
            tokio::try_join!(write, read_audio, read_diagnostic, child.wait())?;
        Ok::<_, std::io::Error>((audio, status))
    })
    .await;
    let (audio, status) = match rendered {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(_)) => return Err(SpeechRenderError::Io),
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(SpeechRenderError::TimedOut);
        }
    };
    if !status.success() {
        return Err(SpeechRenderError::Failed);
    }
    if audio.is_empty() {
        return Err(SpeechRenderError::Empty);
    }
    if u64::try_from(audio.len()).is_err() || audio.len() as u64 > SPEECH_AUDIO_BYTES {
        return Err(SpeechRenderError::TooLarge);
    }
    Ok(audio)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SpeechRenderError {
    Unavailable,
    TimedOut,
    Failed,
    Empty,
    TooLarge,
    Io,
}

impl SpeechRenderError {
    const fn status(self) -> StatusCode {
        match self {
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::TimedOut => StatusCode::GATEWAY_TIMEOUT,
            Self::Failed | Self::Empty | Self::TooLarge | Self::Io => StatusCode::BAD_GATEWAY,
        }
    }

    const fn message(self) -> &'static str {
        match self {
            Self::Unavailable => "external speech backend is unavailable",
            Self::TimedOut => "external speech backend timed out",
            Self::Failed => "external speech backend failed",
            Self::Empty => "external speech backend produced no audio",
            Self::TooLarge => "external speech backend exceeded the audio limit",
            Self::Io => "external speech backend I/O failed",
        }
    }
}

fn speech_error(status: StatusCode, message: &'static str) -> Response {
    let mut response = (status, message).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    install_browser_headers(&mut response);
    response
}

fn install_browser_headers(response: &mut Response) {
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'self'; connect-src 'self' ws:; img-src 'self'; object-src 'none'; \
             script-src 'self' 'wasm-unsafe-eval' \
             'sha256-B50SC8VLF+K2T0Re7VCfyZ3PqVhYIkg+OlhW+7PQyTU='; style-src 'self'; \
             media-src 'self' blob:; \
             base-uri 'none'; \
             frame-ancestors 'none'",
        ),
    );
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
}

/// Whether an address is reachable from this machine alone.
#[must_use]
pub const fn is_loopback(address: SocketAddr) -> bool {
    matches!(address.ip(), IpAddr::V4(ip) if ip.is_loopback())
}

#[cfg(test)]
mod tests;
