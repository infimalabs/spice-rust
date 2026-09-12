//! Canvas publication and the resident's live surface loop.

use super::*;

pub(super) struct StartedSurface<'a> {
    pub(super) capabilities: canvas::axum::Capabilities,
    pub(super) discovery: canvas::Discovery,
    pub(super) control: canvas::axum::Control,
    pub(super) publication: SurfacePublication<'a>,
    pub(super) shutdown: oneshot::Sender<()>,
    pub(super) server_task: tokio::task::JoinHandle<Result<(), io::Error>>,
    pub(super) message_publisher: canvas::stream::MessagePublisher,
    pub(super) progress: &'a mut dyn io::Write,
}

pub(in crate::resident) struct SurfacePublication<'a> {
    discovery: canvas::Discovery,
    published: PublishedDiscovery,
    app_description: String,
    token: Option<String>,
    open: bool,
    endpoint: &'a mut dyn io::Write,
    daemon: Option<ChildStatus>,
}

struct MessageStartup {
    reader: message_surface::BacklogReader,
    diagnostic: Vec<u8>,
}

impl SurfacePublication<'_> {
    fn publish(
        &mut self,
        bus: &Bus,
        paths: &RuntimePaths,
        residency: &ResidencyHead,
        progress: &mut dyn io::Write,
    ) -> CommandResult<()> {
        steering::publish_lane_discoveries(paths, &self.discovery, residency)?;
        steering::publish_inboxes(bus, paths, residency)?;
        self.discovery
            .publish(&self.published.path)
            .map_err(|error| environment_fault("canvas::publish-failed", &error.to_string()))?;
        if let Some(daemon) = self.daemon.take() {
            daemon.published();
        }
        announce_started_surface(
            paths,
            &self.app_description,
            self.token.as_deref(),
            self.open,
            self.endpoint,
            progress,
        );
        Ok(())
    }
}

pub(super) async fn start_surface<'a>(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    residency: &ResidencyHead,
    config: &SurfaceConfig,
    launch: SurfaceLaunch<'a>,
) -> CommandResult<StartedSurface<'a>> {
    let operator = new_uuid();
    let capabilities = surface_capabilities(operator, residency)?;
    let SurfaceLaunch {
        app,
        version,
        binding,
        open,
        progress,
        endpoint,
        daemon,
    } = launch;
    let app_description = app.to_string();
    let CanvasBinding { listen, access } = binding;
    let CanvasAccess { policy, token } = access;
    let (server, control) = canvas::axum::Server::bind_streaming(
        canvas::axum::Binding::new(listen, policy),
        epoch,
        capabilities.clone(),
        version,
        app,
        config.speech.clone(),
        bus.reader(),
        config.canvas.rows(),
    )
    .await
    .map_err(|error| {
        environment_fault(
            "canvas::bind-failed",
            &format!("listening on {listen} failed: {error}"),
        )
    })?;
    let discovery = server
        .discovery(operator)
        .map_err(|error| environment_fault("canvas::discovery-invalid", &error.to_string()))?;
    let message_publisher = server.message_publisher().ok_or_else(|| {
        environment_fault(
            "canvas::message-surface-unavailable",
            "the streaming canvas server has no transcript activity publisher",
        )
    })?;
    let (shutdown, stopped) = oneshot::channel();
    let server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));
    Ok(StartedSurface {
        capabilities,
        discovery: discovery.clone(),
        control,
        publication: SurfacePublication {
            discovery,
            published: PublishedDiscovery {
                path: paths.canvas.clone(),
                lane_root: paths.lane_canvas.clone(),
            },
            app_description,
            token,
            open,
            endpoint,
            daemon,
        },
        shutdown,
        server_task,
        message_publisher,
        progress,
    })
}

fn announce_started_surface(
    paths: &RuntimePaths,
    app_description: &str,
    token: Option<&str>,
    open: bool,
    endpoint: &mut dyn io::Write,
    progress: &mut dyn io::Write,
) {
    let _ = writeln!(progress, "canvas app source: {app_description}");
    announce_canvas(
        &paths.canvas,
        token,
        open,
        || {},
        crate::command_client::system_browser,
        endpoint,
        progress,
    );
    let _ = endpoint.flush();
    let _ = progress.flush();
}

