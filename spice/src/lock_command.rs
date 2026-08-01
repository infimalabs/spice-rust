use std::ffi::OsString;
use std::fs;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::thread::JoinHandle;
use std::time::Duration;

use clap::ArgMatches;
use serde::{Deserialize, Serialize};
use sill::{
    Acquisition, Argv, Cancellation, Completion, Deadlines, Environment, LockFile, LockName,
    Output, Possession, ProcessGroup, Redaction, RestartDisposition, Spec, StdinPolicy,
    StreamPolicy, WaitPolicy,
};
use spice::repository::Repository;
use wire::{Fault, FaultClass, Instant, Outcome, Refusal, Reply, ReplyPosition};

const OUTPUT_DRAIN: Duration = Duration::from_secs(1);
const REAP_DEADLINE: Duration = Duration::from_secs(5);
const TERMINATION_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub(crate) enum RunOutcome {
    Child(ExitStatus),
    Outcome(Outcome<()>),
    Contended { outcome: Outcome<()>, code: i32 },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct LockReport {
    locks: Vec<LockObservation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct LockObservation {
    name: String,
    shard: u32,
    path: PathBuf,
    state: LockState,
    #[serde(skip_serializing_if = "Option::is_none")]
    holder: Option<LockHolder>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata_issue: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum LockState {
    Available,
    Held,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
struct LockHolder {
    pid: u32,
    cwd: PathBuf,
    started: Instant,
}

struct Context {
    repository: Repository,
    config: sill::LockConfig,
}

struct Target {
    name: LockName,
    shards: u32,
    selected: Option<u32>,
}

struct HeldLock {
    possession: Possession,
}

struct SignalRelay {
    handle: signal_hook::iterator::Handle,
    worker: Option<JoinHandle<()>>,
}

impl SignalRelay {
    fn install(token: sill::CancellationToken) -> io::Result<Self> {
        use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};

        let mut signals = signal_hook::iterator::Signals::new([SIGHUP, SIGINT, SIGTERM])?;
        let handle = signals.handle();
        let worker = std::thread::Builder::new()
            .name("spice-lock-signals".to_owned())
            .spawn(move || {
                for _signal in signals.forever() {
                    token.cancel();
                }
            })?;
        Ok(Self {
            handle,
            worker: Some(worker),
        })
    }
}

impl Drop for SignalRelay {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl HeldLock {
    fn publish(&self, holder: &LockHolder) -> io::Result<()> {
        let bytes = wire::encode(holder).map_err(io::Error::other)?;
        let mut file = self.possession.file();
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&bytes)?;
        file.sync_data()
    }

    fn clear(&self) -> io::Result<()> {
        let mut file = self.possession.file();
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.sync_data()
    }
}

impl Drop for HeldLock {
    fn drop(&mut self) {
        let _ = self.clear();
    }
}

pub(crate) fn status() -> Outcome<LockReport> {
    let context = match context() {
        Ok(context) => context,
        Err(outcome) => return map_outcome(outcome),
    };
    let root = lock_root(&context.repository);
    let mut locks = Vec::new();
    for (name, shards) in context.config.named() {
        for shard in 0..shards.get() {
            let path = lock_path(&root, name, shard);
            match inspect(name, shard, &path) {
                Ok(observation) => locks.push(observation),
                Err(outcome) => return map_outcome(outcome),
            }
        }
    }
    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body: LockReport { locks },
    })
}

pub(crate) fn run(matches: &ArgMatches) -> RunOutcome {
    let context = match context() {
        Ok(context) => context,
        Err(outcome) => return RunOutcome::Outcome(outcome),
    };
    let target = match target(matches, &context.config) {
        Ok(target) => target,
        Err(outcome) => return RunOutcome::Outcome(outcome),
    };
    let held = match hold(&context, &target) {
        Ok(held) => held,
        Err(outcome) => return outcome,
    };
    execute(matches, held)
}

fn target(matches: &ArgMatches, config: &sill::LockConfig) -> Result<Target, Outcome<()>> {
    let Some(name) = matches.get_one::<String>("NAME") else {
        return Err(refusal(
            "lock::name-missing",
            None,
            "supply one configured lower-case lock name",
        ));
    };
    let name = match LockName::new(name.clone()) {
        Ok(name) => name,
        Err(error) => {
            return Err(refusal(
                "lock::name-refused",
                Some(name.clone()),
                &error.to_string(),
            ));
        }
    };
    let Some(shards) = config.shards(&name) else {
        return Err(refusal(
            "lock::unknown",
            Some(name.as_str().to_owned()),
            "declare the lock and its shard count in tracked or local configuration",
        ));
    };
    let selected = selected_shard(matches, shards.get())?;
    Ok(Target {
        name,
        shards: shards.get(),
        selected,
    })
}

fn hold(context: &Context, target: &Target) -> Result<HeldLock, RunOutcome> {
    let root = lock_root(&context.repository);
    match acquire(&root, &target.name, target.shards, target.selected) {
        Ok(held) => Ok(held),
        Err(AcquireFailure::Outcome(outcome)) => Err(RunOutcome::Outcome(outcome)),
        Err(AcquireFailure::Contended { kind, path }) => {
            let code = match kind {
                Contention::Busy => context.config.exits().busy().get(),
                Contention::Taken => context.config.exits().taken().get(),
                Contention::Full => context.config.exits().full().get(),
            };
            Err(RunOutcome::Contended {
                outcome: refusal(
                    kind.reason(),
                    Some(path.display().to_string()),
                    kind.remedy(),
                ),
                code,
            })
        }
    }
}

fn execute(matches: &ArgMatches, held: HeldLock) -> RunOutcome {
    let current = match crate::working_directory() {
        Ok(current) => current,
        Err(outcome) => return RunOutcome::Outcome(outcome),
    };
    if let Err(outcome) = publish_holder(&held, &current) {
        return RunOutcome::Outcome(outcome);
    }
    let (running, relay) = match spawn_child(matches, current) {
        Ok(spawned) => spawned,
        Err(outcome) => return RunOutcome::Outcome(outcome),
    };
    let waited = match running.wait() {
        Ok(waited) => waited,
        Err(error) => {
            return RunOutcome::Outcome(environment_fault("lock::wait-failed", &error.to_string()));
        }
    };
    drop(relay);
    let status = match waited.completion() {
        Completion::Exited(status)
        | Completion::TimedOut(status)
        | Completion::Cancelled(status) => *status,
    };
    if let Err(error) = held.clear() {
        return RunOutcome::Outcome(environment_fault(
            "lock::holder-unclearable",
            &error.to_string(),
        ));
    }
    RunOutcome::Child(status)
}

fn publish_holder(held: &HeldLock, current: &Path) -> Result<(), Outcome<()>> {
    let started = Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("lock::clock-invalid", &error.to_string()))?;
    held.publish(&LockHolder {
        pid: std::process::id(),
        cwd: current.to_path_buf(),
        started,
    })
    .map_err(|error| environment_fault("lock::holder-unwritable", &error.to_string()))
}

