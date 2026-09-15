//! Authenticated Canvas socket session routing.

use super::*;
use ::axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use futures_util::StreamExt as _;
use futures_util::stream::FuturesUnordered;
use std::pin::Pin;

const CAPABILITY_REPLACED_REASON: &str = "stream::capability-replaced";
const CAPABILITY_REPLACED_DETAIL: &str =
    "the capability that authenticated this stream is no longer current";

pub(super) type PendingCommands =
    FuturesUnordered<Pin<Box<dyn Future<Output = CommandCompletion> + Send>>>;

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
        .max_message_size(SOCKET_MESSAGE_BYTES)
        .max_frame_size(SOCKET_MESSAGE_BYTES)
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

struct ActiveSession {
    state: Arc<SurfaceState>,
    accepted: wire::Accepted,
    capability: wire::CanonicalUuid,
    retired: CancellationToken,
    subscription: Option<crate::stream::Subscription>,
    broadcasts: Option<bus::BroadcastSubscriber>,
    messages: Option<tokio::sync::broadcast::Receiver<wire::AgentMessage>>,
    commands: PendingCommands,
    lead: Lead,
}

impl ActiveSession {
    fn new(
        state: Arc<SurfaceState>,
        accepted: wire::Accepted,
        capability: wire::CanonicalUuid,
        retired: CancellationToken,
    ) -> Self {
        let broadcasts = state.stream.as_ref().map(crate::stream::Source::subscribe);
        let messages = state
            .stream
            .as_ref()
            .map(crate::stream::Source::subscribe_messages);
        Self {
            state,
            accepted,
            capability,
            retired,
            subscription: None,
            broadcasts,
            messages,
            commands: PendingCommands::new(),
            lead: Lead::Journal,
        }
    }

    async fn next(&mut self, socket: &mut WebSocket, closed: &mut watch::Receiver<bool>) -> Event {
        let live = self
            .subscription
            .as_ref()
            .is_some_and(crate::stream::Subscription::is_live);
        let continuing = self
            .subscription
            .as_ref()
            .is_some_and(crate::stream::Subscription::has_continuation);
        next_event(
            &mut self.lead,
            socket,
            SessionSignals::new(closed, &self.retired),
            &mut self.broadcasts,
            &mut self.messages,
            &mut self.commands,
            SessionReadiness { live, continuing },
        )
        .await
    }

    async fn route(
        &mut self,
        socket: &mut WebSocket,
        closed: &watch::Receiver<bool>,
        event: Event,
    ) -> SessionFlow {
        match event {
            Event::Closed(changed) => self.route_closed(socket, closed, changed).await,
            Event::Retired => {
                send_replaced(
                    socket,
                    self.state.epoch,
                    self.subscription
                        .as_ref()
                        .map(crate::stream::Subscription::id),
                )
                .await;
                SessionFlow::Close
            }
            Event::Journal(notice) => {
                route_broadcast(
                    socket,
                    &self.state,
                    &self.accepted,
                    self.capability,
                    self.subscription
                        .as_mut()
                        .expect("live branch has a subscription"),
                    notice,
                )
                .await
            }
            Event::Continuation => {
                route_broadcast_continuation(
                    socket,
                    &self.state,
                    &self.accepted,
                    self.capability,
                    self.subscription
                        .as_mut()
                        .expect("continuation branch has a subscription"),
                )
                .await
            }
            Event::Message(notice) => {
                route_message_broadcast(
                    socket,
                    &self.state,
                    &self.accepted,
                    self.capability,
                    self.subscription
                        .as_ref()
                        .expect("live branch has a subscription"),
                    notice,
                )
                .await
            }
            Event::Command(completed) => {
                route_command_completion(socket, &self.state, completed).await
            }
            Event::Inbound(message) => self.route_inbound(socket, message).await,
        }
    }

    async fn route_closed(
        &mut self,
        socket: &mut WebSocket,
        closed: &watch::Receiver<bool>,
        changed: Result<(), watch::error::RecvError>,
    ) -> SessionFlow {
        if changed.is_ok() && !*closed.borrow() {
            return SessionFlow::Continue;
        }
        if let Some(frame) = self
            .subscription
            .as_ref()
            .and_then(crate::stream::Subscription::closing)
        {
            send_encoded(socket, &frame).await;
        }
        SessionFlow::Close
    }