pub(super) async fn run_surface(
    bus: &Bus,
    repository: &Repository,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    config: &SurfaceConfig,
    seats: &mut Seats,
    launch: SurfaceLaunch<'_>,
) -> CommandResult<()> {
    let residency = current_residency(bus, epoch)?;
    let timer_commits = bus.reader().subscribe();
    let mut timers = timers::Runtime::start(config.projects.clone(), config.steering.clone())?;
    let mut lifecycle = lifecycle::Runtime::new(&config.lane);
    let mut taste_adjudication = if config.judge_enabled {
        Some(taste_surface::start_adjudication()?)
    } else {
        None
    };
    let started = start_surface(bus, paths, epoch, residency.body(), config, launch).await?;
    let StartedSurface {
        capabilities,
        discovery,
        mut control,
        mut publication,
        shutdown,
        mut server_task,
        message_publisher,
        progress,
    } = started;
    let channels = capture::channels();
    let supervisor_reports = capture::SupervisorReports::bind(paths)?;
    lane_surface::restore_seats(bus, paths, epoch, seats, &channels.reporter)?;
    ingest_transcripts(bus, paths, None, progress)?;
    let mut message_startup = launch_message_startup(bus, paths, epoch, &message_publisher)?;
    let reads = read_materializations::CurrentReads::start(bus, paths, &config.consent)
        .map_err(|error| store_fault("resident::read-materialization-failed", &error))?;
    let command_surface = command_surface::Surface::new(
        bus,
        repository,
        paths,
        epoch,
        command_surface::Runtimes::new(
            lane_surface::Runtime::new(&capabilities, &discovery, channels.reporter.clone()),
            lane_recovery::Runtime::new(channels.reporter.clone()),
        ),
        config,
        &reads,
    );
    let (taste_submitter, taste_completions) = taste_adjudication
        .as_mut()
        .map(taste_surface::AdjudicationRuntime::parts)
        .map_or((None, None), |(submitter, completions)| {
            (Some(submitter), Some(completions))
        });
    publish_ready_surface(
        bus,
        epoch,
        &command_surface,
        seats,
        progress,
        &mut timers,
        &mut lifecycle,
        &mut publication,
    )
    .await?;
    let mut inbox_projections = InboxProjections::new();
    let Some(message_startup) = await_message_startup(
        &mut message_startup,
        bus,
        &command_surface,
        seats,
        &mut lifecycle,
        &mut timers,
        &mut control,
        &mut server_task,
        &mut inbox_projections,
        progress,
    )
    .await?
    else {
        return stop_surface(publication, shutdown, server_task).await;
    };
    let _ = progress.write_all(&message_startup.diagnostic);
    let message_state = message_startup.reader.into_state();
    let session_compactions =
        message_surface::SessionCompactions::new(crate::session_environment());
    drive_surface(
        bus,
        epoch,
        seats,
        &command_surface,
        control,
        publication,
        shutdown,
        server_task,
        message_publisher,
        message_state,
        session_compactions,
        taste_submitter,
        taste_completions,
        channels.records,
        channels.failures,
        channels.exits,
        supervisor_reports,
        progress,
        timers,
        timer_commits,
        lifecycle,
        inbox_projections,
    )
    .await
}

#[allow(
    clippy::too_many_arguments,
    reason = "readiness joins the resident's command, timer, lifecycle, and publication owners"
)]
pub(in crate::resident) async fn publish_ready_surface(
    bus: &Bus,
    epoch: CanonicalUuid,
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    diagnostic: &mut dyn io::Write,
    timers: &mut timers::Runtime,
    lifecycle: &mut lifecycle::Runtime,
    publication: &mut SurfacePublication<'_>,
) -> CommandResult<()> {
    reconcile_wakes(bus, command_surface, seats, diagnostic, timers, lifecycle)?;
    command_surface.synchronize_reads_before_publish()?;
    let residency = current_residency(bus, epoch)?;
    tokio::task::yield_now().await;
    publication.publish(bus, command_surface.paths, residency.body(), diagnostic)
}

