//! Authenticated Canvas socket session routing.

use super::*;
use ::axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};

const CAPABILITY_REPLACED_REASON: &str = "stream::capability-replaced";
const CAPABILITY_REPLACED_DETAIL: &str =
    "the capability that authenticated this stream is no longer current";

pub(super) async fn socket(
    State(state): State<Arc<SurfaceState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !state
        .access
        .admits_socket(query.as_deref(), &state.capabilities)
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
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

/// Which names a bound listener answers to, and the endpoint it publishes.
///
/// A wildcard bind has no address of its own to publish: the operator reaches
/// it by whatever route their network produces, and only the port is known
/// here. Loopback is published instead, because it is the one address this
/// machine can always dial itself, and every local client does.
pub(super) fn reachable_as(bound: SocketAddr) -> (Authority, String) {
    if bound.ip().is_unspecified() {
        let local = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bound.port());
        return (
            Authority::AnyHostAt(bound.port()),
            format!("http://{local}"),
        );
    }
    (
        Authority::Only(bound.to_string()),
        format!("http://{bound}"),
    )
}

/// Admit a request by the endpoint it names, answering with that endpoint.
///
/// The origin is checked against the admitted `Host` rather than against the
/// published one. For a specific bind those are the same string, since only one
/// `Host` is admitted at all. For a wildcard bind they are not, and this is the
/// check that still holds: a browser sends the origin it loaded the page from,
/// so requiring it to match the host it is now addressing keeps another site
/// from driving this surface, whichever of the machine's addresses was used.
pub(super) fn admit_headers<'headers>(
    state: &SurfaceState,
    headers: &'headers HeaderMap,
    origin_required: bool,
) -> Result<&'headers str, ()> {
    let Some(host) = headers.get(HOST).and_then(|value| value.to_str().ok()) else {
        return Err(());
    };
    if !state.authority.admits(host) {
        return Err(());
    }
    let same_origin = format!("http://{host}");
    let origin = headers.get(ORIGIN).and_then(|value| value.to_str().ok());
    if origin_required && origin != Some(same_origin.as_str()) {
        return Err(());
    }
    if origin.is_some_and(|origin| origin != same_origin) {
        return Err(());
    }
    Ok(host)
}

async fn socket_session(mut socket: WebSocket, state: Arc<SurfaceState>) {
    let _session = SessionGuard::enter(&state);
    let mut closed = state.closed.subscribe();
    let Some((accepted, capability)) = open_session(&mut socket, &state, &mut closed).await else {
        return;
    };
    send_encoded(&mut socket, &accepted).await;
    let mut subscription = None;
    let mut broadcasts = state.stream.as_ref().map(crate::stream::Source::subscribe);
    let mut message_broadcasts = state
        .stream
        .as_ref()
        .map(crate::stream::Source::subscribe_messages);
    loop {
        tokio::select! {
            biased;
            changed = closed.changed() => {
                if changed.is_err() || *closed.borrow() {
                    if let Some(frame) = subscription
                        .as_ref()
                        .and_then(crate::stream::Subscription::closing)
                    {
                        send_encoded(&mut socket, &frame).await;
                    }
                    return;
                }
            }
            notice = async {
                broadcasts
                    .as_mut()
                    .expect("enabled branch has a Bus broadcast")
                    .recv()
                    .await
            }, if broadcasts.is_some()
                && subscription
                    .as_ref()
                    .is_some_and(crate::stream::Subscription::is_live) => {
                if matches!(
                    route_broadcast(
                        &mut socket,
                        &state,
                        &accepted,
                        capability,
                        subscription.as_mut().expect("live branch has a subscription"),
                        notice,
                    )
                    .await,
                    SessionFlow::Close
                ) {
                    return;
                }
            }
            notice = async {
                message_broadcasts
                    .as_mut()
                    .expect("enabled branch has a message broadcast")
                    .recv()
                    .await
            }, if message_broadcasts.is_some()
                && subscription
                    .as_ref()
                    .is_some_and(crate::stream::Subscription::is_live) => {
                if matches!(
                    route_message_broadcast(
                        &mut socket,
                        &state,
                        &accepted,
                        capability,
                        subscription.as_ref().expect("live branch has a subscription"),
                        notice,
                    )
                    .await,
                    SessionFlow::Close
                ) {
                    return;
                }
            }
            message = socket.recv() => {
                let Some(Ok(message)) = message else {
                    return;
                };
                let bytes = match message {
                    Message::Binary(bytes) => bytes.to_vec(),
                    Message::Text(text) => text.as_bytes().to_vec(),
                    Message::Ping(_) | Message::Pong(_) => continue,
                    Message::Close(_) => return,
                };
                let flow = if is_stream_request(&bytes) {
                    route_stream(
                        &mut socket,
                        &state,
                        &accepted,
                        capability,
                        &bytes,
                        &mut subscription,
                    )
                    .await
                } else {
                    route_command(&mut socket, &state, &accepted, &bytes).await
                };
                if matches!(flow, SessionFlow::Close) {
                    return;
                }
            }
        }
    }
}

