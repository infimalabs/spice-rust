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
    actions: ActionQueue,
    mut action_wakes: mpsc::Receiver<()>,
) {
    let mut data = SurfaceData::default();
    let mut backoff = crate::reconnect::Backoff::new();
    while active.get() {
        if matches!(&*handles.connection, Connection::Starting) {
            handles.connection.set(Connection::Connecting);
        }
        let result =
            run_connection(&active, &handles, &mut data, &actions, &mut action_wakes).await;
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
        actions.reconnect();
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
    data: &mut SurfaceData,
    actions: &ActionQueue,
    action_wakes: &mut mpsc::Receiver<()>,
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
            &handles.identity,
            &handles.canvas_update,
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
    pump_session(active, handles, data, actions, action_wakes, established)
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
    actions: &ActionQueue,
    action_wakes: &mut mpsc::Receiver<()>,
    established: EstablishedConnection,
) -> Result<(), String> {
    let EstablishedConnection {
        session,
        mut writer,
        mut reader,
        principal,
        mut hydrating,
    } = established;
    send_subscription(&principal, data, &mut writer).await?;
    send_refresh(&principal, &mut writer, actions, &handles.submissions).await?;
    refresh::send_task_details(&principal, &mut writer, actions, data).await?;
    drain_submissions(&principal, &mut writer, actions, &handles.submissions).await?;
    actions.rearm().map_err(|error| error.to_string())?;
    let mut silence = TimeoutFuture::new(LIVENESS_PROBE_MILLIS).fuse();
    let mut lane_refresh = futures_util::future::Fuse::terminated();
    let mut probed = false;
    while active.get() {
        let incoming = reader.next().fuse();
        let action_wake = action_wakes.next().fuse();
        futures_util::pin_mut!(incoming, action_wake);
        let mut rearm = false;
        futures_util::select_biased! {
            () = lane_refresh => {
                refresh::send_lanes(&principal, &mut writer, actions, &handles.submissions).await?;
            },
            frame = incoming => {
                // Any frame proves socket traffic; a refusal names a reachable
                // resident, so a probe response need not be Pong specifically.
                rearm = true;
                probed = false;
                if let Some(deadline) = apply_incoming_frame(
                    frame, &session, handles, data, &principal, &mut writer, actions,
                    !hydrating,
                ).await? {
                    lane_refresh = match deadline {
                        Some(millis) => TimeoutFuture::new(millis).fuse(),
                        None => futures_util::future::Fuse::terminated(),
                    };
                }
                if hydrating && actions.current_reads_settled() {
                    finish_hydration(handles, data, &principal)?;
                    hydrating = false;
                    actions.rearm().map_err(|error| error.to_string())?;
                }
            },
            wake = action_wake => {
                wake.ok_or_else(|| "canvas command channel closed".to_owned())?;
                if hydrating {
                    continue;
                }
                service_action_wake(
                    &principal,
                    data,
                    &mut writer,
                    &handles.submissions,
                    actions,
                ).await?;
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

fn finish_hydration(
    handles: &SurfaceHandles,
    data: &SurfaceData,
    principal: &Principal,
) -> Result<(), String> {
    let complete = data.read().as_ref().is_some_and(CanvasRuntime::is_hydrated);
    if !complete {
        return Err("replacement Canvas scope settled without every current snapshot".to_owned());
    }
    handles.surface.set(data.publication());
    handles.connection.set(Connection::Online {
        epoch: principal.epoch().to_string(),
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn service_action_wake(
    principal: &Principal,
    data: &SurfaceData,
    writer: &mut SocketWriter,
    submissions: &SubmissionHandle,
    actions: &ActionQueue,
) -> Result<(), String> {
    let Some(lease) = actions.lease() else {
        return Ok(());
    };
    dispatch_client_action(principal, data, writer, actions, submissions, lease).await?;
    actions.rearm().map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
async fn apply_incoming_frame(
    frame: Option<Result<Message, WebSocketError>>,
    session: &Session,
    handles: &SurfaceHandles,
    data: &SurfaceData,
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    publish: bool,
) -> Result<Option<Option<u32>>, String> {
    let frame = frame
        .ok_or_else(|| "resident closed the socket".to_owned())?
        .map_err(websocket_error)?;
    let bytes = binary_message(frame)?;
    diagnostics::instant("frame.received", || diagnostics::Metadata {
        frame_bytes: Some(bytes.len()),
        ..diagnostics::Metadata::default()
    });
    let application = handle_incoming(session, handles, data, actions, &bytes, publish)?;
    relay_directives(principal, writer, application.directives).await?;
    send_after_frame(
        principal,
        writer,
        actions,
        &handles.submissions,
        application.refresh,
        application.stale,
    )
    .await?;
    drain_submissions(principal, writer, actions, &handles.submissions).await?;
    actions.rearm().map_err(|error| error.to_string())?;
    Ok(application.lane_refresh)
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_client_action(
    principal: &Principal,
    data: &SurfaceData,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    submissions: &SubmissionHandle,
    mut lease: ActionLease,
) -> Result<(), String> {
    if lease.action().is_topology_mutation() {
        let topology_revision = data
            .read()
            .as_ref()
            .and_then(CanvasRuntime::lanes)
            .map(|lanes| lanes.topology_revision)
            .ok_or_else(|| "topology action has no authoritative lane table".to_owned())?;
        let id = lease.prepare_topology(topology_revision)?;
        if actions.pending_contains(id) {
            return Err(format!("topology command identity {id} is already pending"));
        }
        send_topology_as(principal, writer, id, lease.action()).await?;
        lease.submit_topology();
        Ok(())
    } else if matches!(lease.action(), ClientAction::DrainSubmissions) {
        drop(lease.commit());
        drain_submissions(principal, writer, actions, submissions).await
    } else {
        if matches!(
            lease.action(),
            ClientAction::FastMode(_) | ClientAction::Note { .. } | ClientAction::Steer { .. }
        ) {
            lease.prepare_pending_transfer();
        }
        match dispatch_action(principal, writer, actions, submissions, lease.action()).await {
            Ok(Some(id)) => {
                lease.commit_pending(id);
                Ok(())
            }
            Ok(None) => {
                drop(lease.commit());
                Ok(())
            }
            Err(error) => {
                lease.restore();
                Err(error)
            }
        }
    }
}

async fn send_topology_as(
    principal: &Principal,
    writer: &mut SocketWriter,
    id: CanonicalUuid,
    action: &ClientAction,
) -> Result<(), String> {
    match action {
        ClientAction::Seat {
            topology_revision,
            lane,
            name,
            worktree,
            life,
            vendor,
            model,
            effort,
        } => {
            send_untracked_as(
                principal,
                writer,
                id,
                CommandKind::LaneSeat,
                vec![],
                BorrowedLaneSeat {
                    topology_revision: *topology_revision,
                    lane: *lane,
                    team: *lane,
                    name: name.as_deref(),
                    worktree,
                    life,
                    vendor,
                    model: model.as_deref(),
                    effort: effort.as_deref(),
                    resume: None,
                },
            )
            .await
        }
        ClientAction::RenewLane {
            topology_revision,
            team,
            lane,
            requested,
        } => {
            send_untracked_as(
                principal,
                writer,
                id,
                CommandKind::LaneRenewal,
                vec![],
                LaneRenewal {
                    topology_revision: *topology_revision,
                    team: *team,
                    lane: *lane,
                    requested: *requested,
                },
            )
            .await
        }
        ClientAction::ArrangeTeams {
            topology_revision,
            teams,
        } => {
            send_untracked_as(
                principal,
                writer,
                id,
                CommandKind::TeamArrange,
                vec![],
                BorrowedTeamArrange {
                    topology_revision: *topology_revision,
                    teams,
                },
            )
            .await
        }
        ClientAction::ConfigureTeam {
            topology_revision,
            team,
            member,
            life,
        } => {
            send_untracked_as(
                principal,
                writer,
                id,
                CommandKind::TeamConfigure,
                vec![],
                TeamConfigure {
                    topology_revision: *topology_revision,
                    team: *team,
                    member: *member,
                    life: *life,
                },
            )
            .await
        }
        _ => Err("non-topology action reached topology dispatch".to_owned()),
    }
}

fn topology_pending_kind(action: &ClientAction) -> PendingKind {
    match action {
        ClientAction::Seat { .. } => PendingKind::LaneSeat,
        ClientAction::RenewLane {
            team,
            lane,
            requested,
            ..
        } => PendingKind::LaneRenewal {
            team: *team,
            lane: *lane,
            requested: *requested,
        },
        ClientAction::ConfigureTeam { team, life, .. } => PendingKind::TeamConfigure {
            team: *team,
            life: *life,
        },
        ClientAction::ArrangeTeams { .. } => PendingKind::TeamArrange,
        _ => unreachable!("only topology mutations have scheduler response state"),
    }
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
    identity: &UseStateHandle<Option<CanvasIdentity>>,
    canvas_update: &UseStateHandle<Option<CanvasUpdate>>,
    data: &mut SurfaceData,
    signal: &web_sys::AbortSignal,
) -> Result<EstablishedConnection, String> {
    let (session, writer, mut reader) = open_socket(identity, canvas_update, signal).await?;
    let (principal, hydrating) =
        admit_runtime(&session, &mut reader, connection, surface, data).await?;
    Ok(EstablishedConnection {
        session,
        writer,
        reader,
        principal,
        hydrating,
    })
}

/// Discover the resident endpoint and open the socket its bootstrap names.
async fn open_socket(
    identity: &UseStateHandle<Option<CanvasIdentity>>,
    canvas_update: &UseStateHandle<Option<CanvasUpdate>>,
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
    let generation = response
        .headers()
        .get(wire::hello::CANVAS_GENERATION_HEADER)
        .ok_or_else(|| "bootstrap carried no Canvas app generation".to_owned())?
        .parse::<wire::Digest>()
        .map_err(|error| format!("bootstrap Canvas app generation was invalid: {error}"))?;
    let loaded_generation = loaded_canvas_generation()?;
    canvas_update.set(canvas.and_then(|version| {
        (!version.is_current() || generation != loaded_generation).then(|| CanvasUpdate {
            version,
            generation: generation.clone(),
            loaded_generation: loaded_generation.clone(),
        })
    }));
    if let Err(fault) = Session::admit_canvas(canvas) {
        return Err(format!(
            "{}: {}; explicit Canvas update required",
            fault.reason, fault.detail
        ));
    }
    let discovery: wire::CanvasBootstrap = response
        .json()
        .await
        .map_err(|error| format!("bootstrap response was invalid: {error}"))?;
    let canvas = canvas.expect("current Canvas admission requires its version header");
    identity.set(Some(admit_bootstrap_identity(
        &discovery,
        canvas,
        &generation,
        loaded_generation,
    )?));
    publish_operator_emblem(discovery.operator_gravatar.as_ref());
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

fn admit_bootstrap_identity(
    discovery: &wire::CanvasBootstrap,
    canvas: wire::CanvasVersion,
    generation: &wire::Digest,
    loaded_app: wire::Digest,
) -> Result<CanvasIdentity, String> {
    if discovery.version.canvas != canvas {
        return Err("bootstrap body Canvas version disagreed with its header".to_owned());
    }
    if &discovery.version.app != generation {
        return Err("bootstrap body app generation disagreed with its header".to_owned());
    }
    if discovery.version.wire != wire::WireVersion::CURRENT {
        return Err("bootstrap body wire schema is not current".to_owned());
    }
    Ok(CanvasIdentity {
        version: discovery.version.clone(),
        loaded_app,
    })
}

/// Paint the operator's quote byline emblem from the bootstrap's Gravatar hash.
///
/// The hash crosses once per bootstrap; every operator byline reads the one
/// root property instead of carrying the address in rendered quote HTML.
fn publish_operator_emblem(gravatar: Option<&wire::GravatarHash>) {
    let Some(root) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.document_element())
        .and_then(|root| root.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    let style = root.style();
    match gravatar {
        Some(gravatar) => {
            style
                .set_property(
                    "--operator-emblem",
                    &format!(
                        "url(\"https://gravatar.com/avatar/{}?s=64&d=retro\")",
                        gravatar.as_str()
                    ),
                )
                .expect("the operator emblem CSS property is valid");
            root.set_attribute("data-operator-emblem", "")
                .expect("the operator emblem root attribute is valid");
        }
        None => {
            let _ = style.remove_property("--operator-emblem");
            let _ = root.remove_attribute("data-operator-emblem");
        }
    }
}

pub(super) fn loaded_canvas_generation() -> Result<wire::Digest, String> {
    web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("app"))
        .and_then(|root| root.get_attribute("data-canvas-generation"))
        .ok_or_else(|| "loaded Canvas app carried no generation identity".to_owned())?
        .parse()
        .map_err(|error| format!("loaded Canvas app generation was invalid: {error}"))
}

/// Admit the acceptance frame and publish the runtime that frame names.
async fn admit_runtime(
    session: &Session,
    reader: &mut SocketReader,
    connection: &UseStateHandle<Connection>,
    surface: &UseStateHandle<SurfaceData>,
    data: &mut SurfaceData,
) -> Result<(Principal, bool), String> {
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
    let hydrating = data
        .read()
        .as_ref()
        .is_some_and(|runtime| !runtime.same_scope(&accepted));
    if hydrating {
        let replacement = SurfaceData::default();
        let runtime = {
            let current = data.read();
            CanvasRuntime::rehydrate(
                &accepted,
                current
                    .as_ref()
                    .expect("changed-scope hydration has a prior runtime"),
            )
        };
        *replacement.write() = Some(runtime);
        *data = replacement;
    }
    let principal = {
        let mut data = data.write();
        match data.as_mut() {
            Some(runtime) => {
                if !hydrating {
                    runtime.reconnect(&accepted);
                }
            }
            None => *data = Some(CanvasRuntime::new(&accepted)),
        }
        data.as_ref()
            .expect("accepted connection creates runtime")
            .principal()
            .clone()
    };
    if !hydrating {
        surface.set(data.publication());
        connection.set(Connection::Online {
            epoch: principal.epoch().to_string(),
        });
    }
    Ok((principal, hydrating))
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
    actions: &ActionQueue,
    submissions: &SubmissionHandle,
    action: &ClientAction,
) -> Result<Option<CanonicalUuid>, String> {
    match action {
        ClientAction::DrainSubmissions => {
            drain_submissions(principal, writer, actions, submissions)
                .await
                .map(|_| None)
        }
        ClientAction::TaskDetail { task } => {
            refresh::send_task_detail(principal, writer, actions, task)
                .await
                .map(|_| None)
        }
        ClientAction::TaskRelations { key } => {
            refresh::send_task_relations(principal, writer, actions, key)
                .await
                .map(|_| None)
        }
        ClientAction::WorkHistory { request } => {
            refresh::send_work_history(principal, writer, actions, request)
                .await
                .map(|_| None)
        }
        ClientAction::FastMode(fast_mode) => send_fast_mode(principal, writer, actions, *fast_mode)
            .await
            .map(Some),
        ClientAction::MessageHistory {
            generation,
            lane,
            before,
        } => send_message_history(principal, writer, *generation, *lane, *before)
            .await
            .map(|_| None),
        ClientAction::Seat { .. }
        | ClientAction::RenewLane { .. }
        | ClientAction::ArrangeTeams { .. }
        | ClientAction::ConfigureTeam { .. } => {
            unreachable!("topology mutation dispatch is partitioned")
        }
        action @ (ClientAction::Note { .. } | ClientAction::Steer { .. }) => {
            dispatch_authored_action(principal, writer, actions, action)
                .await
                .map(Some)
        }
    }
}

async fn dispatch_authored_action(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    action: &ClientAction,
) -> Result<CanonicalUuid, String> {
    match action {
        ClientAction::Note {
            id,
            task,
            note,
            after,
        } => send_task_note(principal, writer, actions, *id, task, note, *after).await,
        ClientAction::Steer {
            id,
            fanout,
            lane,
            occupant,
            epoch,
            message,
            attachments,
        } => {
            send_steer(
                principal,
                writer,
                actions,
                *id,
                *fanout,
                *lane,
                *occupant,
                *epoch,
                message,
                attachments,
            )
            .await
        }
        _ => unreachable!("authored action dispatch is partitioned"),
    }
}

async fn send_fast_mode(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    fast_mode: bool,
) -> Result<CanonicalUuid, String> {
    send_typed(
        principal,
        writer,
        actions,
        PendingKind::Settings,
        CommandKind::GlobalSettingsSet,
        vec![],
        GlobalSettingsSet { fast_mode },
    )
    .await
}

async fn send_message_history(
    principal: &Principal,
    writer: &mut SocketWriter,
    generation: u64,
    lane: CanonicalUuid,
    before: Option<wire::MessageLocus>,
) -> Result<(), String> {
    send_stream_request(
        writer,
        &StreamRequest::MessageHistory {
            v: WireVersion::CURRENT,
            id: random_uuid()?,
            epoch: principal.epoch(),
            frontiers: vec![wire::MessageHistoryCursor {
                generation,
                lane,
                before,
            }],
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn send_steer(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    id: CanonicalUuid,
    fanout: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    message: &str,
    attachments: &[DirectiveAttachment],
) -> Result<CanonicalUuid, String> {
    let body = BorrowedSteerSend {
        fanout,
        lane,
        message,
        attachments,
    };
    let expected = vec![Witness::Residency {
        lane,
        occupant: Some(occupant),
        epoch,
    }];
    send_typed_as(
        principal,
        writer,
        actions,
        id,
        PendingKind::Steer,
        CommandKind::SteerSend,
        expected,
        body,
    )
    .await
}

async fn drain_submissions(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    submissions: &SubmissionHandle,
) -> Result<(), String> {
    let ready_wakes = submissions.queue.borrow().ready_wakes();
    for submission in ready_wakes {
        send_typed(
            principal,
            writer,
            actions,
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
            message: submission.message().to_owned(),
            attachments: submission.attachments().to_vec(),
        };
        let expected = vec![Witness::Residency {
            lane: submission.lane,
            occupant: Some(occupant),
            epoch,
        }];
        send_typed_as(
            principal,
            writer,
            actions,
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

async fn send_task_note(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    id: CanonicalUuid,
    task: &TaskHandle,
    note: &str,
    after: Option<wire::Seq>,
) -> Result<CanonicalUuid, String> {
    let born = task
        .born()
        .map_err(|error| format!("task handle {task} has no canonical birth stamp: {error}"))?;
    let body = BorrowedTaskNote { task, note };
    let expected = vec![Witness::Existence { task: born.clone() }];
    send_typed_as(
        principal,
        writer,
        actions,
        id,
        PendingKind::Note {
            task: born,
            text: note.to_owned(),
            after,
        },
        CommandKind::TaskNote,
        expected,
        body,
    )
    .await
}

#[derive(Serialize)]
struct BorrowedTaskNote<'a> {
    task: &'a TaskHandle,
    note: &'a str,
}

#[derive(Serialize)]
struct BorrowedLaneSeat<'a> {
    topology_revision: u64,
    lane: CanonicalUuid,
    team: CanonicalUuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    worktree: &'a str,
    life: &'a str,
    vendor: &'a wire::Name,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resume: Option<&'a str>,
}

#[derive(Serialize)]
struct BorrowedTeamArrange<'a> {
    topology_revision: u64,
    teams: &'a [TeamArrangement],
}

#[derive(Serialize)]
struct BorrowedSteerSend<'a> {
    fanout: CanonicalUuid,
    lane: CanonicalUuid,
    message: &'a str,
    #[serde(skip_serializing_if = "slice_is_empty")]
    attachments: &'a [DirectiveAttachment],
}

fn slice_is_empty<T>(value: &&[T]) -> bool {
    value.is_empty()
}

pub(super) async fn request_external_speech(
    worktree: &str,
    text: &str,
    content_type: &str,
) -> Result<Vec<u8>, String> {
    let request = Request::post(&authenticated_path(
        "/speech",
        browser_auth_token()?.as_deref(),
    ))
    .json(&wire::SpeechRequest {
        text: text.to_owned(),
        worktree: worktree.to_owned(),
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

async fn send_typed<B: Serialize>(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    pending_kind: PendingKind,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: B,
) -> Result<CanonicalUuid, String> {
    let id = random_uuid()?;
    send_typed_as(
        principal,
        writer,
        actions,
        id,
        pending_kind,
        kind,
        expected,
        body,
    )
    .await
}

async fn send_untracked_as<B: Serialize>(
    principal: &Principal,
    writer: &mut SocketWriter,
    id: CanonicalUuid,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: B,
) -> Result<(), String> {
    let command = SteerPane::over(principal)
        .command(id, kind, expected, body)
        .map_err(|error| error.to_string())?;
    let bytes = wire::encode(&command).map_err(|error| error.to_string())?;
    writer
        .send(Message::Bytes(bytes))
        .await
        .map_err(websocket_error)
}

#[allow(
    clippy::too_many_arguments,
    reason = "a replayed command carries its durable identity beside the ordinary typed send"
)]
async fn send_typed_as<B: Serialize>(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    id: CanonicalUuid,
    pending_kind: PendingKind,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: B,
) -> Result<CanonicalUuid, String> {
    if actions.pending_contains(id) {
        return Err(format!("command identity {id} is already pending"));
    }
    let command = SteerPane::over(principal)
        .command(id, kind, expected, body)
        .map_err(|error| error.to_string())?;
    let bytes = wire::encode(&command).map_err(|error| error.to_string())?;
    actions
        .reserve_pending(id, pending_kind, bytes.len())
        .map_err(|error| error.to_string())?;
    if let Err(error) = writer.send(Message::Bytes(bytes)).await {
        actions.cancel_pending(id);
        return Err(websocket_error(error));
    }
    Ok(id)
}

async fn send_subscription(
    principal: &Principal,
    data: &SurfaceData,
    writer: &mut SocketWriter,
) -> Result<(), String> {
    let (subjects, after) = {
        let data = data.read();
        let runtime = data
            .as_ref()
            .ok_or_else(|| "canvas runtime is absent".to_owned())?;
        (runtime.subjects(), runtime.resume())
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

mod refresh;
mod responses;
use refresh::{send_after_frame, send_refresh};
use responses::handle_incoming;