fn launch_message_startup(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    publisher: &canvas::stream::MessagePublisher,
) -> CommandResult<oneshot::Receiver<CommandResult<MessageStartup>>> {
    let residency = current_residency(bus, epoch)?;
    start_message_startup(paths, residency.body(), publisher, bus.reader())
}

fn start_message_startup(
    paths: &RuntimePaths,
    residency: &wire::ResidencyHead,
    publisher: &canvas::stream::MessagePublisher,
    bus: bus::BusReader,
) -> CommandResult<oneshot::Receiver<CommandResult<MessageStartup>>> {
    let paths = paths.clone();
    let residency = residency.clone();
    let publisher = publisher.clone();
    let (finished, receiver) = oneshot::channel();
    std::thread::Builder::new()
        .name("spice-message-startup".to_owned())
        .spawn(move || {
            let mut diagnostic = Vec::new();
            let result = {
                let mut reader = message_surface::BacklogReader::indexed();
                message_surface::restore_agent_messages_indexed(
                    &bus,
                    &paths,
                    &residency,
                    &publisher,
                    &mut reader,
                    &mut diagnostic,
                )
                .map(|()| MessageStartup { reader, diagnostic })
            };
            let _ = finished.send(result);
        })
        .map_err(|error| {
            environment_fault("canvas::message-startup-unavailable", &error.to_string())
        })?;
    Ok(receiver)
}

