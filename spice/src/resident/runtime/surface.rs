//! Canvas publication and the resident's live surface loop.
mod capabilities;
mod input;

use std::sync::Arc;

use super::*;

struct StartedSurface<'a> {
    capabilities: canvas::axum::Capabilities,
    discovery: canvas::Discovery,
    control: canvas::axum::Control,
    publication: SurfacePublication<'a>,
    shutdown: oneshot::Sender<()>,
    server_task: tokio::task::JoinHandle<Result<(), io::Error>>,
    message_publisher: canvas::stream::MessagePublisher,
    progress: &'a mut dyn io::Write,
}

struct SurfacePublication<'a> {
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
        paths: &RuntimePaths,
        residency: &ResidencyHead,
        progress: &mut dyn io::Write,
    ) -> CommandResult<()> {
        steering::publish_lane_discoveries(paths, &self.discovery, residency)?;
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

async fn start_surface<'a>(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    residency: &ResidencyHead,
    config: &SurfaceConfig,
    operator_gravatar: Option<wire::GravatarHash>,
    launch: SurfaceLaunch<'a>,
) -> CommandResult<StartedSurface<'a>> {
    let operator = new_uuid();
    let capabilities = capabilities::build(operator, residency)?;
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
    let speech_paths = paths.clone();
    let generation = canvas::axum::Generation::new(
        epoch,
        capabilities.clone(),
        version,
        app,
        config.speech.clone(),
    )
    .with_speech_resolver(move |worktree| speech_paths.speech_for_worktree(worktree))
    .with_operator_gravatar(operator_gravatar);
    let quote_presenter =
        spice::quote::presenter(paths.tape.clone(), bus.reader(), quote_agent_prose);
    let (server, control) = canvas::axum::Server::bind_streaming(
        canvas::axum::Binding::new(listen, policy),
        generation,
        bus.reader(),
        config.canvas.rows(),
        quote_presenter,
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

/// The lane and recovery runtimes commands run against, sharing one reporter.
fn runtimes<'a>(
    capabilities: &'a canvas::axum::Capabilities,
    discovery: &'a canvas::Discovery,
    reporter: &std::sync::Arc<dyn lane::capture::CaptureReporter>,
    reads: &'a read_materializations::CurrentReads,
) -> command_surface::Runtimes<'a> {
    command_surface::Runtimes::new(
        lane_surface::Runtime::new(capabilities, discovery, reporter.clone(), reads),
        lane_recovery::Runtime::new(reporter.clone()),
    )
}

/// Gravatar hash of the repository's configured `user.email`, absent when unset or blank.
fn operator_gravatar(repository: &Repository) -> CommandResult<Option<wire::GravatarHash>> {
    use sha2::{Digest as _, Sha256};

    let git = gix::open(repository.worktree_root()).map_err(|error| {
        environment_fault("canvas::operator-identity-unavailable", &error.to_string())
    })?;
    let Some(email) = git.config_snapshot().string("user.email") else {
        return Ok(None);
    };
    let email = email.trim_ascii().to_ascii_lowercase();
    if email.is_empty() {
        return Ok(None);
    }
    let hash = format!("{:x}", Sha256::digest(&email))
        .parse()
        .expect("a SHA-256 lowercase hex spelling is a Gravatar hash");
    Ok(Some(hash))
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
    let mut quota = quota::Runtime::start(config.omp_binary.clone());
    let quota_triggers = quota::Triggers::start(bus)?;
    quota.request();
    let mut lifecycle = lifecycle::Runtime::new(&config.lane);
    let mut taste_adjudication = if config.judge_enabled {
        Some(taste_surface::start_adjudication()?)
    } else {
        None
    };
    let emblem = operator_gravatar(repository)?;
    let StartedSurface {
        capabilities,
        discovery,
        mut control,
        mut publication,
        shutdown,
        mut server_task,
        message_publisher,
        progress,
    } = start_surface(bus, paths, epoch, residency.body(), config, emblem, launch).await?;
    let channels = capture::channels();
    let mut supervisor_reports = capture::SupervisorReports::bind(paths)?;
    lane_surface::restore_seats(bus, paths, epoch, seats, progress)?;
    recover_transcripts(bus, paths, epoch, progress)?;
    let mut message_startup = launch_message_startup(bus, paths, epoch, &message_publisher)?;
    let reads = read_materializations::CurrentReads::start(bus, paths, &config.consent)
        .map_err(|error| store_fault("resident::read-materialization-failed", &error))?;
    let command_surface = command_surface::Surface::new(
        bus,
        repository,
        paths,
        epoch,
        runtimes(&capabilities, &discovery, &channels.reporter, &reads),
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
        &mut supervisor_reports,
        &mut publication,
    )
    .await?;
    let mut inbox_deliveries = InboxDeliveries::new();
    let mut lifecycle_reconciliation = lifecycle::Reconciliation::default();
    let Some(message_startup) = await_message_startup(
        &mut message_startup,
        bus,
        &command_surface,
        seats,
        &mut lifecycle,
        &mut timers,
        &mut supervisor_reports,
        &mut control,
        &mut server_task,
        &mut inbox_deliveries,
        &mut lifecycle_reconciliation,
        progress,
    )
    .await?
    else {
        return stop_surface(publication, shutdown, server_task).await;
    };
    let _ = progress.write_all(&message_startup.diagnostic);
    let message_state = Arc::new(std::sync::Mutex::new(message_startup.reader.into_state()));
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
        quota,
        quota_triggers,
        timer_commits,
        lifecycle,
        inbox_deliveries,
        lifecycle_reconciliation,
    )
    .await
}

