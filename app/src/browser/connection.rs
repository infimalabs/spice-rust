use super::*;

/// Run connections one after another for as long as the surface is mounted.
///
/// Each attempt is awaited to completion before the next begins, and the socket
/// it opens lives in that call's locals rather than a handle anything else can
/// reach. So an ending connection has no successor to confuse itself with: the
/// next one does not exist yet. A guard comparing the closing socket against
/// the current one would have nothing to compare, which is why none appears
/// here — the hazard it answers needs two live connections, and spawning a
/// second one is what would reintroduce it.
pub(super) async fn run_surface(
    active: Rc<Cell<bool>>,
    handles: SurfaceHandles,
    mut actions: UnboundedReceiver<ClientAction>,
) {
    let data = Rc::new(RefCell::new(SurfaceData::default()));
    let mut backoff = crate::reconnect::Backoff::new();
    while active.get() {
        if matches!(&*handles.connection, Connection::Starting) {
            handles.connection.set(Connection::Connecting);
        }
        let result = run_connection(&active, &handles, &data, &mut actions).await;
        if !active.get() {
            break;
        }
        // `run_connection` returns `Ok` only where its own `while active.get()`
        // guard stops, and the break above has already taken that exit. An `Ok`
        // arriving here would mean the two guards disagree, so it fails loudly
        // instead of publishing an offline reason no socket ever produced.
        let ended =
            result.expect_err("run_connection returns Ok only once the surface is inactive");
        handles.submissions.queue.borrow_mut().reconnect();
        handles.submissions.publish();
        let delay = backoff.settle(ended.admitted);
        handles.connection.set(Connection::Offline(ended.reason));
        TimeoutFuture::new(delay).await;
    }
}

/// Why one connection ended, and whether it ever became an admitted session.
///
/// Every end is a failure to the surface, but they are not the same evidence. A
/// connection that reached admission proved the resident reachable, so whatever
/// ended it says nothing about how long the next attempt should wait; one that
/// never got there is the only kind that should push the delay out.
struct Ended {
    reason: String,
    admitted: bool,
}

async fn run_connection(
    active: &Rc<Cell<bool>>,
    handles: &SurfaceHandles,
    data: &Rc<RefCell<SurfaceData>>,
    actions: &mut UnboundedReceiver<ClientAction>,
) -> Result<(), Ended> {
    let abort = web_sys::AbortController::new().map_err(|_| Ended {
        reason: "browser could not create a connection abort controller".to_owned(),
        admitted: false,
    })?;
    let signal = abort.signal();
    let established = {
        let handshake = establish_connection(
            &handles.connection,
            &handles.surface,
            &handles.speech,
            data,
            &signal,
        )
        .fuse();
        let deadline = TimeoutFuture::new(CONNECTION_HANDSHAKE_TIMEOUT_MILLIS).fuse();
        futures_util::pin_mut!(handshake, deadline);
        select! {
            result = handshake => result,
            () = deadline => {
                abort.abort();
                Err(format!(
                    "canvas connection handshake timed out after \
                     {CONNECTION_HANDSHAKE_TIMEOUT_MILLIS} ms"
                ))
            }
        }
    }
    .map_err(|reason| Ended {
        reason,
        admitted: false,
    })?;
    // Reaching here *is* the admission: `establish_connection` only returns once
    // the acceptance frame has been admitted and the runtime published. Deriving
    // the flag from control flow rather than from a separate variable keeps the
    // two from ever disagreeing about whether this connection counted.
    pump_session(active, handles, data, actions, established)
        .await
        .map_err(|reason| Ended {
            reason,
            admitted: true,
        })
}