    async fn route_inbound(
        &mut self,
        socket: &mut WebSocket,
        message: Option<Result<Message, ::axum::Error>>,
    ) -> SessionFlow {
        let Some(Ok(message)) = message else {
            return SessionFlow::Close;
        };
        let bytes = match message {
            Message::Binary(bytes) => bytes.to_vec(),
            Message::Text(text) => text.as_bytes().to_vec(),
            Message::Ping(_) | Message::Pong(_) => return SessionFlow::Continue,
            Message::Close(_) => return SessionFlow::Close,
        };
        if is_stream_request(&bytes) {
            route_stream(
                socket,
                &self.state,
                &self.accepted,
                self.capability,
                &bytes,
                &mut self.subscription,
            )
            .await
        } else {
            route_inbound_command(
                socket,
                &self.state,
                &self.accepted,
                &mut self.commands,
                &bytes,
            )
            .await
        }
    }
}

async fn socket_session(mut socket: WebSocket, state: Arc<SurfaceState>) {
    let _session = SessionGuard::enter(&state);
    let mut closed = state.closed.subscribe();
    let Some((accepted, capability, retired)) =
        open_session(&mut socket, &state, &mut closed).await
    else {
        return;
    };
    send_encoded(&mut socket, &accepted).await;
    let mut session = ActiveSession::new(state, accepted, capability, retired);
    loop {
        let event = session.next(&mut socket, &mut closed).await;
        if matches!(
            session.route(&mut socket, &closed, event).await,
            SessionFlow::Close
        ) {
            return;
        }
    }
}

async fn route_inbound_command(
    socket: &mut WebSocket,
    state: &Arc<SurfaceState>,
    accepted: &wire::Accepted,
    commands: &mut PendingCommands,
    bytes: &[u8],
) -> SessionFlow {
    match route_command(socket, state, accepted, bytes).await {
        CommandRoute::Flow(flow) => flow,
        CommandRoute::Pending(command) if command.dark => {
            route_dark_command(socket, state, commands, *command).await
        }
        CommandRoute::Pending(command) => {
            commands.push(Box::pin((*command).complete(Arc::clone(state))));
            SessionFlow::Continue
        }
    }
}

/// Which work source a session evaluates first on one iteration.
///
/// Shutdown is deliberately not a member. It leads every arm because its
/// priority is correctness rather than fairness: a session that can observe
/// close must not route another frame first. Rotating it behind a work source
/// would trade a starvation bug for a close-ordering bug.
#[derive(Clone, Copy)]
pub(super) enum Lead {
    Journal,
    Message,
    Inbound,
    Command,
    Continuation,
}

impl Lead {
    /// Take this turn's lead and leave the next one in its place.
    ///
    /// Advancing is folded into the read so a caller cannot hold the lead
    /// still: a loop that forgot to rotate would be a fixed order again, and
    /// there would be no separate statement for a test to hold responsible.
    const fn take(&mut self) -> Self {
        let leading = *self;
        *self = match leading {
            Self::Journal => Self::Message,
            Self::Message => Self::Inbound,
            Self::Inbound => Self::Command,
            Self::Command => Self::Continuation,
            Self::Continuation => Self::Journal,
        };
        leading
    }
}

/// The receiving half of a session transport.
///
/// Dispatch order is the property worth guarding here, and a live socket can
/// only be made ready by a real peer racing the session. Naming the inbound
/// source lets the rotation be decided against a source that is ready on
/// demand, so the ordering is observed rather than timed.
pub(super) trait Inbound {
    /// Receive the next transport message, or `None` once the peer is gone.
    fn next_message(&mut self) -> impl Future<Output = Option<Result<Message, ::axum::Error>>>;
}

impl Inbound for WebSocket {
    fn next_message(&mut self) -> impl Future<Output = Option<Result<Message, ::axum::Error>>> {
        self.recv()
    }
}