#[allow(
    clippy::too_many_arguments,
    reason = "readiness joins the resident's command, timer, lifecycle, and publication owners"
)]
async fn publish_ready_surface(
    bus: &Bus,
    epoch: CanonicalUuid,
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    diagnostic: &mut dyn io::Write,
    timers: &mut timers::Runtime,
    lifecycle: &mut lifecycle::Runtime,
    door: &mut capture::SupervisorReports,
    publication: &mut SurfacePublication<'_>,
) -> CommandResult<()> {
    reconcile_wakes(
        bus,
        command_surface,
        seats,
        diagnostic,
        timers,
        lifecycle,
        door,
    )?;
    command_surface.synchronize_reads_before_publish()?;
    let residency = current_residency(bus, epoch)?;
    tokio::task::yield_now().await;
    publication.publish(command_surface.paths, residency.body(), diagnostic)
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
    door: &mut capture::SupervisorReports,
    control: &mut canvas::axum::Control,
    server_task: &mut tokio::task::JoinHandle<Result<(), io::Error>>,
    inbox_deliveries: &mut InboxDeliveries,
    lifecycle_reconciliation: &mut lifecycle::Reconciliation,
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
            (turn, request) = control_delivery_turn(
                control, inbox_deliveries, lifecycle_reconciliation.control_enabled(),
            ) => match turn {
                ControlDeliveryTurn::Control => {
                    reconcile_for_control(
                        lifecycle_reconciliation, bus, command_surface, seats, diagnostic, timers,
                        lifecycle, door,
                    )?;
                    if dispatch_control_delivery_burst(
                        command_surface, seats, lifecycle, control, request, inbox_deliveries,
                        lifecycle_reconciliation,
                    ).await? {
                        return Ok(None);
                    }
                }
                ControlDeliveryTurn::Delivery => {
                    deliver_owed_inbox(command_surface, inbox_deliveries).await?;
                }
            },
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
    publication: SurfacePublication<'_>,
    shutdown: oneshot::Sender<()>,
    mut server_task: tokio::task::JoinHandle<Result<(), io::Error>>,
    message_publisher: canvas::stream::MessagePublisher,
    message_state: Arc<std::sync::Mutex<message_surface::MessageState>>,
    mut session_compactions: message_surface::SessionCompactions,
    mut taste_submitter: Option<&mut taste_surface::AdjudicationSubmitter>,
    mut taste_completions: Option<&mut taste_surface::AdjudicationCompletions>,
    mut records: tokio::sync::mpsc::Receiver<lane::capture::CaptureRecord>,
    mut failures: tokio::sync::mpsc::UnboundedReceiver<lane::capture::CaptureFailure>,
    mut exits: tokio::sync::mpsc::UnboundedReceiver<lane::capture::CaptureExit>,
    mut supervisor_reports: capture::SupervisorReports,
    diagnostic: &mut dyn io::Write,
    mut timers: timers::Runtime,
    mut quota: quota::Runtime,
    mut quota_triggers: quota::Triggers,
    mut timer_commits: bus::BroadcastSubscriber,
    mut lifecycle: lifecycle::Runtime,
    mut inbox_deliveries: InboxDeliveries,
    mut lifecycle_reconciliation: lifecycle::Reconciliation,
) -> CommandResult<()> {
    let mut input_lead = input::Lead::Supervisor;
    loop {
        let narration_deadline = message_surface::lock_state(message_state.as_ref())?
            .narration
            .next_deadline();
        tokio::select! {
            // Every reconciliation sweep asks the supervisor door itself, so
            // selecting another ready source first cannot lose an ending.
            // Rotate the three inputs that can remain ready continuously.
            // Record turns own both durable accounting and Canvas publication.
            input = input::next(
                &mut input_lead,
                supervisor_reports.recv(),
                control_delivery_turn(
                    &mut control,
                    &inbox_deliveries,
                    lifecycle_reconciliation.control_enabled(),
                ),
                records.recv(),
            ) => match input {
                input::Input::Supervisor(report) => accept_supervisor_reports(
                    bus, epoch, seats, command_surface, &message_publisher,
                    taste_submitter.as_deref_mut(), &mut lifecycle, &mut session_compactions,
                    supervisor_reports.batch(report), message_state.as_ref(), diagnostic,
                )?,
                input::Input::Control((turn, request)) => match turn {
                    ControlDeliveryTurn::Control => {
                        reconcile_for_control(
                            &mut lifecycle_reconciliation, bus, command_surface, seats, diagnostic,
                            &mut timers, &mut lifecycle, &mut supervisor_reports,
                        )?;
                        if dispatch_control_delivery_burst(
                            command_surface, seats, &mut lifecycle, &mut control, request,
                            &mut inbox_deliveries, &mut lifecycle_reconciliation,
                        ).await? {
                            timers.synchronize(bus)?;
                            return stop_surface(publication, shutdown, server_task).await;
                        }
                    }
                    ControlDeliveryTurn::Delivery => {
                        deliver_owed_inbox(command_surface, &mut inbox_deliveries).await?;
                    }
                },
                input::Input::Record(record) => accept_record_turn(
                    bus, epoch, seats, command_surface, &message_publisher,
                    taste_submitter.as_deref_mut(), &mut session_compactions, record,
                    message_state.as_ref(), diagnostic,
                )?,
            },
            () = narration::wait(narration_deadline) => {
                let steps = message_surface::lock_state(message_state.as_ref())?
                    .narration
                    .due(std::time::Instant::now(), |lane, occupant| {
                        seats.activity(lane).is_some_and(|activity| activity.occupant() == occupant)
                    });
                narration::record(bus, command_surface.paths, epoch, steps, diagnostic)?;
            },
            completed = receive_judgment(&mut taste_completions) => accept_judgment(
                bus, epoch, command_surface, taste_submitter.as_deref_mut(), completed?,
            )?,
            failure = failures.recv() => {
                accept_failure(
                    bus, epoch, seats, command_surface, &mut lifecycle, diagnostic, failure,
                )?;
            }
            exit = exits.recv() => accept_exit(
                bus, epoch, seats, command_surface, &mut lifecycle, diagnostic, exit,
            )?,
            timer = timers.recv() => accept_timer_turn(
                bus, seats, command_surface, &mut lifecycle, &mut supervisor_reports,
                diagnostic, &mut timers, &mut lifecycle_reconciliation, timer,
            )?,
            () = quota_triggers.wait_periodic() => request_quota(&mut quota, &mut quota_triggers),
            completed = quota.recv() => quota.accept(completed),
            committed = timer_commits.recv() => accept_commit_turn(
                bus, seats, command_surface, &message_publisher, &mut lifecycle,
                &mut supervisor_reports, message_state.as_ref(), diagnostic, &mut timers,
                &mut quota, &mut quota_triggers, &mut lifecycle_reconciliation, committed,
            )?,
            ending = &mut server_task => return Err(server_outcome(ending)),
        }
        reap::present_closed_transcripts(
            bus,
            epoch,
            seats,
            command_surface,
            &message_publisher,
            taste_submitter.as_deref_mut(),
            message_state.as_ref(),
            diagnostic,
        )?;
    }
}

