//! Axum listener binding, web-origin checks, and socket capability admission.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ::axum::Router;
use ::axum::body::{Body, Bytes};
use ::axum::extract::{OriginalUri, RawQuery, State};
use ::axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, HOST, IF_NONE_MATCH, ORIGIN};
use ::axum::http::{HeaderMap, HeaderValue, StatusCode};
use ::axum::response::{IntoResponse, Response};
use ::axum::routing::get;
use base64::Engine as _;
use sha2::{Digest as _, Sha256};
use tokio::sync::{Mutex, Notify, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::Discovery;

mod control;
mod history;
mod listener;
mod socket;

use listener::ClosingListener;
use socket::{admit_headers, is_dark, reachable_as, socket};

/// The listener a caller that names no address gets: reachable from this
/// machine alone, on whatever port the kernel has free.
const LOOPBACK_EPHEMERAL: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
const SESSION_DRAIN_GRACE: Duration = Duration::from_secs(1);
/// One-frame native commands share the socket's existing message ceiling.
const SOCKET_MESSAGE_BYTES: usize = 64 * 1_024 * 1_024;
const INLINE_THEME_OPEN: &str = "<script data-app-theme>";
const INLINE_MODULE_OPEN: &str = "<script type=\"module\">";
const INLINE_MODULE_CLOSE: &str = "</script>";
const SCRIPT_OPEN: &str = "<script";
const DEFAULT_SCRIPT_HASH: &str = "B50SC8VLF+K2T0Re7VCfyZ3PqVhYIkg+OlhW+7PQyTU=";

/// Closed capability-to-principal map for one resident generation.
#[derive(Clone, Default)]
pub struct Capabilities {
    principals: Arc<RwLock<BTreeMap<wire::CanonicalUuid, CapabilityBinding>>>,
}

#[derive(Clone, Debug)]
struct CapabilityBinding {
    actor: wire::Actor,
    retired: CancellationToken,
}

struct CapabilityAdmission {
    accepted: wire::Accepted,
    retired: CancellationToken,
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
                entry.insert(CapabilityBinding {
                    actor,
                    retired: CancellationToken::new(),
                });
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
        if principals
            .get(&capability)
            .is_none_or(|binding| &binding.actor != actor)
        {
            return false;
        }
        let retired = principals
            .remove(&capability)
            .expect("matching capability remains under the registry lock")
            .retired;
        drop(principals);
        retired.cancel();
        true
    }

    /// Admit a current hello and derive its principal server-side.
    pub fn admit(
        &self,
        hello: &wire::Hello,
        epoch: wire::CanonicalUuid,
        release: &wire::manifest::ProductVersion,
    ) -> Result<wire::Accepted, OpeningError> {
        self.admit_session(hello, epoch, release)
            .map(|admission| admission.accepted)
    }

    /// Admit one socket and retain the exact binding that authenticated it.
    fn admit_session(
        &self,
        hello: &wire::Hello,
        epoch: wire::CanonicalUuid,
        release: &wire::manifest::ProductVersion,
    ) -> Result<CapabilityAdmission, OpeningError> {
        hello.admit(epoch).map_err(|rejected| match rejected {
            wire::Rejected::Wire(refusal) => OpeningError::Refused(refusal),
            wire::Rejected::Epoch(fault) => OpeningError::Fault(fault),
        })?;
        let (actor, retired) = self
            .principals
            .read()
            .map_err(|_| {
                OpeningError::Fault(wire::Fault {
                    class: wire::FaultClass::Environment,
                    reason: "serve::capability-unavailable".to_owned(),
                    detail: "resident capability registry is unavailable".to_owned(),
                })
            })?
            .get(&hello.capability)
            .map(|binding| (binding.actor.clone(), binding.retired.clone()))
            .ok_or_else(|| {
                OpeningError::Refused(wire::Refusal::bare(wire::RefusalDiagnostic::new(
                    "hello::capability-unknown",
                    None,
                    None,
                    None,
                )))
            })?;
        Ok(CapabilityAdmission {
            accepted: wire::Accepted {
                wire: wire::WireVersion::CURRENT,
                actor,
                epoch,
                release: release.clone(),
            },
            retired,
        })
    }

    fn is_operator(&self, capability: wire::CanonicalUuid) -> bool {
        self.principals.read().ok().is_some_and(|principals| {
            principals
                .get(&capability)
                .is_some_and(|binding| matches!(&binding.actor, wire::Actor::Operator))
        })
    }

    fn matches(&self, capability: wire::CanonicalUuid, actor: &wire::Actor) -> bool {
        self.principals.read().ok().is_some_and(|principals| {
            principals
                .get(&capability)
                .is_some_and(|binding| &binding.actor == actor)
        })
    }

    fn contains(&self, capability: wire::CanonicalUuid) -> bool {
        self.principals
            .read()
            .ok()
            .is_some_and(|principals| principals.contains_key(&capability))
    }

    fn operator(&self) -> Option<wire::CanonicalUuid> {
        let principals = self.principals.read().ok()?;
        let mut operators = principals.iter().filter_map(|(capability, binding)| {
            matches!(&binding.actor, wire::Actor::Operator).then_some(*capability)
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
            Self::Duplicate => "app capability is already registered",
            Self::ResidentIsNotInteractive => "resident actor cannot own an interactive capability",
            Self::NotOperator => "resident discovery requires an operator capability",
            Self::Unavailable => "app capability registry is unavailable",
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
                write!(formatter, "socket opening refused: {}", refusal.reason())
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
    version: wire::Version,
    app: crate::AppSource,
    operator: wire::OperatorIdentity,
    history: history::HistoryLimits,
    admission: Mutex<Admission>,
    admission_changed: watch::Sender<Admission>,
    commands: control::Sender<ControlRequest>,
    stream: Option<crate::stream::Source>,
    closed: watch::Sender<bool>,
    connections: CancellationToken,
    sessions: AtomicUsize,
    sessions_closed: Notify,
    #[cfg(test)]
    admission_observed: watch::Sender<Option<AdmissionObservation>>,
}

/// Network boundary applied before a caller may learn or use operator authority.
#[derive(Clone, Default)]
pub struct Access {
    token: Option<Arc<str>>,
    allow_non_loopback: bool,
}

impl Access {
    /// Local-only surface whose network boundary is its authentication boundary.
    #[must_use]
    pub const fn local() -> Self {
        Self {
            token: None,
            allow_non_loopback: false,
        }
    }

    /// Remotely reachable surface whose surrounding network is trusted.
    #[must_use]
    pub const fn insecure() -> Self {
        Self {
            token: None,
            allow_non_loopback: true,
        }
    }

    /// Remotely reachable surface guarded by one caller-held token.
    #[must_use]
    pub fn authenticated(token: String) -> Self {
        Self {
            token: Some(Arc::from(token)),
            allow_non_loopback: true,
        }
    }

    fn allows_non_loopback(&self) -> bool {
        self.allow_non_loopback
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
            .field("allow_non_loopback", &self.allow_non_loopback)
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
    let secret = query?.strip_prefix("auth=")?;
    // `gloo_net` serializes the Wasm client's sole query parameter with one
    // empty trailing separator. Admit that exact spelling without admitting a
    // second parameter or broadening the credential grammar.
    let secret = secret.strip_suffix('&').unwrap_or(secret);
    (!secret.is_empty() && !secret.contains('&')).then_some(secret)
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
    Reserving(wire::CanonicalUuid),
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
        let mut closed = self.closed.subscribe();
        let mut admission_changed = self.admission_changed.subscribe();
        if *self.admission.lock().await != Admission::Open {
            return Err(EnqueueError::NotAdmitting);
        }
        #[cfg(test)]
        self.admission_observed
            .send_replace(Some(AdmissionObservation::Reserving(command.id())));
        let class = control::class(command.kind());
        let permit = tokio::select! {
            biased;
            changed = closed.changed() => {
                let _ = changed;
                return Err(EnqueueError::NotAdmitting);
            },
            _ = self.connections.cancelled() => return Err(EnqueueError::NotAdmitting),
            changed = admission_changed.changed() => {
                let _ = changed;
                return Err(EnqueueError::NotAdmitting);
            },
            permit = self.commands.reserve(class) => {
                permit.map_err(|()| EnqueueError::ControlClosed)?
            }
        };
        let mut admission = self.admission.lock().await;
        if *admission != Admission::Open {
            return Err(EnqueueError::NotAdmitting);
        }
        let (answer, returned) = oneshot::channel();
        let (sent, response_sent) = oneshot::channel();
        if dark {
            *admission = Admission::Dark;
            self.admission_changed.send_replace(Admission::Dark);
        }
        #[cfg(test)]
        let id = command.id();
        permit.send(
            ControlRequest {
                command,
                answer,
                response_sent,
            },
            dark,
        );
        #[cfg(test)]
        self.admission_observed
            .send_replace(Some(AdmissionObservation::Queued(id)));
        Ok(PendingResponse { returned, sent })
    }

    async fn reopen_after_dark_refusal(&self) {
        let mut admission = self.admission.lock().await;
        if *admission == Admission::Dark {
            *admission = Admission::Open;
            self.admission_changed.send_replace(Admission::Open);
        }
    }

    async fn close(&self) {
        self.close_with_grace(SESSION_DRAIN_GRACE).await;
    }

    async fn close_with_grace(&self, grace: Duration) {
        self.closed.send_replace(true);
        *self.admission.lock().await = Admission::Closed;
        self.admission_changed.send_replace(Admission::Closed);
        let drain = async {
            loop {
                let notified = self.sessions_closed.notified();
                if self.sessions.load(Ordering::Acquire) == 0 {
                    break;
                }
                notified.await;
            }
        };
        let _ = tokio::time::timeout(grace, drain).await;
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

/// Server-owned bounded-priority receiver for authenticated resident commands.
#[derive(Debug)]
pub struct Control {
    receiver: control::Receiver<ControlRequest>,
}

impl Control {
    /// One complete turn through the scheduler's bounded 3:1 fairness rule.
    pub const FAIR_CYCLE_LIMIT: usize = control::FAIR_CYCLE_LIMIT;

    /// Wait for the next request under the scheduler's bounded 3:1 demand rule.
    pub async fn recv(&mut self) -> Option<ControlRequest> {
        self.receiver.recv().await
    }

    /// Receive already-scheduled work without waiting for another producer.
    pub fn try_recv(&mut self) -> Result<ControlRequest, tokio::sync::mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }

    /// Stop admitting senders while preserving requests already in the queue.
    pub fn close(&mut self) {
        self.receiver.close();
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }
}

/// One listener and its generation-scoped router.
#[derive(Debug)]
pub struct Server {
    listener: ClosingListener,
    state: Arc<SurfaceState>,
}

/// Identity, capabilities, app, and operator presentation for one server generation.
#[derive(Debug)]
pub struct Generation {
    epoch: wire::CanonicalUuid,
    capabilities: Capabilities,
    version: wire::Version,
    app: crate::AppSource,
    operator: wire::OperatorIdentity,
}

impl Generation {
    /// Bundle the values that must remain aligned for one bound server generation.
    #[must_use]
    pub fn new(
        epoch: wire::CanonicalUuid,
        capabilities: Capabilities,
        version: wire::Version,
        app: crate::AppSource,
    ) -> Self {
        Self {
            epoch,
            capabilities,
            version,
            app,
            operator: wire::OperatorIdentity::default(),
        }
    }

    /// Publish the effective operator identity through every bootstrap of this generation.
    #[must_use]
    pub fn with_operator_identity(mut self, operator: wire::OperatorIdentity) -> Self {
        self.operator = operator;
        self
    }
}
impl Server {
    /// Bind an ephemeral IPv4 loopback port with one resolved lane app.
    pub async fn bind(generation: Generation) -> Result<(Self, Control), std::io::Error> {
        Self::bind_with_capacity(
            Binding::new(LOOPBACK_EPHEMERAL, Access::local()),
            generation,
            None,
            64,
        )
        .await
    }

    /// Bind the command socket and Bus-backed app stream to one generation.
    ///
    /// `binding` carries the address and access policy the caller owns: an
    /// operator reaching this surface from another machine needs a routable one
    /// and a port that survives a restart, so neither can be decided here.
    pub async fn bind_streaming(
        binding: Binding,
        generation: Generation,
        bus: bus::BusReader,
        rows: usize,
        presenter: crate::stream::QuotePresenter,
        quota: crate::stream::QuotaFeed,
    ) -> Result<(Self, Control), std::io::Error> {
        let epoch = generation.epoch;
        Self::bind_with_capacity(
            binding,
            generation,
            Some(crate::stream::Source::new(bus, epoch, rows, presenter).with_quota(quota)),
            64,
        )
        .await
    }

    #[cfg(test)]
    async fn bind_test_streaming(
        binding: Binding,
        generation: Generation,
        bus: bus::BusReader,
        rows: usize,
    ) -> Result<(Self, Control), std::io::Error> {
        Self::bind_streaming(
            binding,
            generation,
            bus,
            rows,
            crate::stream::QuotePresenter::new(|_| Vec::new()),
            crate::stream::QuotaFeed::new(wire::QuotaGeneration::default()),
        )
        .await
    }

    async fn bind_with_capacity(
        binding: Binding,
        generation: Generation,
        stream: Option<crate::stream::Source>,
        class_capacity: usize,
    ) -> Result<(Self, Control), std::io::Error> {
        let Binding { requested, access } = binding;
        let Generation {
            epoch,
            capabilities,
            version,
            app,
            operator,
        } = generation;
        if !requested.ip().is_loopback() && !access.allows_non_loopback() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "a non-loopback app listener requires explicit access",
            ));
        }
        let app_generation = app
            .current_generation()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        if version.app != *app_generation.identity() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "app identity disagrees with the current served generation",
            ));
        }
        let listener = listener::listen_exclusively(requested).await?;
        let (authority, origin) = reachable_as(listener.local_addr()?);
        let (commands, receiver) = control::channel(class_capacity);
        let (closed, _) = watch::channel(false);
        let (admission_changed, _) = watch::channel(Admission::Open);
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
                    version,
                    app,
                    operator,
                    history: history::HistoryLimits::new(),
                    admission: Mutex::new(Admission::Open),
                    admission_changed,
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

    /// Writer for agent prose on a streaming surface.
    ///
    /// Non-streaming test and compatibility surfaces have no transcript view,
    /// so they return `None` rather than accepting messages nobody can read.
    #[must_use]
    pub fn message_publisher(&self) -> Option<crate::stream::MessagePublisher> {
        self.state
            .stream
            .as_ref()
            .map(crate::stream::Source::message_publisher)
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
    let renderer = uri
        .path()
        .rsplit('/')
        .next()
        .and_then(wire::manifest::renderer::File::parse_filename);
    if socket::admit_asset_headers(&state, &headers, renderer.is_some()).is_err() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let asset = match state.app.served(uri.path()) {
        Ok(Some(asset)) => asset,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        // A new or evicted generation must cross complete admission before it
        // can answer. Any failure here is an answer nobody should cache.
        Err(error) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("app unavailable: {error}"),
            )
                .into_response();
        }
    };
    let content_type = asset.content_type();
    let digest = asset.digest();
    let app_policy = if content_type == "text/html; charset=utf-8" {
        let policy = if renderer.is_some() {
            renderer_content_security_policy(asset.bytes())
        } else {
            app_content_security_policy(asset.bytes())
        };
        match policy {
            Ok(policy) => Some(policy),
            Err(message) => {
                return (StatusCode::SERVICE_UNAVAILABLE, message).into_response();
            }
        }
    } else {
        None
    };
    let etag = format!("\"{digest}\"");
    let mut response = if if_none_match(&headers, &etag) {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        response
    } else {
        Response::new(Body::from(Bytes::from_owner(asset.into_shared_bytes())))
    };
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        ETAG,
        HeaderValue::from_str(&etag).expect("a canonical digest is an admitted ETag"),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    install_browser_headers(&mut response);
    if let Some(policy) = app_policy {
        response
            .headers_mut()
            .insert("content-security-policy", policy);
    }
    if renderer.is_some() {
        response.headers_mut().insert(
            "access-control-allow-origin",
            HeaderValue::from_static("null"),
        );
    }
    response
}

fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|candidate| {
            candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
        })
}

mod bootstrap;
use bootstrap::bootstrap;

fn app_content_security_policy(html: &[u8]) -> Result<HeaderValue, &'static str> {
    let html = std::str::from_utf8(html).map_err(|_| "app unavailable: app.html was not UTF-8")?;
    let (before_theme, rest) = html
        .split_once(INLINE_THEME_OPEN)
        .ok_or("app unavailable: app.html did not carry one inline theme resolver")?;
    let (theme, after_theme) = rest
        .split_once(INLINE_MODULE_CLOSE)
        .ok_or("app unavailable: app.html did not close its inline theme resolver")?;
    let (between, rest) = after_theme
        .split_once(INLINE_MODULE_OPEN)
        .ok_or("app unavailable: app.html did not carry one inline module loader")?;
    let (module, after_module) = rest
        .split_once(INLINE_MODULE_CLOSE)
        .ok_or("app unavailable: app.html did not close its inline module loader")?;
    if [before_theme, between, after_module]
        .iter()
        .any(|section| section.contains(SCRIPT_OPEN) || section.contains(INLINE_MODULE_CLOSE))
    {
        return Err("app unavailable: app.html carried unexpected script elements");
    }
    let theme_hash =
        base64::engine::general_purpose::STANDARD.encode(Sha256::digest(theme.as_bytes()));
    let module_hash =
        base64::engine::general_purpose::STANDARD.encode(Sha256::digest(module.as_bytes()));
    HeaderValue::from_str(&browser_content_security_policy(&[
        &theme_hash,
        &module_hash,
    ]))
    .map_err(|_| "app unavailable: its content security policy was not one HTTP value")
}

