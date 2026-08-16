//! Ordered configured-command execution under one staged and policy authority.

use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sill::{
    Argv, Cancellation, Completion, Deadlines, Environment, Output, ProcessGroup, Redaction,
    RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitOutcome, WaitPolicy,
};
use spice::config::{PreCommitStep, pre_commit_declarations};
use spice::repository::Repository;
use wire::{ConfigSource, Digest, Outcome, Refusal, RefusalDiagnostic, TrustState};

use crate::trust_command::Surface;

pub(super) const PATHS_VARIABLE: &str = wire::SpiceVariable::PreCommitPaths.as_str();
pub(super) const SOURCE_VARIABLE: &str = wire::SpiceVariable::PreCommitSource.as_str();

const OUTPUT_LIMIT: usize = 64 * 1024;
const OUTPUT_DRAIN: Duration = Duration::from_secs(2);
const REAP_DEADLINE: Duration = Duration::from_secs(2);
const TERMINATION_GRACE: Duration = Duration::from_secs(2);

const SOURCE_GATE_ARGV: [&str; 11] = [
    "cargo",
    "run",
    "--quiet",
    "--locked",
    "--package",
    "spice",
    "--bin",
    "spice",
    "--",
    "gate",
    "selection",
];

const GIT_LOCAL_ENVIRONMENT: [&str; 15] = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

/// One ordered configured-command result bound to an immutable staged source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConfiguredReport {
    source: Digest,
    steps: Vec<StepReport>,
}

#[derive(Clone, Copy)]
enum ExecutionMode {
    Ordinary,
    Resolution,
}

impl ConfiguredReport {
    pub(super) fn admits(&self) -> bool {
        self.steps.iter().all(|step| step.findings.is_empty())
    }

    pub(super) fn source(&self) -> &Digest {
        &self.source
    }

    #[cfg(test)]
    pub(super) fn steps(&self) -> &[StepReport] {
        &self.steps
    }

    pub(super) fn refusal_evidence(&self) -> String {
        let mut evidence = String::new();
        for (ordinal, step) in self.steps.iter().enumerate() {
            for finding in &step.findings {
                let _ = writeln!(
                    evidence,
                    "ordinal={ordinal} label={} argv={} kind={} detail={}",
                    json(&step.label),
                    json(&step.argv),
                    finding.kind.as_str(),
                    json(&finding.detail),
                );
            }
        }
        evidence
    }
}

/// Execution evidence for one authored declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StepReport {
    label: String,
    argv: Vec<String>,
    selected_paths: Vec<String>,
    enabled: bool,
    findings: Vec<StepFinding>,
}

#[cfg(test)]
impl StepReport {
    pub(super) fn label(&self) -> &str {
        &self.label
    }

    pub(super) fn selected_paths(&self) -> &[String] {
        &self.selected_paths
    }

    pub(super) const fn enabled(&self) -> bool {
        self.enabled
    }

    pub(super) fn findings(&self) -> &[StepFinding] {
        &self.findings
    }
}

/// One independently classified failure from a configured child.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StepFinding {
    kind: StepFindingKind,
    detail: String,
}

#[cfg(test)]
impl StepFinding {
    pub(super) const fn kind(&self) -> StepFindingKind {
        self.kind
    }

    pub(super) fn detail(&self) -> &str {
        &self.detail
    }
}

/// Closed failure vocabulary for configured process supervision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum StepFindingKind {
    Exit,
    Signal,
    Spawn,
    OutputOverflow,
    Timeout,
    Cancellation,
    Cleanup,
}

impl StepFindingKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Exit => "exit",
            Self::Signal => "signal",
            Self::Spawn => "spawn",
            Self::OutputOverflow => "output-overflow",
            Self::Timeout => "timeout",
            Self::Cancellation => "cancellation",
            Self::Cleanup => "cleanup",
        }
    }
}

/// Pre-execution or authority failure that stops the ordered battery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ConfiguredFailure {
    reason: &'static str,
    detail: String,
    fault: bool,
}

impl ConfiguredFailure {
    pub(super) const fn reason(&self) -> &'static str {
        self.reason
    }

    pub(super) fn detail(&self) -> &str {
        &self.detail
    }

    pub(super) const fn is_fault(&self) -> bool {
        self.fault
    }

    fn refusal(reason: &'static str, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
            fault: false,
        }
    }

    fn fault(reason: &'static str, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
            fault: true,
        }
    }
}