pub(in crate::resident) enum ControlDeliveryTurn {
    Control,
    Delivery,
}

pub(in crate::resident) async fn control_delivery_turn(
    control: &mut canvas::axum::Control,
    inbox_deliveries: &InboxDeliveries,
    control_enabled: bool,
) -> (ControlDeliveryTurn, Option<canvas::axum::ControlRequest>) {
    if inbox_deliveries.is_empty() {
        return if control_enabled {
            (ControlDeliveryTurn::Control, control.recv().await)
        } else {
            std::future::pending().await
        };
    }
    if !control_enabled {
        return (ControlDeliveryTurn::Delivery, None);
    }
    tokio::select! {
        biased;
        request = control.recv() => (ControlDeliveryTurn::Control, request),
        () = std::future::ready(()) => (ControlDeliveryTurn::Delivery, None),
    }
}

async fn dispatch_control_delivery_burst(
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    lifecycle: &mut lifecycle::Runtime,
    control: &mut canvas::axum::Control,
    request: Option<canvas::axum::ControlRequest>,
    inbox_deliveries: &mut InboxDeliveries,
    lifecycle_reconciliation: &mut lifecycle::Reconciliation,
) -> CommandResult<bool> {
    let dispatched = dispatch_control_burst(
        command_surface,
        seats,
        lifecycle,
        control,
        request,
        inbox_deliveries,
    )
    .await?;
    request_lifecycle_reconciliation(lifecycle_reconciliation, &dispatched);
    if !dispatched.reached_dark {
        deliver_owed_inbox(command_surface, inbox_deliveries).await?;
    }
    Ok(dispatched.reached_dark)
}