/// Carry one admitted session until it fails or the surface goes away.
///
/// The select is the entire event loop, and the silence timer is one of its
/// arms rather than a separate poller: it is a real future that completes only
/// when no inbound frame and no operator action reached us first, so an idle
/// board costs nothing until the moment silence becomes worth asking about.
///
/// A half-open socket is the case this exists for. It stays open, reports no
/// error, and simply stops delivering, so the reader future never completes and
/// every assertion downstream of it waits forever. The probe converts that into
/// an ordinary failure, which the one existing reconnect boundary already
/// handles -- deliberately not a second reconnect path, which would race the
/// first and double-subscribe.
async fn pump_session(
    active: &Rc<Cell<bool>>,
    handles: &SurfaceHandles,
    data: &Rc<RefCell<SurfaceData>>,
    actions: &mut UnboundedReceiver<ClientAction>,
    established: EstablishedConnection,
) -> Result<(), String> {
    let EstablishedConnection {
        session,
        mut writer,
        mut reader,
        principal,
        mut pending,
    } = established;
    send_subscription(&principal, data, &mut writer).await?;
    send_refresh(&principal, &mut writer, &mut pending, &handles.submissions).await?;
    drain_submissions(&principal, &mut writer, &mut pending, &handles.submissions).await?;
    let mut silence = TimeoutFuture::new(LIVENESS_PROBE_MILLIS).fuse();
    let mut probed = false;
    while active.get() {
        let incoming = reader.next().fuse();
        let action = actions.next().fuse();
        futures_util::pin_mut!(incoming, action);
        let mut rearm = false;
        select! {
            frame = incoming => {
                // Any frame proves the socket carries traffic, so the answer to
                // a probe need not be the Pong specifically -- a refusal names
                // a reachable resident just as well.
                rearm = true;
                probed = false;
                let frame = frame
                    .ok_or_else(|| "resident closed the socket".to_owned())?
                    .map_err(websocket_error)?;
                let bytes = binary_message(frame)?;
                let (directives, refresh) =
                    handle_incoming(
                        &session,
                        handles,
                        data,
                        &mut pending,
                        &bytes,
                    )?;
                relay_directives(&principal, &mut writer, directives).await?;
                if refresh {
                    send_refresh(
                        &principal,
                        &mut writer,
                        &mut pending,
                        &handles.submissions,
                    )
                    .await?;
                }
                drain_submissions(
                    &principal,
                    &mut writer,
                    &mut pending,
                    &handles.submissions,
                )
                .await?;
            },
            action = action => {
                let action = action.ok_or_else(|| "canvas command channel closed".to_owned())?;
                dispatch_action(
                    &principal,
                    &mut writer,
                    &mut pending,
                    &handles.submissions,
                    action,
                )
                .await?;
            },
            () = silence => {
                if probed {
                    return Err(format!(
                        "canvas socket stayed open without answering a liveness probe within \
                         {LIVENESS_PROBE_MILLIS} ms"
                    ));
                }
                rearm = true;
                probed = true;
                send_probe(&principal, &mut writer).await?;
            },
        }
        if rearm {
            silence = TimeoutFuture::new(LIVENESS_PROBE_MILLIS).fuse();
        }
    }
    Ok(())
}

/// Ask the resident to prove it is still reading this socket.
async fn send_probe(principal: &Principal, writer: &mut SocketWriter) -> Result<(), String> {
    send_stream_request(
        writer,
        &StreamRequest::Ping {
            v: WireVersion::CURRENT,
            id: random_uuid()?,
            epoch: principal.epoch(),
        },
    )
    .await
}

/// Establish every prerequisite of the resident event loop as one boundary.
async fn establish_connection(
    connection: &UseStateHandle<Connection>,
    surface: &UseStateHandle<SurfaceData>,
    speech: &UseStateHandle<wire::SpeechPlayback>,
    data: &Rc<RefCell<SurfaceData>>,
    signal: &web_sys::AbortSignal,
) -> Result<EstablishedConnection, String> {
    let (session, writer, mut reader) = open_socket(speech, signal).await?;
    let principal = admit_runtime(&session, &mut reader, connection, surface, data).await?;
    let pending = BTreeMap::new();
    Ok(EstablishedConnection {
        session,
        writer,
        reader,
        principal,
        pending,
    })
}