#[allow(
    clippy::too_many_arguments,
    reason = "startup admission services the live command and server boundaries"
)]
async fn await_message_startup(
    startup: &mut oneshot::Receiver<CommandResult<MessageStartup>>,
    bus: &Bus,
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    lifecycle: &mut lifecycle::Runtime,
    timers: &mut timers::Runtime,
    control: &mut canvas::axum::Control,
    server_task: &mut tokio::task::JoinHandle<Result<(), io::Error>>,
    inbox_projections: &mut InboxProjections,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<Option<MessageStartup>> {
    loop {
        tokio::select! {
            result = &mut *startup => {
                let ready = result.map_err(|_| environment_fault(
                    "canvas::message-startup-closed",
                    "the message startup worker ended without its terminal result",
                ))?;
                return ready.map(Some);
            }
            request = control.recv(), if inbox_projections.is_empty() => {
                let dispatched = dispatch_control_burst(
                    command_surface, seats, lifecycle, control, request, inbox_projections,
                ).await?;
                if dispatched.reached_dark {
                    return Ok(None);
                }
                if dispatched.lifecycle_released {
                    reconcile_wakes(
                        bus, command_surface, seats, diagnostic, timers, lifecycle,
                    )?;
                }
            }
            () = std::future::ready(()), if !inbox_projections.is_empty() => {
                project_next_inbox(command_surface, inbox_projections)?;
            }
            ending = &mut *server_task => return Err(server_outcome(ending)),
        }
    }
}

/// Announce the app this start serves, and dial it when asked to.
///
/// The endpoint is read back out of the discovery file the resident has just
/// published rather than formatted from the address it bound, so the line an
/// operator is told to open has already passed the ownership, loopback-origin,
/// and generation admission that a client applies to the same file. That is
/// the whole reason there is no separate opener command: one reader admits the
/// endpoint, and `up` is simply its first caller.
///
/// Authenticated access decorates that admitted endpoint only after admission.
/// The token stays in the URL fragment for announcement and browser launch, so
/// it neither changes the discovery origin nor crosses in the browser's HTTP
/// origin.
///
/// A resident whose own discovery fails that admission says so and keeps
/// serving. The surface is up either way, and refusing to run because the
/// operator cannot be handed a URL would trade a working appliance for a
/// cosmetic one.
///
/// Only an endpoint reaches the endpoint stream; a failure to produce one is a
/// diagnostic and goes where the other diagnostics went. A shell that reads the
/// endpoint stream to dial it cannot tell a URL from a sentence explaining why
/// there is not one, so it sees an empty capture instead.
pub(in crate::resident) fn announce_canvas(
    discovery_path: &Path,
    auth_token: Option<&str>,
    open: bool,
    before_current_check: impl FnOnce(),
    browser: impl FnOnce(&str) -> Result<(), String>,
    announcement: &mut dyn io::Write,
    diagnostic: &mut dyn io::Write,
) {
    let result = crate::command_client::open_canvas_result_with(
        discovery_path,
        before_current_check,
        |endpoint| {
            if open {
                browser(&canvas_opening(endpoint, auth_token))
            } else {
                Ok(())
            }
        },
    );
    let _ = match result {
        Ok(opened) => writeln!(
            announcement,
            "canvas endpoint: {}",
            canvas_opening(&opened.endpoint, auth_token)
        ),
        Err(fault) => writeln!(diagnostic, "canvas unavailable: {}", fault.reason),
    };
}

fn canvas_opening(endpoint: &str, auth_token: Option<&str>) -> String {
    auth_token.map_or_else(
        || endpoint.to_owned(),
        |token| format!("{endpoint}#{token}"),
    )
}

fn surface_capabilities(
    operator: CanonicalUuid,
    residency: &ResidencyHead,
) -> CommandResult<canvas::axum::Capabilities> {
    let capabilities = canvas::axum::Capabilities::default();
    capabilities
        .insert(operator, Actor::Operator)
        .map_err(|error| environment_fault("canvas::capability-invalid", &error.to_string()))?;
    for lane in &residency.lanes {
        for capability in &lane.capabilities {
            capabilities
                .insert(*capability, Actor::Lane(lane.lane))
                .map_err(|error| {
                    environment_fault("canvas::capability-invalid", &error.to_string())
                })?;
        }
    }
    Ok(capabilities)
}

#[allow(
    clippy::too_many_arguments,
    reason = "the resident loop owns one receiver for each closed event source"
)]
pub(super) async fn drive_surface(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    mut control: canvas::axum::Control,
    publication: SurfacePublication<'_>,
    shutdown: oneshot::Sender<()>,
    mut server_task: tokio::task::JoinHandle<Result<(), io::Error>>,
    message_publisher: canvas::stream::MessagePublisher,
    mut message_state: message_surface::MessageState,
    mut session_compactions: message_surface::SessionCompactions,
    mut taste_submitter: Option<&mut taste_surface::AdjudicationSubmitter>,
    mut taste_completions: Option<&mut taste_surface::AdjudicationCompletions>,
    mut records: tokio::sync::mpsc::Receiver<lane::capture::CaptureRecord>,
    mut failures: tokio::sync::mpsc::UnboundedReceiver<lane::capture::CaptureFailure>,
    mut exits: tokio::sync::mpsc::UnboundedReceiver<lane::capture::CaptureExit>,
    mut supervisor_reports: capture::SupervisorReports,
    diagnostic: &mut dyn io::Write,
    mut timers: timers::Runtime,
    mut timer_commits: bus::BroadcastSubscriber,
    mut lifecycle: lifecycle::Runtime,
    mut inbox_projections: InboxProjections,
) -> CommandResult<()> {
    let mut session_compaction_poll = session_compaction_poll();
    loop {
        tokio::select! {
            request = control.recv(), if inbox_projections.is_empty() => {
                let dispatched = dispatch_control_burst(
                    command_surface, seats, &mut lifecycle, &mut control, request,
                    &mut inbox_projections,
                ).await?;
                if dispatched.reached_dark {
                    timers.synchronize(bus)?;
                    return stop_surface(publication, shutdown, server_task).await;
                }
                if dispatched.lifecycle_released {
                    reconcile_wakes(
                        bus, command_surface, seats, diagnostic, &mut timers, &mut lifecycle,
                    )?;
                }
            }
            () = std::future::ready(()), if !inbox_projections.is_empty() => {
                project_next_inbox(command_surface, &mut inbox_projections)?;
            }
            record = records.recv() => {
                let residency = current_residency(bus, epoch)?;
                session_compactions.synchronize(
                    bus, epoch, residency.body(), &message_publisher,
                    &mut message_state, diagnostic,
                )?;
                accept_record(
                    bus, epoch, seats, command_surface, &message_publisher,
                    taste_submitter.as_deref_mut(), record, &mut message_state, diagnostic,
                )?;
            }
            completed = receive_judgment(&mut taste_completions) => {
                let completed = completed?;
                taste_submitter
                    .as_deref_mut()
                    .expect("a completion receiver always has its submitter")
                    .finished(&completed);
                taste_surface::complete_judgment(
                    bus,
                    command_surface.paths,
                    epoch,
                    &command_surface.config.taste,
                    &command_surface.config.steering,
                    completed,
                )?;
            }
            failure = failures.recv() => accept_failure(
                bus, epoch, seats, command_surface, &mut lifecycle, failure,
            )?,
            exit = exits.recv() => accept_exit(
                bus, epoch, seats, command_surface, &mut lifecycle, exit,
            )?,
            report = supervisor_reports.recv() => accept_supervisor_report(
                bus, epoch, seats, command_surface, &message_publisher,
                taste_submitter.as_deref_mut(), &mut lifecycle, report,
                &mut message_state, diagnostic,
            )?,
            _ = session_compaction_poll.tick() => {
                let residency = current_residency(bus, epoch)?;
                session_compactions.synchronize(
                    bus, epoch, residency.body(), &message_publisher, &mut message_state,
                    diagnostic,
                )?;
            }
            timer = timers.recv() => {
                let timer = timer.ok_or_else(|| environment_fault(
                    "bus::timer-events-closed",
                    "the production timer event stream closed while the resident was live",
                ))?;
                let fired = timers.consume(bus, timer)?;
                accept_timer_fired(bus, command_surface, &mut lifecycle, fired)?;
                reconcile_wakes(
                    bus, command_surface, seats, diagnostic, &mut timers, &mut lifecycle,
                )?;
            }
            committed = timer_commits.recv() => accept_commit(
                bus, command_surface, seats, diagnostic, &mut timers,
                &mut lifecycle, committed,
            )?,
            ending = &mut server_task => return Err(server_outcome(ending)),
        }
    }
}

