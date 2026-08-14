//! One bounded process boundary around the ordinary staged-change gate.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufRead as _, BufReader, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use gate::{ArmFamily, GateId};
use serde::{Deserialize, Serialize};
use sill::{
    Acquisition, Argv, Cancellation, Completion, Deadlines, Environment, LockFile, LockName,
    Output, Possession, ProcessGroup, Redaction, RestartDisposition, Spec, StdinPolicy,
    StreamPolicy, WaitOutcome, WaitPolicy,
};
use spice::repository::Repository;
use wire::{Fault, FaultClass, Outcome, Refusal, RefusalDiagnostic};

#[expect(
    dead_code,
    reason = "the configured executor lands before the dependent packaged/configured composition task"
)]
mod configured;

const WORKER_ARGUMENT: &str = "__gate-pre-commit-worker";
const REPORT_RECORD: u8 = b'R';
const FAILURE_RECORD: u8 = b'F';
const HARD_DEADLINE: Duration = Duration::from_secs(180);
// The measured exhaustive self-authority path needs more than 150 seconds on
// this host. Preserve every five-second cleanup phase and one second of outer
// envelope slack while admitting that fail-closed path.
const WORK_RUNTIME: Duration = Duration::from_secs(164);
const TERMINATION_GRACE: Duration = Duration::from_secs(5);
const REAP_DEADLINE: Duration = Duration::from_secs(5);
const OUTPUT_DRAIN: Duration = Duration::from_secs(5);
const OUTPUT_LIMIT: usize = 1024 * 1024;
const DISPATCH_LOCK: &str = "pre-commit.lock";

const _: () = assert!(
    WORK_RUNTIME.as_secs()
        + TERMINATION_GRACE.as_secs()
        + REAP_DEADLINE.as_secs()
        + OUTPUT_DRAIN.as_secs()
        < HARD_DEADLINE.as_secs()
);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
enum WorkerPhase {
    Capture,
    TestSelection,
    Family {
        family: ArmFamily,
    },
    Arm {
        ordinal: usize,
        gate_id: GateId,
        family: ArmFamily,
    },
}