/// Discover the resident endpoint and open the socket its bootstrap names.
async fn open_socket(
    speech: &UseStateHandle<wire::SpeechPlayback>,
    signal: &web_sys::AbortSignal,
) -> Result<(Session, SocketWriter, SocketReader), String> {
    let discovery: wire::CanvasBootstrap = Request::get(&authenticated_path(
        "/bootstrap",
        browser_auth_token()?.as_deref(),
    ))
    .abort_signal(Some(signal))
    .send()
    .await
    .map_err(|error| format!("bootstrap request failed: {error}"))?
    .json()
    .await
    .map_err(|error| format!("bootstrap response was invalid: {error}"))?;
    speech.set(discovery.speech.clone());
    let session = Session::open(discovery.capability, discovery.epoch);
    let endpoint = discovery
        .endpoint
        .strip_prefix("http://")
        .ok_or_else(|| "bootstrap endpoint was not the current HTTP origin".to_owned())?;
    let socket = WebSocket::open(&format!(
        "ws://{endpoint}/socket?auth={}",
        discovery.capability
    ))
    .map_err(|error| format!("socket did not open: {error}"))?;
    let (mut writer, reader) = socket.split();
    writer
        .send(Message::Bytes(
            wire::encode(&session.hello()).map_err(|error| error.to_string())?,
        ))
        .await
        .map_err(websocket_error)?;
    Ok((session, writer, reader))
}

/// Admit the acceptance frame and publish the runtime that frame names.
async fn admit_runtime(
    session: &Session,
    reader: &mut SocketReader,
    connection: &UseStateHandle<Connection>,
    surface: &UseStateHandle<SurfaceData>,
    data: &Rc<RefCell<SurfaceData>>,
) -> Result<Principal, String> {
    let accepted_bytes = binary_message(
        reader
            .next()
            .await
            .ok_or_else(|| "socket closed before acceptance".to_owned())?
            .map_err(websocket_error)?,
    )?;
    let accepted = session
        .admit_accepted(&accepted_bytes)
        .map_err(|fault| format!("{}: {}", fault.reason, fault.detail))?;
    let principal = {
        let mut data = data.borrow_mut();
        match data.as_mut() {
            Some(runtime) => {
                runtime.reconnect(&accepted);
            }
            None => *data = Some(CanvasRuntime::new(&accepted)),
        }
        data.as_ref()
            .expect("accepted connection creates runtime")
            .principal()
            .clone()
    };
    surface.set(data.borrow().clone());
    connection.set(Connection::Online {
        actor: principal.actor().to_string(),
        epoch: principal.epoch().to_string(),
    });
    Ok(principal)
}

/// Carry back the stream directives one admitted frame produced.
async fn relay_directives(
    principal: &Principal,
    writer: &mut SocketWriter,
    directives: Vec<StreamDirective>,
) -> Result<(), String> {
    for directive in directives {
        if send_stream_directive(principal, writer, directive).await? {
            return Err("resident stream requested reconnect".to_owned());
        }
    }
    Ok(())
}

/// Send one operator action as the typed command the resident admits.
async fn dispatch_action(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    submissions: &SubmissionHandle,
    action: ClientAction,
) -> Result<(), String> {
    match action {
        ClientAction::DrainSubmissions => {
            drain_submissions(principal, writer, pending, submissions).await
        }
        ClientAction::Refresh => send_refresh(principal, writer, pending, submissions).await,
        ClientAction::Seat {
            topology_revision,
            name,
            worktree,
            life,
            vendor,
        } => {
            let lane = random_uuid()?;
            let body = LaneSeat {
                topology_revision,
                lane,
                lineage_id: lane,
                name,
                worktree,
                life,
                vendor,
                resume: None,
            };
            send_typed(
                principal,
                writer,
                pending,
                PendingKind::LaneSeat,
                CommandKind::LaneSeat,
                vec![],
                body,
            )
            .await
        }
        ClientAction::ArrangeLane {
            topology_revision,
            lane,
            before,
        } => {
            send_typed(
                principal,
                writer,
                pending,
                PendingKind::LaneArrange,
                CommandKind::LaneArrange,
                vec![],
                LaneArrange {
                    topology_revision,
                    lane,
                    before,
                },
            )
            .await
        }
        ClientAction::ArrangeLineages {
            topology_revision,
            lineages,
        } => {
            send_typed(
                principal,
                writer,
                pending,
                PendingKind::LineageArrange,
                CommandKind::LineageArrange,
                vec![],
                LineageArrange {
                    topology_revision,
                    lineages,
                },
            )
            .await
        }
        ClientAction::ConfigureLineage {
            topology_revision,
            lineage,
            member,
            life,
        } => {
            send_lineage_configuration(
                principal,
                writer,
                pending,
                topology_revision,
                lineage,
                member,
                life,
            )
            .await
        }
        ClientAction::Note { task, note } => {
            send_task_note(principal, writer, pending, task, note).await
        }
        ClientAction::Consent {
            request,
            rule,
            answer,
        } => send_consent_decision(principal, writer, pending, request, rule, answer).await,
        ClientAction::Steer {
            lane,
            occupant,
            epoch,
            message,
            attachments,
        } => {
            let body = SteerSend {
                lane,
                message,
                attachments,
            };
            let expected = vec![Witness::Residency {
                lane,
                occupant,
                epoch,
            }];
            send_typed(
                principal,
                writer,
                pending,
                PendingKind::Steer { lane },
                CommandKind::SteerSend,
                expected,
                body,
            )
            .await
        }
    }
}