/// Pay one scheduled delivery turn, then let readiness produced during that
/// synchronous notification reach the current-thread executor before choosing again.
pub(in crate::resident) async fn deliver_owed_inbox(
    command_surface: &command_surface::Surface<'_>,
    inbox_deliveries: &mut InboxDeliveries,
) -> CommandResult<()> {
    deliver_owed_inbox_with(inbox_deliveries, |deliveries| {
        deliver_next_inbox(command_surface, deliveries)
    })
    .await
}

async fn deliver_owed_inbox_with(
    inbox_deliveries: &mut InboxDeliveries,
    deliver: impl FnOnce(&mut InboxDeliveries) -> CommandResult<()>,
) -> CommandResult<()> {
    if !inbox_deliveries.is_empty() {
        deliver(inbox_deliveries)?;
        tokio::task::yield_now().await;
    }
    Ok(())
}

/// Accept one drained batch of supervisor notices in arrival order.
#[allow(
    clippy::too_many_arguments,
    reason = "a batch of supervisor notices crosses the same live capture surfaces as one"
)]
fn accept_supervisor_reports(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    message_publisher: &canvas::stream::MessagePublisher,
    mut taste_judge: Option<&mut taste_surface::AdjudicationSubmitter>,
    lifecycle: &mut lifecycle::Runtime,
    session_compactions: &mut message_surface::SessionCompactions,
    reports: Vec<io::Result<super::supervisor::Notice>>,
    message_state: &std::sync::Mutex<message_surface::MessageState>,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let batch = begin_message_batch(message_publisher)?;
    for report in reports {
        if let Ok(notice) = &report
            && let Some((lane, occupant)) = message_notice_identity(notice)
        {
            let residency = current_residency(bus, epoch)?;
            if residency
                .body()
                .lanes
                .iter()
                .any(|row| row.lane == lane && row.occupant == Some(occupant))
            {
                session_compactions.synchronize(
                    bus,
                    epoch,
                    (residency.body(), lane),
                    &batch,
                    message_state,
                    diagnostic,
                )?;
            }
        }
        accept_supervisor_report(
            bus,
            epoch,
            seats,
            command_surface,
            &batch,
            taste_judge.as_deref_mut(),
            lifecycle,
            report,
            message_state,
            diagnostic,
        )?;
    }
    commit_message_batch(batch)
}