fn session_compaction_poll() -> tokio::time::Interval {
    let interval = message_surface::SESSION_COMPACTION_POLL_INTERVAL;
    let mut poll = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    poll
}

#[allow(
    clippy::too_many_arguments,
    reason = "one supervisor notice crosses the same live capture surfaces as an in-process record"
)]
fn accept_supervisor_report(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    message_publisher: &canvas::stream::MessagePublisher,
    taste_judge: Option<&mut taste_surface::AdjudicationSubmitter>,
    lifecycle: &mut lifecycle::Runtime,
    report: io::Result<super::supervisor::Notice>,
    message_state: &mut message_surface::MessageState,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let notice = match report {
        Ok(notice) => notice,
        Err(error) => {
            let _ = writeln!(diagnostic, "lane supervisor notice refused: {error}");
            return Ok(());
        }
    };
    if let super::supervisor::Notice::Exited { lane, occupant } = notice {
        return accept_exit(
            bus,
            epoch,
            seats,
            command_surface,
            lifecycle,
            Some(lane::capture::CaptureExit { lane, occupant }),
        );
    }
    if let super::supervisor::Notice::Failed {
        lane,
        occupant,
        failure,
    } = notice
    {
        if !lane_surface::renewal_requested_exit(bus, epoch, lane, occupant)? {
            command_surface.observe_lifecycle_death(lifecycle, seats, lane, occupant)?;
        }
        if seats
            .occupant(lane)
            .is_some_and(|sitting| sitting.occupant() == occupant)
        {
            let _detached = seats.vacate(lane);
        }
        return lane_surface::supervisor_failed(bus, epoch, lane, occupant, &failure);
    }
    let residency = current_residency(bus, epoch)?;
    let Some(record) = capture::materialize(command_surface.paths, residency.body(), notice)?
    else {
        return Ok(());
    };
    accept_record(
        bus,
        epoch,
        seats,
        command_surface,
        message_publisher,
        taste_judge,
        Some(record),
        message_state,
        diagnostic,
    )
}

async fn receive_judgment(
    completions: &mut Option<&mut taste_surface::AdjudicationCompletions>,
) -> CommandResult<taste_surface::CompletedJudgment> {
    match completions.as_deref_mut() {
        Some(completions) => completions.recv().await,
        None => std::future::pending().await,
    }
}

