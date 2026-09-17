//! Detached lane supervisors that may outlive any resident generation.

use std::fs::File;
#[cfg(not(test))]
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
#[cfg(not(test))]
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::Arc;
#[cfg(not(test))]
use std::thread;
#[cfg(not(test))]
use std::time::Instant as Monotonic;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lane::capture::CaptureReporter;
use lane::occupant::{DetachedProcess, DetachedWait};
use serde::{Deserialize, Serialize};
use sill::{WaitError, WaitOutcome};
use spice::repository::Repository;
use wire::CanonicalUuid;

pub(super) mod command;
pub(super) mod ending;
#[cfg(unix)]
mod entry;
mod events;
mod reporter;
mod terminal;
#[cfg(test)]
pub(super) mod test_support;

use super::lane_surface::{
    OccupantLaunchRequest, SessionProofError, await_session, occupant_launch,
};
use super::{CommandResult, RuntimePaths, environment_fault, store_fault};
#[cfg(unix)]
pub(crate) use entry::entry;
use events::StateEvents;
#[cfg(all(unix, not(test)))]
use events::StateSignal;
use reporter::DurableReporter;

#[cfg(all(unix, not(test)))]
use std::os::unix::fs::OpenOptionsExt as _;
#[cfg(all(unix, not(test)))]
use std::os::unix::process::CommandExt as _;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt as _;

const ENTRY: &str = "__lane-supervisor";
// Launch requests are consumed by this executable generation. Durable state
// is also consumed by replacement residents, so version its schema separately.
const REQUEST_VERSION: u32 = 3;
const STATE_VERSION: u32 = 2;
// The fast-mode release bumped the shared version without changing State.
const REQUEST_V3_STATE_VERSION: u32 = 3;
const REQUEST_BYTES: u64 = 64 * 1024;
const STATE_BYTES: u64 = 64 * 1024;
#[cfg(not(test))]
const START_HORIZON: Duration = Duration::from_secs(15);

/// Minimal best-effort notification projected from already-durable tape.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Notice {
    Record {
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        stream: String,
        boundary: String,
        locus: wire::TapeLocus,
    },
    Exited {
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
    },
    Failed {
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        failure: String,
    },
    /// The occupant opened a confirm dialog, stated here as one consent
    /// request; the answer returns through the lane's kick door.
    Asked {
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        request: CanonicalUuid,
        operation: String,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    worktree: String,
    vendor: String,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    resume: Option<String>,
    fast_mode: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    state: PathBuf,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    supervisor: u32,
    worktree: String,
    vendor: String,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    phase: Phase,
    group: Option<u32>,
    session: Option<wire::LaneSession>,
    git: Option<String>,
    started_millis: Option<u64>,
    runtime_millis: Option<u64>,
    grace_millis: Option<u64>,
    reap_millis: Option<u64>,
    completion: CompletionField,
    failure: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(transparent)]
struct CompletionField(Option<PersistedCompletion>);

impl CompletionField {
    const fn none() -> Self {
        Self(None)
    }

    const fn some(completion: PersistedCompletion) -> Self {
        Self(Some(completion))
    }

    const fn as_ref(&self) -> Option<&PersistedCompletion> {
        self.0.as_ref()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum PersistedCompletion {
    Exited { status_raw: i32 },
    TimedOut { status_raw: i32 },
    Cancelled { status_raw: i32 },
}

impl PersistedCompletion {
    fn capture(completion: &sill::Completion) -> Self {
        let status_raw = status_raw(completion.status());
        match completion {
            sill::Completion::Exited(_) => Self::Exited { status_raw },
            sill::Completion::TimedOut(_) => Self::TimedOut { status_raw },
            sill::Completion::Cancelled(_) => Self::Cancelled { status_raw },
        }
    }

    fn decode(&self) -> io::Result<sill::Completion> {
        let (status_raw, constructor): (i32, fn(ExitStatus) -> sill::Completion) = match self {
            Self::Exited { status_raw } => (*status_raw, sill::Completion::Exited),
            Self::TimedOut { status_raw } => (*status_raw, sill::Completion::TimedOut),
            Self::Cancelled { status_raw } => (*status_raw, sill::Completion::Cancelled),
        };
        status_from_raw(status_raw).map(constructor)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Starting,
    Running,
    Exited,
    Failed,
}

impl State {
    fn validate(self) -> io::Result<Self> {
        match (self.phase, self.completion.as_ref()) {
            (Phase::Exited, Some(_)) | (Phase::Starting | Phase::Running | Phase::Failed, None) => {
                Ok(self)
            }
            (Phase::Exited, None) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "exited supervisor state has no typed completion",
            )),
            (phase, Some(_)) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{phase:?} supervisor state carries a terminal completion"),
            )),
        }
    }
}

/// Session and process evidence returned after a detached launch is proven.
pub(super) struct Launch {
    pub(super) occupant: CanonicalUuid,
    pub(super) group: u32,
    pub(super) session: wire::LaneSession,
    pub(super) git: String,
    pub(super) process: Box<dyn DetachedProcess>,
}