/// Fold one captured record into its session's compaction state, then present it.
#[allow(
    clippy::too_many_arguments,
    reason = "one record crosses the same live capture surfaces as a batch of notices"
)]
fn accept_record_turn(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    message_publisher: &canvas::stream::MessagePublisher,
    taste_judge: Option<&mut taste_surface::AdjudicationSubmitter>,
    session_compactions: &mut message_surface::SessionCompactions,
    record: Option<lane::capture::CaptureRecord>,
    message_state: &std::sync::Mutex<message_surface::MessageState>,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let batch = begin_message_batch(message_publisher)?;
    if let Some(lane) = record.as_ref().map(|record| record.lane) {
        let residency = current_residency(bus, epoch)?;
        session_compactions.synchronize(
            bus,
            epoch,
            (residency.body(), lane),
            &batch,
            message_state,
            diagnostic,
        )?;
    }
    accept_record(
        bus,
        epoch,
        seats,
        command_surface,
        &batch,
        taste_judge,
        record,
        message_state,
        diagnostic,
    )?;
    commit_message_batch(batch)
}

fn message_notice_identity(
    notice: &super::supervisor::Notice,
) -> Option<(CanonicalUuid, CanonicalUuid)> {
    use super::supervisor::Notice;
    match notice {
        Notice::Record { lane, occupant, .. }
        | Notice::Exited { lane, occupant }
        | Notice::Failed { lane, occupant, .. } => Some((*lane, *occupant)),
        Notice::Asked { .. } => None,
    }
}

