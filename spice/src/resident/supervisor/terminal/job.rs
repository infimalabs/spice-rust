//! A job: one command the lane keeps beyond the call that started it.
//!
//! Every command the occupant runs is kept by a detached keeper process,
//! `spice __lane-job`, that owns nothing but the command: it runs the argv
//! in the command's directory under its environment with both streams
//! appended to one log, ends the command's process group at its deadline or
//! when asked to stop, and publishes how the command ended. A command
//! started with events has its stdout read by the keeper instead, which
//! still copies it to the log and publishes its lines as events before the
//! ending. The keeper is its own process group under nobody's supervision,
//! so the command survives the turn, the occupant, and the supervisor that
//! started it; its result waits in the job's state for whoever asks. The
//! lane observes a job through its durable state: a direct kernel watch on
//! the job's directory wakes every read, and the keeper's exit names a
//! keeper that died without an ending. The keeper reads its stop request
//! the same way, and an ending or event nobody hears within the grace wakes
//! the lane, so its next occupant hears it.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant as Monotonic};

use serde::{Deserialize, Serialize};
use sill::Environment;
use steer::Key;
use wire::CanonicalUuid;

use super::super::super::RuntimePaths;
use super::super::super::daemon::observer;
use super::super::{
    REQUEST_BYTES, STATE_BYTES, millis_since_epoch, now_millis, process_exists_raw, publish_json,
    read_json, signal_group,
};

mod events;
mod keeper;
mod launch;
mod service;

pub(in crate::resident) use events::Event;
pub(in crate::resident::supervisor) use keeper::keep;
pub(in crate::resident) use keeper::{Control, Handle, Observed};
use keeper::{Signal, watch};
use launch::{KeeperLaunch, Launch, failed_start_error, launch_detached};
use service::ServiceClaim;
pub(in crate::resident) use service::{ServiceAdmission, ServiceAuthority};

/// The hidden entry the keeper runs under.
pub(in crate::resident::supervisor) const ENTRY: &str = "__lane-job";

const VERSION: u32 = 1;
pub(super) const REQUEST: &str = "request.json";
pub(super) const STATE: &str = "state.json";
const LOG: &str = "log";
const STOP: &str = "stop";
const OPERATOR_STOP: &str = "operator-stop";
pub(super) const DELIVERED: &str = "delivered";
const WAKE: &str = "wake";

/// Bytes of the log a reading keeps: the end, where a failure explains
/// itself.
pub(super) const LOG_TAIL: usize = 16 * 1024;

/// How long a command has to leave on its own once the keeper ends it.
const TERMINATE_GRACE: Duration = Duration::from_secs(1);

/// How long an occupant has to hear an ending or an event before the keeper
/// wakes the lane to deliver it.
const DELIVERY_GRACE: Duration = Duration::from_secs(2);

/// What the keeper reads: the command as the model wrote it, the argv the
/// lane runs, and where and under what to run it.
#[derive(Deserialize, Serialize)]
struct Request {
    version: u32,
    id: String,
    command: String,
    argv: Vec<OsString>,
    cwd: PathBuf,
    environment: Environment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_millis: Option<u64>,
    /// What the results carry ahead of the command's output, as omp noted it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    notices: Vec<String>,
    /// The exact active claim that owns an unbounded service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    service_claim: Option<ServiceClaim>,
    /// The lane to wake when nobody hears the ending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lane: Option<CanonicalUuid>,
    /// Whether the command's stdout lines are published as events.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    events: bool,
}

/// What the keeper publishes: who keeps the command, and how it ended.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(in crate::resident) struct State {
    version: u32,
    pub(in crate::resident) id: String,
    pub(in crate::resident) command: String,
    pub(in crate::resident) keeper: u32,
    pub(in crate::resident) started_millis: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::resident) timeout_millis: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::resident) ending: Option<Ending>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::resident) ended_millis: Option<u64>,
}

impl State {
    /// The ending with its wall time, once the keeper has published one.
    fn ended(&self) -> Option<Ended> {
        let ending = self.ending.clone()?;
        let wall = self
            .ended_millis
            .map(|ended| Duration::from_millis(ended.saturating_sub(self.started_millis)));
        Some(Ended { ending, wall })
    }
}

