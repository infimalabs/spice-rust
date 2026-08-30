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

pub(super) struct SurfacePublication<'a> {
    discovery: canvas::Discovery,
    published: PublishedDiscovery,
    app_description: String,
    token: Option<String>,
    open: bool,
    endpoint: &'a mut dyn io::Write,
    daemon: Option<ChildStatus>,
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
    let app = canvas::AppSource::resolve(&paths.worktree)
        .map_err(|error| environment_fault("canvas::app-source-invalid", &error.to_string()))?;
    let app_description = app.to_string();
    let SurfaceLaunch {
        binding,
        open,
        progress,
        endpoint,
        daemon,
    } = launch;
    let CanvasBinding { listen, access } = binding;
    let CanvasAccess { policy, token } = access;
    let (server, control) = canvas::axum::Server::bind_streaming(
        canvas::axum::Binding::new(listen, policy),
        epoch,
        capabilities.clone(),
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
    let timers = timers::Runtime::start(config.projects.clone(), config.steering.clone())?;
    let lifecycle = lifecycle::Runtime::new(&config.lane);
    let mut taste_adjudication = if config.judge_enabled {
        Some(taste_surface::start_adjudication()?)
    } else {
        None
    };
    let started = start_surface(bus, paths, epoch, residency.body(), config, launch).await?;
    let StartedSurface {
        capabilities,
        discovery,
        control,
        publication,
        shutdown,
        server_task,
        message_publisher,
        progress,
    } = started;
    let capture::Channels {
        reporter,
        records,
        failures,
        exits,
    } = capture::channels();
    let supervisor_reports = capture::SupervisorReports::bind(paths)?;
    lane_surface::restore_seats(bus, paths, epoch, seats, &reporter)?;
    ingest_transcripts(bus, paths, None, progress)?;
    let residency = current_residency(bus, epoch)?;
    let message_snapshot = bus
        .fact_snapshot()
        .map_err(|error| store_fault("canvas::message-time-unavailable", &error.to_string()))?;
    let mut message_reader = message_surface::BacklogReader::from_snapshot(&message_snapshot)?;
    message_surface::restore_agent_messages(
        paths,
        residency.body(),
        &message_publisher,
        &mut message_reader,
        progress,
    )?;
    let reads = read_materializations::CurrentReads::start(bus, paths, &config.consent)
        .map_err(|error| store_fault("resident::read-materialization-failed", &error))?;
    let command_surface = command_surface::Surface::new(
        bus,
        repository,
        paths,
        epoch,
        command_surface::Runtimes::new(
            lane_surface::Runtime::new(&capabilities, &discovery, reporter.clone()),
            lane_recovery::Runtime::new(reporter),
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
        message_reader.into_state(),
        taste_submitter,
        taste_completions,
        records,
        failures,
        exits,
        supervisor_reports,
        progress,
        timers,
        timer_commits,
        lifecycle,
    )
    .await
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
    mut publication: SurfacePublication<'_>,
    shutdown: oneshot::Sender<()>,
    mut server_task: tokio::task::JoinHandle<Result<(), io::Error>>,
    message_publisher: canvas::stream::MessagePublisher,
    mut message_state: message_surface::MessageState,
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
) -> CommandResult<()> {
    reconcile_wakes(
        bus,
        command_surface,
        seats,
        diagnostic,
        &mut timers,
        &mut lifecycle,
    )?;
    command_surface.synchronize_reads_before_publish()?;
    let residency = current_residency(bus, epoch)?;
    // Discovery is a serving contract, not just a bound-socket record. This
    // resident runs on a current-thread runtime, so give the spawned server task
    // one turn before clients can observe the discovery file.
    tokio::task::yield_now().await;
    publication.publish(bus, command_surface.paths, residency.body(), diagnostic)?;
    loop {
        tokio::select! {
            request = control.recv() => {
                if dispatch_control(
                    command_surface, seats, &mut lifecycle, &mut control, request,
                ).await? {
                    let refresh = timers.synchronize(bus)?;
                    refresh_warning_inboxes(bus, command_surface.paths, refresh)?;
                    return stop_surface(publication, shutdown, server_task).await;
                }
            }
            record = records.recv() => accept_record(
                bus, epoch, seats, command_surface, &message_publisher,
                taste_submitter.as_deref_mut(), record, &mut message_state, diagnostic,
            )?,
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
        command_surface.observe_lifecycle_death(lifecycle, seats, lane, occupant)?;
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
        timers::Fired::ClaimWarning(delivery) => {
            let lane = delivery.lane();
            let residency = bus
                .read(move |read| delivery.current_residency(read))
                .map_err(|error| store_fault("steer::residency-unreadable", &error.to_string()))?;
            if let Some(residency) = residency {
                steering::publish_inbox(bus, command_surface.paths, residency.body(), lane)?;
            }
            Ok(())
        }
        timers::Fired::Stale
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
    command_surface.observe_lifecycle_death(lifecycle, seats, failure.lane, failure.occupant)?;
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
    command_surface.observe_lifecycle_death(lifecycle, seats, exit.lane, exit.occupant)?;
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
        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
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
    let refresh = timers.synchronize(bus)?;
    refresh_warning_inboxes(bus, command_surface.paths, refresh)?;
    let wake = command_surface.reconcile_lifecycle(lifecycle, seats, diagnostic)?;
    timers.synchronize_wake(wake)
}

pub(in crate::resident) fn refresh_warning_inboxes(
    bus: &Bus,
    paths: &RuntimePaths,
    refresh: impl IntoIterator<Item = CanonicalUuid>,
) -> CommandResult<()> {
    let residency = bus
        .residency_head()
        .map_err(|error| store_fault("steer::residency-unreadable", &error.to_string()))?;
    let Some(residency) = residency else {
        return Ok(());
    };
    for lane in refresh {
        if residency.body().lanes.iter().any(|row| row.lane == lane) {
            steering::publish_inbox(bus, paths, residency.body(), lane)?;
        }
    }
    Ok(())
}
pub(in crate::resident) async fn dispatch_control(
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    lifecycle: &mut lifecycle::Runtime,
    control: &mut canvas::axum::Control,
    request: Option<canvas::axum::ControlRequest>,
) -> CommandResult<bool> {
    let request = request.ok_or_else(|| {
        environment_fault("canvas::control-closed", "canvas control receiver closed")
    })?;
    command_surface.prepare_explicit_lifecycle(lifecycle, request.command())?;
    let dark = is_dark_command(request.command());
    let outcome = command_surface.dispatch(seats, request.command());
    let reached_dark = dark && matches!(outcome, Outcome::Reply(_));
    let response = request.respond(outcome);
    if let Ok(response) = response {
        response.wait().await;
    }
    if reached_dark {
        control.close();
    }
    Ok(reached_dark)
}

fn is_dark_command(command: &wire::AdmittedCommand) -> bool {
    matches!(
        command.body(),
        Intent::UpChange(wire::intent::UpChange {
            state: ResidentState::Dark
        })
    )
}