fn accept_timer_fired(
    bus: &Bus,
    command_surface: &command_surface::Surface<'_>,
    lifecycle: &mut lifecycle::Runtime,
    fired: timers::Fired,
) -> CommandResult<()> {
    match fired {
        timers::Fired::LaneResume(lane) => command_surface.expire_lifecycle_hold(lifecycle, lane),
        timers::Fired::Steering => {
            let residency = bus
                .residency_head()
                .map_err(|error| store_fault("steer::residency-unreadable", &error.to_string()))?;
            if let Some(residency) = residency {
                steering::publish_inboxes(bus, command_surface.paths, residency.body())?;
            }
            Ok(())
        }
        timers::Fired::Stale
        | timers::Fired::DeferredTaskWoken(_)
        | timers::Fired::ClaimLapse
        | timers::Fired::ClaimRenew
        | timers::Fired::TaskWake(_) => Ok(()),
    }
}

async fn stop_surface(
    publication: SurfacePublication<'_>,
    shutdown: oneshot::Sender<()>,
    server_task: tokio::task::JoinHandle<Result<(), io::Error>>,
) -> CommandResult<()> {
    drop(publication);
    let _ = shutdown.send(());
    match server_task.await {
        Ok(Ok(())) => Ok(()),
        ending => Err(server_outcome(ending)),
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one capture record crosses all resident-owned fold surfaces"
)]
fn accept_record(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    message_publisher: &canvas::stream::MessagePublisher,
    taste_judge: Option<&mut taste_surface::AdjudicationSubmitter>,
    record: Option<lane::capture::CaptureRecord>,
    message_state: &mut message_surface::MessageState,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let record = record.ok_or_else(|| {
        environment_fault(
            "lane::capture-records-closed",
            "lane capture record receiver closed",
        )
    })?;
    let surfaces = CaptureSurfaces::new(
        command_surface.paths,
        &command_surface.config.taste,
        &command_surface.config.steering,
        message_publisher,
    );
    if let Some(taste_judge) = taste_judge {
        message_surface::accept_capture_record_deferred(
            bus,
            epoch,
            seats,
            &surfaces,
            taste_judge,
            &record,
            message_surface::CaptureViews::new(message_state, diagnostic),
        )
    } else {
        accept_capture_record(
            bus,
            epoch,
            seats,
            &surfaces,
            None,
            &record,
            message_surface::CaptureViews::new(message_state, diagnostic),
        )
    }
}

fn accept_failure(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    lifecycle: &mut lifecycle::Runtime,
    failure: Option<lane::capture::CaptureFailure>,
) -> CommandResult<()> {
    let failure = failure.ok_or_else(|| {
        environment_fault(
            "lane::capture-failures-closed",
            "lane capture failure receiver closed",
        )
    })?;
    if !lane_surface::renewal_requested_exit(bus, epoch, failure.lane, failure.occupant)? {
        command_surface.observe_lifecycle_death(
            lifecycle,
            seats,
            failure.lane,
            failure.occupant,
        )?;
    }
    lane_surface::capture_failed(bus, epoch, seats, &failure)
}

fn accept_exit(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    lifecycle: &mut lifecycle::Runtime,
    exit: Option<lane::capture::CaptureExit>,
) -> CommandResult<()> {
    let exit = exit.ok_or_else(|| {
        environment_fault(
            "lane::capture-exits-closed",
            "lane process exit receiver closed",
        )
    })?;
    if !lane_surface::renewal_requested_exit(bus, epoch, exit.lane, exit.occupant)? {
        command_surface.observe_lifecycle_death(lifecycle, seats, exit.lane, exit.occupant)?;
    }
    lane_surface::occupant_exited(bus, epoch, seats, exit)
}

fn accept_commit(
    bus: &Bus,
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    diagnostic: &mut dyn io::Write,
    timers: &mut timers::Runtime,
    lifecycle: &mut lifecycle::Runtime,
    committed: Result<JournalCommit, tokio::sync::broadcast::error::RecvError>,
) -> CommandResult<()> {
    match committed {
        Ok(commit) => {
            command_surface.refresh_reads()?;
            if timers.covers(commit) {
                return Ok(());
            }
            reconcile_wakes(bus, command_surface, seats, diagnostic, timers, lifecycle)
        }
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
            command_surface.refresh_reads()?;
            reconcile_wakes(bus, command_surface, seats, diagnostic, timers, lifecycle)
        }
        Err(tokio::sync::broadcast::error::RecvError::Closed) => Err(environment_fault(
            "bus::lifecycle-commits-closed",
            "the durable commit stream closed while the resident was live",
        )),
    }
}

