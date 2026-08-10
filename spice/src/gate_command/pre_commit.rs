//! One bounded process boundary around the canonical commit battery.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use gate::{ArmFamily, BatteryReport, GateId, RepositoryExecutionEvent, Selector};
use serde::{Deserialize, Serialize};
use sill::{
    Argv, Cancellation, Completion, Deadlines, Environment, Output, ProcessGroup, Redaction,
    RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitOutcome, WaitPolicy,
};
use wire::{Fault, FaultClass, Outcome, Refusal};

const WORKER_ARGUMENT: &str = "__gate-pre-commit-worker";
const REPORT_RECORD: u8 = b'R';
const FAILURE_RECORD: u8 = b'F';
const HARD_DEADLINE: Duration = Duration::from_secs(180);
const WORK_RUNTIME: Duration = Duration::from_secs(135);
const TERMINATION_GRACE: Duration = Duration::from_secs(5);
const REAP_DEADLINE: Duration = Duration::from_secs(5);
const OUTPUT_DRAIN: Duration = Duration::from_secs(5);
const OUTPUT_LIMIT: usize = 1024 * 1024;

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
    fn from_event(event: RepositoryExecutionEvent) -> Option<Self> {
        match event {
            RepositoryExecutionEvent::ArmInvoked {
                position,
                gate_id,
                family,
            } => Some(Self::Arm {
                ordinal: position + 1,
                gate_id,
                family,
            }),
            RepositoryExecutionEvent::FamilyInvoked { family } => Some(Self::Family { family }),
            // Complete-family acquisition announces its analyses as a batch.
            // Those records are inventory, not a cursor, so they cannot name
            // which analysis owns the wall clock when a deadline lands.
            RepositoryExecutionEvent::AnalysisInvoked { .. } => None,
        }
    }

    fn render(&self) -> String {
        match self {
            Self::Capture => "capture".to_owned(),
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
    armed: bool,
}

impl WorkerFiles {
    fn create() -> io::Result<Self> {
        let root = env::temp_dir().join(format!(
            "spice-commit-gate-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        fs::create_dir(&root)?;
        let progress = root.join("progress.jsonl");
        let terminal = root.join("terminal.bin");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&progress)?;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&terminal)?;
        Ok(Self {
            root,
            progress,
            terminal,
            armed: true,
        })
    }

    fn cleanup(mut self) -> io::Result<()> {
        fs::remove_dir_all(&self.root)?;
        self.armed = false;
        Ok(())
    }
}

impl Drop for WorkerFiles {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

pub(super) fn run() -> Outcome<BatteryReport> {
    let files = match WorkerFiles::create() {
        Ok(files) => files,
        Err(error) => return fault("gate::pre-commit-supervisor-unavailable", error),
    };
    let answer = supervise(&files);
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

fn supervise(files: &WorkerFiles) -> Outcome<BatteryReport> {
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

fn classify(files: &WorkerFiles, waited: &WaitOutcome) -> Outcome<BatteryReport> {
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
        Completion::Exited(_) => terminal(&files.terminal, &files.progress),
    }
}

fn terminal(path: &Path, progress: &Path) -> Outcome<BatteryReport> {
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
        REPORT_RECORD => match BatteryReport::decode(document) {
            Ok(report) => super::pre_commit_report(Ok(report)),
            Err(error) => worker_lost(
                progress,
                format!("worker published an invalid battery report: {error}"),
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

fn worker(progress: &Path, terminal: &Path) -> io::Result<()> {
    let _containment = gate::measure::inherit_process_group().map_err(io::Error::other)?;
    let mut progress = OpenOptions::new().append(true).open(progress)?;
    write_phase(&mut progress, &WorkerPhase::Capture)?;
    let mut phase_failure = None;
    let result = spice::gate::run_traced(Selector::staged(), &mut |event| {
        let Some(phase) = WorkerPhase::from_event(event) else {
            return;
        };
        if phase_failure.is_none()
            && let Err(error) = write_phase(&mut progress, &phase)
        {
            phase_failure = Some(error);
        }
    });
    if let Some(error) = phase_failure {
        return write_failure(
            terminal,
            &WorkerFailure {
                reason: "gate::pre-commit-progress-unavailable".to_owned(),
                detail: error.to_string(),
                fault: true,
            },
        );
    }
    match result {
        Ok(report) => {
            let mut document = vec![REPORT_RECORD];
            document.extend(report.encode().map_err(io::Error::other)?);
            fs::write(terminal, document)
        }
        Err(error) => write_failure(
            terminal,
            &WorkerFailure {
                reason: error.reason().to_owned(),
                detail: error.detail().to_owned(),
                fault: error.is_fault(),
            },
        ),
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

fn worker_failure(failure: WorkerFailure) -> Outcome<BatteryReport> {
    if failure.fault {
        Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: failure.reason,
            detail: failure.detail,
        })
    } else {
        Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                failure.reason,
                None,
                Some("one complete canonical Rust battery capture".to_owned()),
                Some(failure.detail),
            )
            .with_detail(
                "repair the named capture input; the Rust battery is the sole execution path",
            ),
        ))
    }
}

fn deadline(progress: &Path) -> Outcome<BatteryReport> {
    let phase = last_phase(progress);
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            "gate::pre-commit-deadline-elapsed",
            Some("canonical-commit-gate".to_owned()),
            Some(format!(
                "one complete ordered battery and process-group cleanup inside {} seconds",
                HARD_DEADLINE.as_secs(),
            )),
            Some(format!(
                "phase={phase}; findings=incomplete; worker=timed-out; work_cutoff_seconds={}",
                WORK_RUNTIME.as_secs(),
            )),
        )
        .with_detail("repair or reduce the named gate phase, then retry the same plain git commit"),
    ))
}