async fn drain_submissions(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    submissions: &SubmissionHandle,
) -> Result<(), String> {
    let ready_wakes = submissions.queue.borrow().ready_wakes();
    for submission in ready_wakes {
        send_typed(
            principal,
            writer,
            pending,
            PendingKind::WakeSubmission {
                lane: submission.lane,
                submission: submission.id,
            },
            CommandKind::LaneState,
            vec![],
            LaneState {
                lane: submission.lane,
                action: LaneAction::Wake,
                worktree: None,
            },
        )
        .await?;
        submissions.queue.borrow_mut().wake_sent(submission.id)?;
        submissions.publish();
    }
    let ready = submissions.queue.borrow().ready();
    for submission in ready {
        let (occupant, epoch) = submission.residency().ok_or_else(|| {
            format!(
                "composer submission {} became steerable without residency",
                submission.id
            )
        })?;
        let body = SteerSend {
            lane: submission.lane,
            message: submission.message.clone(),
            attachments: submission.attachments.clone(),
        };
        let expected = vec![Witness::Residency {
            lane: submission.lane,
            occupant,
            epoch,
        }];
        send_typed_as(
            principal,
            writer,
            pending,
            submission.id,
            PendingKind::SteerSubmission {
                lane: submission.lane,
            },
            CommandKind::SteerSend,
            expected,
            body,
        )
        .await?;
        submissions.queue.borrow_mut().sent(submission.id)?;
        submissions.publish();
    }
    Ok(())
}

async fn send_lineage_configuration(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    topology_revision: u64,
    lineage: CanonicalUuid,
    member: Option<CanonicalUuid>,
    life: LineageLife,
) -> Result<(), String> {
    send_typed(
        principal,
        writer,
        pending,
        PendingKind::LineageConfigure(lineage),
        CommandKind::LineageConfigure,
        vec![],
        LineageConfigure {
            topology_revision,
            lineage_id: lineage,
            member,
            life,
            manual_projects: None,
        },
    )
    .await
}

async fn send_task_note(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    task: TaskHandle,
    note: String,
) -> Result<(), String> {
    let body = TaskNote {
        task: task.clone(),
        note,
    };
    let expected = vec![Witness::Existence { task: task.clone() }];
    send_typed(
        principal,
        writer,
        pending,
        PendingKind::Note(task),
        CommandKind::TaskNote,
        expected,
        body,
    )
    .await
}

async fn send_consent_decision(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    request: CanonicalUuid,
    rule: wire::Name,
    answer: Answer,
) -> Result<(), String> {
    let body = wire::intent::ConsentDecide { request, answer };
    let expected = vec![
        Witness::Consent {
            request,
            answer: Answer::Asked,
        },
        Witness::Rule { name: rule },
    ];
    send_typed(
        principal,
        writer,
        pending,
        PendingKind::ConsentDecision,
        CommandKind::ConsentDecide,
        expected,
        body,
    )
    .await
}

pub(super) async fn request_external_speech(
    text: &str,
    content_type: &str,
) -> Result<Vec<u8>, String> {
    let request = Request::post(&authenticated_path(
        "/speech",
        browser_auth_token()?.as_deref(),
    ))
    .json(&wire::SpeechRequest {
        text: text.to_owned(),
    })
    .map_err(|error| format!("speech request could not be encoded: {error}"))?;
    let response = request
        .send()
        .await
        .map_err(|error| format!("speech request failed: {error}"))?;
    if !response.ok() {
        let status = response.status();
        let detail = response
            .text()
            .await
            .unwrap_or_else(|_| "speech renderer returned no detail".to_owned());
        return Err(bounded_speech_error(&format!(
            "speech renderer returned {status}: {detail}"
        )));
    }
    let actual = response.headers().get("content-type").unwrap_or_default();
    if actual != content_type {
        return Err(format!(
            "speech renderer returned {actual:?}, expected {content_type:?}"
        ));
    }
    let bytes = response
        .binary()
        .await
        .map_err(|error| format!("speech audio response was invalid: {error}"))?;
    if bytes.is_empty() {
        return Err("speech renderer returned empty audio".to_owned());
    }
    Ok(bytes)
}