/// Durable supervisor state found for an occupied lane during replacement.
pub(super) enum Adoption {
    /// The same supervisor and agent remain live.
    Running(Launch),
    /// The supervisor reaped the agent while no resident was listening.
    Exited(Launch),
    /// The supervisor could not preserve a safely adoptable agent.
    Failed(String),
}

/// Launch a new detached supervisor from this exact executable generation.
#[cfg(not(test))]
pub(super) use launch_detached as launch;

#[cfg(not(test))]
pub(super) fn launch_detached(
    paths: &RuntimePaths,
    request: OccupantLaunchRequest<'_>,
    _reporter: Arc<dyn CaptureReporter>,
) -> CommandResult<Launch> {
    #[cfg(not(unix))]
    {
        let _ = (paths, request);
        return Err(environment_fault(
            "lane::supervisor-unsupported",
            "detached lane supervision requires Unix process groups",
        ));
    }
    #[cfg(unix)]
    launch_unix(paths, request)
}

#[cfg(test)]
pub(super) use launch_direct as launch;

#[cfg(test)]
pub(super) fn launch_direct(
    paths: &RuntimePaths,
    request: OccupantLaunchRequest<'_>,
    reporter: Arc<dyn CaptureReporter>,
) -> CommandResult<Launch> {
    let occupant = request.occupant;
    let launch = occupant_launch(paths, request, reporter)?;
    let git = launch.git;
    let proof = launch.session;
    let running = sill::spawn_observed(launch.spec, launch.observation)
        .map_err(|error| environment_fault("lane::supervisor-launch-failed", &error.to_string()))?;
    let group = running.id();
    let session = await_session(proof).map_err(|error| wire::Outcome::Refusal(error.refusal()))?;
    Ok(Launch {
        occupant,
        group,
        session,
        git,
        process: Box::new(DirectHandle(running)),
    })
}

#[cfg(all(unix, not(test)))]
fn launch_unix(paths: &RuntimePaths, request: OccupantLaunchRequest<'_>) -> CommandResult<Launch> {
    let directory = supervisor_directory(paths, request.lane, request.occupant);
    fs::create_dir_all(&directory).map_err(|error| {
        environment_fault("lane::supervisor-state-unavailable", &error.to_string())
    })?;
    let state = directory.join("state.json");
    let request_path = directory.join("request.json");
    let encoded = Request {
        version: REQUEST_VERSION,
        worktree: request.worktree.to_owned(),
        vendor: request.vendor.as_str().to_owned(),
        epoch: request.epoch,
        lane: request.lane,
        occupant: request.occupant,
        resume: request.resume.session_id().map(str::to_owned),
        fast_mode: request.fast_mode,
        model: request.model.map(str::to_owned),
        effort: request.effort.map(str::to_owned),
        state: state.clone(),
    };
    publish_json(&request_path, &encoded).map_err(|error| {
        environment_fault("lane::supervisor-request-unavailable", &error.to_string())
    })?;
    let executable = std::env::current_exe().map_err(|error| {
        environment_fault(
            "lane::supervisor-executable-unavailable",
            &error.to_string(),
        )
    })?;
    let log = supervisor_log(&directory).map_err(|error| {
        environment_fault("lane::supervisor-log-unavailable", &error.to_string())
    })?;
    let stdout = log.try_clone().map_err(|error| {
        environment_fault("lane::supervisor-log-unavailable", &error.to_string())
    })?;
    let events = StateEvents::watch(&state).map_err(|error| {
        environment_fault("lane::supervisor-observer-unavailable", &error.to_string())
    })?;
    let mut supervisor = ProcessCommand::new(executable)
        .arg(ENTRY)
        .arg(&request_path)
        .current_dir(request.worktree)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(|error| environment_fault("lane::supervisor-launch-failed", &error.to_string()))?;
    events.observe_exit(supervisor.id());
    let _reaper = thread::Builder::new()
        .name("spice-supervisor-reaper".to_owned())
        .spawn(move || {
            let _ = supervisor.wait();
        });
    await_running(paths, &state, request.lane, request.occupant, events)
}

/// Adopt the exact live supervisor already naming a durable lane occupant.
pub(super) fn adopt(
    paths: &RuntimePaths,
    row: &wire::LaneResidency,
) -> CommandResult<Option<Adoption>> {
    let Some(occupant) = row.occupant else {
        return Ok(None);
    };
    let Some((path, state)) = ending::durable_state(paths, row.lane, occupant)? else {
        return Ok(None);
    };
    match state.phase {
        Phase::Running if process_exists(state.supervisor)? => state_launch(paths, path, state)
            .map(Adoption::Running)
            .map(Some),
        Phase::Running => Ok(Some(Adoption::Failed(
            ending::SUPERVISOR_DISAPPEARED.to_owned(),
        ))),
        Phase::Exited => state_launch(paths, path, state)
            .map(Adoption::Exited)
            .map(Some),
        Phase::Failed => Ok(Some(Adoption::Failed(ending::failure_detail(
            state.failure,
        )))),
        Phase::Starting => Ok(Some(Adoption::Failed(
            ending::DID_NOT_FINISH_STARTING.to_owned(),
        ))),
    }
}