/// One event a session acted on, named so the sources can be reordered.
pub(super) enum Event {
    Closed(Result<(), tokio::sync::watch::error::RecvError>),
    Retired,
    Journal(Result<bus::JournalCommit, tokio::sync::broadcast::error::RecvError>),
    Message(Result<wire::AgentMessage, tokio::sync::broadcast::error::RecvError>),
    Inbound(Option<Result<Message, ::axum::Error>>),
    Command(Option<CommandCompletion>),
    Continuation,
}

/// The two lifecycle signals that permanently end an authenticated session.
pub(super) struct SessionSignals<'session> {
    closed: &'session mut tokio::sync::watch::Receiver<bool>,
    retired: &'session CancellationToken,
}

/// Work branches enabled by the subscription's current delivery state.
#[derive(Clone, Copy)]
pub(super) struct SessionReadiness {
    live: bool,
    continuing: bool,
}

impl SessionReadiness {
    #[cfg(test)]
    pub(super) const fn live(continuing: bool) -> Self {
        Self {
            live: true,
            continuing,
        }
    }
}

impl<'session> SessionSignals<'session> {
    pub(super) const fn new(
        closed: &'session mut tokio::sync::watch::Receiver<bool>,
        retired: &'session CancellationToken,
    ) -> Self {
        Self { closed, retired }
    }
}

/// Await the next event for one session, rotating which work source leads.
///
/// `tokio::select!` under `biased` evaluates its arms in source order every
/// iteration, so a source that is ready every time keeps every later arm from
/// running at all. One fixed order therefore lets a committing fleet's journal
/// broadcasts hold off this session's own operator commands and agent messages
/// without a bound, which per-connection isolation does not help: the flood and
/// the starved requests share the connection.
///
/// Rotating which of the five work sources is evaluated first bounds the wait
/// at four iterations rather than at the journal falling quiet. No source is
/// ever *disabled* to make room, because a disabled broadcast receiver stops
/// draining its channel and converts a scheduling question into a lag question,
/// changing the retained-cursor overflow behavior this rotation must leave
/// alone.
pub(super) async fn next_event(
    lead: &mut Lead,
    socket: &mut impl Inbound,
    signals: SessionSignals<'_>,
    broadcasts: &mut Option<bus::BroadcastSubscriber>,
    messages: &mut Option<tokio::sync::broadcast::Receiver<wire::AgentMessage>>,
    commands: &mut PendingCommands,
    readiness: SessionReadiness,
) -> Event {
    let SessionReadiness { live, continuing } = readiness;
    let journal = broadcasts.is_some() && live;
    let transcript = messages.is_some() && live;
    let command = !commands.is_empty();
    let SessionSignals { closed, retired } = signals;
    match lead.take() {
        Lead::Journal => tokio::select! {
            biased;
            changed = closed.changed() => Event::Closed(changed),
            () = retired.cancelled() => Event::Retired,
            notice = recv_journal(broadcasts), if journal => Event::Journal(notice),
            notice = recv_message(messages), if transcript => Event::Message(notice),
            message = socket.next_message() => Event::Inbound(message),
            completed = commands.next(), if command => Event::Command(completed),
            _ = std::future::ready(()), if continuing => Event::Continuation,
        },
        Lead::Message => tokio::select! {
            biased;
            changed = closed.changed() => Event::Closed(changed),
            () = retired.cancelled() => Event::Retired,
            notice = recv_message(messages), if transcript => Event::Message(notice),
            message = socket.next_message() => Event::Inbound(message),
            completed = commands.next(), if command => Event::Command(completed),
            _ = std::future::ready(()), if continuing => Event::Continuation,
            notice = recv_journal(broadcasts), if journal => Event::Journal(notice),
        },
        Lead::Inbound => tokio::select! {
            biased;
            changed = closed.changed() => Event::Closed(changed),
            () = retired.cancelled() => Event::Retired,
            message = socket.next_message() => Event::Inbound(message),
            completed = commands.next(), if command => Event::Command(completed),
            _ = std::future::ready(()), if continuing => Event::Continuation,
            notice = recv_journal(broadcasts), if journal => Event::Journal(notice),
            notice = recv_message(messages), if transcript => Event::Message(notice),
        },
        Lead::Command => tokio::select! {
            biased;
            changed = closed.changed() => Event::Closed(changed),
            () = retired.cancelled() => Event::Retired,
            completed = commands.next(), if command => Event::Command(completed),
            _ = std::future::ready(()), if continuing => Event::Continuation,
            notice = recv_journal(broadcasts), if journal => Event::Journal(notice),
            notice = recv_message(messages), if transcript => Event::Message(notice),
            message = socket.next_message() => Event::Inbound(message),
        },
        Lead::Continuation => tokio::select! {
            biased;
            changed = closed.changed() => Event::Closed(changed),
            () = retired.cancelled() => Event::Retired,
            _ = std::future::ready(()), if continuing => Event::Continuation,
            notice = recv_journal(broadcasts), if journal => Event::Journal(notice),
            notice = recv_message(messages), if transcript => Event::Message(notice),
            message = socket.next_message() => Event::Inbound(message),
            completed = commands.next(), if command => Event::Command(completed),
        },
    }
}

