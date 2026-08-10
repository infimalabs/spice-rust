//! One bounded process boundary around the canonical commit battery.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufRead as _, BufReader, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use gate::{
    ArmFamily, BatteryReport, GateId, RepositoryExecutionEvent, Selector, TestSelectionReport,
    WORKSPACE_TEST_CONCURRENCY_CAP,
};
use serde::{Deserialize, Serialize};
use sill::{
    Argv, Cancellation, Completion, Deadlines, Environment, Output, ProcessGroup, Redaction,
    RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitOutcome, WaitPolicy,
};
use wire::{Fault, FaultClass, Outcome, Refusal, RefusalDiagnostic};

const WORKER_ARGUMENT: &str = "__gate-pre-commit-worker";
const REPORT_RECORD: u8 = b'R';
const FAILURE_RECORD: u8 = b'F';
const SELECTION_RECORD: u8 = b'S';
const HARD_DEADLINE: Duration = Duration::from_secs(180);
const WORK_RUNTIME: Duration = Duration::from_secs(135);
const TERMINATION_GRACE: Duration = Duration::from_secs(5);
const REAP_DEADLINE: Duration = Duration::from_secs(5);
const OUTPUT_DRAIN: Duration = Duration::from_secs(5);
const OUTPUT_LIMIT: usize = 1024 * 1024;
const AUTHORITY_SIZE: usize = 32;

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
            RepositoryExecutionEvent::AnalysisInvoked { .. }
            | RepositoryExecutionEvent::ToolchainAnalysisCompleted { .. } => None,
        }
    }

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
    armed: bool,
}