fn browser_auth_token() -> Result<Option<String>, String> {
    let hash = web_sys::window()
        .ok_or_else(|| "browser window is unavailable".to_owned())?
        .location()
        .hash()
        .map_err(|_| "browser URL fragment is unavailable".to_owned())?;
    Ok(hash
        .strip_prefix('#')
        .filter(|token| !token.is_empty())
        .map(str::to_owned))
}

fn authenticated_path(path: &str, token: Option<&str>) -> String {
    token.map_or_else(|| path.to_owned(), |token| format!("{path}?auth={token}"))
}

fn bounded_speech_error(error: &str) -> String {
    let mut end = error.len().min(240);
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    error[..end].to_owned()
}

async fn send_refresh(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    submissions: &SubmissionHandle,
) -> Result<(), String> {
    send_if_offered(
        principal,
        writer,
        pending,
        PendingKind::Board,
        CommandKind::BoardRead,
        vec![],
        BoardRead { family: None },
    )
    .await?;
    let awaiting_residency = submissions.queue.borrow().awaiting_residency();
    send_if_offered(
        principal,
        writer,
        pending,
        PendingKind::Lanes {
            wakes: awaiting_residency,
        },
        CommandKind::LaneRead,
        vec![],
        LaneRead {},
    )
    .await?;
    let covered = submissions.queue.borrow().accepted();
    send_if_offered(
        principal,
        writer,
        pending,
        PendingKind::Inbox { covered },
        CommandKind::SteerInbox,
        vec![],
        SteerInbox { lane: None },
    )
    .await?;
    send_if_offered(
        principal,
        writer,
        pending,
        PendingKind::Consent,
        CommandKind::ConsentRead,
        vec![],
        ConsentRead {},
    )
    .await?;
    send_if_offered(
        principal,
        writer,
        pending,
        PendingKind::Policy,
        CommandKind::ConsentPolicy,
        vec![],
        ConsentPolicy {},
    )
    .await?;
    send_if_offered(
        principal,
        writer,
        pending,
        PendingKind::Effort,
        CommandKind::EffortRead,
        vec![],
        EffortRead { task: None },
    )
    .await
}

async fn send_if_offered<B: Serialize>(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    pending_kind: PendingKind,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: B,
) -> Result<(), String> {
    if !SteerPane::over(principal).offers(kind) {
        return Ok(());
    }
    send_typed(
        principal,
        writer,
        pending,
        pending_kind,
        kind,
        expected,
        body,
    )
    .await
}

async fn send_typed<B: Serialize>(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    pending_kind: PendingKind,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: B,
) -> Result<(), String> {
    let id = random_uuid()?;
    send_typed_as(
        principal,
        writer,
        pending,
        id,
        pending_kind,
        kind,
        expected,
        body,
    )
    .await
}

#[allow(
    clippy::too_many_arguments,
    reason = "a replayed command carries its durable identity beside the ordinary typed send"
)]
async fn send_typed_as<B: Serialize>(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    id: CanonicalUuid,
    pending_kind: PendingKind,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: B,
) -> Result<(), String> {
    let command = SteerPane::over(principal)
        .command(id, kind, expected, body)
        .map_err(|error| error.to_string())?;
    let bytes = wire::encode(&command).map_err(|error| error.to_string())?;
    pending.insert(id, pending_kind);
    writer
        .send(Message::Bytes(bytes))
        .await
        .map_err(websocket_error)
}