fn reconcile_wakes(
    bus: &Bus,
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    diagnostic: &mut dyn io::Write,
    timers: &mut timers::Runtime,
    lifecycle: &mut lifecycle::Runtime,
) -> CommandResult<()> {
    timers.synchronize(bus)?;
    let wake = command_surface.reconcile_lifecycle(lifecycle, seats, diagnostic)?;
    timers.synchronize_wake(wake)
}
pub(in crate::resident) struct ControlDispatch {
    pub(in crate::resident) reached_dark: bool,
    pub(in crate::resident) lifecycle_released: bool,
    inbox_projection: Option<CanonicalUuid>,
}

pub(in crate::resident) async fn dispatch_control(
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    lifecycle: &mut lifecycle::Runtime,
    control: &mut canvas::axum::Control,
    request: Option<canvas::axum::ControlRequest>,
) -> CommandResult<ControlDispatch> {
    let request = request.ok_or_else(|| {
        environment_fault("canvas::control-closed", "canvas control receiver closed")
    })?;
    let replayed = command_surface.replay(seats, request.command());
    let is_replay = replayed.is_some();
    let lifecycle_released = if is_replay {
        false
    } else {
        command_surface.prepare_explicit_lifecycle(lifecycle, request.command())?
    };
    let dark = is_dark_command(request.command());
    let dispatched = replayed.map_or_else(
        || command_surface.dispatch_fresh(seats, request.command()),
        CommandDispatch::ordinary,
    );
    let (outcome, effects) = dispatched.into_parts();
    let reached_dark = !is_replay && dark && matches!(outcome, Outcome::Reply(_));
    complete_response(request, outcome).await;
    if reached_dark {
        control.close();
    }
    Ok(ControlDispatch {
        reached_dark,
        lifecycle_released,
        inbox_projection: effects.inbox_projection,
    })
}

pub(in crate::resident) async fn dispatch_control_burst(
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    lifecycle: &mut lifecycle::Runtime,
    control: &mut canvas::axum::Control,
    request: Option<canvas::axum::ControlRequest>,
    inbox_projections: &mut InboxProjections,
) -> CommandResult<ControlDispatch> {
    let mut request = request;
    let mut burst = ControlDispatch {
        reached_dark: false,
        lifecycle_released: false,
        inbox_projection: None,
    };
    for turn in 0..canvas::axum::Control::FAIR_CYCLE_LIMIT {
        let dispatched =
            dispatch_control(command_surface, seats, lifecycle, control, request.take()).await?;
        if let Some(lane) = dispatched.inbox_projection {
            inbox_projections.insert(lane);
        }
        burst.reached_dark |= dispatched.reached_dark;
        burst.lifecycle_released |= dispatched.lifecycle_released;
        if burst.reached_dark
            || burst.lifecycle_released
            || turn + 1 == canvas::axum::Control::FAIR_CYCLE_LIMIT
        {
            break;
        }
        request = match control.try_recv() {
            Ok(request) => Some(request),
            Err(
                tokio::sync::mpsc::error::TryRecvError::Empty
                | tokio::sync::mpsc::error::TryRecvError::Disconnected,
            ) => break,
        };
    }
    Ok(burst)
}

async fn complete_response(request: canvas::axum::ControlRequest, outcome: Outcome<CommandAnswer>) {
    if let Ok(response) = request.respond(outcome) {
        response.wait().await;
    }
}

pub(in crate::resident) fn project_next_inbox(
    command_surface: &command_surface::Surface<'_>,
    inbox_projections: &mut InboxProjections,
) -> CommandResult<()> {
    let lane = inbox_projections
        .pop()
        .expect("a projection turn requires one retained lane");
    if matches!(
        command_surface.project_inbox_once(lane)?,
        steering::FoldAttempt::Moved
    ) {
        inbox_projections.insert(lane);
    }
    Ok(())
}

fn is_dark_command(command: &wire::AdmittedCommand) -> bool {
    matches!(
        command.body(),
        Intent::UpChange(wire::intent::UpChange {
            state: ResidentState::Dark
        })
    )
}
