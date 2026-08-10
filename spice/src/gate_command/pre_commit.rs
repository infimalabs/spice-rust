//! One bounded process boundary around the canonical commit battery.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
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
const RECEIPT_LIMIT: usize = 1024 * 1024;
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

enum SelectionExecution {
    Reuse(gate::RustAnalysisWorkspace),
    SelectedRoot,
}

type SelectionStart = Arc<(Mutex<Option<SelectionExecution>>, Condvar)>;

struct WorkerFiles {
    root: PathBuf,
    progress: PathBuf,
    terminal: PathBuf,
    receipt: PathBuf,
    authority: PathBuf,
    armed: bool,
}

impl WorkerFiles {
    fn create(receipt_document: &[u8], authority_document: &[u8]) -> io::Result<Self> {
        if receipt_document.len() > RECEIPT_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "verifier compiler receipt exceeds its bounded artifact limit",
            ));
        }
        if authority_document.len() != AUTHORITY_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "verifier compiler receipt authority has an invalid size",
            ));
        }
        let root = env::temp_dir().join(format!(
            "spice-commit-gate-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        DirBuilder::new().mode(0o700).create(&root)?;
        let progress = root.join("progress.jsonl");
        let terminal = root.join("terminal.bin");
        let receipt = root.join("compiler-receipt.json");
        let authority = root.join("compiler-authority.bin");
        let files = Self {
            root,
            progress,
            terminal,
            receipt,
            authority,
            armed: true,
        };
        drop(create_private_file(&files.progress)?);
        drop(create_private_file(&files.terminal)?);
        let mut receipt_file = create_private_file(&files.receipt)?;
        receipt_file.write_all(receipt_document)?;
        receipt_file.flush()?;
        let mut authority_file = create_private_file(&files.authority)?;
        authority_file.write_all(authority_document)?;
        authority_file.flush()?;
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
    run_with(prepare_compilers, supervise)
}

struct CompilerPreparation {
    cargo_jobs: usize,
    receipt: Vec<u8>,
    authority: Vec<u8>,
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
    let receipt =
        spice::gate::verifier_compiler_receipt(Selector::staged(), cargo_jobs, &authority)
            .map_err(run_error_failure)?;
    Ok(CompilerPreparation {
        cargo_jobs,
        receipt,
        authority,
    })
}

fn fresh_authority() -> Vec<u8> {
    let mut authority = Vec::with_capacity(AUTHORITY_SIZE);
    authority.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    authority.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    authority
}

fn run_with(
    compiler: impl FnOnce() -> Result<CompilerPreparation, WorkerFailure>,
    supervisor: impl FnOnce(&WorkerFiles, usize) -> Outcome<BatteryReport>,
) -> Outcome<BatteryReport> {
    let prepared = match compiler() {
        Ok(prepared) => prepared,
        Err(error) => return worker_failure(error),
    };
    let files = match WorkerFiles::create(&prepared.receipt, &prepared.authority) {
        Ok(files) => files,
        Err(error) => return fault("gate::pre-commit-supervisor-unavailable", error),
    };
    let answer = supervisor(&files, prepared.cargo_jobs);
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
    let Some(receipt) = arguments.next() else {
        eprintln!("commit-gate worker requires a compiler receipt path");
        return Some(1);
    };
    let Some(authority) = arguments.next() else {
        eprintln!("commit-gate worker requires a compiler authority path");
        return Some(1);
    };
    let Some(cargo_jobs) = arguments
        .next()
        .and_then(|argument| argument.to_string_lossy().parse::<usize>().ok())
    else {
        eprintln!("commit-gate worker requires a numeric Cargo job cap");
        return Some(1);
    };
    if arguments.next().is_some() {
        eprintln!("commit-gate worker received trailing arguments");
        return Some(1);
    }
    Some(
        match worker(
            Path::new(&progress),
            Path::new(&terminal),
            Path::new(&receipt),
            Path::new(&authority),
            cargo_jobs,
        ) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("commit-gate worker failed to publish its terminal: {error}");
                1
            }
        },
    )
}