#[derive(Clone, Copy)]
struct ExecutionBounds {
    runtime: Duration,
    output_drain: Duration,
    reap: Duration,
    grace: Duration,
    output_limit: usize,
}

impl ExecutionBounds {
    const fn production(runtime: Duration) -> Self {
        Self {
            runtime,
            output_drain: OUTPUT_DRAIN,
            reap: REAP_DEADLINE,
            grace: TERMINATION_GRACE,
            output_limit: OUTPUT_LIMIT,
        }
    }
}

struct DispatchPolicy {
    builtins: bool,
    surface: Surface,
    configuration: PathBuf,
    source: ConfigSource,
    tracked: Option<String>,
    commands: Vec<PreCommitStep>,
}

impl DispatchPolicy {
    fn load(repository: &Repository) -> Result<Self, ConfiguredFailure> {
        let loaded = spice::config::capture_repository(repository, &[]).map_err(|error| {
            ConfiguredFailure::refusal("gate::pre-commit-policy-refused", error.to_string())
        })?;
        let policy = loaded.resolved().root().policy();
        let builtins = policy.pre_commit_builtins();
        let commands = policy.pre_commit().to_vec();
        let (configuration, source) = pre_commit_declarations(&loaded).map_or_else(
            || {
                (
                    loaded.locations().tracked().to_path_buf(),
                    ConfigSource::Tracked,
                )
            },
            |declarations| {
                (
                    declarations.configuration().to_path_buf(),
                    declarations.source(),
                )
            },
        );
        Ok(Self {
            builtins,
            surface: Surface::from_loaded(&loaded),
            configuration,
            source,
            tracked: loaded.tracked().map(ToOwned::to_owned),
            commands,
        })
    }

    fn verify_selected(&self, selected_root: &Path) -> Result<(), ConfiguredFailure> {
        let selected = match fs::read_to_string(selected_root.join("spice-rust.toml")) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(ConfiguredFailure::fault(
                    "gate::pre-commit-policy-unavailable",
                    error.to_string(),
                ));
            }
        };
        if selected == self.tracked {
            return Ok(());
        }
        Err(ConfiguredFailure::refusal(
            "gate::pre-commit-policy-source-mismatch",
            format!(
                "configuration={} live_present={} staged_present={}",
                json(&self.configuration.display().to_string()),
                self.tracked.is_some(),
                selected.is_some(),
            ),
        ))
    }

    fn verify_current(&self, repository: &Repository) -> Result<(), ConfiguredFailure> {
        match self.surface.is_current(repository) {
            Ok(true) => Ok(()),
            Ok(false) => Err(ConfiguredFailure::refusal(
                "gate::pre-commit-policy-moved",
                format!(
                    "configuration={} source={} digest={}",
                    json(&self.configuration.display().to_string()),
                    self.source.as_str(),
                    self.surface.digest(),
                ),
            )),
            Err(error) => Err(ConfiguredFailure::refusal(
                "gate::pre-commit-policy-moved",
                format!(
                    "configuration={} source={} digest={}; reacquisition={}",
                    json(&self.configuration.display().to_string()),
                    self.source.as_str(),
                    self.surface.digest(),
                    json(&error.to_string()),
                ),
            )),
        }
    }

    fn require_approved(&self, repository: &Repository) -> Result<(), ConfiguredFailure> {
        let state = repository.admit_state().map_err(|error| {
            ConfiguredFailure::fault("gate::pre-commit-trust-unavailable", error.to_string())
        })?;
        let reader = journal::JournalReader::open(state.journal()).map_err(|error| {
            ConfiguredFailure::refusal(
                "gate::pre-commit-configuration-untrusted",
                format!(
                    "{} trust_authority={}",
                    self.trust_refusal_detail(None),
                    json(&error.to_string()),
                ),
            )
        })?;
        let head = reader
            .read(|read| read.trust_head(self.surface.digest()))
            .map_err(|error| {
                ConfiguredFailure::fault("gate::pre-commit-trust-unavailable", error.to_string())
            })?;
        let standing = head.as_ref().map(|head| head.body().state);
        if standing == Some(TrustState::Granted) {
            return Ok(());
        }
        Err(ConfiguredFailure::refusal(
            "gate::pre-commit-configuration-untrusted",
            self.trust_refusal_detail(standing),
        ))
    }

    fn trust_refusal_detail(&self, standing: Option<TrustState>) -> String {
        format!(
            concat!(
                "surface=configuration digest={} standing={} provenance={} ",
                "configuration={} commands={}; run `spice trust grant {}` only after reviewing these exact bytes and command words"
            ),
            self.surface.digest(),
            standing.map_or("absent", trust_state_name),
            self.source.as_str(),
            json(&self.configuration.display().to_string()),
            json(&self.commands),
            self.surface.digest(),
        )
    }
}