#[cfg(all(unix, not(test)))]
fn await_running(
    paths: &RuntimePaths,
    path: &Path,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    events: StateEvents,
) -> CommandResult<Launch> {
    let started = Monotonic::now();
    let mut supervisor_exited = false;
    loop {
        match read_state(path) {
            Ok(state) if !matches_state(&state, lane, occupant) => {
                return Err(store_fault(
                    "lane::supervisor-state-mismatched",
                    "detached supervisor published another lane or occupant",
                ));
            }
            Ok(state) if matches!(state.phase, Phase::Running | Phase::Exited) => {
                // A short-lived agent can publish its session and end between
                // two parent observations. The terminal state still carries
                // the complete launch authority; returning its handle lets the
                // resident commit the start and then observe the exact ending
                // instead of turning a harmless race into resident failure.
                let events = (state.phase == Phase::Running).then_some(events);
                let door = command::kick_path(paths, lane);
                return state_launch_inner(path.to_path_buf(), door, state, events);
            }
            Ok(state) if state.phase == Phase::Failed => {
                return Err(store_fault(
                    "lane::supervisor-start-failed",
                    state
                        .failure
                        .as_deref()
                        .unwrap_or("supervisor failed without detail"),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(environment_fault(
                    "lane::supervisor-state-unavailable",
                    &error.to_string(),
                ));
            }
        }
        if supervisor_exited {
            return Err(environment_fault(
                "lane::supervisor-start-failed",
                "detached supervisor exited before publishing a proven session",
            ));
        }
        let remaining = START_HORIZON.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(environment_fault(
                "lane::supervisor-start-timeout",
                "detached supervisor did not publish a proven session before its start horizon",
            ));
        }
        match events.wait_for(remaining) {
            Ok(Some(StateSignal::Changed)) => {}
            Ok(Some(StateSignal::SupervisorExited)) => supervisor_exited = true,
            Ok(Some(StateSignal::ObserverFailed(error))) => {
                return Err(environment_fault(
                    "lane::supervisor-observer-failed",
                    &error,
                ));
            }
            Err(error) => {
                return Err(environment_fault(
                    "lane::supervisor-observer-failed",
                    &error.to_string(),
                ));
            }
            Ok(None) => {
                return Err(environment_fault(
                    "lane::supervisor-start-timeout",
                    "detached supervisor did not publish a proven session before its start horizon",
                ));
            }
        }
    }
}

fn state_launch(paths: &RuntimePaths, path: PathBuf, state: State) -> CommandResult<Launch> {
    let door = command::kick_path(paths, state.lane);
    #[cfg(unix)]
    let events = if state.phase == Phase::Running {
        Some(StateEvents::arm(&path, state.supervisor).map_err(|error| {
            environment_fault("lane::supervisor-observer-unavailable", &error.to_string())
        })?)
    } else {
        None
    };
    state_launch_inner(
        path,
        door,
        state,
        #[cfg(unix)]
        events,
    )
}

fn state_launch_inner(
    path: PathBuf,
    door: PathBuf,
    state: State,
    #[cfg(unix)] events: Option<StateEvents>,
) -> CommandResult<Launch> {
    let group = required(state.group, "process group")?;
    let session = required(state.session.clone(), "session proof")?;
    let git = required(state.git.clone(), "launch Git identity")?;
    let handle = Handle::new(
        path,
        door,
        &state,
        #[cfg(unix)]
        events,
    )?;
    Ok(Launch {
        occupant: state.occupant,
        group,
        session,
        git,
        process: Box::new(handle),
    })
}

fn run(request_path: &Path) -> io::Result<()> {
    let request: Request = read_json(request_path, REQUEST_BYTES)?;
    if request.version != REQUEST_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported supervisor request version {}", request.version),
        ));
    }
    let starting = State {
        version: STATE_VERSION,
        supervisor: std::process::id(),
        worktree: request.worktree.clone(),
        vendor: request.vendor.clone(),
        lane: request.lane,
        occupant: request.occupant,
        phase: Phase::Starting,
        group: None,
        session: None,
        git: None,
        started_millis: None,
        runtime_millis: None,
        grace_millis: None,
        reap_millis: None,
        completion: CompletionField::none(),
        failure: None,
    };
    publish_json(&request.state, &starting)?;
    if let Err(error) = run_agent(&request, starting.clone()) {
        let mut failed = read_state(&request.state).unwrap_or(starting);
        failed.phase = Phase::Failed;
        failed.failure = Some(error.to_string());
        publish_json(&request.state, &failed)?;
        return Err(error);
    }
    Ok(())
}