/// Read one journal commit from the enabled Bus broadcast.
async fn recv_journal(
    broadcasts: &mut Option<bus::BroadcastSubscriber>,
) -> Result<bus::JournalCommit, tokio::sync::broadcast::error::RecvError> {
    broadcasts
        .as_mut()
        .expect("enabled branch has a Bus broadcast")
        .recv()
        .await
}

/// Read one agent message from the enabled transcript broadcast.
async fn recv_message(
    messages: &mut Option<tokio::sync::broadcast::Receiver<wire::AgentMessage>>,
) -> Result<wire::AgentMessage, tokio::sync::broadcast::error::RecvError> {
    messages
        .as_mut()
        .expect("enabled branch has a message broadcast")
        .recv()
        .await
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
    let delivery = match subscription.advance(
        state.stream.as_ref().expect("broadcast has a source"),
        commit,
    ) {
        Ok(delivery) => delivery,
        Err(failure) => {
            let frame = failure.frame(state.epoch, Some(subscription.id()));
            send_encoded(socket, &frame).await;
            return SessionFlow::Close;
        }
    };
    send_live_delivery(socket, state, accepted, capability, subscription, delivery).await
}

async fn route_broadcast_continuation(
    socket: &mut WebSocket,
    state: &SurfaceState,
    accepted: &wire::Accepted,
    capability: wire::CanonicalUuid,
    subscription: &mut crate::stream::Subscription,
) -> SessionFlow {
    let delivery = match subscription.continue_live(
        state
            .stream
            .as_ref()
            .expect("continuation has a stream source"),
    ) {
        Ok(delivery) => delivery,
        Err(failure) => {
            let frame = failure.frame(state.epoch, Some(subscription.id()));
            send_encoded(socket, &frame).await;
            return SessionFlow::Close;
        }
    };
    send_live_delivery(socket, state, accepted, capability, subscription, delivery).await
}

async fn send_live_delivery(
    socket: &mut WebSocket,
    state: &SurfaceState,
    accepted: &wire::Accepted,
    capability: wire::CanonicalUuid,
    subscription: &mut crate::stream::Subscription,
    delivery: Option<crate::stream::LiveDelivery>,
) -> SessionFlow {
    if !state.capabilities.matches(capability, &accepted.actor) {
        send_replaced(socket, state.epoch, Some(subscription.id())).await;
        return SessionFlow::Close;
    }
    let Some(delivery) = delivery else {
        return SessionFlow::Continue;
    };
    for frame in delivery.frames() {
        if !send_encoded(socket, frame).await {
            return SessionFlow::Close;
        }
    }
    subscription.admit(delivery);
    SessionFlow::Continue
}