/// One typed dispatch policy and one staged source retained for the whole battery.
pub(super) struct Dispatch {
    repository: Repository,
    policy: DispatchPolicy,
    anchor: spice::gate::PreparedArm<'static>,
}

impl Dispatch {
    pub(super) fn capture() -> Result<Self, ConfiguredFailure> {
        let current = env::current_dir().map_err(|error| {
            ConfiguredFailure::fault(
                "gate::pre-commit-working-directory-unavailable",
                error.to_string(),
            )
        })?;
        let repository = Repository::discover(&current).map_err(|error| {
            ConfiguredFailure::fault("gate::pre-commit-repository-unresolved", error.to_string())
        })?;
        let state = repository.admit_state().map_err(|error| {
            ConfiguredFailure::fault("gate::pre-commit-state-unavailable", error.to_string())
        })?;
        fs::create_dir_all(state.locks()).map_err(|error| {
            ConfiguredFailure::fault("gate::pre-commit-state-unavailable", error.to_string())
        })?;
        let anchor = spice::gate::prepare_arm(gate::Selector::staged(), gate::GateId::Staging)
            .map_err(|error| {
                ConfiguredFailure::refusal(
                    "gate::pre-commit-staged-source-refused",
                    error.to_string(),
                )
            })?;
        let policy = DispatchPolicy::load(&repository)?;
        policy.verify_selected(anchor.root())?;
        let dispatch = Self {
            repository,
            policy,
            anchor,
        };
        dispatch.verify()?;
        Ok(dispatch)
    }

    pub(super) const fn builtins_enabled(&self) -> bool {
        self.policy.builtins
    }

    pub(super) fn has_configured(&self) -> bool {
        !self.policy.commands.is_empty()
    }

    pub(super) fn configured_count(&self) -> usize {
        self.policy.commands.len()
    }

    pub(super) fn omit_full_verifier_proven_configured(&mut self) -> usize {
        omit_full_verifier_proven(&mut self.policy.commands)
    }

    pub(super) fn source(&self) -> &Digest {
        self.anchor.source()
    }

    pub(super) fn changes(&self) -> &[String] {
        self.anchor.changes()
    }

    pub(super) fn verify(&self) -> Result<(), ConfiguredFailure> {
        self.anchor.verify_source().map_err(|error| {
            ConfiguredFailure::refusal(
                "gate::pre-commit-staged-source-moved",
                format!("source={}; {}", self.source(), error.detail()),
            )
        })?;
        self.policy.verify_current(&self.repository)
    }

    pub(super) fn rebind(self, expected: &Digest) -> Result<Self, ConfiguredFailure> {
        let Self {
            repository,
            policy,
            anchor: _,
        } = self;
        let anchor = spice::gate::prepare_arm(gate::Selector::staged(), gate::GateId::Staging)
            .map_err(|error| {
                ConfiguredFailure::refusal(
                    "gate::pre-commit-staged-source-refused",
                    error.to_string(),
                )
            })?;
        if anchor.source() != expected {
            return Err(ConfiguredFailure::refusal(
                "gate::pre-commit-staged-source-moved",
                format!(
                    "dispatcher source {expected} differs from rebound source {}",
                    anchor.source(),
                ),
            ));
        }
        policy.verify_selected(anchor.root())?;
        let rebound = Self {
            repository,
            policy,
            anchor,
        };
        rebound.verify()?;
        Ok(rebound)
    }