fn run_agent(request: &Request, mut state: State) -> io::Result<()> {
    let repository = Repository::discover(&request.worktree).map_err(io::Error::other)?;
    let paths = RuntimePaths::from_repository(&repository).map_err(outcome_error)?;
    let vendor = request
        .vendor
        .parse::<wire::Name>()
        .map_err(io::Error::other)?;
    let resume = request
        .resume
        .as_ref()
        .map(|identity| vendor::SessionId::new(identity.clone()).map(vendor::Resume::Session))
        .transpose()
        .map_err(io::Error::other)?
        .unwrap_or(vendor::Resume::Fresh);
    let reporter = Arc::new(DurableReporter::new(&paths)?);
    let capture_reporter: Arc<dyn CaptureReporter> = reporter.clone();
    let launch = occupant_launch(
        &paths,
        OccupantLaunchRequest {
            worktree: &request.worktree,
            vendor: &vendor,
            epoch: request.epoch,
            lane: request.lane,
            occupant: request.occupant,
            resume,
            fast_mode: request.fast_mode,
            model: request.model.as_deref(),
            effort: request.effort.as_deref(),
        },
        capture_reporter,
    )
    .map_err(outcome_error)?;
    let git = launch.git;
    let proof = launch.session;
    let started_millis = now_millis()?;
    let jobs = terminal::Jobs::for_lane(&paths, request.lane);
    let terminal = terminal::Terminal::from_launch(&launch.spec, jobs);
    let mut running = sill::spawn_observed(launch.spec, launch.observation)?;
    let group = running.id();
    let runtime = running.spec().deadlines.runtime();
    let grace = running.spec().grace;
    let reap = running.spec().deadlines.reap();
    let _door = launch
        .opening
        .map(|opening| {
            let driver = command::Driver::new(running.take_stdin()?, launch.inbox, terminal);
            reporter.drive(Arc::clone(&driver));
            driver.open(&opening)?;
            let door = command::listen_for_kicks(&paths, request.lane, Arc::clone(&driver))?;
            driver.catch_up();
            Ok::<_, io::Error>(door)
        })
        .transpose()?;
    let session = await_session(proof).map_err(session_error)?;
    state.phase = Phase::Running;
    state.group = Some(group);
    state.session = Some(session);
    state.git = Some(git);
    state.started_millis = Some(started_millis);
    state.runtime_millis = Some(duration_millis(runtime)?);
    state.grace_millis = Some(duration_millis(grace)?);
    state.reap_millis = Some(duration_millis(reap)?);
    publish_json(&request.state, &state)?;
    let outcome = match running.wait() {
        Ok(outcome) => outcome,
        Err(error) => {
            let failure = error.to_string();
            state.phase = Phase::Failed;
            state.failure = Some(failure.clone());
            publish_json(&request.state, &state)?;
            reporter.send(&Notice::Failed {
                lane: request.lane,
                occupant: request.occupant,
                failure,
            });
            return Ok(());
        }
    };
    if let Some(observation_failure) = outcome.observation_failure() {
        let failure = observation_failure.to_string();
        state.phase = Phase::Failed;
        state.completion = CompletionField::none();
        state.failure = Some(failure.clone());
        publish_json(&request.state, &state)?;
        reporter.send(&Notice::Failed {
            lane: request.lane,
            occupant: request.occupant,
            failure,
        });
        return Ok(());
    }
    state.phase = Phase::Exited;
    state.completion = CompletionField::some(PersistedCompletion::capture(outcome.completion()));
    publish_json(&request.state, &state)?;
    reporter.send(&Notice::Exited {
        lane: request.lane,
        occupant: request.occupant,
    });
    Ok(())
}

/// A field a running supervisor's published state must carry.
fn required<T>(value: Option<T>, name: &str) -> CommandResult<T> {
    value.ok_or_else(|| {
        store_fault(
            "lane::supervisor-state-invalid",
            &format!("running supervisor has no {name}"),
        )
    })
}

struct Handle {
    path: PathBuf,
    /// The kick door a command-driven occupant's driver listens at.
    door: PathBuf,
    owner: String,
    group: u32,
    started_millis: u64,
    runtime: Duration,
    grace: Duration,
    reap: Duration,
    #[cfg(unix)]
    events: Option<StateEvents>,
}

impl Handle {
    fn new(
        path: PathBuf,
        door: PathBuf,
        state: &State,
        #[cfg(unix)] events: Option<StateEvents>,
    ) -> CommandResult<Self> {
        Ok(Self {
            path,
            door,
            owner: format!("lane {} detached supervisor", state.lane),
            group: required(state.group, "group")?,
            started_millis: required(state.started_millis, "start instant")?,
            runtime: Duration::from_millis(required(state.runtime_millis, "runtime")?),
            grace: Duration::from_millis(required(state.grace_millis, "termination grace")?),
            reap: Duration::from_millis(required(state.reap_millis, "reap bound")?),
            #[cfg(unix)]
            events,
        })
    }

    fn state(&self) -> Result<State, WaitError> {
        read_state(&self.path).map_err(|source| self.wait_error("reading supervisor state", source))
    }

