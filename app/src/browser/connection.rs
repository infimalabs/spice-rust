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
    let data = SurfaceData::default();
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
    data: &SurfaceData,
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
    data: &SurfaceData,
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
    let (thread_timer_sender, mut thread_timers) = mpsc::unbounded::<()>();
    let mut thread_refresh = ThreadRefresh::default();
    while active.get() {
        let incoming = reader.next().fuse();
        let action = actions.next().fuse();
        let thread_timer = thread_timers.next().fuse();
        futures_util::pin_mut!(incoming, action, thread_timer);
        let mut rearm = false;
        futures_util::select_biased! {
            tick = thread_timer => {
                tick.ok_or_else(|| "canvas thread refresh timer closed".to_owned())?;
                if let Some(cursor) = thread_refresh.take_due() {
                    send_stream_request(
                        &mut writer,
                        &StreamRequest::Thread {
                            v: WireVersion::CURRENT,
                            id: random_uuid()?,
                            epoch: principal.epoch(),
                            cursor,
                        },
                    )
                    .await?;
                }
            },
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
                let (directives, refresh, refresh_thread) =
                    handle_incoming(
                        &session,
                        handles,
                        data,
                        &mut pending,
                        &bytes,
                    )?;
                if let Some(cursor) = refresh_thread
                    && thread_refresh.observe(cursor)
                {
                    let timer = thread_timer_sender.clone();
                    spawn_local(async move {
                        TimeoutFuture::new(THREAD_REFRESH_COALESCE_MILLIS).await;
                        let _ = timer.unbounded_send(());
                    });
                }
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
    data: &SurfaceData,
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
    let response = Request::get(&authenticated_path(
        "/bootstrap",
        browser_auth_token()?.as_deref(),
    ))
    .abort_signal(Some(signal))
    .send()
    .await
    .map_err(|error| format!("bootstrap request failed: {error}"))?;
    let canvas = response
        .headers()
        .get(wire::hello::CANVAS_VERSION_HEADER)
        .map(|value| {
            value
                .parse::<wire::CanvasVersion>()
                .map_err(|error| format!("bootstrap Canvas version header was invalid: {error}"))
        })
        .transpose()?;
    if let Err(fault) = Session::admit_canvas(canvas) {
        web_sys::window()
            .ok_or_else(|| "browser window is unavailable".to_owned())?
            .location()
            .reload()
            .map_err(|_| "browser could not load the current Canvas application".to_owned())?;
        return Err(format!(
            "{}: {}; loading the current Canvas application",
            fault.reason, fault.detail
        ));
    }
    let discovery: wire::CanvasBootstrap = response
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
    data: &SurfaceData,
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
        let mut data = data.write();
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
    surface.set(data.publication());
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
            send_lane_seat(
                principal,
                writer,
                pending,
                topology_revision,
                name,
                worktree,
                life,
                vendor,
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
            send_lineage_arrangement(principal, writer, pending, topology_revision, lineages).await
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
            send_steer(
                principal,
                writer,
                pending,
                lane,
                occupant,
                epoch,
                message,
                attachments,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn send_steer(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    message: String,
    attachments: Vec<DirectiveAttachment>,
) -> Result<(), String> {
    let body = SteerSend {
        fanout: random_uuid()?,
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

#[allow(clippy::too_many_arguments)]
async fn send_lane_seat(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    topology_revision: u64,
    name: Option<String>,
    worktree: String,
    life: String,
    vendor: wire::Name,
) -> Result<(), String> {
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

async fn send_lineage_arrangement(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    topology_revision: u64,
    lineages: Vec<LineageArrangement>,
) -> Result<(), String> {
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
            fanout: submission.fanout,
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
    let born = task
        .born()
        .map_err(|error| format!("task handle {task} has no canonical inception stamp: {error}"))?;
    let body = TaskNote {
        task: task.clone(),
        note,
    };
    let expected = vec![Witness::Existence { task: born }];
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
    data: &SurfaceData,
    writer: &mut SocketWriter,
) -> Result<(), String> {
    let (subjects, after, pending_thread) = {
        let data = data.read();
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

mod responses;
use responses::handle_incoming;