async fn send_subscription(
    principal: &Principal,
    data: &Rc<RefCell<SurfaceData>>,
    writer: &mut SocketWriter,
) -> Result<(), String> {
    let (subjects, after, pending_thread) = {
        let data = data.borrow();
        let runtime = data
            .as_ref()
            .ok_or_else(|| "canvas runtime is absent".to_owned())?;
        (
            runtime.subjects(),
            runtime.resume(),
            runtime.pending_thread(),
        )
    };
    send_stream_request(
        writer,
        &StreamRequest::Subscribe {
            v: WireVersion::CURRENT,
            id: random_uuid()?,
            epoch: principal.epoch(),
            subjects,
            after,
        },
    )
    .await?;
    if let Some(cursor) = pending_thread {
        send_stream_request(
            writer,
            &StreamRequest::Thread {
                v: WireVersion::CURRENT,
                id: random_uuid()?,
                epoch: principal.epoch(),
                cursor,
            },
        )
        .await?;
    }
    Ok(())
}

async fn send_stream_directive(
    principal: &Principal,
    writer: &mut SocketWriter,
    directive: StreamDirective,
) -> Result<bool, String> {
    let request = match directive {
        StreamDirective::Continue { subject, cursor } => StreamRequest::Continue {
            v: WireVersion::CURRENT,
            id: random_uuid()?,
            epoch: principal.epoch(),
            subject,
            cursor,
        },
        StreamDirective::Thread { cursor } => StreamRequest::Thread {
            v: WireVersion::CURRENT,
            id: random_uuid()?,
            epoch: principal.epoch(),
            cursor,
        },
        StreamDirective::Reconnect => return Ok(true),
    };
    send_stream_request(writer, &request).await?;
    Ok(false)
}

async fn send_stream_request(
    writer: &mut SocketWriter,
    request: &StreamRequest,
) -> Result<(), String> {
    writer
        .send(Message::Bytes(
            wire::encode(request).map_err(|error| error.to_string())?,
        ))
        .await
        .map_err(websocket_error)
}

/// Apply one admitted stream frame, publishing the surface it changed.
///
/// A liveness answer reports no state, and the caller has already taken the
/// only thing it carries by observing that a frame arrived. Answering early
/// skips the surface publication below, which would otherwise hand the view a
/// fresh clone and redraw an idle board on every probe.
fn handle_stream_frame(
    session: &Session,
    surface: &UseStateHandle<SurfaceData>,
    data: &Rc<RefCell<SurfaceData>>,
    bytes: &[u8],
) -> Result<(Vec<StreamDirective>, bool), String> {
    let admitted = session
        .admit_stream(bytes)
        .map_err(|fault| format!("{}: {}", fault.reason, fault.detail))?;
    if matches!(admitted.get(), wire::StreamFrame::Pong { .. }) {
        return Ok((Vec::new(), false));
    }
    let refresh = stale_reads(&admitted).any();
    let directives = data
        .borrow_mut()
        .as_mut()
        .ok_or_else(|| "stream frame arrived before runtime acceptance".to_owned())?
        .apply_stream(&admitted);
    surface.set(data.borrow().clone());
    Ok((directives, refresh))
}

fn handle_incoming(
    session: &Session,
    handles: &SurfaceHandles,
    data: &Rc<RefCell<SurfaceData>>,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    bytes: &[u8],
) -> Result<(Vec<StreamDirective>, bool), String> {
    let crossing: serde_json::Value = wire::decode(bytes).map_err(|error| error.to_string())?;
    if crossing.get("stream").is_some() {
        return handle_stream_frame(session, &handles.surface, data, bytes);
    }
    let identity: ResponseIdentity = wire::decode(bytes).map_err(|error| error.to_string())?;
    let kind = pending
        .remove(&identity.id)
        .ok_or_else(|| format!("response named unknown command {}", identity.id))?;
    let mut data = data.borrow_mut();
    let runtime = data
        .as_mut()
        .ok_or_else(|| "response arrived before runtime acceptance".to_owned())?;
    let refresh = apply_command_response(session, handles, runtime, identity.id, kind, bytes)?;
    handles.surface.set(data.clone());
    Ok((Vec::new(), refresh))
}