impl WorkerPhase {
    fn render(&self) -> String {
        match self {
            Self::Capture => "capture".to_owned(),
            Self::TestSelection => "test-selection".to_owned(),
            Self::Family { family } => format!("family family={}", family_name(*family)),
            Self::Arm {
                ordinal,
                gate_id,
                family,
            } => format!(
                "arm ordinal={ordinal} gate={} family={}",
                gate_id.as_str(),
                family_name(*family),
            ),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkerFailure {
    reason: String,
    detail: String,
    fault: bool,
}

struct WorkerFiles {
    root: PathBuf,
    progress: PathBuf,
    terminal: PathBuf,
    report: PathBuf,
    armed: bool,
}

impl WorkerFiles {
    fn create() -> io::Result<Self> {
        let temporary = fs::canonicalize(env::temp_dir())?;
        let stem = format!(
            "spice-commit-gate-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        );
        let root = temporary.join(&stem);
        DirBuilder::new().mode(0o700).create(&root)?;
        let progress = root.join("progress.jsonl");
        let terminal = root.join("terminal.bin");
        // The private worker directory is removed before the command returns.
        // A refusing report is its sibling so the path printed to the operator
        // remains readable after supervisor cleanup.
        let report = temporary.join(format!("{stem}-report.json"));
        let files = Self {
            root,
            progress,
            terminal,
            report,
            armed: true,
        };
        drop(create_private_file(&files.progress)?);
        drop(create_private_file(&files.terminal)?);
        Ok(files)
    }

    fn cleanup(mut self) -> io::Result<()> {
        fs::remove_dir_all(&self.root)?;
        self.armed = false;
        Ok(())
    }
}

fn create_private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

impl Drop for WorkerFiles {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "the pre-commit binary rim must reject a recursion marker before launching its worker"
)]
pub(super) fn run() -> Outcome<spice::change_gate::ChangeGateReport> {
    if let Some(source) = env::var_os(configured::SOURCE_VARIABLE) {
        return configured::recursion_refusal(&source);
    }
    let _dispatch = match DispatchGuard::acquire() {
        Ok(dispatch) => dispatch,
        Err(outcome) => return outcome,
    };
    run_with(supervise)
}

#[derive(Debug)]
struct DispatchGuard {
    _possession: Possession,
}

impl DispatchGuard {
    fn acquire<T>() -> Result<Self, Outcome<T>> {
        let current = env::current_dir()
            .map_err(|error| fault("gate::pre-commit-working-directory-unavailable", error))?;
        let repository = Repository::discover(&current)
            .map_err(|error| fault("gate::pre-commit-repository-unresolved", error))?;
        Self::acquire_in(&repository)
    }

    fn acquire_in<T>(repository: &Repository) -> Result<Self, Outcome<T>> {
        let state = repository
            .admit_state()
            .map_err(|error| fault("gate::pre-commit-state-unavailable", error))?;
        fs::create_dir_all(state.locks())
            .map_err(|error| fault("gate::pre-commit-lock-unavailable", error))?;
        let name = LockName::new("pre-commit").expect("pre-commit is a valid fixed lock name");
        let lock = LockFile::new(name, state.locks().join(DISPATCH_LOCK));
        match lock.try_acquire() {
            Ok(Acquisition::Acquired(possession)) => Ok(Self {
                _possession: possession,
            }),
            Ok(Acquisition::Contended) => Err(Outcome::Refusal(Refusal::evidence(
                "gate::pre-commit-dispatch-active",
                format!("lock={}", lock.path().display()),
                "wait for the active commit gate to finish; configured commands cannot recursively redispatch it",
            ))),
            Err(error) => Err(fault("gate::pre-commit-lock-unavailable", error)),
        }
    }
}

fn run_with(
    supervisor: impl FnOnce(&WorkerFiles) -> Outcome<spice::change_gate::ChangeGateReport>,
) -> Outcome<spice::change_gate::ChangeGateReport> {
    let files = match WorkerFiles::create() {
        Ok(files) => files,
        Err(error) => return fault("gate::pre-commit-supervisor-unavailable", error),
    };
    let answer = supervisor(&files);
    match files.cleanup() {
        Ok(()) => answer,
        Err(error) => fault("gate::pre-commit-cleanup-failed", error),
    }
}

pub(super) fn worker_entry(mut arguments: impl Iterator<Item = OsString>) -> Option<i32> {
    let _executable = arguments.next()?;
    if arguments.next().as_deref() != Some(OsStr::new(WORKER_ARGUMENT)) {
        return None;
    }
    let Some(progress) = arguments.next() else {
        eprintln!("commit-gate worker requires a progress path");
        return Some(1);
    };
    let Some(terminal) = arguments.next() else {
        eprintln!("commit-gate worker requires a terminal path");
        return Some(1);
    };
    if arguments.next().is_some() {
        eprintln!("commit-gate worker received trailing arguments");
        return Some(1);
    }
    Some(match worker(Path::new(&progress), Path::new(&terminal)) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("commit-gate worker failed to publish its terminal: {error}");
            1
        }
    })
}

fn supervise(files: &WorkerFiles) -> Outcome<spice::change_gate::ChangeGateReport> {
    let executable = match env::current_exe() {
        Ok(executable) => executable,
        Err(error) => return fault("gate::pre-commit-executable-unavailable", error),
    };
    let spec = match worker_spec(
        executable,
        [
            OsString::from(WORKER_ARGUMENT),
            files.progress.as_os_str().to_owned(),
            files.terminal.as_os_str().to_owned(),
        ],
        ProcessBounds::production(),
    ) {
        Ok(spec) => spec,
        Err(error) => return fault("gate::pre-commit-worker-invalid", error),
    };
    let running = match sill::spawn(spec) {
        Ok(running) => running,
        Err(error) => return fault("gate::pre-commit-worker-unavailable", error),
    };
    let waited = match running.wait() {
        Ok(waited) => waited,
        Err(error) => {
            return worker_lost(
                &files.progress,
                format!("worker wait or reap failed: {error}"),
            );
        }
    };
    classify(files, &waited)
}

fn classify(
    files: &WorkerFiles,
    waited: &WaitOutcome,
) -> Outcome<spice::change_gate::ChangeGateReport> {
    if !waited.outputs_drained() {
        return worker_lost(
            &files.progress,
            "worker output did not drain inside its reserved deadline".to_owned(),
        );
    }
    match waited.completion() {
        Completion::TimedOut(_) => deadline(&files.progress),
        Completion::Cancelled(_) => worker_lost(
            &files.progress,
            "worker was cancelled outside the commit deadline".to_owned(),
        ),
        Completion::Exited(status) if !status.success() => worker_lost(
            &files.progress,
            format!(
                "worker exited {:?}; stderr={}",
                status.code(),
                captured(waited.stderr()),
            ),
        ),
        Completion::Exited(_) => terminal(&files.terminal, &files.progress, &files.report),
    }
}

fn terminal(
    path: &Path,
    progress: &Path,
    report_artifact: &Path,
) -> Outcome<spice::change_gate::ChangeGateReport> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return worker_lost(
                progress,
                format!("terminal record could not be read: {error}"),
            );
        }
    };
    let Some((kind, document)) = bytes.split_first() else {
        return worker_lost(progress, "worker published no terminal record".to_owned());
    };
    match *kind {
        REPORT_RECORD => match spice::change_gate::ChangeGateReport::decode(document) {
            Ok(report) => super::pre_commit_report(Ok(report), report_artifact),
            Err(error) => worker_lost(
                progress,
                format!("worker published an invalid change-gate report: {error}"),
            ),
        },
        FAILURE_RECORD => match serde_json::from_slice::<WorkerFailure>(document) {
            Ok(failure) => worker_failure(failure),
            Err(error) => worker_lost(
                progress,
                format!("worker published an invalid failure record: {error}"),
            ),
        },
        other => worker_lost(
            progress,
            format!("worker published unknown terminal record {other}"),
        ),
    }
}