impl WorkerFiles {
    fn create() -> io::Result<Self> {
        let root = env::temp_dir().join(format!(
            "spice-commit-gate-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        DirBuilder::new().mode(0o700).create(&root)?;
        let progress = root.join("progress.jsonl");
        let terminal = root.join("terminal.bin");
        let files = Self {
            root,
            progress,
            terminal,
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

pub(super) fn run() -> Outcome<BatteryReport> {
    run_with(supervise)
}

struct CompilerPreparation {
    cargo_jobs: usize,
    receipt: Vec<u8>,
    authority: Vec<u8>,
    reusable_workspace: Option<gate::RustAnalysisWorkspace>,
}

fn prepare_compilers() -> Result<CompilerPreparation, WorkerFailure> {
    let cargo_jobs = std::thread::available_parallelism()
        .map_err(|error| WorkerFailure {
            reason: "gate::pre-commit-parallelism-unavailable".to_owned(),
            detail: error.to_string(),
            fault: true,
        })?
        .get();
    let authority = fresh_authority();
    let preparation =
        spice::gate::verifier_compiler_preparation(Selector::staged(), cargo_jobs, &authority)
            .map_err(run_error_failure)?;
    let (receipt, reusable_workspace) = preparation.into_parts();
    Ok(CompilerPreparation {
        cargo_jobs,
        receipt,
        authority,
        reusable_workspace,
    })
}

fn fresh_authority() -> Vec<u8> {
    let mut authority = Vec::with_capacity(AUTHORITY_SIZE);
    authority.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    authority.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    authority
}

fn run_with(
    supervisor: impl FnOnce(&WorkerFiles) -> Outcome<BatteryReport>,
) -> Outcome<BatteryReport> {
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
        SELECTION_RECORD => match TestSelectionReport::decode(document) {
            Ok(report) => selection_refusal(report),
            Err(error) => worker_lost(
                progress,
                format!("worker published an invalid test-selection report: {error}"),
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
    let phase_failure = Arc::new(Mutex::new(None::<String>));
    let trace_failure = Arc::clone(&phase_failure);
    let trace_path = progress_path.to_path_buf();
    let mut trace = move |event| {
        let Some(phase) = WorkerPhase::from_event(event) else {
            return;
        };
        let Ok(mut failure) = trace_failure.lock() else {
            return;
        };
        if failure.is_some() {
            return;
        }
        let result = OpenOptions::new()
            .append(true)
            .open(&trace_path)
            .and_then(|mut file| write_phase(&mut file, &phase));
        if let Err(error) = result {
            *failure = Some(error.to_string());
        }
    };
    let mut prepared = match spice::gate::prepare_traced(Selector::staged(), &mut trace) {
        Ok(prepared) => prepared,
        Err(error) => return write_failure(terminal, &run_error_failure(error)),
    };

    write_phase(&mut progress, &WorkerPhase::TestSelection)?;
    let test_input = SelectedTestInput::capture(&prepared)?;
    let compiler = thread::scope(|scope| {
        let compiler = scope.spawn(prepare_compilers);
        join_compiler(compiler.join())
    });
    let compiler = match compiler {
        Ok(compiler) => compiler,
        Err(error) => return write_failure(terminal, &error),
    };
    let (selection, result) = thread::scope(|scope| -> io::Result<_> {
        let selected = scope.spawn(|| test_input.execute(compiler.reusable_workspace.as_ref()));
        let result = prepared
            .admit_verifier_compiler_receipt(
                &compiler.receipt,
                compiler.cargo_jobs,
                &compiler.authority,
            )
            .and_then(|evidence| prepared.execute(&evidence))
            .map_err(run_error_failure);
        if !selected.is_finished() {
            write_phase(&mut progress, &WorkerPhase::TestSelection)?;
        }
        Ok((selected.join(), result))
    })?;
    publish_worker_result(&mut progress, terminal, &phase_failure, selection, result)
}

struct SelectedTestInput {
    selected_root: PathBuf,
    writable_root: PathBuf,
    source: wire::Digest,
    changes: Vec<String>,
    environment: Environment,
}

impl SelectedTestInput {
    fn capture(prepared: &spice::gate::PreparedBattery<'_>) -> io::Result<Self> {
        let repository =
            spice::repository::Repository::discover(Path::new(".")).map_err(io::Error::other)?;
        Ok(Self {
            selected_root: prepared.root().to_path_buf(),
            writable_root: repository.worktree_root().to_path_buf(),
            source: prepared.source().clone(),
            changes: prepared.changes().to_vec(),
            environment: ambient_environment().map_err(io::Error::other)?,
        })
    }

    fn execute(
        self,
        reusable_workspace: Option<&gate::RustAnalysisWorkspace>,
    ) -> (
        Result<TestSelectionReport, gate::WorkspaceTestError>,
        Vec<u8>,
    ) {
        let mut output = Vec::new();
        let roots = gate::WorkspaceTestRoots::new(&self.selected_root, &self.writable_root);
        let roots = reusable_workspace.map_or(roots, |workspace| roots.with_reusable(workspace));
        let report = gate::run_selected_workspace_tests_at(
            roots,
            self.source,
            &self.changes,
            self.environment,
            WORKSPACE_TEST_CONCURRENCY_CAP,
            &mut output,
        );
        (report, output)
    }
}

fn join_compiler(
    result: thread::Result<Result<CompilerPreparation, WorkerFailure>>,
) -> Result<CompilerPreparation, WorkerFailure> {
    result.unwrap_or_else(|_| {
        Err(WorkerFailure {
            reason: "gate::verifier-compiler-worker-lost".to_owned(),
            detail: "compiler preparation worker panicked".to_owned(),
            fault: true,
        })
    })
}

fn publish_worker_result(
    progress: &mut File,
    terminal: &Path,
    phase_failure: &Arc<Mutex<Option<String>>>,
    selection: thread::Result<(
        Result<TestSelectionReport, gate::WorkspaceTestError>,
        Vec<u8>,
    )>,
    result: Result<BatteryReport, WorkerFailure>,
) -> io::Result<()> {
    let (selection, selection_output) = match selection {
        Ok(selection) => selection,
        Err(_) => {
            return write_failure(
                terminal,
                &WorkerFailure {
                    reason: "gate::test-selection-worker-lost".to_owned(),
                    detail: "selected-test coordinator panicked".to_owned(),
                    fault: true,
                },
            );
        }
    };
    progress.write_all(&selection_output)?;
    progress.flush()?;
    let selection = match selection {
        Ok(report) => report,
        Err(error) => {
            return write_failure(
                terminal,
                &WorkerFailure {
                    reason: "gate::test-selection-refused".to_owned(),
                    detail: error.to_string(),
                    fault: false,
                },
            );
        }
    };
    let encoded_selection = selection.encode().map_err(io::Error::other)?;
    writeln!(
        progress,
        "[rust-test-selection-report] {}",
        String::from_utf8_lossy(&encoded_selection),
    )?;
    progress.flush()?;
    if !selection.admits() {
        let mut document = vec![SELECTION_RECORD];
        document.extend(encoded_selection);
        return fs::write(terminal, document);
    }
    let phase_failure = phase_failure
        .lock()
        .ok()
        .and_then(|failure| failure.clone());
    if let Some(error) = phase_failure {
        return write_failure(
            terminal,
            &WorkerFailure {
                reason: "gate::pre-commit-progress-unavailable".to_owned(),
                detail: error,
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
        Err(error) => write_failure(terminal, &error),
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
    } else if failure.reason == "gate::test-selection-refused" {
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
                Some("one complete canonical Rust battery capture".to_owned()),
                Some(observed.clone()),
            )
            .with_detail(format!(
                concat!(
                    "{}\n",
                    "repair the named capture input; the Rust battery is the sole execution path"
                ),
                observed,
            )),
        ))
    }
}

fn run_error_failure(error: spice::gate::RunError) -> WorkerFailure {
    WorkerFailure {
        reason: error.reason().to_owned(),
        detail: error.detail().to_owned(),
        fault: error.is_fault(),
    }
}

fn selection_refusal(report: TestSelectionReport) -> Outcome<BatteryReport> {
    let evidence = match report.encode().and_then(|bytes| {
        String::from_utf8(bytes).map_err(|error| {
            serde_json::Error::io(io::Error::new(io::ErrorKind::InvalidData, error))
        })
    }) {
        Ok(evidence) => evidence,
        Err(error) => {
            return fault("gate::test-selection-report-unavailable", error);
        }
    };
    Outcome::Refusal(Refusal::evidence(
        "gate::test-selection-coverage-unknown",
        evidence,
        concat!(
            "publish complete staged-source-to-test reach evidence before narrowing the ",
            "commit test set",
        ),
    ))
}

fn deadline(progress: &Path) -> Outcome<BatteryReport> {
    let phase = last_phase(progress);
    Outcome::Refusal(Refusal::bare(
        RefusalDiagnostic::new(
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
mod tests;
