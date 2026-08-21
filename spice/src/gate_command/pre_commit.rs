//! One bounded process boundary around the ordinary staged-change gate.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufRead as _, BufReader, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gate::{ArmFamily, GateId};
use serde::{Deserialize, Serialize};
use sill::{
    Acquisition, Argv, Cancellation, Completion, Deadlines, Environment, LockFile, LockName,
    Output, Possession, ProcessGroup, Redaction, RestartDisposition, Spec, StdinPolicy,
    StreamPolicy, WaitOutcome, WaitPolicy,
};
use spice::repository::Repository;
use wire::{Digest, Fault, FaultClass, Outcome, Refusal, RefusalDiagnostic, Reply, ReplyPosition};

mod configured;
mod dispatch;
mod terminal;

use dispatch::{dispatch, dispatch_resolution};
#[cfg(test)]
use terminal::report_outcome;
use terminal::{classify, classify_resolution};

const WORKER_ARGUMENT: &str = "__gate-pre-commit-worker";
const RESOLUTION_WORKER_ARGUMENT: &str = "__gate-resolution-worker";
const RESOLUTION_SELECTION_WORKER_ARGUMENT: &str = "__gate-resolution-selection-worker";
const REPORT_RECORD: u8 = b'R';
const FAILURE_RECORD: u8 = b'F';
const HARD_DEADLINE: Duration = Duration::from_secs(180);
// Preserve every five-second cleanup phase and one second of outer envelope
// slack around the bounded selected and configured work.
const WORK_RUNTIME: Duration = Duration::from_secs(164);
const TERMINATION_GRACE: Duration = Duration::from_secs(5);
const REAP_DEADLINE: Duration = Duration::from_secs(5);
const OUTPUT_DRAIN: Duration = Duration::from_secs(5);
const OUTPUT_LIMIT: usize = 1024 * 1024;
const DISPATCH_LOCK: &str = "pre-commit.lock";
const APPLIANCE_GENERATION_FENCE_VERSION: u64 = 1;

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
    Policy,
    TestSelection,
    Configured {
        steps: usize,
    },
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
            Self::Policy => "policy".to_owned(),
            Self::TestSelection => "test-selection".to_owned(),
            Self::Configured { steps } => format!("configured steps={steps}"),
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

const PRE_COMMIT_REPORT_VERSION: u64 = 3;
const RESOLUTION_REPORT_VERSION: u64 = 3;

/// One complete packaged-plus-configured pre-commit battery result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Report {
    version: u64,
    source: Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    packaged: Option<spice::change_gate::ChangeGateReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    configured: Option<configured::ConfiguredReport>,
}

impl Report {
    fn new(
        source: Digest,
        packaged: Option<spice::change_gate::ChangeGateReport>,
        configured: Option<configured::ConfiguredReport>,
    ) -> Result<Self, String> {
        let report = Self {
            version: PRE_COMMIT_REPORT_VERSION,
            source,
            packaged,
            configured,
        };
        report.validate()?;
        Ok(report)
    }

    pub(crate) const fn packaged(&self) -> Option<&spice::change_gate::ChangeGateReport> {
        self.packaged.as_ref()
    }

    fn configured(&self) -> Option<&configured::ConfiguredReport> {
        self.configured.as_ref()
    }

    pub(crate) fn admits(&self) -> bool {
        self.packaged
            .as_ref()
            .is_none_or(spice::change_gate::ChangeGateReport::admits)
            && self
                .configured
                .as_ref()
                .is_none_or(configured::ConfiguredReport::admits)
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != PRE_COMMIT_REPORT_VERSION {
            return Err(format!(
                "unsupported pre-commit report version {}; expected {PRE_COMMIT_REPORT_VERSION}",
                self.version,
            ));
        }
        if self.packaged.is_none() && self.configured.is_none() {
            return Err("pre-commit report carries an empty effective battery".to_owned());
        }
        if self
            .packaged
            .as_ref()
            .is_some_and(|report| report.selection().source() != &self.source)
        {
            return Err("packaged pre-commit report names a different staged source".to_owned());
        }
        if self
            .configured
            .as_ref()
            .is_some_and(|report| report.source() != &self.source)
        {
            return Err("configured pre-commit report names a different staged source".to_owned());
        }
        Ok(())
    }

    fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, String> {
        let report: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        report.validate()?;
        Ok(report)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case", tag = "coverage")]
enum ResolutionPackagedReport {
    Selected { report: Vec<u8> },
}

impl ResolutionPackagedReport {
    fn selected(report: &spice::change_gate::ChangeGateReport) -> Result<Self, String> {
        Ok(Self::Selected {
            report: report.encode().map_err(|error| error.to_string())?,
        })
    }

    fn validate(&self, source: &Digest) -> Result<bool, String> {
        match self {
            Self::Selected { report } => {
                let report = spice::change_gate::ChangeGateReport::decode(report)
                    .map_err(|error| error.to_string())?;
                if report.selection().source() != source || report.tests().source() != source {
                    return Err(
                        "resolution packaged evidence names a different staged source".to_owned(),
                    );
                }
                Ok(report.admits())
            }
        }
    }

    fn refusal_evidence(&self) -> String {
        match self {
            Self::Selected { report } => spice::change_gate::ChangeGateReport::decode(report)
                .map_or_else(
                    |error| format!("invalid selected resolution evidence: {error}"),
                    |report| report.refusal_fold(),
                ),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResolutionReport {
    version: u64,
    source: Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    packaged: Option<ResolutionPackagedReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    configured: Option<configured::ConfiguredReport>,
}

impl ResolutionReport {
    fn new(
        source: Digest,
        packaged: Option<ResolutionPackagedReport>,
        configured: Option<configured::ConfiguredReport>,
    ) -> Result<Self, String> {
        let report = Self {
            version: RESOLUTION_REPORT_VERSION,
            source,
            packaged,
            configured,
        };
        report.validate()?;
        Ok(report)
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != RESOLUTION_REPORT_VERSION {
            return Err(format!(
                "unsupported resolution report version {}; expected {RESOLUTION_REPORT_VERSION}",
                self.version,
            ));
        }
        if self.packaged.is_none() && self.configured.is_none() {
            return Err("resolution report carries an empty effective battery".to_owned());
        }
        if let Some(packaged) = &self.packaged {
            let _ = packaged.validate(&self.source)?;
        }
        if self
            .configured
            .as_ref()
            .is_some_and(|report| report.source() != &self.source)
        {
            return Err(
                "resolution configured evidence names a different staged source".to_owned(),
            );
        }
        Ok(())
    }

    fn admits(&self) -> bool {
        self.packaged
            .as_ref()
            .is_none_or(|report| report.validate(&self.source) == Ok(true))
            && self
                .configured
                .as_ref()
                .is_none_or(configured::ConfiguredReport::admits)
    }

    fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    fn decode(bytes: &[u8]) -> Result<Self, String> {
        let report: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        report.validate()?;
        Ok(report)
    }

    fn refusal_evidence(&self) -> String {
        let mut evidence = self
            .packaged
            .as_ref()
            .map_or_else(String::new, ResolutionPackagedReport::refusal_evidence);
        if let Some(configured) = self.configured.as_ref().filter(|report| !report.admits()) {
            evidence.push_str(&configured.refusal_evidence());
        }
        evidence
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkerFailure {
    reason: String,
    detail: String,
    fault: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ApplianceGenerationFence {
    version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    appliance: Option<Digest>,
}

impl ApplianceGenerationFence {
    const fn unbound() -> Self {
        Self {
            version: APPLIANCE_GENERATION_FENCE_VERSION,
            appliance: None,
        }
    }

    const fn bound(appliance: Digest) -> Self {
        Self {
            version: APPLIANCE_GENERATION_FENCE_VERSION,
            appliance: Some(appliance),
        }
    }

    const fn expected(&self) -> Option<&Digest> {
        self.appliance.as_ref()
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != APPLIANCE_GENERATION_FENCE_VERSION {
            return Err(format!(
                "unsupported appliance generation fence version {}; expected {APPLIANCE_GENERATION_FENCE_VERSION}",
                self.version,
            ));
        }
        Ok(())
    }

    fn encode(&self) -> Result<OsString, String> {
        self.validate()?;
        let bytes = wire::encode(self).map_err(|error| error.to_string())?;
        String::from_utf8(bytes)
            .map(OsString::from)
            .map_err(|error| error.to_string())
    }

    fn decode(document: &OsStr) -> Result<Self, String> {
        let bytes = document.as_encoded_bytes();
        let fence: Self = wire::decode(bytes).map_err(|error| error.to_string())?;
        fence.validate()?;
        let canonical = wire::encode(&fence).map_err(|error| error.to_string())?;
        if canonical != bytes {
            return Err("appliance generation fence is not canonical".to_owned());
        }
        Ok(fence)
    }
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
pub(super) fn run() -> Outcome<Report> {
    if let Some(source) = env::var_os(configured::SOURCE_VARIABLE) {
        return configured::recursion_refusal(&source);
    }
    let generation = if env::var_os(spice::install::INSTALLED_PRE_COMMIT_ENVIRONMENT).is_some() {
        match capture_appliance_generation_fence() {
            Ok(generation) => generation,
            Err(failure) => return worker_failure(failure),
        }
    } else {
        ApplianceGenerationFence::unbound()
    };
    let _dispatch = match DispatchGuard::acquire() {
        Ok(dispatch) => dispatch,
        Err(outcome) => return outcome,
    };
    run_with(|files| supervise(files, &generation))
}

/// Run selected evidence for one allocator-owned merge resolution.
#[expect(
    clippy::disallowed_methods,
    reason = "the resolution binary rim rejects configured-command recursion before supervision"
)]
pub(super) fn run_resolution(repository: &Repository) -> Outcome<()> {
    if let Some(source) = env::var_os(configured::SOURCE_VARIABLE) {
        return configured::recursion_refusal(&source);
    }
    let _dispatch = match DispatchGuard::acquire_in(repository) {
        Ok(dispatch) => dispatch,
        Err(outcome) => return outcome,
    };
    run_with_resolution(|files| supervise_resolution(files, repository.worktree_root()))
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

fn run_with(supervisor: impl FnOnce(&WorkerFiles) -> Outcome<Report>) -> Outcome<Report> {
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

fn run_with_resolution(supervisor: impl FnOnce(&WorkerFiles) -> Outcome<()>) -> Outcome<()> {
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

#[expect(
    clippy::disallowed_methods,
    reason = "the hidden worker rim must reject configured-command recursion before reading caller paths"
)]
pub(super) fn worker_entry(mut arguments: impl Iterator<Item = OsString>) -> Option<i32> {
    let _executable = arguments.next()?;
    let worker = arguments.next()?;
    if worker.as_os_str() != OsStr::new(WORKER_ARGUMENT)
        && worker.as_os_str() != OsStr::new(RESOLUTION_WORKER_ARGUMENT)
        && worker.as_os_str() != OsStr::new(RESOLUTION_SELECTION_WORKER_ARGUMENT)
    {
        return None;
    }
    if worker.as_os_str() == OsStr::new(RESOLUTION_SELECTION_WORKER_ARGUMENT) {
        if arguments.next().is_some() {
            eprintln!("resolution selection worker received trailing arguments");
            return Some(1);
        }
        let outcome = resolution_selection_worker();
        crate::render::render(&outcome, crate::render::Form::Text)
            .expect("typed resolution selection outcome renders as text")
            .speak();
        return Some(match outcome {
            Outcome::Reply(_) => crate::REPLY_EXIT,
            Outcome::Refusal(_) => crate::REFUSAL_EXIT,
            Outcome::Fault(_) => crate::FAULT_EXIT,
        });
    }
    if let Some(source) = env::var_os(configured::SOURCE_VARIABLE) {
        let refusal: Outcome<()> = configured::recursion_refusal(&source);
        crate::render::render(&refusal, crate::render::Form::Text)
            .expect("typed recursion refusal renders as text")
            .speak();
        return Some(2);
    }
    let Some(progress) = arguments.next() else {
        eprintln!("commit-gate worker requires a progress path");
        return Some(1);
    };
    let Some(terminal) = arguments.next() else {
        eprintln!("commit-gate worker requires a terminal path");
        return Some(1);
    };
    let generation = (worker.as_os_str() == OsStr::new(WORKER_ARGUMENT))
        .then(|| arguments.next())
        .flatten();
    if worker.as_os_str() == OsStr::new(WORKER_ARGUMENT) && generation.is_none() {
        eprintln!("commit-gate worker requires an appliance generation fence");
        return Some(1);
    }
    if arguments.next().is_some() {
        eprintln!("commit-gate worker received trailing arguments");
        return Some(1);
    }
    let result = if worker.as_os_str() == OsStr::new(RESOLUTION_WORKER_ARGUMENT) {
        resolution_worker(Path::new(&progress), Path::new(&terminal))
    } else {
        worker_run(
            Path::new(&progress),
            Path::new(&terminal),
            generation
                .as_deref()
                .expect("the pre-commit worker requires its appliance generation fence"),
        )
    };
    Some(match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("commit-gate worker failed to publish its terminal: {error}");
            1
        }
    })
}

#[expect(
    clippy::disallowed_methods,
    reason = "the private configured resolution rim acquires the source and path authorities injected by its supervising worker"
)]
fn resolution_selection_worker() -> Outcome<()> {
    let expected_source = match env::var(configured::SOURCE_VARIABLE) {
        Ok(source) => source,
        Err(error) => {
            return fault(
                "gate::resolution-source-authority-missing",
                format!("{}: {error}", configured::SOURCE_VARIABLE),
            );
        }
    };
    let expected_paths = match env::var(configured::PATHS_VARIABLE)
        .map_err(|error| error.to_string())
        .and_then(|paths| {
            serde_json::from_str::<Vec<String>>(&paths).map_err(|error| error.to_string())
        }) {
        Ok(paths) => paths,
        Err(error) => {
            return fault(
                "gate::resolution-path-authority-invalid",
                format!("{}: {error}", configured::PATHS_VARIABLE),
            );
        }
    };
    // SAFETY: `worker_entry` runs before the binary creates any threads, this
    // private leaf never returns to ordinary command dispatch, and the copied
    // authorities above remain available for the terminal comparison. Removing
    // them here prevents nested workspace-test binaries from mistaking their
    // own commit-worker fixtures for configured-command recursion.
    unsafe {
        env::remove_var(configured::SOURCE_VARIABLE);
        env::remove_var(configured::PATHS_VARIABLE);
    }
    let mut progress = Vec::new();
    let result = spice::change_gate::run_resolution(&mut progress);
    match result {
        Ok(report)
            if report.selection().source().to_string() == expected_source
                && report.tests().changed_sources() == expected_paths
                && report.admits() =>
        {
            Outcome::Reply(Reply {
                position: ReplyPosition::Direct {},
                body: (),
            })
        }
        Ok(report)
            if report.selection().source().to_string() != expected_source
                || report.tests().changed_sources() != expected_paths =>
        {
            Outcome::Refusal(Refusal::evidence(
                "gate::resolution-configured-source-moved",
                format!(
                    "expected_source={expected_source}; observed_source={}; expected_paths={expected_paths:?}; observed_paths={:?}",
                    report.selection().source(),
                    report.tests().changed_sources(),
                ),
                "retry task completion after the staged source and path set stop moving",
            ))
        }
        Ok(report) => {
            progress.extend_from_slice(report.refusal_fold().as_bytes());
            Outcome::Refusal(Refusal::evidence(
                "gate::resolution-configured-regressed",
                String::from_utf8_lossy(&progress).into_owned(),
                "repair the selected source finding, then retry task completion",
            ))
        }
        Err(error) if error.is_fault() => Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: error.reason().to_owned(),
            detail: error.detail().to_owned(),
        }),
        Err(error) => Outcome::Refusal(Refusal::evidence(
            error.reason(),
            error.detail(),
            "repair the selected source finding, then retry task completion",
        )),
    }
}

fn supervise(files: &WorkerFiles, generation: &ApplianceGenerationFence) -> Outcome<Report> {
    let executable = match env::current_exe() {
        Ok(executable) => executable,
        Err(error) => return fault("gate::pre-commit-executable-unavailable", error),
    };
    let generation = match generation.encode() {
        Ok(generation) => generation,
        Err(error) => return fault("gate::pre-commit-worker-invalid", error),
    };
    let spec = match worker_spec(
        executable,
        [
            OsString::from(WORKER_ARGUMENT),
            files.progress.as_os_str().to_owned(),
            files.terminal.as_os_str().to_owned(),
            generation,
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

fn supervise_resolution(files: &WorkerFiles, root: &Path) -> Outcome<()> {
    let executable = match env::current_exe() {
        Ok(executable) => executable,
        Err(error) => return fault("gate::pre-commit-executable-unavailable", error),
    };
    let spec = match worker_spec_at(
        executable,
        [
            OsString::from(RESOLUTION_WORKER_ARGUMENT),
            files.progress.as_os_str().to_owned(),
            files.terminal.as_os_str().to_owned(),
        ],
        root,
        ProcessBounds::production(),
    ) {
        Ok(spec) => spec,
        Err(error) => return fault("gate::resolution-worker-invalid", error),
    };
    let running = match sill::spawn(spec) {
        Ok(running) => running,
        Err(error) => return fault("gate::resolution-worker-unavailable", error),
    };
    let waited = match running.wait() {
        Ok(waited) => waited,
        Err(error) => {
            return resolution_worker_lost(
                &files.progress,
                format!("resolution worker wait or reap failed: {error}"),
            );
        }
    };
    classify_resolution(files, &waited)
}

fn worker_run(progress_path: &Path, terminal: &Path, generation: &OsStr) -> io::Result<()> {
    let _containment = gate::measure::inherit_process_group().map_err(io::Error::other)?;
    let mut progress = OpenOptions::new().append(true).open(progress_path)?;
    let started = Instant::now();
    let result = ApplianceGenerationFence::decode(generation)
        .map_err(|detail| WorkerFailure {
            reason: "gate::pre-commit-generation-fence-invalid".to_owned(),
            detail,
            fault: true,
        })
        .and_then(|generation| {
            verify_appliance_generation_fence(&generation)?;
            write_phase(&mut progress, &WorkerPhase::Capture).map_err(worker_io_failure)?;
            dispatch(&mut progress, started)
        });
    match result {
        Ok(report) => {
            let mut document = vec![REPORT_RECORD];
            document.extend(report.encode().map_err(io::Error::other)?);
            fs::write(terminal, document)
        }
        Err(failure) => write_failure(terminal, &failure),
    }
}

fn capture_appliance_generation_fence() -> Result<ApplianceGenerationFence, WorkerFailure> {
    let current = env::current_dir().map_err(|error| WorkerFailure {
        reason: "gate::pre-commit-working-directory-unavailable".to_owned(),
        detail: error.to_string(),
        fault: true,
    })?;
    let repository = Repository::discover(&current).map_err(|error| WorkerFailure {
        reason: "gate::pre-commit-repository-unresolved".to_owned(),
        detail: error.to_string(),
        fault: true,
    })?;
    current_appliance(&repository, None).map(ApplianceGenerationFence::bound)
}

fn verify_appliance_generation_fence(
    generation: &ApplianceGenerationFence,
) -> Result<(), WorkerFailure> {
    generation.validate().map_err(|detail| WorkerFailure {
        reason: "gate::pre-commit-generation-fence-invalid".to_owned(),
        detail,
        fault: true,
    })?;
    let Some(_) = generation.expected() else {
        return Ok(());
    };
    let current = env::current_dir().map_err(|error| WorkerFailure {
        reason: "gate::pre-commit-working-directory-unavailable".to_owned(),
        detail: error.to_string(),
        fault: true,
    })?;
    let repository = Repository::discover(&current).map_err(|error| WorkerFailure {
        reason: "gate::pre-commit-repository-unresolved".to_owned(),
        detail: error.to_string(),
        fault: true,
    })?;
    current_appliance(&repository, generation.expected()).map(|_| ())
}

fn current_appliance(
    repository: &Repository,
    expected: Option<&Digest>,
) -> Result<Digest, WorkerFailure> {
    let executable = env::current_exe().map_err(|error| WorkerFailure {
        reason: "gate::pre-commit-executable-unavailable".to_owned(),
        detail: error.to_string(),
        fault: true,
    })?;
    let audit = spice::install::audit_appliance(repository, &executable).map_err(|error| {
        WorkerFailure {
            reason: "gate::pre-commit-appliance-audit-failed".to_owned(),
            detail: error.to_string(),
            fault: true,
        }
    })?;
    if let Some(observed) = audit.receipt().filter(|observed| {
        audit.is_current() && expected.is_none_or(|expected| *observed == expected)
    }) {
        return Ok(observed.clone());
    }
    let observed = audit.receipt();
    Err(appliance_generation_moved(
        expected,
        format!(
            "state={}; receipt={}; detail={}",
            audit.state().as_str(),
            observed.map_or("absent", Digest::as_str),
            audit.detail(),
        ),
    ))
}

fn appliance_generation_moved(
    appliance: Option<&Digest>,
    observed: impl std::fmt::Display,
) -> WorkerFailure {
    WorkerFailure {
        reason: "gate::pre-commit-appliance-generation-moved".to_owned(),
        detail: format!(
            "appliance={}; observed={observed}",
            appliance.map_or("unavailable", Digest::as_str),
        ),
        fault: false,
    }
}

fn resolution_worker(progress_path: &Path, terminal: &Path) -> io::Result<()> {
    let _containment = gate::measure::inherit_process_group().map_err(io::Error::other)?;
    let mut progress = OpenOptions::new().append(true).open(progress_path)?;
    let started = Instant::now();
    write_phase(&mut progress, &WorkerPhase::Capture)?;
    match dispatch_resolution(&mut progress, started) {
        Ok(report) => {
            let mut document = vec![REPORT_RECORD];
            document.extend(report.encode().map_err(io::Error::other)?);
            fs::write(terminal, document)
        }
        Err(failure) => write_failure(terminal, &failure),
    }
}

fn worker_io_failure(error: io::Error) -> WorkerFailure {
    WorkerFailure {
        reason: "gate::pre-commit-progress-unavailable".to_owned(),
        detail: error.to_string(),
        fault: true,
    }
}

fn configured_failure(error: configured::ConfiguredFailure) -> WorkerFailure {
    WorkerFailure {
        reason: error.reason().to_owned(),
        detail: error.detail().to_owned(),
        fault: error.is_fault(),
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

fn worker_failure(failure: WorkerFailure) -> Outcome<Report> {
    failure_outcome(failure)
}

fn resolution_worker_failure(failure: WorkerFailure) -> Outcome<()> {
    failure_outcome(failure)
}

fn failure_outcome<T>(failure: WorkerFailure) -> Outcome<T> {
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

fn deadline(progress: &Path) -> Outcome<Report> {
    deadline_outcome(progress)
}

fn resolution_deadline(progress: &Path) -> Outcome<()> {
    deadline_outcome(progress)
}

fn deadline_outcome<T>(progress: &Path) -> Outcome<T> {
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

fn worker_lost(progress: &Path, detail: String) -> Outcome<Report> {
    worker_lost_outcome(progress, detail)
}

fn resolution_worker_lost(progress: &Path, detail: String) -> Outcome<()> {
    worker_lost_outcome(progress, detail)
}

fn worker_lost_outcome<T>(progress: &Path, detail: String) -> Outcome<T> {
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
                    line.strip_prefix("[change-gate-phase] stage=")
                        .map(|phase| format!("change-gate {phase}"))
                })
                .or_else(|| {
                    line.strip_prefix("[change-gate-policy] gate=")
                        .and_then(|progress| progress.split_whitespace().next())
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
    let current = env::current_dir().map_err(|error| error.to_string())?;
    worker_spec_at(executable, arguments, &current, bounds)
}

fn worker_spec_at(
    executable: impl Into<OsString>,
    arguments: impl IntoIterator<Item = OsString>,
    root: &Path,
    bounds: ProcessBounds,
) -> Result<Spec, String> {
    let capture = StreamPolicy::Capture {
        limit: OUTPUT_LIMIT,
    };
    Ok(Spec {
        argv: Argv::new(executable, arguments).map_err(|error| error.to_string())?,
        cwd: root.to_path_buf(),
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