/// Consume one fired timer, then reconcile the lifecycle it may have moved.
#[allow(
    clippy::too_many_arguments,
    reason = "a timer turn crosses the timers, the lifecycle, and the door its sweep asks"
)]
fn accept_timer_turn(
    bus: &Bus,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    lifecycle: &mut lifecycle::Runtime,
    door: &mut capture::SupervisorReports,
    diagnostic: &mut dyn io::Write,
    timers: &mut timers::Runtime,
    reconciliation: &mut lifecycle::Reconciliation,
    timer: Option<bus::timer::TimerEvent>,
) -> CommandResult<()> {
    let timer = timer.ok_or_else(|| {
        environment_fault(
            "bus::timer-events-closed",
            "the production timer event stream closed while the resident was live",
        )
    })?;
    let fired = timers.consume(bus, timer)?;
    accept_timer_fired(bus, command_surface, lifecycle, fired)?;
    reconcile_lifecycle(
        reconciliation,
        bus,
        command_surface,
        seats,
        diagnostic,
        timers,
        lifecycle,
        door,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "one supervisor notice crosses the same live capture surfaces as an in-process record"
)]
fn accept_supervisor_report<P: message_surface::AgentMessagePublisher>(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    message_publisher: &P,
    taste_judge: Option<&mut taste_surface::AdjudicationSubmitter>,
    lifecycle: &mut lifecycle::Runtime,
    report: io::Result<super::supervisor::Notice>,
    message_state: &std::sync::Mutex<message_surface::MessageState>,
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
            diagnostic,
            Some(lane::capture::CaptureExit { lane, occupant }),
        );
    }
    if let super::supervisor::Notice::Failed {
        lane,
        occupant,
        failure,
    } = notice
    {
        return super::reap::account_failure(
            bus, epoch, seats, lifecycle, lane, occupant, &failure, diagnostic,
        );
    }
    let residency = current_residency(bus, epoch)?;
    let Some(record) = accept_ask(command_surface, residency.body(), notice, diagnostic)? else {
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

/// Decide a lane's consent request, or hand every other notice on as the
/// record it carries.
fn accept_ask(
    command_surface: &command_surface::Surface<'_>,
    residency: &wire::ResidencyHead,
    notice: super::supervisor::Notice,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<Option<lane::capture::CaptureRecord>> {
    let super::supervisor::Notice::Asked {
        lane,
        occupant,
        request,
        operation,
    } = notice
    else {
        return capture::materialize(command_surface.paths, residency, notice, diagnostic);
    };
    let ask = consent_surface::LaneAsk {
        lane,
        occupant,
        request,
        operation,
    };
    let asked = command_surface.ask_consent(residency, ask, diagnostic);
    super::deferral::defer_store_fault(asked, diagnostic, "lane consent").map(|()| None)
}

/// Settle one finished taste adjudication with its submitter and the journal.
fn accept_judgment(
    bus: &Bus,
    epoch: CanonicalUuid,
    command_surface: &command_surface::Surface<'_>,
    taste_submitter: Option<&mut taste_surface::AdjudicationSubmitter>,
    completed: taste_surface::CompletedJudgment,
) -> CommandResult<()> {
    taste_submitter
        .expect("a completion receiver always has its submitter")
        .finished(&completed);
    taste_surface::complete_judgment(
        bus,
        command_surface.paths,
        epoch,
        &command_surface.config.taste,
        &command_surface.config.steering,
        completed,
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
                steering::notify_inboxes(command_surface.paths, residency.body());
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
fn accept_record<P: message_surface::AgentMessagePublisher>(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    message_publisher: &P,
    taste_judge: Option<&mut taste_surface::AdjudicationSubmitter>,
    record: Option<lane::capture::CaptureRecord>,
    message_state: &std::sync::Mutex<message_surface::MessageState>,
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
    )
    .narration(command_surface.config.lane.narration());
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

fn begin_message_batch(
    publisher: &canvas::stream::MessagePublisher,
) -> CommandResult<canvas::stream::MessageBatch> {
    publisher
        .batch()
        .map_err(|error| store_fault("canvas::message-publish-failed", &error.to_string()))
}

fn commit_message_batch(batch: canvas::stream::MessageBatch) -> CommandResult<()> {
    batch
        .commit()
        .map(drop)
        .map_err(|error| store_fault("canvas::message-publish-failed", &error.to_string()))
}

fn accept_failure(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    lifecycle: &mut lifecycle::Runtime,
    diagnostic: &mut dyn io::Write,
    failure: Option<lane::capture::CaptureFailure>,
) -> CommandResult<()> {
    let failure = failure.ok_or_else(|| {
        environment_fault(
            "lane::capture-failures-closed",
            "lane capture failure receiver closed",
        )
    })?;
    let accounted = (|| {
        if !lane_surface::renewal_requested_exit(bus, epoch, failure.lane, failure.occupant)? {
            command_surface.observe_lifecycle_death(
                lifecycle,
                seats,
                failure.lane,
                failure.occupant,
                diagnostic,
            )?;
        }
        lane_surface::capture_failed(bus, epoch, seats, &failure)
    })();
    super::deferral::defer_store_fault(accounted, diagnostic, "capture failure accounting")
}

fn accept_exit(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    lifecycle: &mut lifecycle::Runtime,
    diagnostic: &mut dyn io::Write,
    exit: Option<lane::capture::CaptureExit>,
) -> CommandResult<()> {
    let exit = exit.ok_or_else(|| {
        environment_fault(
            "lane::capture-exits-closed",
            "lane process exit receiver closed",
        )
    })?;
    let accounted = super::reap::account_exit(
        bus,
        command_surface.paths,
        epoch,
        seats,
        lifecycle,
        exit,
        diagnostic,
    );
    super::deferral::defer_store_fault(accounted, diagnostic, "exit accounting")
}

fn request_quota(quota: &mut quota::Runtime, triggers: &mut quota::Triggers) {
    triggers.accept_periodic();
    quota.request();
}

/// Let one journal commit settle the reconciliation and the sweep it may run.
///
/// Every presented record commits a fact, so this arm fires throughout a
/// lane's ending; the sweep asks the door before it judges any seat.
#[allow(
    clippy::too_many_arguments,
    reason = "a commit turn crosses the message surfaces and the lifecycle it reconciles"
)]
fn accept_commit_turn(
    bus: &Bus,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    message_publisher: &canvas::stream::MessagePublisher,
    lifecycle: &mut lifecycle::Runtime,
    door: &mut capture::SupervisorReports,
    message_state: &std::sync::Mutex<message_surface::MessageState>,
    diagnostic: &mut dyn io::Write,
    timers: &mut timers::Runtime,
    quota: &mut quota::Runtime,
    quota_triggers: &mut quota::Triggers,
    reconciliation: &mut lifecycle::Reconciliation,
    committed: Result<JournalCommit, tokio::sync::broadcast::error::RecvError>,
) -> CommandResult<()> {
    match accept_commit(
        bus,
        command_surface,
        seats,
        diagnostic,
        timers,
        lifecycle,
        door,
        committed,
    )? {
        Some(commit) => reconciliation.settle_commit(commit),
        None => reconciliation.settle(),
    }
    if quota_triggers.accept_commits(bus)? {
        quota.request();
    }
    settle_spoken_acknowledgements(bus, message_publisher, message_state, diagnostic)
}

#[allow(
    clippy::too_many_arguments,
    reason = "one commit reconciles the timers, the lifecycle, and the door its sweep asks"
)]
fn accept_commit(
    bus: &Bus,
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    diagnostic: &mut dyn io::Write,
    timers: &mut timers::Runtime,
    lifecycle: &mut lifecycle::Runtime,
    door: &mut capture::SupervisorReports,
    committed: Result<JournalCommit, tokio::sync::broadcast::error::RecvError>,
) -> CommandResult<Option<JournalCommit>> {
    match committed {
        Ok(commit) => {
            command_surface.refresh_reads()?;
            if !timers.covers(commit) {
                super::deferral::defer_store_fault(
                    reconcile_wakes(
                        bus,
                        command_surface,
                        seats,
                        diagnostic,
                        timers,
                        lifecycle,
                        door,
                    ),
                    diagnostic,
                    "lifecycle reconciliation",
                )?;
            }
            Ok(Some(commit))
        }
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
            command_surface.refresh_reads()?;
            super::deferral::defer_store_fault(
                reconcile_wakes(
                    bus,
                    command_surface,
                    seats,
                    diagnostic,
                    timers,
                    lifecycle,
                    door,
                ),
                diagnostic,
                "lifecycle reconciliation",
            )?;
            Ok(None)
        }
        Err(tokio::sync::broadcast::error::RecvError::Closed) => Err(environment_fault(
            "bus::lifecycle-commits-closed",
            "the durable commit stream closed while the resident was live",
        )),
    }
}