    pub(super) fn execute_configured(
        &self,
        runtime: Duration,
        cancellation: Cancellation,
    ) -> Result<Option<ConfiguredReport>, ConfiguredFailure> {
        self.execute_configured_mode(runtime, cancellation, ExecutionMode::Ordinary)
    }

    pub(super) fn execute_resolution_configured(
        &self,
        runtime: Duration,
        cancellation: Cancellation,
    ) -> Result<Option<ConfiguredReport>, ConfiguredFailure> {
        self.execute_configured_mode(runtime, cancellation, ExecutionMode::Resolution)
    }

    fn execute_configured_mode(
        &self,
        runtime: Duration,
        cancellation: Cancellation,
        mode: ExecutionMode,
    ) -> Result<Option<ConfiguredReport>, ConfiguredFailure> {
        if !self.has_configured() {
            return Ok(None);
        }
        let base = ambient_environment()?;
        if let Some((_, source)) = base
            .iter()
            .find(|(name, _)| name == OsStr::new(SOURCE_VARIABLE))
        {
            return Err(ConfiguredFailure::refusal(
                "gate::pre-commit-recursion",
                format!(
                    "configured command redispatched spice gate pre-commit from staged source {}",
                    json(&source.to_string_lossy()),
                ),
            ));
        }
        let source = self.source().clone();
        let paths = self.anchor.changes().to_vec();
        let root = self.repository.worktree_root().to_path_buf();
        let mut verify = || {
            self.verify()?;
            self.policy.require_approved(&self.repository)
        };
        verify()?;
        execute_steps(
            &self.policy.commands,
            &source,
            &paths,
            &root,
            &base,
            ExecutionBounds::production(runtime),
            cancellation,
            mode,
            &mut verify,
        )
        .map(Some)
    }
}

fn omit_full_verifier_proven(commands: &mut Vec<PreCommitStep>) -> usize {
    let before = commands.len();
    commands.retain(|command| !full_verifier_proves(command));
    before - commands.len()
}

fn full_verifier_proves(command: &PreCommitStep) -> bool {
    command.scopes().paths().is_empty()
        && command
            .run()
            .iter()
            .map(String::as_str)
            .eq(SOURCE_GATE_ARGV)
}

pub(super) const fn cleanup_reserve() -> Duration {
    OUTPUT_DRAIN
        .saturating_add(REAP_DEADLINE)
        .saturating_add(TERMINATION_GRACE)
}

#[expect(
    clippy::too_many_arguments,
    reason = "commands, staged authority, process policy, and the movement guard are independent execution inputs"
)]
fn execute_steps(
    commands: &[PreCommitStep],
    source: &Digest,
    staged_paths: &[String],
    root: &Path,
    base_environment: &[(OsString, OsString)],
    bounds: ExecutionBounds,
    cancellation: Cancellation,
    mode: ExecutionMode,
    verify: &mut impl FnMut() -> Result<(), ConfiguredFailure>,
) -> Result<ConfiguredReport, ConfiguredFailure> {
    let started = Instant::now();
    let mut reports = Vec::with_capacity(commands.len());
    for command in commands {
        verify()?;
        let selected_paths = scoped_paths(command, staged_paths)?;
        if !command.scopes().paths().is_empty() && selected_paths.is_empty() {
            reports.push(step_report(
                command,
                command.run().to_vec(),
                selected_paths,
                false,
                Vec::new(),
            ));
            continue;
        }
        let argv = effective_argv(command, mode);
        let remaining = bounds.runtime.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            reports.push(step_report(
                command,
                argv,
                selected_paths,
                true,
                vec![finding(
                    StepFindingKind::Timeout,
                    "shared configured-command deadline elapsed before spawn",
                )],
            ));
            break;
        }
        let environment = command_environment(base_environment, source, &selected_paths)?;
        let spec = command_spec(
            command.label(),
            &argv,
            root,
            environment,
            remaining,
            bounds,
            cancellation.clone(),
        )?;
        let findings = match sill::spawn(spec) {
            Ok(running) => match running.wait() {
                Ok(waited) => classify_wait(&waited),
                Err(error) => vec![finding(StepFindingKind::Cleanup, error.to_string())],
            },
            Err(error) => vec![finding(StepFindingKind::Spawn, error.to_string())],
        };
        let terminal = findings.iter().any(|finding| {
            matches!(
                finding.kind,
                StepFindingKind::Timeout | StepFindingKind::Cancellation | StepFindingKind::Cleanup
            )
        });
        reports.push(step_report(command, argv, selected_paths, true, findings));
        verify()?;
        if terminal {
            break;
        }
    }
    verify()?;
    Ok(ConfiguredReport {
        source: source.clone(),
        steps: reports,
    })
}