    fn wait_error(&self, operation: &'static str, source: io::Error) -> WaitError {
        WaitError::Io {
            owner: self.owner.clone(),
            operation,
            source,
        }
    }

    fn terminal(&self) -> Result<Option<WaitOutcome>, WaitError> {
        let state = self.state()?;
        match state.phase {
            Phase::Exited => state.completion.as_ref().map_or_else(
                || {
                    Err(self.wait_error(
                        "decoding supervisor exit",
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "exited supervisor state has no typed completion",
                        ),
                    ))
                },
                |completion| {
                    completion
                        .decode()
                        .map(WaitOutcome::externally_reaped_completion)
                        .map(Some)
                        .map_err(|error| self.wait_error("decoding supervisor exit", error))
                },
            ),
            Phase::Failed => Err(self.wait_error(
                "observing supervisor",
                io::Error::other(
                    state
                        .failure
                        .unwrap_or_else(|| "supervisor failed".to_owned()),
                ),
            )),
            Phase::Starting | Phase::Running => Ok(None),
        }
    }

    fn signal(&self, signal: i32, operation: &'static str) -> Result<(), WaitError> {
        signal_group(self.group, signal).map_err(|error| self.wait_error(operation, error))
    }
}

impl DetachedProcess for Handle {
    fn group(&self) -> u32 {
        self.group
    }

    fn runtime_remaining(&self) -> Duration {
        let elapsed = now_millis()
            .ok()
            .and_then(|now| now.checked_sub(self.started_millis))
            .map(Duration::from_millis)
            .unwrap_or_default();
        self.runtime.saturating_sub(elapsed)
    }

    fn termination_grace(&self) -> Duration {
        self.grace
    }

    fn reap_bound(&self) -> Duration {
        self.reap
    }

    fn wait_for(self: Box<Self>, duration: Duration) -> Result<DetachedWait, WaitError> {
        #[cfg(unix)]
        match self.observe_until(Some(duration))? {
            Some(outcome) => Ok(DetachedWait::Exited(outcome)),
            None => Ok(DetachedWait::Running(self)),
        }
        #[cfg(not(unix))]
        {
            let _ = duration;
            Err(self.wait_error(
                "waiting for supervisor state change",
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "detached supervisor observation requires Unix",
                ),
            ))
        }
    }

    fn terminate(&self) -> Result<(), WaitError> {
        // An occupant driven over its own protocol is interrupted there; the
        // signal is for one nobody drives, or one that stays after the ask.
        if command::abort(&self.door).is_ok() {
            return Ok(());
        }
        self.signal(libc::SIGTERM, "terminating supervisor-owned process group")
    }

    fn kill(&self) -> Result<(), WaitError> {
        self.signal(libc::SIGKILL, "killing supervisor-owned process group")
    }

    fn wait(self: Box<Self>) -> Result<WaitOutcome, WaitError> {
        #[cfg(unix)]
        {
            self.observe_until(None)?.ok_or_else(|| {
                self.wait_error(
                    "waiting for supervisor state change",
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        "unbounded supervisor wait ended without a terminal state",
                    ),
                )
            })
        }
        #[cfg(not(unix))]
        {
            Err(self.wait_error(
                "waiting for supervisor state change",
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "detached supervisor observation requires Unix",
                ),
            ))
        }
    }
}

#[cfg(test)]
struct DirectHandle(sill::Running);

#[cfg(test)]
impl DetachedProcess for DirectHandle {
    fn group(&self) -> u32 {
        self.0.id()
    }

    fn runtime_remaining(&self) -> Duration {
        self.0.runtime_remaining()
    }

    fn termination_grace(&self) -> Duration {
        self.0.spec().grace
    }

    fn reap_bound(&self) -> Duration {
        self.0.spec().deadlines.reap()
    }

    fn wait_for(self: Box<Self>, duration: Duration) -> Result<DetachedWait, WaitError> {
        match self.0.wait_for(duration)? {
            sill::WaitStep::Exited(outcome) => Ok(DetachedWait::Exited(outcome)),
            sill::WaitStep::Running(running) => Ok(DetachedWait::Running(Box::new(Self(*running)))),
        }
    }

    fn terminate(&self) -> Result<(), WaitError> {
        self.0.signal_terminate()
    }

    fn kill(&self) -> Result<(), WaitError> {
        self.0.signal_kill()
    }

    fn wait(self: Box<Self>) -> Result<WaitOutcome, WaitError> {
        self.0.wait()
    }
}

fn supervisor_directory(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> PathBuf {
    paths
        .supervisors
        .join(lane.to_string())
        .join(occupant.to_string())
}

pub(super) fn capture_path(paths: &RuntimePaths) -> PathBuf {
    #[cfg(unix)]
    {
        // `sockaddr_un.sun_path` is only 104 bytes on Darwin. Repository and
        // worktree roots are not bounded that tightly, so the ephemeral notice
        // door uses one content-addressed leaf beneath the canonical short
        // system temporary path. Durable state remains repository-local.
        let identity = blake3::hash(paths.common.as_os_str().as_encoded_bytes());
        PathBuf::from("/tmp").join(format!("spice-s-{identity}.sock"))
    }
    #[cfg(not(unix))]
    {
        paths.runtime.join("supervisor-capture.sock")
    }
}

#[cfg(not(test))]
fn supervisor_log(directory: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(directory.join("supervisor.log"))
}

fn publish_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    atomic::Destination::new(path)
        .map_err(io::Error::other)?
        .publish(|file| file.write_all(&bytes))
        .map_err(io::Error::other)
}

