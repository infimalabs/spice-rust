async fn run_surface(
    bus: &Bus,
    repository: &Repository,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    config: &SurfaceConfig,
    seats: &mut Seats,
    launch: SurfaceLaunch<'_>,
) -> CommandResult<()> {
    // This resident runs on one Tokio thread. Restore every durable occupant
    // and ingest its existing transcript before publishing discovery: doing
    // synchronous recovery after `tokio::spawn` would leave a discoverable
    // socket whose server task cannot be polled until recovery returns.
    let capture::Channels {
        reporter,
        records,
        failures,
        exits,
    } = capture::channels();
    lane_surface::restore_seats(bus, paths, epoch, seats, &reporter)?;
    ingest_transcripts(bus, paths, None, launch.progress)?;

    let residency = current_residency(bus, epoch)?;
    let timer_commits = bus.reader().subscribe();
    let timers = timers::Runtime::start(config.projects.clone(), config.steering.clone())?;
    let lifecycle = lifecycle::Runtime::new(&config.lane);
    let mut taste_judge = taste_surface::ProcessAdjudicator::default();
    let started = start_surface(bus, paths, epoch, residency.body(), config, launch).await?;
    let StartedSurface {
        capabilities,
        discovery,
        control,
        published,
        shutdown,
        server_task,
        message_publisher,
        progress,
    } = started;
    let command_surface = command_surface::Surface::new(
        bus,
        repository,
        paths,
        epoch,
        lane_surface::Runtime::new(&capabilities, &discovery, reporter.clone()),
        lane_recovery::Runtime::new(reporter),
        config,
    );
    drive_surface(
        bus,
        epoch,
        seats,
        &command_surface,
        control,
        published,
        shutdown,
        server_task,
        message_publisher,
        &mut taste_judge,
        records,
        failures,
        exits,
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
pub(super) fn announce_canvas(
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
async fn drive_surface(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    mut control: canvas::axum::Control,
    published: PublishedDiscovery,
    shutdown: oneshot::Sender<()>,
    mut server_task: tokio::task::JoinHandle<Result<(), io::Error>>,
    message_publisher: canvas::stream::MessagePublisher,
    taste_judge: &mut dyn taste_surface::Adjudicator,
    mut records: tokio::sync::mpsc::Receiver<lane::capture::CaptureRecord>,
    mut failures: tokio::sync::mpsc::UnboundedReceiver<lane::capture::CaptureFailure>,
    mut exits: tokio::sync::mpsc::UnboundedReceiver<lane::capture::CaptureExit>,
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
    let mut tool_previews = CallPreviews::default();
    loop {
        tokio::select! {
            request = control.recv() => {
                if dispatch_control(
                    command_surface, seats, &mut lifecycle, &mut control, request,
                ).await? {
                    return stop_surface(published, shutdown, server_task).await;
                }
            }
            record = records.recv() => accept_record(
                bus, epoch, seats, command_surface, &message_publisher,
                taste_judge, record, &mut tool_previews, diagnostic,
            )?,
            failure = failures.recv() => accept_failure(
                bus, epoch, seats, command_surface, &mut lifecycle, failure,
            )?,
            exit = exits.recv() => accept_exit(
                bus, epoch, seats, command_surface, &mut lifecycle, exit,
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
        timers::Fired::Stale | timers::Fired::ClaimLapse | timers::Fired::TaskWake(_) => Ok(()),
    }
}

async fn stop_surface(
    published: PublishedDiscovery,
    shutdown: oneshot::Sender<()>,
    server_task: tokio::task::JoinHandle<Result<(), io::Error>>,
) -> CommandResult<()> {
    drop(published);
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
    taste_judge: &mut dyn taste_surface::Adjudicator,
    record: Option<lane::capture::CaptureRecord>,
    tool_previews: &mut CallPreviews,
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
    let judge = command_surface
        .config
        .judge_enabled
        .then_some(taste_judge as &mut dyn taste_surface::Adjudicator);
    accept_capture_record(
        bus,
        epoch,
        seats,
        &surfaces,
        judge,
        &record,
        message_surface::CaptureViews::new(tool_previews, diagnostic),
    )
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
async fn dispatch_control(
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
    if reached_dark {
        if let Ok(response) = response {
            response.wait().await;
        }
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
