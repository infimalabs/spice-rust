//! Axum loopback binding, web-origin checks, and socket capability admission.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::error::Error;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use ::axum::Router;
use ::axum::body::Body;
use ::axum::extract::State;
use ::axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use ::axum::http::header::{CONTENT_TYPE, HOST, ORIGIN};
use ::axum::http::{HeaderMap, HeaderValue, StatusCode};
use ::axum::response::{IntoResponse, Response};
use ::axum::routing::get;

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
    ) -> Result<Self, std::io::Error> {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let authority = address.to_string();
        let origin = format!("http://{authority}");
        Ok(Self {
            listener,
            state: Arc::new(SurfaceState {
                authority,
                origin,
                epoch,
                capabilities,
            }),
        })
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
        let router = Router::new()
            .route("/socket", get(socket))
            .fallback(app)
            .with_state(self.state);
        ::axum::serve(self.listener, router).await
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
    let Some(Ok(message)) = socket.recv().await else {
        return;
    };
    let bytes = match message {
        Message::Binary(bytes) => bytes.to_vec(),
        Message::Text(text) => text.as_bytes().to_vec(),
        Message::Close(_) | Message::Ping(_) | Message::Pong(_) => return,
    };
    let encoded = match wire::decode::<wire::Hello>(&bytes) {
        Ok(hello) => match state.capabilities.admit(&hello, state.epoch) {
            Ok(accepted) => wire::encode(&accepted),
            Err(OpeningError::Refused(refusal)) => wire::encode(&refusal),
            Err(OpeningError::Fault(fault)) => wire::encode(&fault),
        },
        Err(error) => wire::encode(&wire::Fault {
            class: wire::FaultClass::Protocol,
            reason: "hello::undecodable".to_owned(),
            detail: error.to_string(),
        }),
    };
    if let Ok(bytes) = encoded {
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
        let server = Server::bind(epoch(), capabilities).await.expect("binds");
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
        let server = Server::bind(epoch(), Capabilities::default())
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