fn supervise(files: &WorkerFiles, cargo_jobs: usize) -> Outcome<BatteryReport> {
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
            files.receipt.as_os_str().to_owned(),
            files.authority.as_os_str().to_owned(),
            OsString::from(cargo_jobs.to_string()),
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

fn worker(
    progress_path: &Path,
    terminal: &Path,
    receipt_path: &Path,
    authority_path: &Path,
    cargo_jobs: usize,
) -> io::Result<()> {
    let _containment = gate::measure::inherit_process_group().map_err(io::Error::other)?;
    let receipt = match read_receipt(receipt_path) {
        Ok(receipt) => receipt,
        Err(error) => {
            return write_failure(
                terminal,
                &WorkerFailure {
                    reason: "gate::verifier-receipt-unavailable".to_owned(),
                    detail: error.to_string(),
                    fault: true,
                },
            );
        }
    };
    let authority = match read_authority(authority_path) {
        Ok(authority) => authority,
        Err(error) => {
            return write_failure(
                terminal,
                &WorkerFailure {
                    reason: "gate::verifier-receipt-authority-unavailable".to_owned(),
                    detail: error.to_string(),
                    fault: true,
                },
            );
        }
    };
    fs::remove_file(receipt_path)?;
    fs::remove_file(authority_path)?;

    let mut progress = OpenOptions::new().append(true).open(progress_path)?;
    write_phase(&mut progress, &WorkerPhase::Capture)?;
    let phase_failure = Arc::new(Mutex::new(None::<String>));
    let trace_failure = Arc::clone(&phase_failure);
    let selection_start = Arc::new((Mutex::new(None), Condvar::new()));
    let trace_selection_start = Arc::clone(&selection_start);
    let trace_path = progress_path.to_path_buf();
    let mut trace = move |event| {
        if let RepositoryExecutionEvent::ToolchainAnalysisCompleted {
            gate_id: GateId::RustTypecheck,
            reusable_workspace: Some(workspace),
        } = &event
        {
            release_selection(&trace_selection_start, Some(workspace.clone()));
        }
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
        Err(error) => {
            return write_failure(
                terminal,
                &WorkerFailure {
                    reason: error.reason().to_owned(),
                    detail: error.detail().to_owned(),
                    fault: error.is_fault(),
                },
            );
        }
    };
    let evidence = match prepared.admit_verifier_compiler_receipt(&receipt, cargo_jobs, &authority)
    {
        Ok(evidence) => evidence,
        Err(error) => {
            return write_failure(
                terminal,
                &WorkerFailure {
                    reason: error.reason().to_owned(),
                    detail: error.detail().to_owned(),
                    fault: error.is_fault(),
                },
            );
        }
    };

    write_phase(&mut progress, &WorkerPhase::TestSelection)?;
    let repository =
        spice::repository::Repository::discover(Path::new(".")).map_err(io::Error::other)?;
    let environment = ambient_environment().map_err(io::Error::other)?;
    let selected_root = prepared.root().to_path_buf();
    let writable_root = repository.worktree_root().to_path_buf();
    let source = prepared.source().clone();
    let changes = prepared.changes().to_vec();
    release_selection(&selection_start, None);
    let (selection, result) = thread::scope(|scope| -> io::Result<_> {
        let selected_start = Arc::clone(&selection_start);
        let selected = scope.spawn(move || {
            let reusable_workspace = await_selection(&selected_start);
            let mut output = Vec::new();
            let roots = gate::WorkspaceTestRoots::new(&selected_root, &writable_root);
            let roots = match reusable_workspace.as_ref() {
                Some(workspace) => roots.with_reusable(workspace),
                None => roots,
            };
            let report = gate::run_selected_workspace_tests_at(
                roots,
                source,
                &changes,
                environment,
                WORKSPACE_TEST_CONCURRENCY_CAP,
                &mut output,
            );
            (report, output)
        });
        let result = prepared.execute(&evidence);
        release_selection(&selection_start, None);
        if !selected.is_finished() {
            write_phase(&mut progress, &WorkerPhase::TestSelection)?;
        }
        Ok((selected.join(), result))
    })?;
    publish_worker_result(&mut progress, terminal, &phase_failure, selection, result)
}

fn publish_worker_result(
    progress: &mut File,
    terminal: &Path,
    phase_failure: &Arc<Mutex<Option<String>>>,
    selection: thread::Result<(
        Result<TestSelectionReport, gate::WorkspaceTestError>,
        Vec<u8>,
    )>,
    result: Result<BatteryReport, spice::gate::RunError>,
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

fn read_receipt(path: &Path) -> io::Result<Vec<u8>> {
    let receipt = read_bounded(path, RECEIPT_LIMIT)?;
    if receipt.len() > RECEIPT_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "verifier compiler receipt exceeds its bounded artifact limit",
        ));
    }
    Ok(receipt)
}