fn effective_argv(command: &PreCommitStep, mode: ExecutionMode) -> Vec<String> {
    let argv = command.run();
    if !matches!(mode, ExecutionMode::Resolution)
        || !command.scopes().paths().is_empty()
        || argv.len() < 4
        || argv[argv.len() - 3] != "--"
        || argv[argv.len() - 2..] != ["gate", "selection"]
    {
        return argv.to_vec();
    }
    let mut effective = argv[..argv.len() - 3].to_vec();
    effective.push("--".to_owned());
    effective.push(super::RESOLUTION_SELECTION_WORKER_ARGUMENT.to_owned());
    effective
}

fn scoped_paths(
    command: &PreCommitStep,
    staged_paths: &[String],
) -> Result<Vec<String>, ConfiguredFailure> {
    let scopes = command.scopes().paths();
    if scopes.is_empty() {
        return Ok(staged_paths.to_vec());
    }
    let mut selected = Vec::new();
    for path in staged_paths {
        let mut matches = false;
        for scope in scopes {
            matches |= gate::tracked_path_selector_matches(scope, path).map_err(|error| {
                ConfiguredFailure::fault("gate::pre-commit-scope-invalid", error)
            })?;
        }
        if matches {
            selected.push(path.clone());
        }
    }
    Ok(selected)
}

fn command_spec(
    label: &str,
    argv: &[String],
    root: &Path,
    environment: Environment,
    runtime: Duration,
    bounds: ExecutionBounds,
    cancellation: Cancellation,
) -> Result<Spec, ConfiguredFailure> {
    let (program, arguments) = argv
        .split_first()
        .expect("typed pre-commit argv is nonempty");
    let capture = StreamPolicy::Capture {
        limit: bounds.output_limit,
    };
    Ok(Spec {
        argv: Argv::new(program, arguments.iter()).map_err(|error| {
            ConfiguredFailure::fault("gate::pre-commit-command-invalid", error.to_string())
        })?,
        cwd: root.to_path_buf(),
        environment,
        group: ProcessGroup::New,
        deadlines: Deadlines::new(runtime, bounds.output_drain, bounds.reap).map_err(|error| {
            ConfiguredFailure::fault("gate::pre-commit-command-invalid", error.to_string())
        })?,
        wait: WaitPolicy::new(
            format!("configured pre-commit step {label}"),
            cancellation,
            RestartDisposition::Never,
        )
        .map_err(|error| {
            ConfiguredFailure::fault("gate::pre-commit-command-invalid", error.to_string())
        })?,
        output: Output::new(StdinPolicy::Null, capture, capture),
        redaction: Redaction::All,
        grace: bounds.grace,
    })
}

fn command_environment(
    base: &[(OsString, OsString)],
    source: &Digest,
    paths: &[String],
) -> Result<Environment, ConfiguredFailure> {
    let mut entries = base
        .iter()
        .filter(|(name, _)| !git_local_environment(name))
        .cloned()
        .collect::<BTreeMap<_, _>>();
    entries.remove(OsStr::new(PATHS_VARIABLE));
    entries.remove(OsStr::new(SOURCE_VARIABLE));
    entries.insert(OsString::from(PATHS_VARIABLE), OsString::from(json(paths)));
    entries.insert(
        OsString::from(SOURCE_VARIABLE),
        OsString::from(source.to_string()),
    );
    Environment::new(entries).map_err(|error| {
        ConfiguredFailure::fault("gate::pre-commit-environment-invalid", error.to_string())
    })
}

fn git_local_environment(name: &OsStr) -> bool {
    GIT_LOCAL_ENVIRONMENT
        .iter()
        .any(|candidate| name == OsStr::new(candidate))
        || name.to_str().is_some_and(|name| {
            name.starts_with("GIT_CONFIG_KEY_") || name.starts_with("GIT_CONFIG_VALUE_")
        })
}