fn worker(progress_path: &Path, terminal: &Path) -> io::Result<()> {
    let _containment = gate::measure::inherit_process_group().map_err(io::Error::other)?;
    let mut progress = OpenOptions::new().append(true).open(progress_path)?;
    write_phase(&mut progress, &WorkerPhase::Capture)?;
    write_phase(&mut progress, &WorkerPhase::TestSelection)?;
    match spice::change_gate::run(&mut progress) {
        Ok(report) => {
            let mut document = vec![REPORT_RECORD];
            document.extend(report.encode().map_err(io::Error::other)?);
            fs::write(terminal, document)
        }
        Err(error) => write_failure(terminal, &change_gate_failure(error)),
    }
}

fn write_phase(file: &mut File, phase: &WorkerPhase) -> io::Result<()> {
    serde_json::to_writer(&mut *file, phase).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.flush()
}

fn write_failure(path: &Path, failure: &WorkerFailure) -> io::Result<()> {
    let mut document = vec![FAILURE_RECORD];
    document.extend(serde_json::to_vec(failure).map_err(io::Error::other)?);
    fs::write(path, document)
}

fn worker_failure(failure: WorkerFailure) -> Outcome<spice::change_gate::ChangeGateReport> {
    if failure.fault {
        Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: failure.reason,
            detail: failure.detail,
        })
    } else if matches!(
        failure.reason.as_str(),
        "gate::change-gate-tests-refused" | "gate::change-gate-test-coverage-unknown"
    ) {
        Outcome::Refusal(Refusal::evidence(
            failure.reason,
            failure.detail,
            "repair the named staged-source test selection or failing selected target",
        ))
    } else {
        let observed = failure.detail;
        Outcome::Refusal(Refusal::bare(
            RefusalDiagnostic::new(
                failure.reason,
                None,
                Some("one complete selected change-gate capture".to_owned()),
                Some(observed.clone()),
            )
            .with_detail(format!(
                concat!(
                    "{}\n",
                    "repair the named capture input; the selected change gate is the sole execution path"
                ),
                observed,
            )),
        ))
    }
}

fn change_gate_failure(error: spice::change_gate::ChangeGateError) -> WorkerFailure {
    WorkerFailure {
        reason: error.reason().to_owned(),
        detail: error.detail().to_owned(),
        fault: error.is_fault(),
    }
}