fn apply_command_response(
    session: &Session,
    handles: &SurfaceHandles,
    runtime: &mut CanvasRuntime,
    id: CanonicalUuid,
    kind: PendingKind,
    bytes: &[u8],
) -> Result<bool, String> {
    Ok(match kind {
        PendingKind::Board => {
            runtime.apply_board(&admit_response(session, bytes)?);
            false
        }
        // A read of the lane table and an arrangement of it answer with the
        // same authoritative table, so successful replies apply identically.
        // Teams move only when that answer lands. A refused mutation differs:
        // it means this client lost its optimistic revision, so it immediately
        // re-reads the winning table rather than retaining the stale render.
        PendingKind::Lanes { wakes } => {
            apply_lane_response(session, handles, runtime, &wakes, false, bytes)?
        }
        PendingKind::LaneArrange | PendingKind::LineageArrange => {
            apply_lane_response(session, handles, runtime, &[], true, bytes)?
        }
        PendingKind::Inbox { covered } => {
            apply_inbox_response(session, handles, runtime, &covered, bytes)?
        }
        PendingKind::Consent => {
            runtime.apply_consent(&admit_response(session, bytes)?);
            false
        }
        PendingKind::Policy => {
            runtime.apply_policy(&admit_response(session, bytes)?);
            false
        }
        PendingKind::Effort => {
            runtime.apply_effort(&admit_response(session, bytes)?);
            false
        }
        PendingKind::LaneSeat => {
            let admitted: crate::Admitted<Response<wire::intent::LaneChangeAnswer>> =
                admit_response(session, bytes)?;
            runtime.apply_lane_change(&admitted);
            true
        }
        PendingKind::WakeSubmission { lane, submission } => {
            apply_wake_submission_response(session, handles, runtime, submission, lane, bytes)?
        }
        PendingKind::LineageConfigure(lineage) => apply_lineage_configuration(
            session,
            &handles.lineage_controls,
            runtime,
            lineage,
            bytes,
        )?,
        PendingKind::Steer { lane } => {
            let admitted: crate::Admitted<Response<wire::intent::DirectiveAnswer>> =
                admit_response(session, bytes)?;
            runtime.apply_steer(&admitted, lane);
            true
        }
        PendingKind::SteerSubmission { lane } => {
            apply_submission_response(session, handles, runtime, id, lane, bytes)?
        }
        PendingKind::Note(task) => {
            let admitted: crate::Admitted<Response<TaskAnswer>> = admit_response(session, bytes)?;
            runtime.apply_note(&admitted, &task.to_string());
            false
        }
        PendingKind::ConsentDecision => {
            let admitted: crate::Admitted<Response<wire::intent::ConsentAnswer>> =
                admit_response(session, bytes)?;
            runtime.apply_decision(&admitted);
            false
        }
    })
}

fn apply_lane_response(
    session: &Session,
    handles: &SurfaceHandles,
    runtime: &mut CanvasRuntime,
    wakes: &[CanonicalUuid],
    refresh_on_failure: bool,
    bytes: &[u8],
) -> Result<bool, String> {
    let admitted: crate::Admitted<Response<wire::intent::LaneAnswer>> =
        admit_response(session, bytes)?;
    let wake_lanes = handles.submissions.queue.borrow().awaiting_lanes(wakes);
    if let wire::Outcome::Reply(reply) = &admitted.get().outcome {
        for lane in &wake_lanes {
            let row = reply.body.lanes.iter().find(|row| row.lane == *lane);
            if let Some(row) = row
                && row.bound
                && let Some(occupant) = row.occupant
            {
                handles.submissions.queue.borrow_mut().bind_woken_lane(
                    *lane,
                    occupant,
                    reply.body.epoch,
                );
            } else {
                let restored = handles
                    .submissions
                    .queue
                    .borrow_mut()
                    .refuse_woken_lane(*lane)?;
                restore_lane_submissions(
                    handles,
                    *lane,
                    restored,
                    "Agent wake succeeded, but a fresh lane snapshot did not establish an \
                     occupant. Review the lane and retry.",
                );
            }
        }
        handles
            .lineage_controls
            .dispatch(LineageControlAction::SyncLanes {
                answer: reply.body.clone(),
                settle: false,
            });
    } else if let Some(issue) = outcome_issue(
        "Agent woke, but Canvas could not confirm fresh residency",
        &admitted.get().outcome,
    ) {
        for lane in &wake_lanes {
            let restored = handles
                .submissions
                .queue
                .borrow_mut()
                .refuse_woken_lane(*lane)?;
            restore_lane_submissions(handles, *lane, restored, &issue);
        }
    }
    if !wake_lanes.is_empty() {
        handles.submissions.publish();
    }
    let refresh = refresh_on_failure && topology_mutation_requires_refresh(&admitted.get().outcome);
    runtime.apply_lanes(&admitted);
    Ok(refresh)
}