fn renderer_content_security_policy(html: &[u8]) -> Result<HeaderValue, &'static str> {
    let html = std::str::from_utf8(html).map_err(|_| "renderer document is not UTF-8")?;
    const IMPORT_MAP: &str = "<script type=\"importmap\">";
    let hash = if let Some((_, rest)) = html.split_once(IMPORT_MAP) {
        let (map, after) = rest
            .split_once("</script>")
            .ok_or("renderer import map is not closed")?;
        if after.contains(IMPORT_MAP) {
            return Err("renderer import map is repeated");
        }
        format!(
            " 'sha256-{}'",
            base64::engine::general_purpose::STANDARD.encode(Sha256::digest(map.as_bytes()))
        )
    } else {
        String::new()
    };
    HeaderValue::from_str(&format!(
        "default-src 'none'; script-src 'self'{hash}; style-src 'unsafe-inline'; \
         img-src data:; font-src data:; connect-src 'none'; object-src 'none'; \
         base-uri 'none'; form-action 'none'; frame-src 'none'; frame-ancestors 'self'"
    ))
    .map_err(|_| "renderer content security policy is not one HTTP value")
}

fn browser_content_security_policy(script_hashes: &[&str]) -> String {
    let script_hashes = script_hashes
        .iter()
        .map(|hash| format!(" 'sha256-{hash}'"))
        .collect::<String>();
    format!(
        "default-src 'self'; connect-src 'self' ws:; img-src 'self' data: https://gravatar.com; \
         object-src 'none'; \
         script-src 'self' 'wasm-unsafe-eval'{script_hashes}; style-src 'self'; \
         media-src 'self' blob:; base-uri 'none'; frame-ancestors 'none'"
    )
}

fn install_browser_headers(response: &mut Response) {
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_str(&browser_content_security_policy(&[DEFAULT_SCRIPT_HASH]))
            .expect("the default browser policy is one static HTTP value"),
    );
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
}

#[cfg(test)]
mod tests;