/// Follow every commit with the cards that spoke an acknowledgement ahead of
/// the journal, so any committed retirement re-presents its card from the
/// journal disposition. Canvas being unavailable is a
/// diagnostic, as it is for live publication, not a resident fault.
fn settle_spoken_acknowledgements(
    bus: &Bus,
    message_publisher: &canvas::stream::MessagePublisher,
    message_state: &std::sync::Mutex<message_surface::MessageState>,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let settled = (|| {
        let batch = begin_message_batch(message_publisher)?;
        message_surface::settle_pending_presentations(bus, &batch, message_state)?;
        commit_message_batch(batch)
    })();
    message_surface::canvas_diagnostic(settled, diagnostic)
}

fn request_lifecycle_reconciliation(
    reconciliation: &mut lifecycle::Reconciliation,
    dispatched: &ControlDispatch,
) {
    if let Some(witness) = dispatched.lifecycle_witness {
        reconciliation.request(witness);
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one reconciliation crosses the timers, the lifecycle, and the door its sweep asks"
)]
fn reconcile_for_control(
    reconciliation: &mut lifecycle::Reconciliation,
    bus: &Bus,
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    diagnostic: &mut dyn io::Write,
    timers: &mut timers::Runtime,
    lifecycle: &mut lifecycle::Runtime,
    door: &mut capture::SupervisorReports,
) -> CommandResult<()> {
    if reconciliation.control_settles() {
        reconcile_lifecycle(
            reconciliation,
            bus,
            command_surface,
            seats,
            diagnostic,
            timers,
            lifecycle,
            door,
        )?;
    }
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "one reconciliation crosses the timers, the lifecycle, and the door its sweep asks"
)]
fn reconcile_lifecycle(
    reconciliation: &mut lifecycle::Reconciliation,
    bus: &Bus,
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    diagnostic: &mut dyn io::Write,
    timers: &mut timers::Runtime,
    lifecycle: &mut lifecycle::Runtime,
    door: &mut capture::SupervisorReports,
) -> CommandResult<()> {
    // A deferred sweep leaves the demand pending, so the next control turn
    // drives it again instead of a timer having to notice.
    let swept = reconcile_wakes(
        bus,
        command_surface,
        seats,
        diagnostic,
        timers,
        lifecycle,
        door,
    );
    if swept.is_ok() {
        reconciliation.settle();
    }
    super::deferral::defer_store_fault(swept, diagnostic, "lifecycle reconciliation")
}