fn spawn_child(
    matches: &ArgMatches,
    current: PathBuf,
) -> Result<(sill::Running, SignalRelay), Outcome<()>> {
    let command = command(matches)?;
    let environment = Environment::new(crate::bootstrap_environment_entries())
        .map_err(|error| environment_fault("lock::environment-invalid", &error.to_string()))?;
    let (program, arguments) = command
        .split_first()
        .expect("clap requires the lock run command operand");
    let cancellation = sill::CancellationToken::new();
    let relay = SignalRelay::install(cancellation.clone())
        .map_err(|error| environment_fault("lock::signals-unavailable", &error.to_string()))?;
    let spec = process_spec(program, arguments, current, environment, cancellation)
        .map_err(|error| environment_fault("lock::process-invalid", &error.to_string()))?;
    let running = sill::spawn(spec)
        .map_err(|error| environment_fault("lock::spawn-failed", &error.to_string()))?;
    Ok((running, relay))
}

fn context() -> Result<Context, Outcome<()>> {
    let current = crate::working_directory()?;
    let repository = Repository::discover(&current)
        .map_err(|error| environment_fault("lock::repository-unresolved", &error.to_string()))?;
    let resolved = spice::config::load_coexisting_repository(&repository, &[])
        .map_err(|error| configuration_refusal(&error))?;
    Ok(Context {
        repository,
        config: resolved.root().lock().clone(),
    })
}