/// How a job ended and how long it ran, as the keeper recorded them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::resident) struct Ended {
    pub(in crate::resident) ending: Ending,
    /// From the keeper's start to the ending; unknown for a lost job.
    pub(in crate::resident) wall: Option<Duration>,
}

/// How a kept command ended.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(in crate::resident) enum Ending {
    Exited {
        code: i32,
    },
    Signalled {
        signal: i32,
    },
    TimedOut,
    Stopped,
    Failed {
        reason: String,
    },
    /// The keeper is gone without publishing an ending; read, never written.
    Lost,
}

/// The last of a job's log, and whether earlier output was left behind.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(in crate::resident) struct Tail {
    pub(in crate::resident) text: String,
    pub(in crate::resident) truncated: bool,
}

/// Where one lane's jobs live, and the lane to wake for an ending nobody
/// hears.
#[derive(Clone, Debug)]
pub(in crate::resident) struct Jobs {
    root: PathBuf,
    lane: Option<CanonicalUuid>,
}

/// One kept command, by the directory that records it.
#[derive(Clone, Debug)]
pub(in crate::resident) struct Job {
    directory: PathBuf,
    id: String,
    command: String,
    cwd: PathBuf,
    timeout: Option<Duration>,
    service_claim: Option<ServiceClaim>,
    notices: Vec<String>,
    events: bool,
    keeper: Option<u32>,
}

impl Jobs {
    /// This lane's jobs, beneath the lane's own supervisor records.
    pub(in crate::resident) fn for_lane(paths: &RuntimePaths, lane: CanonicalUuid) -> Self {
        Self::beneath(&paths.supervisors, lane)
    }

    /// This lane's jobs beneath one root of per-lane supervisor records.
    pub(in crate::resident) fn beneath(supervisors: &Path, lane: CanonicalUuid) -> Self {
        Self {
            root: supervisors.join(lane.to_string()).join("jobs"),
            lane: Some(lane),
        }
    }

    /// Start keeping one command.
    ///
    /// # Errors
    ///
    /// Returns the failure of recording the request, revalidating a service's
    /// claim after publication, launching the keeper, or removing a failed
    /// start's record. Only a launch known not to own a process is removed.
    pub(in crate::resident) fn start(
        &self,
        command: &str,
        argv: Vec<OsString>,
        cwd: &Path,
        environment: &Environment,
        timeout: Option<Duration>,
        service: Option<ServiceAdmission<'_>>,
        notices: &[String],
        events: bool,
    ) -> io::Result<Job> {
        self.start_with(
            Launch {
                command,
                argv,
                cwd,
                environment,
                timeout,
                notices,
                events,
            },
            service,
            |directory, request, cwd, environment| {
                launch_detached(directory, request, cwd, environment)
            },
            |path| fs::remove_dir_all(path),
        )
    }

    /// Record one job and establish its keeper's process ownership with `launch`.
    fn start_with(
        &self,
        job: Launch<'_>,
        service: Option<ServiceAdmission<'_>>,
        launch: impl FnOnce(&Path, &Path, &Path, &Environment) -> KeeperLaunch,
        cleanup: impl FnOnce(&Path) -> io::Result<()>,
    ) -> io::Result<Job> {
        let service_claim = service.as_ref().map(|service| service.claim.clone());
        fs::create_dir_all(&self.root)?;
        let (id, directory) = self.claim()?;
        let request_path = directory.join(REQUEST);
        publish_json(
            &request_path,
            &Request {
                version: VERSION,
                id: id.clone(),
                command: job.command.to_owned(),
                argv: job.argv,
                cwd: job.cwd.to_path_buf(),
                environment: job.environment.clone(),
                timeout_millis: job
                    .timeout
                    .map(|timeout| u64::try_from(timeout.as_millis()))
                    .transpose()
                    .map_err(io::Error::other)?,
                notices: job.notices.to_vec(),
                service_claim: service_claim.clone(),
                lane: self.lane,
                events: job.events,
            },
        )?;
        if let Some(service) = service {
            let validation = service.validate().and_then(|()| {
                (!directory.join(STOP).exists())
                    .then_some(())
                    .ok_or_else(|| {
                        io::Error::new(
                            ErrorKind::PermissionDenied,
                            "the service task claim ended during launch",
                        )
                    })
            });
            if let Err(error) = validation {
                return Err(failed_start_error(&directory, error, cleanup));
            }
        }
        let keeper = match launch(&directory, &request_path, job.cwd, job.environment) {
            KeeperLaunch::Started(keeper) => keeper,
            KeeperLaunch::NotStarted(error) => {
                return Err(failed_start_error(&directory, error, cleanup));
            }
            KeeperLaunch::Unknown(error) => return Err(error),
        };
        Ok(Job {
            directory,
            id,
            command: job.command.to_owned(),
            cwd: job.cwd.to_path_buf(),
            timeout: job.timeout,
            service_claim,
            notices: job.notices.to_vec(),
            events: job.events,
            keeper: Some(keeper),
        })
    }