fn worker_lost(progress: &Path, detail: String) -> Outcome<BatteryReport> {
    let phase = last_phase(progress);
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: "gate::pre-commit-worker-lost".to_owned(),
        detail: format!("phase={phase}; findings=incomplete; {detail}"),
    })
}

fn fault(reason: &str, error: impl std::fmt::Display) -> Outcome<BatteryReport> {
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
        .filter_map(|line| serde_json::from_str::<WorkerPhase>(&line).ok())
        .last()
        .map_or_else(|| "unavailable".to_owned(), |phase| phase.render())
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
mod tests {
    use super::*;
    use std::thread;
    use std::time::Instant;

    #[test]
    fn fixed_envelope_reserves_cleanup_inside_180_seconds() {
        let bounds = ProcessBounds::production();
        let envelope = bounds.runtime + bounds.grace + bounds.reap + bounds.output_drain;
        assert_eq!(envelope, Duration::from_secs(150));
        assert!(envelope < HARD_DEADLINE);
    }

    #[test]
    fn deadline_terminates_the_worker_group_and_names_its_last_phase() {
        let files = WorkerFiles::create().expect("create deadline worker files");
        let pid = files.root.join("descendant.pid");
        let script = concat!(
            "printf '%s\\n' '{\"phase\":\"family\",\"family\":\"toolchain\"}' >> \"$1\"; ",
            "sleep 30 & child=$!; printf '%s\\n' \"$child\" > \"$2\"; wait"
        );
        let bounds = ProcessBounds {
            runtime: Duration::from_millis(100),
            output_drain: Duration::from_millis(100),
            reap: Duration::from_millis(100),
            grace: Duration::from_millis(100),
        };
        let spec = worker_spec(
            "/bin/sh",
            [
                OsString::from("-c"),
                OsString::from(script),
                OsString::from("commit-gate-deadline-fixture"),
                files.progress.as_os_str().to_owned(),
                pid.as_os_str().to_owned(),
            ],
            bounds,
        )
        .expect("declare deadline worker");
        let started = Instant::now();
        let waited = sill::spawn(spec)
            .expect("launch deadline worker")
            .wait()
            .expect("terminate and reap deadline worker group");
        assert!(matches!(waited.completion(), Completion::TimedOut(_)));
        assert!(started.elapsed() < Duration::from_secs(2));

        let pid = fs::read_to_string(pid)
            .expect("read descendant pid")
            .trim()
            .parse::<u32>()
            .expect("descendant pid is numeric");
        for _ in 0..100 {
            if !process_exists(pid) {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!process_exists(pid), "deadline leaked descendant {pid}");

        let Outcome::Refusal(refusal) = deadline(&files.progress) else {
            panic!("deadline is a typed refusal");
        };
        assert_eq!(refusal.reason(), "gate::pre-commit-deadline-elapsed");
        let observed = refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed)
            .expect("deadline retains phase evidence");
        assert!(observed.contains("phase=family family=toolchain"));
        assert!(observed.contains("findings=incomplete"));
        files.cleanup().expect("remove deadline worker files");
    }

    #[test]
    fn abnormal_worker_exit_is_typed_and_retains_the_last_exact_phase() {
        let files = WorkerFiles::create().expect("create lost-worker files");
        let script = concat!(
            "printf '%s\\n' ",
            "'{\"phase\":\"arm\",\"ordinal\":12,\"gate_id\":\"rust-typecheck\",",
            "\"family\":\"toolchain\"}' >> \"$1\"; exit 17"
        );
        let spec = worker_spec(
            "/bin/sh",
            [
                OsString::from("-c"),
                OsString::from(script),
                OsString::from("commit-gate-lost-worker-fixture"),
                files.progress.as_os_str().to_owned(),
            ],
            ProcessBounds {
                runtime: Duration::from_secs(1),
                output_drain: Duration::from_millis(100),
                reap: Duration::from_millis(100),
                grace: Duration::from_millis(100),
            },
        )
        .expect("declare lost worker");
        let waited = sill::spawn(spec)
            .expect("launch lost worker")
            .wait()
            .expect("reap lost worker");

        let Outcome::Fault(fault) = classify(&files, &waited) else {
            panic!("worker loss is a typed fault");
        };
        assert_eq!(fault.reason, "gate::pre-commit-worker-lost");
        assert!(
            fault.detail.contains(concat!(
                "phase=arm ordinal=12 gate=rust-typecheck family=toolchain; ",
                "findings=incomplete"
            )),
            "worker loss omitted exact phase evidence: {}",
            fault.detail,
        );
        assert!(fault.detail.contains("worker exited Some(17)"));
        files.cleanup().expect("remove lost-worker files");
    }

    #[cfg(unix)]
    fn process_exists(pid: u32) -> bool {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: signal zero changes no process state and probes this exact pid.
        unsafe { libc::kill(pid, 0) == 0 }
    }
}