fn deadline(progress: &Path) -> Outcome<spice::change_gate::ChangeGateReport> {
    let phase = last_phase(progress);
    Outcome::Refusal(Refusal::bare(
        RefusalDiagnostic::new(
            "gate::pre-commit-deadline-elapsed",
            Some("canonical-commit-gate".to_owned()),
            Some(format!(
                "one complete selected change gate and process-group cleanup inside {} seconds",
                HARD_DEADLINE.as_secs(),
            )),
            Some(format!(
                "phase={phase}; findings=incomplete; worker=timed-out; work_cutoff_seconds={}",
                WORK_RUNTIME.as_secs(),
            )),
        )
        .with_detail(format!(
            "phase={phase}; repair or reduce this gate phase, then retry the same plain git commit"
        )),
    ))
}

fn worker_lost(progress: &Path, detail: String) -> Outcome<spice::change_gate::ChangeGateReport> {
    let phase = last_phase(progress);
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: "gate::pre-commit-worker-lost".to_owned(),
        detail: format!("phase={phase}; findings=incomplete; {detail}"),
    })
}

fn fault<T>(reason: &str, error: impl std::fmt::Display) -> Outcome<T> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: error.to_string(),
    })
}

fn last_phase(path: &Path) -> String {
    let Ok(file) = File::open(path) else {
        return "unavailable".to_owned();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| {
            serde_json::from_str::<WorkerPhase>(&line)
                .map(|phase| phase.render())
                .ok()
                .or_else(|| {
                    line.strip_prefix("[change-gate-policy] gate=")
                        .filter(|gate| GateId::BATTERY.iter().any(|known| known.as_str() == *gate))
                        .map(|gate| format!("arm gate={gate}"))
                })
                .or_else(|| {
                    line.strip_prefix("[rust-test] ")
                        .map(|phase| format!("selected-tests {phase}"))
                })
        })
        .last()
        .unwrap_or_else(|| "unavailable".to_owned())
}

fn captured(output: Option<&sill::CapturedOutput>) -> String {
    output.map_or_else(
        || "<none>".to_owned(),
        |output| String::from_utf8_lossy(output.bytes()).trim().to_owned(),
    )
}

const fn family_name(family: ArmFamily) -> &'static str {
    match family {
        ArmFamily::Core => "core",
        ArmFamily::Toolchain => "toolchain",
        ArmFamily::Metrics => "metrics",
        ArmFamily::Hygiene => "hygiene",
    }
}

#[derive(Clone, Copy)]
struct ProcessBounds {
    runtime: Duration,
    output_drain: Duration,
    reap: Duration,
    grace: Duration,
}

impl ProcessBounds {
    const fn production() -> Self {
        Self {
            runtime: WORK_RUNTIME,
            output_drain: OUTPUT_DRAIN,
            reap: REAP_DEADLINE,
            grace: TERMINATION_GRACE,
        }
    }
}

fn worker_spec(
    executable: impl Into<OsString>,
    arguments: impl IntoIterator<Item = OsString>,
    bounds: ProcessBounds,
) -> Result<Spec, String> {
    let capture = StreamPolicy::Capture {
        limit: OUTPUT_LIMIT,
    };
    Ok(Spec {
        argv: Argv::new(executable, arguments).map_err(|error| error.to_string())?,
        cwd: env::current_dir().map_err(|error| error.to_string())?,
        environment: ambient_environment().map_err(|error| error.to_string())?,
        group: ProcessGroup::New,
        deadlines: Deadlines::new(bounds.runtime, bounds.output_drain, bounds.reap)
            .map_err(|error| error.to_string())?,
        wait: WaitPolicy::new(
            "canonical commit gate worker",
            Cancellation::Never,
            RestartDisposition::Never,
        )
        .map_err(|error| error.to_string())?,
        output: Output::new(StdinPolicy::Null, capture, capture),
        redaction: Redaction::All,
        grace: bounds.grace,
    })
}

#[expect(
    clippy::disallowed_methods,
    reason = "the canonical commit-gate process rim captures the caller environment once"
)]
fn ambient_environment() -> Result<Environment, sill::SpecError> {
    Environment::new(env::vars_os())
}

#[cfg(test)]
#[path = "pre_commit/tests.rs"]
mod extended_tests;