    /// A directory of this lane's for a new job, named by the microsecond
    /// key the system names everything with, so a listing is a timeline; a
    /// microsecond already taken yields the next.
    fn claim(&self) -> io::Result<(String, PathBuf)> {
        let mut micros = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_micros(),
        )
        .map_err(io::Error::other)?;
        loop {
            let id = Key::for_micros(micros)
                .map_err(io::Error::other)?
                .to_string();
            let directory = self.root.join(&id);
            match fs::create_dir(&directory) {
                Ok(()) => return Ok((id, directory)),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => micros += 1,
                Err(error) => return Err(error),
            }
        }
    }

    /// One job of this lane, by the identity the model quotes: a job exists
    /// from the moment its request is recorded, before its keeper speaks.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` for an identity that names no job.
    pub(in crate::resident) fn open(&self, id: &str) -> io::Result<Job> {
        let named = Key::shaped(id);
        let directory = self.root.join(id);
        let request: Request = named
            .then(|| read_json(&directory.join(REQUEST), REQUEST_BYTES))
            .transpose()
            .map_err(|error| match error.kind() {
                ErrorKind::NotFound => io::Error::new(ErrorKind::NotFound, format!("no job {id}")),
                _ => error,
            })?
            .ok_or_else(|| io::Error::new(ErrorKind::NotFound, format!("no job {id}")))?;
        let keeper = read_state(&directory)?.map(|state| state.keeper);
        Ok(Job {
            directory,
            id: request.id,
            command: request.command,
            cwd: request.cwd,
            timeout: request.timeout_millis.map(Duration::from_millis),
            service_claim: request.service_claim,
            notices: request.notices,
            events: request.events,
            keeper,
        })
    }

    /// The jobs whose ending nobody has heard, oldest first, without opening
    /// the record of any job already delivered: a lane's row is read far more
    /// often than a job ends.
    ///
    /// # Errors
    ///
    /// Returns the failure of reading the lane's job directory.
    pub(in crate::resident) fn kept(&self) -> io::Result<Vec<Job>> {
        let mut jobs = Vec::new();
        for id in self.ids(|directory| !directory.join(DELIVERED).exists())? {
            if let Some(job) = self.present(&id)? {
                jobs.push(job);
            }
        }
        Ok(jobs)
    }

    /// The names of delivered jobs, newest first, from `at` (or the newest when
    /// none is named) toward the oldest. Names cost a directory read; a
    /// history page opens only the records it shows.
    ///
    /// # Errors
    ///
    /// Returns the failure of reading the lane's job directory.
    pub(in crate::resident) fn delivered(&self, at: Option<&str>) -> io::Result<Vec<String>> {
        let mut ids = self.ids(|directory| directory.join(DELIVERED).exists())?;
        if let Some(at) = at {
            ids.truncate(ids.partition_point(|id| id.as_str() <= at));
        }
        ids.reverse();
        Ok(ids)
    }

    /// The names of the jobs whose directories are `wanted`, oldest first: the
    /// keys order themselves.
    fn ids(&self, wanted: impl Fn(&Path) -> bool) -> io::Result<Vec<String>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut ids = Vec::new();
        for entry in entries {
            let entry = entry?;
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if wanted(&entry.path()) {
                ids.push(id);
            }
        }
        ids.sort_unstable();
        Ok(ids)
    }

    /// The job named, or none when its directory is not a job's record.
    ///
    /// # Errors
    ///
    /// Returns the failure of reading a record that exists.
    pub(in crate::resident) fn present(&self, id: &str) -> io::Result<Option<Job>> {
        match self.open(id) {
            Ok(job) => Ok(Some(job)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The wall instant by which every unfinished deadline-bounded job of this
    /// lane has ended and had its ending delivered: the keeper's start plus the
    /// job's timeout, the keeper's terminate grace, and the delivery grace. A
    /// job whose keeper has not spoken counts from its recorded request, which
    /// precedes the keeper. Services and ended jobs bound nothing; `None` when
    /// no job remains or the lane's job records are unreadable.
    pub(in crate::resident) fn finite_horizon(&self) -> Option<wire::Instant> {
        let millis = self.finite_horizon_millis().ok()??;
        let at = jiff::Timestamp::from_millisecond(i64::try_from(millis).ok()?).ok()?;
        wire::Instant::new(at).ok()
    }

    fn finite_horizon_millis(&self) -> io::Result<Option<u64>> {
        let mut horizon = None;
        for job in self.kept()? {
            let Some(timeout) = job.timeout else {
                continue;
            };
            let started = match job.state()? {
                Some(state) if state.ending.is_some() => continue,
                Some(state) => state.started_millis,
                None => millis_since_epoch(fs::metadata(job.directory.join(REQUEST))?.modified()?)?,
            };
            let bound = Duration::from_millis(started)
                .saturating_add(timeout)
                .saturating_add(TERMINATE_GRACE)
                .saturating_add(DELIVERY_GRACE);
            let bound = u64::try_from(bound.as_millis()).unwrap_or(u64::MAX);
            horizon = Some(horizon.map_or(bound, |horizon: u64| horizon.max(bound)));
        }
        Ok(horizon)
    }
}

/// Stop every running service whose exact task possession is no longer active.
///
/// Finite jobs are lane-owned and never enter this reconciliation. A malformed
/// undelivered record is stopped fail-closed because it cannot prove a bound.
pub(in crate::resident) fn reconcile_services(
    paths: &RuntimePaths,
    snapshot: &bus::TaskSnapshot,
    diagnostic: &mut dyn io::Write,
) {
    let lanes = match fs::read_dir(&paths.supervisors) {
        Ok(lanes) => lanes,
        Err(error) if error.kind() == ErrorKind::NotFound => return,
        Err(error) => {
            let _ = writeln!(diagnostic, "service reconciliation unavailable: {error}");
            return;
        }
    };
    for lane in lanes {
        let Ok(lane) = lane else {
            continue;
        };
        let Some(name) = lane.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(lane) = name.parse::<CanonicalUuid>() else {
            continue;
        };
        Jobs::for_lane(paths, lane).reconcile_services(snapshot, diagnostic);
    }
}

impl Jobs {
    fn reconcile_services(&self, snapshot: &bus::TaskSnapshot, diagnostic: &mut dyn io::Write) {
        let ids = match self.ids(|directory| !directory.join(DELIVERED).exists()) {
            Ok(ids) => ids,
            Err(error) => {
                let _ = writeln!(diagnostic, "service records unavailable: {error}");
                return;
            }
        };
        for id in ids {
            let directory = self.root.join(&id);
            let job = match self.open(&id) {
                Ok(job) => job,
                Err(error) => {
                    let _ = File::create(directory.join(STOP));
                    let _ = writeln!(
                        diagnostic,
                        "stopping unbound job {id}: its durable request is invalid: {error}"
                    );
                    continue;
                }
            };
            if job.timeout.is_some()
                || job
                    .state()
                    .ok()
                    .flatten()
                    .is_some_and(|state| state.ending.is_some())
            {
                continue;
            }
            let owned = job
                .service_claim
                .as_ref()
                .is_some_and(|claim| snapshot.tasks().iter().any(|heads| claim.matches(heads)));
            if !owned {
                let stopped = job.stop();
                let _ = writeln!(
                    diagnostic,
                    "stopping service {} after its task claim ended{}",
                    job.id,
                    stopped
                        .err()
                        .map_or_else(String::new, |error| format!(": {error}"))
                );
            }
        }
    }
}

/// The state a job directory holds, with a keeper that vanished before
/// publishing an ending read as lost; `None` before the keeper has spoken.
fn read_state(directory: &Path) -> io::Result<Option<State>> {
    let mut state: State = match read_json(&directory.join(STATE), STATE_BYTES) {
        Ok(state) => state,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if state.ending.is_none()
        && state.keeper != std::process::id()
        && !process_exists_raw(state.keeper).unwrap_or(true)
    {
        state.ending = Some(Ending::Lost);
    }
    Ok(Some(state))
}

impl Job {
    pub(in crate::resident) fn id(&self) -> &str {
        &self.id
    }

    pub(in crate::resident) fn command(&self) -> &str {
        &self.command
    }

    /// How long the command may run before the keeper ends it, if bounded.
    pub(in crate::resident) fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    /// What the results carry ahead of the command's output.
    pub(in crate::resident) fn notices(&self) -> &[String] {
        &self.notices
    }

    /// Whether the command's stdout lines are published as events.
    pub(in crate::resident) fn events(&self) -> bool {
        self.events
    }

    /// The events published after the last the lane heard, oldest first.
    ///
    /// # Errors
    ///
    /// Returns the failure of reading the heard record or an event.
    pub(in crate::resident) fn unheard_events(&self) -> io::Result<Vec<(u64, Event)>> {
        events::after(&self.directory, events::heard(&self.directory)?)
    }

    /// Record that the lane heard every event through `sequence`.
    ///
    /// # Errors
    ///
    /// Returns the failure of publishing the heard record.
    pub(in crate::resident) fn hear_events(&self, sequence: u64) -> io::Result<()> {
        events::hear(&self.directory, sequence)
    }

    /// How the job ended, if it has.
    pub(in crate::resident) fn ended(&self) -> Option<Ended> {
        self.state().ok().flatten().and_then(|state| state.ended())
    }

    /// The job's published state, if the keeper has spoken.
    ///
    /// # Errors
    ///
    /// Returns the failure of reading the state document.
    pub(in crate::resident) fn state(&self) -> io::Result<Option<State>> {
        read_state(&self.directory)
    }
    fn kind(&self) -> wire::intent::LaneJobKind {
        use wire::intent::LaneJobKind;
        if self.events {
            LaneJobKind::Watch
        } else if self.timeout.is_none() {
            LaneJobKind::Service
        } else {
            LaneJobKind::Command
        }
    }

    fn phase(&self, state: Option<&State>) -> io::Result<wire::intent::LaneJobPhase> {
        use wire::intent::LaneJobPhase;
        Ok(if state.is_some_and(|state| state.ending.is_some()) {
            LaneJobPhase::Ended
        } else if self.directory.join(STOP).try_exists()? {
            LaneJobPhase::Stopping
        } else if state.is_some() {
            LaneJobPhase::Running
        } else {
            LaneJobPhase::Starting
        })
    }

    /// Metadata only; listing does not read logs or claim result delivery.
    pub(in crate::resident) fn listing(self) -> io::Result<wire::intent::KeptJob> {
        if !Key::shaped(&self.id) {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "job key is not native base52",
            ));
        }
        let state = self.state()?;
        Ok(wire::intent::KeptJob {
            kind: self.kind(),
            phase: self.phase(state.as_ref())?,
            started_millis: state.as_ref().map(|state| state.started_millis),
            id: self.id,
            command: self.command,
        })
    }

    /// Bounded operator evidence without claiming the agent's result delivery.
    pub(in crate::resident) fn inspect(
        self,
        target: wire::intent::LaneJobRef,
    ) -> io::Result<wire::intent::LaneJobAnswer> {
        let state = self.state()?;
        let ending = state
            .as_ref()
            .and_then(|state| state.ending.as_ref())
            .map(|ending| match ending {
                Ending::Exited { code } => format!("Exited {code}"),
                Ending::Signalled { signal } => format!("Signal {signal}"),
                Ending::TimedOut => "Deadline exceeded".to_owned(),
                Ending::Stopped => "Stopped by request".to_owned(),
                Ending::Failed { reason } => format!("Failed: {reason}"),
                Ending::Lost => "Keeper lost without an ending".to_owned(),
            });
        let output = self.tail()?;
        Ok(wire::intent::LaneJobAnswer {
            target,
            kind: self.kind(),
            phase: self.phase(state.as_ref())?,
            cwd: self.cwd.to_string_lossy().into_owned(),
            started_millis: state.as_ref().map(|state| state.started_millis),
            timeout_millis: self
                .timeout
                .map(|timeout| u64::try_from(timeout.as_millis()))
                .transpose()
                .map_err(io::Error::other)?,
            ended_millis: state.as_ref().and_then(|state| state.ended_millis),
            ending,
            command: self.command,
            output: output.text,
            output_truncated: output.truncated,
        })
    }

    /// Ask the keeper to end the command.
    ///
    /// # Errors
    ///
    /// Returns the failure of writing the stop request.
    pub(in crate::resident) fn stop(&self) -> io::Result<()> {
        File::create(self.directory.join(STOP)).map(drop)
    }

    /// Operator termination leaves the ending eligible for agent notification.
    pub(in crate::resident) fn stop_by_operator(&self) -> io::Result<()> {
        File::create(self.directory.join(OPERATOR_STOP))?;
        self.stop()
    }

    /// Claim the record that the lane has heard how the job ended: in the
    /// answer to the call that ran it, in a job answer that reported its
    /// ending, or as steering once it was kept. The record is created at most
    /// once, so of two tellers racing to it exactly one wins; the loser has
    /// nothing left to say.
    ///
    /// # Errors
    ///
    /// Returns the failure of creating the record other than its existence.
    pub(in crate::resident) fn claim_heard(&self) -> io::Result<bool> {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.directory.join(DELIVERED))
        {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Give back a claim whose telling never reached the lane, so the next
    /// seat's catch-up owes it again.
    ///
    /// # Errors
    ///
    /// Returns the failure of removing the record other than its absence.
    pub(in crate::resident) fn release_heard(&self) -> io::Result<()> {
        match fs::remove_file(self.directory.join(DELIVERED)) {
            Err(error) if error.kind() != ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Complete keeper-owned output, including bytes omitted from previews.
    pub(super) fn log_path(&self) -> PathBuf {
        self.directory.join(LOG)
    }

    /// The last of the log, bounded.
    ///
    /// # Errors
    ///
    /// Returns the failure of reading the log; a log not yet written is empty.
    pub(in crate::resident) fn tail(&self) -> io::Result<Tail> {
        let mut file = match File::open(self.log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Tail::default()),
            Err(error) => return Err(error),
        };
        let length = file.metadata()?.len();
        let limit = u64::try_from(LOG_TAIL).map_err(io::Error::other)?;
        let truncated = length > limit;
        if truncated {
            file.seek(SeekFrom::End(
                -i64::try_from(limit).map_err(io::Error::other)?,
            ))?;
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(limit)
            .read_to_end(&mut bytes)?;
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        // Lossy decoding can expand invalid bytes. Keep the displayed UTF-8
        // body within the same byte budget without cutting a code point.
        let mut start = text.len().saturating_sub(LOG_TAIL);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        if start > 0 {
            text.drain(..start);
        }
        Ok(Tail {
            text,
            truncated: truncated || start > 0,
        })
    }

    /// Observe the job until it ends, the call is cancelled or interrupted,
    /// or `bound` elapses.
    ///
    /// # Errors
    ///
    /// Returns the failure of watching the job's state or of reading it.
    pub(in crate::resident) fn observe(
        &self,
        control: &Control,
        bound: Duration,
    ) -> io::Result<Observed> {
        self.observe_with(control, bound, &mut || {})
    }

    /// Observe the job as [`Job::observe`] does, calling `progress` once
    /// the watch is armed on every look at a running job and once more on
    /// seeing its ending: the keeper publishes every event before the
    /// ending, so the last call sees them all.
    ///
    /// # Errors
    ///
    /// Returns the failure of watching the job's state or of reading it.
    pub(in crate::resident) fn observe_with(
        &self,
        control: &Control,
        bound: Duration,
        progress: &mut dyn FnMut(),
    ) -> io::Result<Observed> {
        let mut keeper = self.keeper;
        let deadline = Monotonic::now() + bound;
        let mut observed_exit = false;
        loop {
            watch(&self.directory, &control.sender, Signal::woke)?;
            if let Some(state) = self.state()? {
                if let Some(ended) = state.ended() {
                    progress();
                    return Ok(Observed::Ended(ended));
                }
                keeper = keeper.or(Some(state.keeper));
            }
            progress();
            if let (Some(pid), false) = (keeper, observed_exit) {
                observed_exit = true;
                if pid != std::process::id() {
                    observer::wait_for_exit(pid, control.sender.clone());
                }
            }
            let remaining = deadline.saturating_duration_since(Monotonic::now());
            match control.receiver.recv_timeout(remaining) {
                Ok(Signal::Changed) => {}
                Ok(Signal::KeeperExited) => {
                    progress();
                    if let Some(ended) = self.state()?.and_then(|state| state.ended()) {
                        return Ok(Observed::Ended(ended));
                    }
                    return Ok(Observed::Ended(Ended {
                        ending: Ending::Lost,
                        wall: None,
                    }));
                }
                Ok(Signal::ObserverFailed(reason)) => return Err(io::Error::other(reason)),
                Ok(Signal::Cancel) => return Ok(Observed::Cancelled),
                Ok(Signal::Interrupt) => return Ok(Observed::Interrupted),
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(Observed::Elapsed),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(
                        ErrorKind::BrokenPipe,
                        "job observer disconnected",
                    ));
                }
            }
        }
    }
}

/// Job records a test writes without a keeper: this process stands as the
/// keeper, so a test says what the command printed and how it ended.
#[cfg(test)]
impl Jobs {
    /// Record one job as its call declared it; `None` timeout is a service.
    pub(in crate::resident) fn record_for_tests(
        &self,
        command: &str,
        events: bool,
        timeout_millis: Option<u64>,
    ) -> Job {
        fs::create_dir_all(&self.root).expect("job root");
        let (id, directory) = self.claim().expect("job directory");
        publish_json(
            &directory.join(REQUEST),
            &Request {
                version: VERSION,
                id: id.clone(),
                command: command.to_owned(),
                argv: Vec::new(),
                cwd: directory.clone(),
                environment: Environment::empty(),
                timeout_millis,
                notices: Vec::new(),
                service_claim: None,
                lane: self.lane,
                events,
            },
        )
        .expect("job request");
        publish_json(
            &directory.join(STATE),
            &State {
                version: VERSION,
                id: id.clone(),
                command: command.to_owned(),
                keeper: std::process::id(),
                started_millis: now_millis().expect("clock"),
                timeout_millis,
                ending: None,
                ended_millis: None,
            },
        )
        .expect("job state");
        self.open(&id).expect("recorded job opens")
    }
}

#[cfg(test)]
impl Job {
    /// Publish event `sequence` as the keeper's pump would.
    pub(in crate::resident) fn print_for_tests(&self, sequence: u64, event: &Event) {
        publish_json(&events::record(&self.directory, sequence), event).expect("event");
    }

    /// Publish the job's ending as the keeper would.
    pub(in crate::resident) fn end_for_tests(&self, ending: Ending) {
        let mut state = self.state().expect("state reads").expect("state");
        state.ending = Some(ending);
        state.ended_millis = Some(now_millis().expect("clock"));
        publish_json(&self.directory.join(STATE), &state).expect("ending");
    }
}