fn reconcile_wakes(
    bus: &Bus,
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    diagnostic: &mut dyn io::Write,
    timers: &mut timers::Runtime,
    lifecycle: &mut lifecycle::Runtime,
    door: &mut capture::SupervisorReports,
) -> CommandResult<()> {
    timers.synchronize(bus)?;
    super::reap::reap_ended_seats(bus, command_surface, seats, lifecycle, diagnostic, door)?;
    let (paths, epoch) = (command_surface.paths, command_surface.epoch());
    lane_surface::reconcile_seats(bus, paths, epoch, seats, diagnostic)?;
    let wake = command_surface.reconcile_lifecycle(lifecycle, seats, diagnostic)?;
    timers.synchronize_wake(wake)
}
pub(in crate::resident) struct ControlDispatch {
    pub(in crate::resident) reached_dark: bool,
    lifecycle_witness: Option<lifecycle::ReconciliationWitness>,
    inbox_delivery: Option<CanonicalUuid>,
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
    let command = request.command().id();
    let epoch = request.command().epoch();
    let dispatched = replayed.map_or_else(
        || command_surface.dispatch_fresh(seats, request.command()),
        CommandDispatch::ordinary,
    );
    let (outcome, effects) = dispatched.into_parts();
    let lifecycle_witness = lifecycle_released.then(|| {
        let position = match &outcome {
            Outcome::Reply(reply) => Some(reply.position),
            Outcome::Refusal(_) | Outcome::Fault(_) => None,
        };
        lifecycle::ReconciliationWitness::new(command, epoch, position)
    });
    let reached_dark = !is_replay && dark && matches!(outcome, Outcome::Reply(_));
    complete_response(request, outcome).await;
    if reached_dark {
        control.close();
    }
    Ok(ControlDispatch {
        reached_dark,
        lifecycle_witness,
        inbox_delivery: effects.inbox_delivery,
    })
}

pub(in crate::resident) async fn dispatch_control_burst(
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    lifecycle: &mut lifecycle::Runtime,
    control: &mut canvas::axum::Control,
    request: Option<canvas::axum::ControlRequest>,
    inbox_deliveries: &mut InboxDeliveries,
) -> CommandResult<ControlDispatch> {
    let mut request = request;
    let mut burst = ControlDispatch {
        reached_dark: false,
        lifecycle_witness: None,
        inbox_delivery: None,
    };
    for turn in 0..canvas::axum::Control::FAIR_CYCLE_LIMIT {
        let dispatched =
            dispatch_control(command_surface, seats, lifecycle, control, request.take()).await?;
        if let Some(lane) = dispatched.inbox_delivery {
            inbox_deliveries.insert(lane);
        }
        burst.reached_dark |= dispatched.reached_dark;
        if dispatched.lifecycle_witness.is_some() {
            burst.lifecycle_witness = dispatched.lifecycle_witness;
        }
        if burst.reached_dark
            || burst.lifecycle_witness.is_some()
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

pub(in crate::resident) fn deliver_next_inbox(
    command_surface: &command_surface::Surface<'_>,
    inbox_deliveries: &mut InboxDeliveries,
) -> CommandResult<()> {
    let residency = command_surface.inbox_residency()?;
    let Some(delivery) = inbox_deliveries.next(&residency) else {
        return Ok(());
    };
    command_surface.notify_inbox(delivery.lane());
    inbox_deliveries.complete(delivery);
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