async fn route_message_broadcast(
    socket: &mut WebSocket,
    state: &SurfaceState,
    accepted: &wire::Accepted,
    capability: wire::CanonicalUuid,
    subscription: &crate::stream::Subscription,
    notice: Result<wire::AgentMessage, tokio::sync::broadcast::error::RecvError>,
) -> SessionFlow {
    if !state.capabilities.matches(capability, &accepted.actor) {
        send_replaced(socket, state.epoch, Some(subscription.id())).await;
        return SessionFlow::Close;
    }
    match notice {
        Ok(message) => {
            let Some(frame) = subscription.message(message) else {
                return SessionFlow::Continue;
            };
            if send_encoded(socket, &frame).await {
                SessionFlow::Continue
            } else {
                SessionFlow::Close
            }
        }
        Err(tokio::sync::broadcast::error::RecvError::Lagged(dropped)) => {
            let frame = subscription
                .overflow(dropped)
                .unwrap_or_else(|failure| failure.frame(state.epoch, Some(subscription.id())));
            send_encoded(socket, &frame).await;
            SessionFlow::Close
        }
        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
            let frame = stream_fault(
                state.epoch,
                Some(subscription.id()),
                wire::FaultClass::Environment,
                "stream::message-broadcast-closed",
                "resident transcript-activity broadcast closed",
            );
            send_encoded(socket, &frame).await;
            SessionFlow::Close
        }
    }
}

async fn route_broadcast(
    socket: &mut WebSocket,
    state: &SurfaceState,
    accepted: &wire::Accepted,
    capability: wire::CanonicalUuid,
    subscription: &mut crate::stream::Subscription,
    notice: Result<bus::JournalCommit, tokio::sync::broadcast::error::RecvError>,
) -> SessionFlow {
    if !state.capabilities.matches(capability, &accepted.actor) {
        send_replaced(socket, state.epoch, Some(subscription.id())).await;
        return SessionFlow::Close;
    }
    match notice {
        Ok(commit) => {
            send_broadcast_update(socket, state, accepted, capability, subscription, commit).await
        }
        Err(tokio::sync::broadcast::error::RecvError::Lagged(dropped)) => {
            let frame = subscription
                .overflow(dropped)
                .unwrap_or_else(|failure| failure.frame(state.epoch, Some(subscription.id())));
            send_encoded(socket, &frame).await;
            SessionFlow::Close
        }
        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
            let frame = stream_fault(
                state.epoch,
                Some(subscription.id()),
                wire::FaultClass::Environment,
                "stream::broadcast-closed",
                "resident Bus cursor broadcast closed",
            );
            send_encoded(socket, &frame).await;
            SessionFlow::Close
        }
    }
}