async fn open_session(
    socket: &mut WebSocket,
    state: &SurfaceState,
    closed: &mut watch::Receiver<bool>,
) -> Option<(wire::Accepted, wire::CanonicalUuid, CancellationToken)> {
    let message = receive_message(socket, closed).await?;
    let bytes = match message {
        Message::Binary(bytes) => bytes.to_vec(),
        Message::Text(text) => text.as_bytes().to_vec(),
        Message::Close(_) | Message::Ping(_) | Message::Pong(_) => return None,
    };
    match wire::decode::<wire::Hello>(&bytes) {
        Ok(hello) => match state.capabilities.admit_session(&hello, state.epoch) {
            Ok(admission) => Some((admission.accepted, hello.capability, admission.retired)),
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
                Err(crate::stream::Failure::Refusal(wire::Refusal::bare(
                    wire::RefusalDiagnostic::new(
                        "stream::already-subscribed",
                        Some("session".to_owned()),
                        Some("one subscription".to_owned()),
                        Some("a second subscription".to_owned()),
                    ),
                )))
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
            Some(subscription) => subscription.continue_page(source, &request),
            None => Err(crate::stream::Failure::Refusal(wire::Refusal::bare(
                wire::RefusalDiagnostic::new(
                    "stream::subscription-required",
                    Some("session".to_owned()),
                    Some("an active subscription".to_owned()),
                    Some("no subscription".to_owned()),
                ),
            ))),
        },
        wire::StreamRequest::Thread { .. } => source.thread(accepted, &request),
        wire::StreamRequest::MessageHistory { .. } => {
            message_history_off_runtime(source, accepted, &request).await
        }
        wire::StreamRequest::Ping { id, .. } => pong(state.epoch, &request, *id),
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

/// Answer a stream-level ping once the request is admitted.
///
/// Admitted like every other request, so a probe from a replaced generation is
/// told so rather than being reassured by a Pong. The client only needs *an*
/// answer to prove the socket still carries traffic, so a refusal serves that
/// purpose as well as a Pong does.
fn pong(
    epoch: wire::CanonicalUuid,
    request: &wire::StreamRequest,
    id: wire::CanonicalUuid,
) -> Result<wire::StreamFrame, crate::stream::Failure> {
    request
        .admit(epoch)
        .map(|()| wire::StreamFrame::Pong {
            v: wire::WireVersion::CURRENT,
            id,
            epoch,
        })
        .map_err(crate::stream::Failure::admission)
}

/// Answer one history request from the blocking pool, not the runtime thread.
///
/// The provider behind [`crate::stream::Source::message_history`] may chain
/// several multi-megabyte tape windows and hold its thread for seconds. The
/// surface runtime is one thread shared by every socket, live publication,
/// and the resident loop, so the read leaves it and the answer returns through
/// the join. Requests on this one socket still run one at a time.
async fn message_history_off_runtime(
    source: &crate::stream::Source,
    accepted: &wire::Accepted,
    request: &wire::StreamRequest,
) -> Result<wire::StreamFrame, crate::stream::Failure> {
    let source = source.clone();
    let accepted = accepted.clone();
    let request = request.clone();
    tokio::task::spawn_blocking(move || source.message_history(&accepted, &request))
        .await
        .unwrap_or_else(|error| {
            Err(crate::stream::Failure::Fault(wire::Fault {
                class: wire::FaultClass::Environment,
                reason: "stream::message-history-abandoned".to_owned(),
                detail: error.to_string(),
            }))
        })
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
) -> CommandRoute {
    let crossing = match wire::decode::<wire::Command<serde_json::Value>>(bytes) {
        Ok(command) => command,
        Err(error) => {
            send_encoded(socket, &undecodable_command(error)).await;
            return CommandRoute::Flow(SessionFlow::Continue);
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
            return CommandRoute::Flow(SessionFlow::Continue);
        }
    };
    let dark = is_dark(&command);
    let response = CommandResponse::from(&command);
    let pending = match state.enqueue(command).await {
        Ok(pending) => pending,
        Err(EnqueueError::NotAdmitting) if *state.closed.borrow() => {
            return CommandRoute::Flow(SessionFlow::Close);
        }
        Err(EnqueueError::NotAdmitting) => {
            send_encoded(socket, &response.bind(closing_refusal())).await;
            return CommandRoute::Flow(SessionFlow::Continue);
        }
        Err(EnqueueError::ControlClosed) => {
            send_encoded(socket, &response.bind(closing_refusal())).await;
            return CommandRoute::Flow(SessionFlow::Close);
        }
    };
    let PendingResponse { returned, sent } = pending;
    CommandRoute::Pending(Box::new(PendingCommand {
        response,
        returned,
        sent,
        dark,
    }))
}

enum CommandRoute {
    Flow(SessionFlow),
    Pending(Box<PendingCommand>),
}

struct PendingCommand {
    response: CommandResponse,
    returned: tokio::sync::oneshot::Receiver<wire::Outcome<wire::intent::CommandAnswer>>,
    sent: tokio::sync::oneshot::Sender<()>,
    dark: bool,
}

pub(super) enum CommandCompletion {
    Closed,
    Returned(Box<ReturnedCommand>),
}

pub(super) struct ReturnedCommand {
    response: CommandResponse,
    returned:
        Result<wire::Outcome<wire::intent::CommandAnswer>, tokio::sync::oneshot::error::RecvError>,
    sent: tokio::sync::oneshot::Sender<()>,
    dark: bool,
}

#[derive(Clone, Copy)]
struct CommandResponse {
    id: wire::CanonicalUuid,
    epoch: wire::CanonicalUuid,
}

impl CommandResponse {
    fn from(command: &wire::AdmittedCommand) -> Self {
        Self {
            id: command.id(),
            epoch: command.epoch(),
        }
    }

    fn bind(
        self,
        outcome: wire::Outcome<wire::intent::CommandAnswer>,
    ) -> wire::Response<wire::intent::CommandAnswer> {
        wire::Response {
            v: wire::WireVersion::CURRENT,
            id: self.id,
            epoch: self.epoch,
            outcome,
        }
    }
}

impl PendingCommand {
    async fn complete(self, state: Arc<SurfaceState>) -> CommandCompletion {
        if *state.closed.borrow() || state.connections.is_cancelled() {
            return CommandCompletion::Closed;
        }
        let mut closed = state.closed.subscribe();
        let returned = tokio::select! {
            biased;
            changed = closed.changed() => {
                let _ = changed;
                return CommandCompletion::Closed;
            },
            _ = state.connections.cancelled() => return CommandCompletion::Closed,
            returned = self.returned => returned,
        };
        CommandCompletion::Returned(Box::new(ReturnedCommand {
            response: self.response,
            returned,
            sent: self.sent,
            dark: self.dark,
        }))
    }
}

async fn route_dark_command(
    socket: &mut WebSocket,
    state: &Arc<SurfaceState>,
    commands: &mut PendingCommands,
    command: PendingCommand,
) -> SessionFlow {
    while let Some(completed) = commands.next().await {
        if matches!(
            route_command_completion(socket, state, Some(completed)).await,
            SessionFlow::Close
        ) {
            return SessionFlow::Close;
        }
    }
    let completed = command.complete(Arc::clone(state)).await;
    route_command_completion(socket, state, Some(completed)).await
}

async fn route_command_completion(
    socket: &mut WebSocket,
    state: &SurfaceState,
    completed: Option<CommandCompletion>,
) -> SessionFlow {
    let Some(CommandCompletion::Returned(completed)) = completed else {
        return SessionFlow::Close;
    };
    let ReturnedCommand {
        response,
        returned,
        sent,
        dark,
    } = *completed;
    let Ok(outcome) = returned else {
        return SessionFlow::Close;
    };
    let reached_dark = dark && matches!(outcome, wire::Outcome::Reply(_));
    send_encoded(socket, &response.bind(outcome)).await;
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
    wire::Outcome::Refusal(wire::Refusal::bare(wire::RefusalDiagnostic::new(
        "resident::not-admitting",
        Some("resident-state".to_owned()),
        Some("seated".to_owned()),
        Some("quiescing".to_owned()),
    )))
}

async fn send_encoded<T: serde::Serialize>(socket: &mut WebSocket, value: &T) -> bool {
    let Ok(bytes) = wire::encode(value) else {
        return false;
    };
    socket.send(Message::Binary(bytes.into())).await.is_ok()
}