fn read_authority(path: &Path) -> io::Result<Vec<u8>> {
    let authority = read_bounded(path, AUTHORITY_SIZE)?;
    if authority.len() != AUTHORITY_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "verifier compiler receipt authority has an invalid size",
        ));
    }
    Ok(authority)
}

fn read_bounded(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let read_limit = u64::try_from(limit)
        .map_err(io::Error::other)?
        .saturating_add(1);
    let mut document = Vec::with_capacity(limit.min(4096));
    File::open(path)?
        .take(read_limit)
        .read_to_end(&mut document)?;
    Ok(document)
}

fn release_selection(start: &SelectionStart, workspace: Option<gate::RustAnalysisWorkspace>) {
    let (selected, signal) = &**start;
    let mut selected = selected
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if selected.is_some() {
        return;
    }
    *selected = Some(workspace.map_or(SelectionExecution::SelectedRoot, SelectionExecution::Reuse));
    signal.notify_one();
}

fn await_selection(start: &SelectionStart) -> Option<gate::RustAnalysisWorkspace> {
    let (selected, signal) = &**start;
    let mut selected = selected
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    while selected.is_none() {
        selected = signal
            .wait(selected)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    match selected.as_ref() {
        Some(SelectionExecution::Reuse(workspace)) => Some(workspace.clone()),
        Some(SelectionExecution::SelectedRoot) => None,
        None => unreachable!("selection condition was observed ready"),
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
        Outcome::Refusal(Refusal::bare(
            RefusalDiagnostic::new(
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
mod tests {
    use super::*;
    use gate::CoverageCompleteness;
    use std::cell::Cell;
    use std::os::unix::fs::PermissionsExt as _;
    use std::thread;
    use std::time::Instant;

    const TEST_AUTHORITY: [u8; AUTHORITY_SIZE] = [0x5a; AUTHORITY_SIZE];

    #[test]
    fn fixed_envelope_reserves_cleanup_inside_180_seconds() {
        let bounds = ProcessBounds::production();
        let envelope = bounds.runtime + bounds.grace + bounds.reap + bounds.output_drain;
        assert_eq!(envelope, Duration::from_secs(150));
        assert!(envelope < HARD_DEADLINE);
    }

    #[test]
    fn compiler_preparation_and_receipt_publication_precede_supervision() {
        let phase = Cell::new(0);
        let answer = run_with(
            || {
                assert_eq!(phase.get(), 0);
                phase.set(1);
                Ok(CompilerPreparation {
                    cargo_jobs: 3,
                    receipt: b"source-bound compiler receipt".to_vec(),
                    authority: TEST_AUTHORITY.to_vec(),
                })
            },
            |files, cargo_jobs| {
                assert_eq!(phase.get(), 1);
                assert_eq!(cargo_jobs, 3);
                assert_eq!(
                    fs::read(&files.receipt).expect("read published compiler receipt"),
                    b"source-bound compiler receipt"
                );
                assert_eq!(
                    fs::read(&files.authority).expect("read receipt authority"),
                    TEST_AUTHORITY
                );
                phase.set(2);
                fault("gate::supervisor-fixture", "bounded supervisor entered")
            },
        );

        assert_eq!(phase.get(), 2);
        let Outcome::Fault(fault) = answer else {
            panic!("fixture supervisor returns its typed marker fault");
        };
        assert_eq!(fault.reason, "gate::supervisor-fixture");
    }

    #[test]
    fn compiler_receipt_artifact_is_private_and_size_bounded() {
        let document = b"source-bound compiler receipt";
        let files =
            WorkerFiles::create(document, &TEST_AUTHORITY).expect("create private receipt files");
        assert_eq!(
            fs::metadata(&files.root)
                .expect("receipt root metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for path in [
            &files.progress,
            &files.terminal,
            &files.receipt,
            &files.authority,
        ] {
            assert_eq!(
                fs::metadata(path)
                    .expect("worker artifact metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert_eq!(
            read_receipt(&files.receipt).expect("read receipt"),
            document
        );
        assert_eq!(
            read_authority(&files.authority).expect("read receipt authority"),
            TEST_AUTHORITY
        );
        fs::write(&files.receipt, vec![0; RECEIPT_LIMIT + 1])
            .expect("write oversized receipt fixture");
        assert!(read_receipt(&files.receipt).is_err());
        fs::write(&files.authority, [0; AUTHORITY_SIZE + 1])
            .expect("write oversized authority fixture");
        assert!(read_authority(&files.authority).is_err());
        files.cleanup().expect("remove private receipt files");

        let oversized = vec![0; RECEIPT_LIMIT + 1];
        assert!(WorkerFiles::create(&oversized, &TEST_AUTHORITY).is_err());
        assert!(WorkerFiles::create(document, &TEST_AUTHORITY[..AUTHORITY_SIZE - 1]).is_err());
    }

    #[test]
    fn unknown_staged_source_coverage_refuses_with_the_exact_selection_report() {
        let report = gate::TestEvidenceGraph::select_tests(
            wire::Digest::compute(wire::Domain::GateTree, b"unknown staged selection"),
            vec!["tools/outside.rs".to_owned()],
            Vec::new(),
            Vec::new(),
            CoverageCompleteness::unknown("changed Rust source is outside every workspace package")
                .expect("unknown reason"),
        )
        .expect("unknown selection remains typed");

        let outcome = selection_refusal(report.clone());
        let Outcome::Refusal(refusal) = &outcome else {
            panic!("unknown source-to-test coverage must refuse");
        };
        assert_eq!(refusal.reason(), "gate::test-selection-coverage-unknown");
        let rendering = crate::render::render_commit_gate(&outcome, crate::render::Form::Text)
            .expect("selection refusal renders")
            .expect("a selection refusal is not silent");
        assert_eq!(rendering.stream(), crate::render::Stream::Err);
        let evidence = std::str::from_utf8(rendering.document())
            .expect("selection evidence is UTF-8")
            .lines()
            .next()
            .expect("selection evidence precedes its summary");
        assert_eq!(
            TestSelectionReport::decode(evidence.as_bytes()).expect("decode refusal evidence"),
            report
        );
    }

    #[test]
    fn deadline_terminates_the_worker_group_and_names_its_last_phase() {
        let files = WorkerFiles::create(b"receipt fixture", &TEST_AUTHORITY)
            .expect("create deadline worker files");
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
            .and_then(RefusalDiagnostic::observed)
            .expect("deadline retains phase evidence");
        assert!(observed.contains("phase=family family=toolchain"));
        assert!(observed.contains("findings=incomplete"));
        files.cleanup().expect("remove deadline worker files");
    }

    #[test]
    fn abnormal_worker_exit_is_typed_and_retains_the_last_exact_phase() {
        let files = WorkerFiles::create(b"receipt fixture", &TEST_AUTHORITY)
            .expect("create lost-worker files");
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