fn read_state(path: &Path) -> io::Result<State> {
    let value: serde_json::Value = read_json(path, STATE_BYTES)?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "supervisor state has no valid contract version",
            )
        })?;
    if !matches!(version, STATE_VERSION | REQUEST_V3_STATE_VERSION) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "supervisor state contract v{} is not owned by current Spice {} contract v{}",
                version,
                env!("CARGO_PKG_VERSION"),
                STATE_VERSION
            ),
        ));
    }
    let state: State = serde_json::from_value(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    state.validate()
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path, limit: u64) -> io::Result<T> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "supervisor document exceeds its bound",
        ));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn matches_state(state: &State, lane: CanonicalUuid, occupant: CanonicalUuid) -> bool {
    state.lane == lane && state.occupant == occupant
}

fn now_millis() -> io::Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_millis();
    u64::try_from(millis).map_err(io::Error::other)
}

fn duration_millis(duration: Duration) -> io::Result<u64> {
    u64::try_from(duration.as_millis()).map_err(io::Error::other)
}

fn session_error(error: SessionProofError) -> io::Error {
    let refusal = error.refusal();
    io::Error::other(format!("session proof refused as {refusal:?}"))
}

fn outcome_error(outcome: wire::Outcome<()>) -> io::Error {
    io::Error::other(format!("{outcome:?}"))
}

#[cfg(unix)]
fn status_raw(status: ExitStatus) -> i32 {
    status.into_raw()
}

#[cfg(not(unix))]
fn status_raw(_status: ExitStatus) -> i32 {
    1
}

#[cfg(unix)]
fn status_from_raw(raw: i32) -> io::Result<ExitStatus> {
    Ok(ExitStatus::from_raw(raw))
}

#[cfg(not(unix))]
fn status_from_raw(_raw: i32) -> io::Result<ExitStatus> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "external exit status requires Unix",
    ))
}

#[cfg(unix)]
fn signal_group(group: u32, signal: i32) -> io::Result<()> {
    let group = i32::try_from(group).map_err(io::Error::other)?;
    // SAFETY: a negative PID addresses the exact process group named by the
    // detached supervisor state; the conversion above excludes overflow.
    if unsafe { libc::kill(-group, signal) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(unix))]
fn signal_group(_group: u32, _signal: i32) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "process-group signals require Unix",
    ))
}

fn process_exists(pid: u32) -> CommandResult<bool> {
    process_exists_raw(pid).map_err(|error| {
        environment_fault("lane::supervisor-liveness-unavailable", &error.to_string())
    })
}

#[cfg(unix)]
fn process_exists_raw(pid: u32) -> io::Result<bool> {
    let pid = i32::try_from(pid).map_err(io::Error::other)?;
    // SAFETY: signal zero changes no process state and only probes this exact PID.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(error),
    }
}