async fn send_broadcast_update(
    socket: &mut WebSocket,
    state: &SurfaceState,
    accepted: &wire::Accepted,
    capability: wire::CanonicalUuid,
    subscription: &mut crate::stream::Subscription,
    commit: bus::JournalCommit,
) -> SessionFlow {
    let frames = match subscription.advance(
        state.stream.as_ref().expect("broadcast has a source"),
        commit,
    ) {
        Ok(frames) => frames,
        Err(failure) => {
            let frame = failure.frame(state.epoch, Some(subscription.id()));
            send_encoded(socket, &frame).await;
            return SessionFlow::Close;
        }
    };
    if !state.capabilities.matches(capability, &accepted.actor) {
        send_replaced(socket, state.epoch, Some(subscription.id())).await;
        return SessionFlow::Close;
    }
    for frame in frames {
        if !send_encoded(socket, &frame).await {
            return SessionFlow::Close;
        }
    }
    SessionFlow::Continue
}

async fn open_session(
    socket: &mut WebSocket,
    state: &SurfaceState,
    closed: &mut watch::Receiver<bool>,
) -> Option<(wire::Accepted, wire::CanonicalUuid)> {
    let message = receive_message(socket, closed).await?;
    let bytes = match message {
        Message::Binary(bytes) => bytes.to_vec(),
        Message::Text(text) => text.as_bytes().to_vec(),
        Message::Close(_) | Message::Ping(_) | Message::Pong(_) => return None,
    };
    match wire::decode::<wire::Hello>(&bytes) {
        Ok(hello) => match state.capabilities.admit(&hello, state.epoch) {
            Ok(accepted) => Some((accepted, hello.capability)),
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

fn is_stream_request(bytes: &[u8]) -> bool {
    wire::decode::<serde_json::Value>(bytes)
        .ok()
        .and_then(|value| {
            value
                .as_object()
                .and_then(|object| object.get("stream"))
                .cloned()
        })
        .is_some()
}

async fn route_stream(
    socket: &mut WebSocket,
    state: &SurfaceState,
    accepted: &wire::Accepted,
    capability: wire::CanonicalUuid,
    bytes: &[u8],
    subscription: &mut Option<crate::stream::Subscription>,
) -> SessionFlow {
    if !state.capabilities.matches(capability, &accepted.actor) {
        send_replaced(
            socket,
            state.epoch,
            subscription.as_ref().map(crate::stream::Subscription::id),
        )
        .await;
        return SessionFlow::Close;
    }
    let request = match wire::decode::<wire::StreamRequest>(bytes) {
        Ok(request) => request,
        Err(error) => {
            let frame = stream_fault(
                state.epoch,
                None,
                wire::FaultClass::Protocol,
                "stream::undecodable",
                &error.to_string(),
            );
            send_encoded(socket, &frame).await;
            return SessionFlow::Continue;
        }
    };
    let Some(source) = state.stream.as_ref() else {
        let frame = stream_fault(
            state.epoch,
            Some(request.id()),
            wire::FaultClass::Environment,
            "stream::unavailable",
            "this canvas socket has no resident Bus stream",
        );
        send_encoded(socket, &frame).await;
        return SessionFlow::Continue;
    };

    let result = match &request {
        wire::StreamRequest::Subscribe { .. } => {
            if subscription.is_some() {
                Err(crate::stream::Failure::Refusal(wire::Refusal {
                    reason: "stream::already-subscribed".to_owned(),
                    witness: Some("session".to_owned()),
                    expected: Some("one subscription".to_owned()),
                    observed: Some("a second subscription".to_owned()),
                    remedy: "open another authenticated socket for another subscription".to_owned(),
                }))
            } else {
                match source.begin(accepted, &request) {
                    Ok((opened, frames)) => {
                        if !state.capabilities.matches(capability, &accepted.actor) {
                            send_replaced(socket, state.epoch, Some(request.id())).await;
                            return SessionFlow::Close;
                        }
                        for frame in frames {
                            if !send_encoded(socket, &frame).await {
                                return SessionFlow::Close;
                            }
                        }
                        *subscription = Some(opened);
                        return SessionFlow::Continue;
                    }
                    Err(failure) => Err(failure),
                }
            }
        }
        wire::StreamRequest::Continue { .. } => match subscription.as_mut() {
            Some(subscription) => subscription.continue_page(&request),
            None => Err(crate::stream::Failure::Refusal(wire::Refusal {
                reason: "stream::subscription-required".to_owned(),
                witness: Some("session".to_owned()),
                expected: Some("an active subscription".to_owned()),
                observed: Some("no subscription".to_owned()),
                remedy: "subscribe before continuing a packed view".to_owned(),
            })),
        },
        wire::StreamRequest::Thread { .. } => source.thread(accepted, &request),
        // Admitted like every other request, so a probe from a replaced
        // generation is told so rather than being reassured by a Pong. The
        // client only needs *an* answer to prove the socket still carries
        // traffic, so a refusal serves that purpose as well as a Pong does.
        wire::StreamRequest::Ping { id, .. } => request
            .admit(state.epoch)
            .map(|()| wire::StreamFrame::Pong {
                v: wire::WireVersion::CURRENT,
                id: *id,
                epoch: state.epoch,
            })
            .map_err(crate::stream::Failure::admission),
    };
    if !state.capabilities.matches(capability, &accepted.actor) {
        send_replaced(socket, state.epoch, Some(request.id())).await;
        return SessionFlow::Close;
    }
    match result {
        Ok(frame) => {
            if send_encoded(socket, &frame).await {
                SessionFlow::Continue
            } else {
                SessionFlow::Close
            }
        }
        Err(failure) => {
            let frame = failure.frame(state.epoch, Some(request.id()));
            send_encoded(socket, &frame).await;
            SessionFlow::Continue
        }
    }
}

async fn send_replaced(
    socket: &mut WebSocket,
    epoch: wire::CanonicalUuid,
    id: Option<wire::CanonicalUuid>,
) {
    let frame = stream_fault(
        epoch,
        id,
        wire::FaultClass::Protocol,
        CAPABILITY_REPLACED_REASON,
        CAPABILITY_REPLACED_DETAIL,
    );
    send_encoded(socket, &frame).await;
}

fn stream_fault(
    epoch: wire::CanonicalUuid,
    id: Option<wire::CanonicalUuid>,
    class: wire::FaultClass,
    reason: &str,
    detail: &str,
) -> wire::StreamFrame {
    wire::StreamFrame::Fault {
        v: wire::WireVersion::CURRENT,
        epoch,
        id,
        fault: wire::Fault {
            class,
            reason: reason.to_owned(),
            detail: detail.to_owned(),
        },
    }
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
        Err(EnqueueError::NotAdmitting) if *state.closed.borrow() => {
            return SessionFlow::Close;
        }
        Err(EnqueueError::NotAdmitting) => {
            send_encoded(socket, &response.response(closing_refusal())).await;
            return SessionFlow::Continue;
        }
        Err(EnqueueError::ControlClosed) => {
            send_encoded(socket, &response.response(closing_refusal())).await;
            return SessionFlow::Close;
        }
    };
    let PendingResponse { returned, sent } = pending;
    let mut closed = state.closed.subscribe();
    let returned = tokio::select! {
        biased;
        changed = closed.changed() => {
            let _ = changed;
            return SessionFlow::Close;
        },
        _ = state.connections.cancelled() => return SessionFlow::Close,
        returned = returned => returned,
    };
    let Ok(outcome) = returned else {
        return SessionFlow::Close;
    };
    let reached_dark = dark && matches!(outcome, wire::Outcome::Reply(_));
    send_encoded(socket, &response.response(outcome)).await;
    let _ = sent.send(());
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

pub(super) fn is_dark(command: &wire::AdmittedCommand) -> bool {
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

async fn send_encoded<T: serde::Serialize>(socket: &mut WebSocket, value: &T) -> bool {
    let Ok(bytes) = wire::encode(value) else {
        return false;
    };
    socket.send(Message::Binary(bytes.into())).await.is_ok()
}