fn apply_inbox_response(
    session: &Session,
    handles: &SurfaceHandles,
    runtime: &mut CanvasRuntime,
    covered: &BTreeMap<CanonicalUuid, String>,
    bytes: &[u8],
) -> Result<bool, String> {
    let admitted: crate::Admitted<Response<wire::intent::InboxAnswer>> =
        admit_response(session, bytes)?;
    let answered = if let wire::Outcome::Reply(reply) = &admitted.get().outcome {
        handles.submissions.queue.borrow_mut().observe_backend(
            reply.body.lanes.iter().flat_map(|lane| lane.pending.iter()),
            covered,
        )?;
        handles.submissions.publish();
        true
    } else {
        false
    };
    runtime.apply_inbox(&admitted);
    Ok(answered && !handles.submissions.queue.borrow().accepted().is_empty())
}

fn apply_submission_response(
    session: &Session,
    handles: &SurfaceHandles,
    runtime: &mut CanvasRuntime,
    id: CanonicalUuid,
    lane: CanonicalUuid,
    bytes: &[u8],
) -> Result<bool, String> {
    let admitted: crate::Admitted<Response<wire::intent::DirectiveAnswer>> =
        admit_response(session, bytes)?;
    match &admitted.get().outcome {
        wire::Outcome::Reply(reply) => handles.submissions.queue.borrow_mut().accept(
            id,
            reply.body.directive,
            reply.body.key.clone(),
            reply.body.disposition,
        )?,
        wire::Outcome::Refusal(_) | wire::Outcome::Fault(_) => {
            let restored = handles.submissions.queue.borrow_mut().refuse(id)?;
            let issue = outcome_issue("Message was not sent", &admitted.get().outcome)
                .expect("the refused or faulted steering outcome has an issue");
            restore_lane_submissions(handles, lane, restored, &issue);
        }
    }
    handles.submissions.publish();
    runtime.apply_steer(&admitted, lane);
    Ok(true)
}

fn apply_wake_submission_response(
    session: &Session,
    handles: &SurfaceHandles,
    runtime: &mut CanvasRuntime,
    submission: CanonicalUuid,
    lane: CanonicalUuid,
    bytes: &[u8],
) -> Result<bool, String> {
    let admitted: crate::Admitted<Response<wire::intent::LaneChangeAnswer>> =
        admit_response(session, bytes)?;
    match &admitted.get().outcome {
        wire::Outcome::Reply(_) => handles
            .submissions
            .queue
            .borrow_mut()
            .wake_accepted(submission)?,
        wire::Outcome::Refusal(_) | wire::Outcome::Fault(_) => {
            let restored = handles
                .submissions
                .queue
                .borrow_mut()
                .refuse_wake(submission)?;
            let issue = outcome_issue("Could not wake this agent", &admitted.get().outcome)
                .expect("the refused or faulted wake outcome has an issue");
            restore_lane_submissions(handles, lane, restored, &issue);
        }
    }
    handles.submissions.publish();
    runtime.apply_lane_change(&admitted);
    Ok(matches!(&admitted.get().outcome, wire::Outcome::Reply(_)))
}

fn restore_lane_submissions(
    handles: &SurfaceHandles,
    lane: CanonicalUuid,
    restored: Vec<crate::composer_drafts::ComposerDraft>,
    issue: &str,
) {
    handles
        .composer_drafts
        .dispatch(ComposerAction::RestoreAll {
            lane: lane.to_string(),
            drafts: restored,
        });
    handles.composer_drafts.dispatch(ComposerAction::SetIssue {
        lane: lane.to_string(),
        issue: issue.to_owned(),
    });
}

fn outcome_issue<T>(action: &str, outcome: &wire::Outcome<T>) -> Option<String> {
    match outcome {
        wire::Outcome::Reply(_) => None,
        wire::Outcome::Refusal(refusal) => Some(format!(
            "{action}: {}. {}",
            refusal.reason(),
            refusal.explanation()
        )),
        wire::Outcome::Fault(fault) => {
            Some(format!("{action}: {}. {}", fault.reason, fault.detail))
        }
    }
}