fn classify_wait(waited: &WaitOutcome) -> Vec<StepFinding> {
    let mut findings = Vec::new();
    if !waited.outputs_drained() {
        findings.push(finding(
            StepFindingKind::Cleanup,
            "captured output did not drain before its cleanup deadline",
        ));
    }
    let mut overflow = Vec::new();
    if waited.stdout().is_some_and(|output| output.truncated()) {
        overflow.push("stdout");
    }
    if waited.stderr().is_some_and(|output| output.truncated()) {
        overflow.push("stderr");
    }
    if !overflow.is_empty() {
        findings.push(finding(
            StepFindingKind::OutputOverflow,
            format!(
                "{} exceeded the per-stream capture limit",
                overflow.join("+")
            ),
        ));
    }
    match waited.completion() {
        Completion::TimedOut(status) => findings.push(finding(
            StepFindingKind::Timeout,
            format!(
                "status={}; stdout={}; stderr={}",
                status_text(status),
                captured(waited.stdout()),
                captured(waited.stderr())
            ),
        )),
        Completion::Cancelled(status) => findings.push(finding(
            StepFindingKind::Cancellation,
            format!(
                "status={}; stdout={}; stderr={}",
                status_text(status),
                captured(waited.stdout()),
                captured(waited.stderr())
            ),
        )),
        Completion::Exited(status) if !status.success() => {
            use std::os::unix::process::ExitStatusExt as _;
            let (kind, completion) = status.signal().map_or_else(
                || (StepFindingKind::Exit, format!("code={:?}", status.code())),
                |signal| (StepFindingKind::Signal, format!("signal={signal}")),
            );
            findings.push(finding(
                kind,
                format!(
                    "{completion}; stdout={}; stderr={}",
                    captured(waited.stdout()),
                    captured(waited.stderr())
                ),
            ));
        }
        Completion::Exited(_) => {}
    }
    findings
}

fn status_text(status: &std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt as _;
    status.signal().map_or_else(
        || format!("code={:?}", status.code()),
        |signal| format!("signal={signal}"),
    )
}

fn captured(output: Option<&sill::CapturedOutput>) -> String {
    output.map_or_else(
        || "null".to_owned(),
        |output| json(&String::from_utf8_lossy(output.bytes())),
    )
}

fn step_report(
    command: &PreCommitStep,
    argv: Vec<String>,
    selected_paths: Vec<String>,
    enabled: bool,
    findings: Vec<StepFinding>,
) -> StepReport {
    StepReport {
        label: command.label().to_owned(),
        argv,
        selected_paths,
        enabled,
        findings,
    }
}

fn finding(kind: StepFindingKind, detail: impl Into<String>) -> StepFinding {
    StepFinding {
        kind,
        detail: detail.into(),
    }
}

fn trust_state_name(state: TrustState) -> &'static str {
    match state {
        TrustState::Pended => "pended",
        TrustState::Granted => "granted",
        TrustState::Revoked => "revoked",
    }
}

pub(super) fn recursion_refusal<T>(source: &OsStr) -> Outcome<T> {
    Outcome::Refusal(Refusal::bare(
        RefusalDiagnostic::new(
            "gate::pre-commit-recursion",
            Some("one non-recursive configured pre-commit dispatch".to_owned()),
            Some("SPICE_RS_PRE_COMMIT_SOURCE absent at the command rim".to_owned()),
            Some(format!(
                "SPICE_RS_PRE_COMMIT_SOURCE={}",
                json(&source.to_string_lossy()),
            )),
        )
        .with_detail(
            "remove `spice gate pre-commit` from policy.pre_commit; configured commands cannot redispatch their own gate",
        ),
    ))
}

fn json(value: &(impl Serialize + ?Sized)) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"<unencodable>\"".to_owned())
}

#[expect(
    clippy::disallowed_methods,
    reason = "the configured pre-commit command rim captures and then sanitizes the caller environment once"
)]
fn ambient_environment() -> Result<Vec<(OsString, OsString)>, ConfiguredFailure> {
    let mut environment = env::vars_os().collect::<Vec<_>>();
    environment.sort();
    Ok(environment)
}

#[cfg(test)]
#[path = "configured_tests.rs"]
mod tests;