fn lock_root(repository: &Repository) -> PathBuf {
    repository.common_dir().join(".spice/locks")
}

fn lock_path(root: &Path, name: &LockName, shard: u32) -> PathBuf {
    root.join(name.as_str()).join(shard.to_string())
}

fn inspect(name: &LockName, shard: u32, path: &Path) -> Result<LockObservation, Outcome<()>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(observation(
                name,
                shard,
                path,
                LockState::Available,
                None,
                None,
            ));
        }
        Err(error) => {
            return Err(environment_fault(
                "lock::metadata-unavailable",
                &format!("{}: {error}", path.display()),
            ));
        }
    };
    if !metadata.file_type().is_file() {
        return Err(environment_fault(
            "lock::target-invalid",
            &format!("{} is not a regular lock file", path.display()),
        ));
    }
    let file = LockFile::new(name.clone(), path);
    match file.try_acquire() {
        Ok(Acquisition::Acquired(possession)) => {
            drop(possession);
            Ok(observation(
                name,
                shard,
                path,
                LockState::Available,
                None,
                None,
            ))
        }
        Ok(Acquisition::Contended) => {
            let (holder, issue) = read_holder(path);
            Ok(observation(
                name,
                shard,
                path,
                LockState::Held,
                holder,
                issue,
            ))
        }
        Err(error) => Err(environment_fault(
            "lock::inspection-failed",
            &format!("{}: {error}", path.display()),
        )),
    }
}

fn observation(
    name: &LockName,
    shard: u32,
    path: &Path,
    state: LockState,
    holder: Option<LockHolder>,
    metadata_issue: Option<String>,
) -> LockObservation {
    LockObservation {
        name: name.as_str().to_owned(),
        shard,
        path: path.to_path_buf(),
        state,
        holder,
        metadata_issue,
    }
}

fn read_holder(path: &Path) -> (Option<LockHolder>, Option<String>) {
    match fs::read(path) {
        Ok(bytes) if bytes.is_empty() => (None, Some("holder metadata is empty".to_owned())),
        Ok(bytes) => match wire::decode(&bytes) {
            Ok(holder) => (Some(holder), None),
            Err(error) => (
                None,
                Some(format!("holder metadata does not decode: {error}")),
            ),
        },
        Err(error) => (
            None,
            Some(format!("holder metadata is unreadable: {error}")),
        ),
    }
}

fn selected_shard(matches: &ArgMatches, count: u32) -> Result<Option<u32>, Outcome<()>> {
    let Some(value) = matches.get_one::<String>("shard") else {
        return Ok(None);
    };
    let shard = value.parse::<u32>().map_err(|error| {
        refusal(
            "lock::shard-refused",
            Some(value.clone()),
            &format!("supply a zero-based integer shard: {error}"),
        )
    })?;
    if shard >= count {
        return Err(refusal(
            "lock::shard-refused",
            Some(shard.to_string()),
            &format!("supply a shard below the configured count {count}"),
        ));
    }
    Ok(Some(shard))
}

enum AcquireFailure {
    Outcome(Outcome<()>),
    Contended { kind: Contention, path: PathBuf },
}

#[derive(Clone, Copy)]
enum Contention {
    Busy,
    Taken,
    Full,
}

impl Contention {
    const fn reason(self) -> &'static str {
        match self {
            Self::Busy => "lock::busy",
            Self::Taken => "lock::taken",
            Self::Full => "lock::full",
        }
    }

    const fn remedy(self) -> &'static str {
        match self {
            Self::Busy => "retry after the named lock holder exits",
            Self::Taken => "select another shard or retry after its holder exits",
            Self::Full => "retry after one configured pool shard becomes available",
        }
    }
}