#[cfg(not(unix))]
fn process_exists_raw(_pid: u32) -> io::Result<bool> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "process probes require Unix",
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;

    use super::*;

    fn uuid(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131165{last:02}")
            .parse()
            .expect("fixture UUID")
    }

    fn paths(root: &Path) -> RuntimePaths {
        RuntimePaths {
            worktree: root.to_path_buf(),
            common: root.join("common"),
            runtime: root.join("runtime"),
            data: root.join("data"),
            journal: root.join("journal"),
            resident: root.join("resident"),
            canvas: root.join("canvas"),
            lane_canvas: root.join("lane-canvas"),
            inbox: root.join("inbox"),
            legacy_inbox: root.join("legacy-inbox"),
            degraded: root.join("degraded"),
            tape: root.join("tape"),
            supervisors: root.join("supervisors"),
            stage: root.join("stage"),
        }
    }

    fn terminal_state(
        root: &Path,
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        session: wire::LaneSession,
        completion: PersistedCompletion,
    ) -> State {
        State {
            version: STATE_VERSION,
            supervisor: std::process::id(),
            worktree: root.to_string_lossy().into_owned(),
            vendor: "codex".to_owned(),
            lane,
            occupant,
            phase: Phase::Exited,
            group: Some(412),
            session: Some(session),
            git: Some("fixture-head".to_owned()),
            started_millis: Some(now_millis().expect("fixture time")),
            runtime_millis: Some(4 * 60 * 60 * 1_000),
            grace_millis: Some(20_000),
            reap_millis: Some(20_000),
            completion: CompletionField::some(completion),
            failure: None,
        }
    }

    #[test]
    fn replacement_adopts_running_and_terminal_supervisors_across_request_versions() {
        let root = std::env::temp_dir().join(format!(
            "spice-terminal-supervisor-{}-{}",
            std::process::id(),
            now_millis().expect("fixture time")
        ));
        let paths = paths(&root);
        let lane = uuid(1);
        let occupant = uuid(2);
        let directory = supervisor_directory(&paths, lane, occupant);
        fs::create_dir_all(&directory).expect("supervisor directory");
        let session = wire::LaneSession {
            lane,
            occupant,
            worktree: root.to_string_lossy().into_owned(),
            vendor: "codex".parse().expect("fixture vendor"),
            launch: None,
            epoch: uuid(3),
            stream: "stdout".to_owned(),
            identity: "terminal-session".to_owned(),
            locus: wire::TapeLocus {
                member: 0,
                offset: 0,
                length: 1,
            },
        };
        let row = wire::LaneResidency {
            lane,
            team: lane,
            name: None,
            worktree: root.to_string_lossy().into_owned(),
            life: "drive".to_owned(),
            vendor: "codex".parse().expect("fixture vendor"),
            model: None,
            effort: None,
            occupant: Some(occupant),
            lineage: vec![occupant],
            capabilities: vec![uuid(4)],
            session: Some(session),
            disposition: "occupied".to_owned(),
        };

        // State v2 and the request-v3 release's identical state must both
        // survive resident replacement, including subsequent supervisor writes.
        for version in [2, 3] {
            for expected in [
                PersistedCompletion::Exited { status_raw: 0 },
                PersistedCompletion::TimedOut { status_raw: 0 },
                PersistedCompletion::Cancelled { status_raw: 7 << 8 },
            ] {
                let mut terminal = terminal_state(
                    &root,
                    lane,
                    occupant,
                    row.session.clone().expect("fixture session"),
                    expected.clone(),
                );
                terminal.version = version;
                let mut running = terminal.clone();
                running.phase = Phase::Running;
                running.completion = CompletionField::none();
                let path = directory.join("state.json");
                publish_json(&path, &running).expect("running state publishes");
                let adopted = adopt(&paths, &row)
                    .expect("running state reads")
                    .expect("running supervisor is adoptable");
                let Adoption::Running(adopted) = adopted else {
                    panic!("live supervisor was not adopted");
                };
                assert_eq!(adopted.occupant, occupant);
                assert_eq!(adopted.group, 412);
                assert_eq!(
                    adopted.session,
                    row.session.clone().expect("fixture session")
                );
                let DetachedWait::Running(process) = adopted
                    .process
                    .wait_for(Duration::ZERO)
                    .expect("adopted supervisor remains running")
                else {
                    panic!("live supervisor was mistaken for a terminal state");
                };

                publish_json(&path, &terminal).expect("terminal state publishes");
                let outcome = process.wait().expect("adopted supervisor completion reads");
                assert_eq!(PersistedCompletion::capture(outcome.completion()), expected);
                let adopted = adopt(&paths, &row)
                    .expect("terminal state reads")
                    .expect("terminal supervisor is adoptable");
                let Adoption::Exited(adopted) = adopted else {
                    panic!("terminal supervisor was mistaken for a live launch");
                };
                assert_eq!(adopted.occupant, occupant);
                assert_eq!(adopted.group, 412);
                let outcome = adopted.process.wait().expect("terminal status reads");
                assert_eq!(PersistedCompletion::capture(outcome.completion()), expected);
            }
        }
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn every_typed_completion_round_trips_without_conflating_code_zero() {
        let root = std::env::temp_dir();
        let lane = uuid(5);
        let occupant = uuid(6);
        let session = wire::LaneSession {
            lane,
            occupant,
            worktree: root.to_string_lossy().into_owned(),
            vendor: "codex".parse().expect("fixture vendor"),
            launch: None,
            epoch: uuid(7),
            stream: "stdout".to_owned(),
            identity: "round-trip-session".to_owned(),
            locus: wire::TapeLocus {
                member: 0,
                offset: 0,
                length: 1,
            },
        };
        let completions = [
            PersistedCompletion::Exited { status_raw: 0 },
            PersistedCompletion::TimedOut { status_raw: 0 },
            PersistedCompletion::Cancelled { status_raw: 7 << 8 },
        ];

        for expected in completions {
            let state = terminal_state(&root, lane, occupant, session.clone(), expected.clone());
            let bytes = serde_json::to_vec(&state).expect("state encodes");
            let decoded: State = serde_json::from_slice(&bytes).expect("state decodes");
            let decoded = decoded.validate().expect("state validates");
            assert_eq!(decoded.completion.as_ref(), Some(&expected));
        }
        for phase in [Phase::Starting, Phase::Running, Phase::Failed] {
            let mut state = terminal_state(
                &root,
                lane,
                occupant,
                session.clone(),
                PersistedCompletion::Exited { status_raw: 0 },
            );
            state.phase = phase;
            state.completion = CompletionField::none();
            let value = serde_json::to_value(&state).expect("nonterminal state encodes");
            assert!(
                value
                    .get("completion")
                    .is_some_and(serde_json::Value::is_null),
                "{phase:?} state omitted its required null completion"
            );
            serde_json::from_value::<State>(value)
                .expect("required null completion decodes")
                .validate()
                .expect("nonterminal null completion validates");
        }
    }

    #[test]
    fn current_state_refuses_missing_unknown_and_phase_mismatched_completion() {
        let root = std::env::temp_dir().join(format!(
            "spice-invalid-supervisor-{}-{}",
            std::process::id(),
            now_millis().expect("fixture time")
        ));
        fs::create_dir_all(&root).expect("fixture directory creates");
        let lane = uuid(8);
        let occupant = uuid(9);
        let session = wire::LaneSession {
            lane,
            occupant,
            worktree: root.to_string_lossy().into_owned(),
            vendor: "codex".parse().expect("fixture vendor"),
            launch: None,
            epoch: uuid(10),
            stream: "stdout".to_owned(),
            identity: "invalid-session".to_owned(),
            locus: wire::TapeLocus {
                member: 0,
                offset: 0,
                length: 1,
            },
        };
        let valid = serde_json::to_value(terminal_state(
            &root,
            lane,
            occupant,
            session,
            PersistedCompletion::Exited { status_raw: 0 },
        ))
        .expect("valid state encodes");
        let cases = [
            ("missing", {
                let mut value = valid.clone();
                value
                    .as_object_mut()
                    .expect("state is an object")
                    .remove("completion");
                value
            }),
            ("exited-null", {
                let mut value = valid.clone();
                value["completion"] = serde_json::Value::Null;
                value
            }),
            ("running-terminal", {
                let mut value = valid.clone();
                value["phase"] = serde_json::Value::String("running".to_owned());
                value
            }),
            ("unknown-kind", {
                let mut value = valid.clone();
                value["completion"]["kind"] = serde_json::Value::String("status_only".to_owned());
                value
            }),
            ("status-only", {
                let mut value = valid.clone();
                value
                    .as_object_mut()
                    .expect("state is an object")
                    .remove("completion");
                value["status_raw"] = serde_json::Value::from(0);
                value
            }),
        ];
        let path = root.join("state.json");

        for version in [2, 3] {
            for (label, value) in &cases {
                let mut value = value.clone();
                value["version"] = serde_json::Value::from(version);
                fs::write(
                    &path,
                    serde_json::to_vec(&value).expect("invalid fixture encodes"),
                )
                .expect("invalid fixture writes");
                let error = read_state(&path)
                    .err()
                    .unwrap_or_else(|| panic!("v{version} {label} state reached adoption"));
                assert_eq!(
                    error.kind(),
                    io::ErrorKind::InvalidData,
                    "v{version} {label}"
                );
            }
        }
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn contract_v1_request_is_refused_before_state_publication() {
        let root = std::env::temp_dir().join(format!(
            "spice-v1-supervisor-request-{}-{}",
            std::process::id(),
            now_millis().expect("fixture time")
        ));
        fs::create_dir_all(&root).expect("fixture directory creates");
        let request_path = root.join("request.json");
        let state_path = root.join("state.json");
        publish_json(
            &request_path,
            &Request {
                version: 1,
                worktree: root.to_string_lossy().into_owned(),
                vendor: "codex".to_owned(),
                epoch: uuid(3),
                lane: uuid(1),
                occupant: uuid(2),
                resume: None,
                fast_mode: false,
                model: None,
                effort: None,
                state: state_path.clone(),
            },
        )
        .expect("legacy request fixture publishes");

        let error = run(&request_path).expect_err("v1 request refuses");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error
                .to_string()
                .contains("unsupported supervisor request version 1")
        );
        assert!(!state_path.exists(), "refused request published state");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn unsupported_state_versions_are_refused_without_a_legacy_execution_path() {
        let root = std::env::temp_dir().join(format!(
            "spice-v1-supervisor-{}-{}",
            std::process::id(),
            now_millis().expect("fixture time")
        ));
        fs::create_dir_all(&root).expect("fixture directory creates");
        let state = root.join("state.json");
        for version in [1, 4] {
            fs::write(
                &state,
                serde_json::to_vec(&serde_json::json!({
                    "version": version,
                    "phase": "exited",
                    "status_raw": 0,
                }))
                .expect("unsupported fixture encodes"),
            )
            .expect("unsupported fixture writes");

            let error = match read_state(&state) {
                Ok(_) => panic!("v{version} state has no current execution path"),
                Err(error) => error,
            };
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            let diagnostic = error.to_string();
            assert!(
                diagnostic.contains(&format!("contract v{version}")),
                "{diagnostic}"
            );
            assert!(
                diagnostic.contains(&format!(
                    "current Spice {} contract v{STATE_VERSION}",
                    env!("CARGO_PKG_VERSION"),
                )),
                "{diagnostic}"
            );
        }
        fs::remove_dir_all(root).expect("fixture cleanup");
    }
}