fn acquire(
    root: &Path,
    name: &LockName,
    shards: u32,
    selected: Option<u32>,
) -> Result<HeldLock, AcquireFailure> {
    let directory = root.join(name.as_str());
    fs::create_dir_all(&directory).map_err(|error| {
        AcquireFailure::Outcome(environment_fault(
            "lock::directory-unavailable",
            &format!("{}: {error}", directory.display()),
        ))
    })?;
    if let Some(shard) = selected {
        let path = lock_path(root, name, shard);
        return acquire_one(name, &path)?.ok_or(AcquireFailure::Contended {
            kind: Contention::Taken,
            path,
        });
    }
    if shards == 1 {
        let path = lock_path(root, name, 0);
        return acquire_one(name, &path)?.ok_or(AcquireFailure::Contended {
            kind: Contention::Busy,
            path,
        });
    }
    for shard in 0..shards {
        let path = lock_path(root, name, shard);
        if let Some(held) = acquire_one(name, &path)? {
            return Ok(held);
        }
    }
    Err(AcquireFailure::Contended {
        kind: Contention::Full,
        path: directory,
    })
}

fn acquire_one(name: &LockName, path: &Path) -> Result<Option<HeldLock>, AcquireFailure> {
    let lock = LockFile::new(name.clone(), path);
    match lock.try_acquire() {
        Ok(Acquisition::Acquired(possession)) => Ok(Some(HeldLock { possession })),
        Ok(Acquisition::Contended) => Ok(None),
        Err(error) => Err(AcquireFailure::Outcome(environment_fault(
            "lock::acquisition-failed",
            &format!("{}: {error}", path.display()),
        ))),
    }
}

fn command(matches: &ArgMatches) -> Result<Vec<OsString>, Outcome<()>> {
    matches
        .get_many::<OsString>("CMD")
        .map(|values| values.cloned().collect())
        .filter(|values: &Vec<_>| !values.is_empty())
        .ok_or_else(|| {
            refusal(
                "lock::command-missing",
                None,
                "supply a child command after --",
            )
        })
}

fn process_spec(
    program: &OsString,
    arguments: &[OsString],
    cwd: PathBuf,
    environment: Environment,
    cancellation: sill::CancellationToken,
) -> Result<Spec, sill::SpecError> {
    Ok(Spec {
        argv: Argv::new(program, arguments.iter().cloned())?,
        cwd,
        environment,
        group: ProcessGroup::New,
        deadlines: Deadlines::new(Duration::MAX, OUTPUT_DRAIN, REAP_DEADLINE)?,
        wait: WaitPolicy::new(
            "spice lock run child",
            Cancellation::Token(cancellation),
            RestartDisposition::Never,
        )?,
        output: Output::new(
            StdinPolicy::Inherit,
            StreamPolicy::Inherit,
            StreamPolicy::Inherit,
        ),
        redaction: Redaction::None,
        grace: TERMINATION_GRACE,
    })
}

fn configuration_refusal(error: &spice::config::LoadError) -> Outcome<()> {
    let mut details = Vec::new();
    if let Some(repository) = error.repository() {
        details.push(repository.to_string());
    }
    details.extend(error.acquisition().iter().map(ToString::to_string));
    if let Some(resolution) = error.resolution() {
        details.extend(resolution.issues().iter().map(ToString::to_string));
    }
    refusal(
        "lock::configuration-refused",
        (!details.is_empty()).then(|| details.join(" | ")),
        "repair the named tracked or local configuration issue and retry",
    )
}

fn map_outcome<B>(outcome: Outcome<()>) -> Outcome<B> {
    match outcome {
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
        Outcome::Reply(_) => unreachable!("error paths cannot carry replies"),
    }
}

fn refusal(reason: &str, observed: Option<String>, remedy: &str) -> Outcome<()> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: None,
        expected: None,
        observed,
        remedy: remedy.to_owned(),
    })
}

fn environment_fault(reason: &str, detail: &str) -> Outcome<()> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}
