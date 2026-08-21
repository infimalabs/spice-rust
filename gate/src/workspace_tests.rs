//! Deterministic, bounded execution of every workspace Rust test target.
//! Inventory and selection stay here; shard execution lives in `coordinator`.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use sill::{
    Argv, Cancellation, CancellationToken, Completion, Deadlines, Environment, Observation,
    ObserveError, ObservedRecord, Output, OutputStream, ProcessGroup, RecordObserver, Redaction,
    RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitOutcome, WaitPolicy,
};
use syn::{Expr, Item, Lit, LitStr, Meta};

use crate::WORKSPACE_TEST_CONCURRENCY_CAP;

mod coordinator;
mod selection;

use coordinator::{
    Coordinator, RunPolicy, captured_excerpt, completion_name, output_error, process_spec,
};
#[cfg(test)]
use coordinator::{LastTest, isolate_shard_outputs, shard_test_threads, target_arguments};
use selection::{
    CargoReachEscalation, exhaustive_selection_report, package_inventory, selection_plan,
};
#[cfg(test)]
use selection::{CargoReachEscalationReason, selection_report};

/// Exact workspace packages reached through canonical retained Cargo metadata.
pub(crate) struct WorkspacePackageReach {
    checks: Vec<CargoCheckSelection>,
    roots: Vec<PathBuf>,
}

impl WorkspacePackageReach {
    pub(crate) fn cargo_check_arguments(&self) -> Vec<Vec<String>> {
        let mut invocations = Vec::new();
        let all_targets = self
            .checks
            .iter()
            .filter(|check| check.all_targets)
            .collect::<Vec<_>>();
        if !all_targets.is_empty() {
            let mut arguments = vec![
                "check".to_owned(),
                "--locked".to_owned(),
                "--all-targets".to_owned(),
            ];
            for check in &all_targets {
                arguments.extend(["--package".to_owned(), check.name.clone()]);
            }
            if let Some(features) =
                qualified_required_features(all_targets.iter().flat_map(|check| {
                    check
                        .required_features
                        .iter()
                        .map(|feature| (check.name.as_str(), feature.as_str()))
                }))
            {
                arguments.extend(["--features".to_owned(), features]);
            }
            invocations.push(arguments);
        }
        for check in self.checks.iter().filter(|check| !check.all_targets) {
            let mut arguments = vec![
                "check".to_owned(),
                "--locked".to_owned(),
                "--package".to_owned(),
                check.name.clone(),
            ];
            for target in &check.targets {
                arguments.push(target.flag.to_owned());
                if let Some(name) = &target.name {
                    arguments.push(name.clone());
                }
            }
            if let Some(features) = qualified_required_features(
                check
                    .required_features
                    .iter()
                    .map(|feature| (check.name.as_str(), feature.as_str())),
            ) {
                arguments.extend(["--features".to_owned(), features]);
            }
            invocations.push(arguments);
        }
        invocations
    }

    pub(crate) fn reaches_root(&self, root: &Path) -> bool {
        let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        self.roots.contains(&root)
    }
}

pub(crate) fn workspace_package_reach_at(
    root: &Path,
    changed_sources: &[String],
    environment: &Environment,
    cargo: &OsStr,
) -> Result<WorkspacePackageReach, WorkspaceTestError> {
    let inventory = acquire_inventory(root, environment, cargo)?;
    let checks = selection::cargo_check_selections(&inventory, root, changed_sources)?;
    Ok(WorkspacePackageReach {
        roots: checks.iter().map(|check| check.root.clone()).collect(),
        checks,
    })
}

#[derive(Clone, Debug)]
struct CargoCheckSelection {
    name: String,
    root: PathBuf,
    all_targets: bool,
    targets: Vec<CargoTargetSelection>,
    required_features: Vec<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CargoTargetSelection {
    flag: &'static str,
    name: Option<String>,
    required_features: Vec<String>,
}

fn qualified_required_features<'a>(
    features: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<String> {
    let features = features
        .into_iter()
        .map(|(package, feature)| format!("{package}/{feature}"))
        .collect::<BTreeSet<_>>();
    (!features.is_empty()).then(|| features.into_iter().collect::<Vec<_>>().join(","))
}

/// Whether the compiled Cargo workspace engine applies to this selected tree.
///
/// This predicate runs before metadata or tool discovery, so an undeclared or
/// irrelevant engine cannot make Cargo a dependency of a generic repository.
#[must_use]
pub fn cargo_workspace_tests_applicable(
    root: &Path,
    changed_sources: &[String],
    exhaustive: bool,
) -> bool {
    root.join("Cargo.toml").is_file()
        && (exhaustive
            || changed_sources
                .iter()
                .any(|path| selection::test_relevant_source(path)))
}

/// Keep each post-prebuild Cargo invocation small enough for the full verifier's
/// bounded workspace branch to make steady progress under the absolute verifier cap.
/// Targets are source-weight balanced across the minimum shard count so one
/// cluster of larger integration targets cannot pin the coordinator tail.
const TARGETS_PER_SHARD: usize = 8;
/// Retain finer work units inside the full verifier so a constrained two-lane
/// schedule can rebalance long test targets without paying one Cargo launch
/// for every target.
const FULL_VERIFIER_TARGETS_PER_SHARD: usize = 4;
/// Below three targets, observed Cargo startup and publication time outweighs
/// target cardinality, so these tail batches retain stable metadata order.
const CARGO_STARTUP_DOMINATED_TARGET_LIMIT: usize = 2;
/// No individual test shard may cross its local three-minute stall ceiling.
const SHARD_DEADLINE: Duration = Duration::from_secs(180);
/// The aggregate coordinator may outlive one shard while bounded siblings finish.
const COORDINATOR_DEADLINE: Duration = Duration::from_secs(255);
const PROGRESS_INTERVAL: Duration = Duration::from_secs(30);
const METADATA_DEADLINE: Duration = Duration::from_secs(30);
const OUTPUT_DRAIN_DEADLINE: Duration = Duration::from_secs(5);
const REAP_DEADLINE: Duration = Duration::from_secs(5);
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
const CAPTURE_LIMIT: usize = 4 * 1024 * 1024;
const FAILURE_EXCERPT_LIMIT: usize = 16 * 1024;
const RECORD_FRAME_LIMIT: usize = 256 * 1024;
const RECORD_QUEUE_CAPACITY: usize = 4_096;
const CARGO_TARGET_DIR: &str = "CARGO_TARGET_DIR";
const CARGO_BUILD_BUILD_DIR: &str = "CARGO_BUILD_BUILD_DIR";
/// Test shards own ephemeral temporary roots, but an ambient compiler-wrapper
/// daemon can outlive those roots and retain their dead `TMPDIR` paths.
const RUSTC_WRAPPER: &str = concat!("RUSTC_", "WRAPPER");
const PRE_COMMIT_SOURCE: &str = wire::SpiceVariable::PreCommitSource.as_str();
const TEST_WRITABLE_ROOT: &str = "SPICE_WORKSPACE_TEST_WRITABLE_ROOT"; // env-policy: allow
const SHARED_OUTPUTS_ENV: &str = "SPICE_WORKSPACE_TEST_SHARED_OUTPUTS"; // env-policy: allow
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

/// Successful typed inventory of every target the canonical test execution ran.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceTestExecution {
    tests: Vec<String>,
}

impl WorkspaceTestExecution {
    /// Stable package-qualified test-target identities in execution inventory order.
    #[must_use]
    pub fn tests(&self) -> &[String] {
        &self.tests
    }

    /// Join canonical target execution with authoritative per-test coverage.
    /// Complete coverage includes every executed target and may name unexecuted suite targets.
    pub fn publish(
        &self,
        source: wire::Digest,
        changed_sources: Vec<String>,
        coverage: Vec<crate::TestCoverage>,
        completeness: crate::CoverageCompleteness,
    ) -> Result<crate::CanonicalTestExecution, WorkspaceTestError> {
        crate::CanonicalTestExecution::new(
            source,
            changed_sources,
            self.tests.clone(),
            coverage,
            completeness,
        )
        .map_err(|error| WorkspaceTestError::new(error.to_string()))
    }

    /// Publish an explicit non-clean graph input when no coverage authority completed.
    pub fn publish_unknown(
        &self,
        source: wire::Digest,
        changed_sources: Vec<String>,
        reason: impl Into<String>,
    ) -> Result<crate::CanonicalTestExecution, WorkspaceTestError> {
        let completeness = crate::CoverageCompleteness::unknown(reason)
            .map_err(|error| WorkspaceTestError::new(error.to_string()))?;
        self.publish(source, changed_sources, Vec::new(), completeness)
    }

    fn from_inventory(inventory: &Inventory) -> Self {
        let mut tests = inventory
            .packages
            .iter()
            .flat_map(|package| {
                package
                    .targets
                    .iter()
                    .map(|target| format!("{}::{}", package.name, target.display()))
            })
            .collect::<Vec<_>>();
        tests.sort();
        Self { tests }
    }
}

/// A refusal from the bounded workspace-test coordinator.
#[derive(Debug)]
pub struct WorkspaceTestError {
    detail: String,
}

impl WorkspaceTestError {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

impl fmt::Display for WorkspaceTestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "workspace test coordinator failed: {}",
            self.detail
        )
    }
}

impl std::error::Error for WorkspaceTestError {}

/// Source and writable roots bound to one selected workspace-test run.
#[derive(Clone, Copy, Debug)]
pub struct WorkspaceTestRoots<'a> {
    selected: &'a Path,
    writable: &'a Path,
    reusable: Option<&'a crate::RustAnalysisWorkspace>,
}

/// Fixed process budget for one selected workspace-test slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceTestBudget {
    concurrency: usize,
    cargo_jobs: usize,
    test_threads: usize,
}

impl WorkspaceTestBudget {
    /// Bind outer Cargo lanes, per-Cargo compiler jobs, and test-harness threads.
    ///
    /// # Errors
    ///
    /// Refuses zero values or outer concurrency above the standalone coordinator cap.
    pub fn new(
        concurrency: usize,
        cargo_jobs: usize,
        test_threads: usize,
    ) -> Result<Self, WorkspaceTestError> {
        if !(1..=WORKSPACE_TEST_CONCURRENCY_CAP).contains(&concurrency) {
            return Err(WorkspaceTestError::new(format!(
                "workspace test concurrency must be between 1 and {WORKSPACE_TEST_CONCURRENCY_CAP}, got {concurrency}",
            )));
        }
        if cargo_jobs == 0 || test_threads == 0 {
            return Err(WorkspaceTestError::new(
                "workspace test Cargo jobs and harness threads must be nonzero",
            ));
        }
        Ok(Self {
            concurrency,
            cargo_jobs,
            test_threads,
        })
    }
}

impl<'a> WorkspaceTestRoots<'a> {
    /// Bind an immutable selected root to its live worktree's output owner.
    #[must_use]
    pub const fn new(selected: &'a Path, writable: &'a Path) -> Self {
        Self {
            selected,
            writable,
            reusable: None,
        }
    }

    /// Reuse the exact workspace completed by native analysis of this source.
    #[must_use]
    pub const fn with_reusable(mut self, reusable: &'a crate::RustAnalysisWorkspace) -> Self {
        self.reusable = Some(reusable);
        self
    }
}

/// Run every workspace package's all-target test inventory under bounded shards.
///
/// # Errors
///
/// Returns metadata, partition, launch, supervision, timeout, or test failures.
pub fn run(
    environment: Environment,
    concurrency: usize,
    output: &mut impl Write,
) -> Result<WorkspaceTestExecution, WorkspaceTestError> {
    let root = env::current_dir().map_err(|error| {
        WorkspaceTestError::new(format!("working directory unavailable: {error}"))
    })?;
    run_at(&root, environment, concurrency, output)
}

/// Run the canonical workspace-test coordinator against one already selected root.
///
/// This is the same inventory, partitioner, process contract, and coordinator as
/// [`run`]; only the working root is supplied by the caller that owns an
/// immutable source selection.
///
/// # Errors
///
/// Returns metadata, partition, launch, supervision, timeout, or test failures.
pub fn run_at(
    root: &Path,
    environment: Environment,
    concurrency: usize,
    output: &mut impl Write,
) -> Result<WorkspaceTestExecution, WorkspaceTestError> {
    run_at_with_prebuild(root, environment, concurrency, output, true)
}

/// Prebuild every canonical workspace test target and the normal `xtask`
/// launcher into one shared Cargo build cache.
///
/// The full verifier calls this setup boundary after native compiler admission
/// and before starting the supervised test branches. [`run_prebuilt_shared_at`]
/// then reuses that completed native build cache after wasm analysis and the
/// Canvas compiler-ready boundary, without repeating setup inside the test
/// clock. The launcher build keeps `cargo xtask app` from recompiling its native
/// entry point before it can start Trunk.
///
/// # Errors
///
/// Returns metadata, environment, launch, supervision, timeout, or compiler failures.
pub fn prebuild_shared_at(
    root: &Path,
    environment: Environment,
    output: &mut impl Write,
) -> Result<(), WorkspaceTestError> {
    let (inventory, environment, policy) =
        prepare_execution(root, environment, WORKSPACE_TEST_CONCURRENCY_CAP)?;
    if !policy.shared_outputs {
        return Err(WorkspaceTestError::new(
            "shared workspace-test prebuild requires shared Cargo output authority",
        ));
    }
    prebuild_all_tests(root, &environment, OsStr::new("cargo"), &inventory, output)?;
    prebuild_canvas_launcher(root, &environment, OsStr::new("cargo"), output)
}

/// Run every canonical workspace test target from a completed shared-output prebuild.
///
/// This entry point is reserved for the full verifier's supervised child.
/// Standalone shared-output execution continues through [`run_at`], which
/// performs its own mandatory prebuild before any lane shares the build cache.
///
/// # Errors
///
/// Returns metadata, partition, launch, supervision, timeout, or test failures.
pub fn run_prebuilt_shared_at(
    root: &Path,
    environment: Environment,
    concurrency: usize,
    output: &mut impl Write,
) -> Result<WorkspaceTestExecution, WorkspaceTestError> {
    run_at_with_prebuild(root, environment, concurrency, output, false)
}

fn run_at_with_prebuild(
    root: &Path,
    environment: Environment,
    concurrency: usize,
    output: &mut impl Write,
    prebuild: bool,
) -> Result<WorkspaceTestExecution, WorkspaceTestError> {
    let (inventory, environment, policy) = prepare_execution(root, environment, concurrency)?;
    if !prebuild && !policy.shared_outputs {
        return Err(WorkspaceTestError::new(
            "prebuilt workspace-test execution requires shared Cargo output authority",
        ));
    }
    let execution = WorkspaceTestExecution::from_inventory(&inventory);
    if prebuild && policy.shared_outputs {
        prebuild_all_tests(root, &environment, OsStr::new("cargo"), &inventory, output)?;
    } else if !prebuild {
        validate_prebuilt_binary_outputs(&inventory, &environment)?;
    }
    let targets_per_shard = if prebuild {
        TARGETS_PER_SHARD
    } else {
        FULL_VERIFIER_TARGETS_PER_SHARD
    };
    let shards = partition_with_target_limit(&inventory, targets_per_shard);
    validate_partition_with_target_limit(&inventory, &shards, targets_per_shard)?;
    let policy = policy.for_shards(shards.len());
    Coordinator::new(
        shards,
        root.to_path_buf(),
        environment,
        OsString::from("cargo"),
        policy,
        output,
    )
    .run()?;
    Ok(execution)
}

fn prepare_execution(
    root: &Path,
    environment: Environment,
    concurrency: usize,
) -> Result<(Inventory, Environment, RunPolicy), WorkspaceTestError> {
    let environment = without_pre_commit_dispatch(environment)?;
    let environment = with_direct_rustc(environment)?;
    let (environment, policy) = policy_from_environment(environment, concurrency)?;
    let inventory = acquire_inventory(root, &environment, OsStr::new("cargo"))?;
    let environment = inventory.execution_environment(environment, root, root, true)?;
    Ok((inventory, environment, policy))
}

fn policy_from_environment(
    environment: Environment,
    concurrency: usize,
) -> Result<(Environment, RunPolicy), WorkspaceTestError> {
    let mut entries = environment
        .entries()
        .iter()
        .cloned()
        .collect::<BTreeMap<OsString, OsString>>();
    let shared_outputs = entries
        .remove(OsStr::new(SHARED_OUTPUTS_ENV))
        .is_some_and(|value| value == OsStr::new("1"));
    let policy = if shared_outputs {
        RunPolicy::production_shared(concurrency)?
    } else {
        RunPolicy::production(concurrency)?
    };
    let environment =
        Environment::new(entries).map_err(|error| WorkspaceTestError::new(error.to_string()))?;
    Ok((environment, policy))
}

fn prebuild_all_tests(
    root: &Path,
    environment: &Environment,
    cargo: &OsStr,
    inventory: &Inventory,
    output: &mut impl Write,
) -> Result<(), WorkspaceTestError> {
    let features = qualified_required_features(inventory.packages.iter().flat_map(|package| {
        package.targets.iter().flat_map(|target| {
            target
                .required_features
                .iter()
                .map(|feature| (package.name.as_str(), feature.as_str()))
        })
    }));
    let mut arguments = ["test", "--locked", "--workspace", "--all-targets"]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    let mut command = "cargo test --locked --workspace --all-targets".to_owned();
    if let Some(features) = features {
        arguments.extend([OsString::from("--features"), OsString::from(&features)]);
        command.push_str(" --features ");
        command.push_str(&features);
    }
    arguments.push(OsString::from("--no-run"));
    command.push_str(" --no-run");
    run_prebuild_command(
        root,
        environment,
        cargo,
        output,
        &command,
        arguments,
        "workspace-test prebuild",
    )?;
    repair_missing_binary_outputs(root, environment, cargo, inventory, output)
}

fn repair_missing_binary_outputs(
    root: &Path,
    environment: &Environment,
    cargo: &OsStr,
    inventory: &Inventory,
    output: &mut impl Write,
) -> Result<(), WorkspaceTestError> {
    for missing in missing_binary_outputs(inventory, environment)? {
        let command = format!(
            "cargo build --locked --package {:?} --bin {:?}",
            missing.binary.package_id, missing.binary.target_name,
        );
        let owner = format!(
            "CARGO_BIN_EXE repair for package {:?} binary target {:?} at {}",
            missing.binary.package_name,
            missing.binary.target_name,
            missing.path.display(),
        );
        run_prebuild_command(
            root,
            environment,
            cargo,
            output,
            &command,
            [
                OsStr::new("build"),
                OsStr::new("--locked"),
                OsStr::new("--package"),
                OsStr::new(&missing.binary.package_id),
                OsStr::new("--bin"),
                OsStr::new(&missing.binary.target_name),
            ],
            &owner,
        )?;
    }
    validate_prebuilt_binary_outputs(inventory, environment)
}

fn validate_prebuilt_binary_outputs(
    inventory: &Inventory,
    environment: &Environment,
) -> Result<(), WorkspaceTestError> {
    let missing = missing_binary_outputs(inventory, environment)?;
    if missing.is_empty() {
        return Ok(());
    }
    let detail = missing
        .iter()
        .map(|missing| {
            format!(
                "{} (package {:?}, binary target {:?})",
                missing.path.display(),
                missing.binary.package_name,
                missing.binary.target_name,
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    Err(WorkspaceTestError::new(format!(
        "prebuilt CARGO_BIN_EXE output is missing or not executable: {detail}"
    )))
}

fn missing_binary_outputs<'a>(
    inventory: &'a Inventory,
    environment: &Environment,
) -> Result<Vec<MissingBinaryOutput<'a>>, WorkspaceTestError> {
    let target_directory = environment
        .entries()
        .iter()
        .find_map(|(name, value)| {
            (name == OsStr::new(CARGO_TARGET_DIR)).then(|| PathBuf::from(value))
        })
        .ok_or_else(|| {
            WorkspaceTestError::new("prebuilt CARGO_BIN_EXE validation requires CARGO_TARGET_DIR")
        })?;
    Ok(inventory
        .binary_outputs
        .iter()
        .filter_map(|binary| {
            let path = target_directory.join("debug").join(format!(
                "{}{}",
                binary.target_name,
                std::env::consts::EXE_SUFFIX,
            ));
            (!executable_file(&path)).then_some(MissingBinaryOutput { binary, path })
        })
        .collect())
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(target_family = "unix")]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(target_family = "unix"))]
    {
        true
    }
}

fn prebuild_canvas_launcher(
    root: &Path,
    environment: &Environment,
    cargo: &OsStr,
    output: &mut impl Write,
) -> Result<(), WorkspaceTestError> {
    run_prebuild_command(
        root,
        environment,
        cargo,
        output,
        "cargo build --locked -p xtask --bin xtask",
        ["build", "--locked", "-p", "xtask", "--bin", "xtask"],
        "Canvas launcher prebuild",
    )
}

fn run_prebuild_command<I, S>(
    root: &Path,
    environment: &Environment,
    cargo: &OsStr,
    output: &mut impl Write,
    command: &str,
    arguments: I,
    owner: &str,
) -> Result<(), WorkspaceTestError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    writeln!(output, "[rust-test] stage=prebuild command={command:?}").map_err(output_error)?;
    output.flush().map_err(output_error)?;
    let started = Instant::now();
    let spec = process_spec(
        cargo,
        arguments,
        root,
        environment.clone(),
        SHARD_DEADLINE,
        Cancellation::Never,
        owner,
    )?;
    let outcome = sill::spawn(spec)
        .map_err(|error| WorkspaceTestError::new(format!("prebuild launch failed: {error}")))?
        .wait()
        .map_err(|error| WorkspaceTestError::new(format!("prebuild wait failed: {error}")))?;
    let capture_failed = !outcome.outputs_drained()
        || outcome
            .stdout()
            .is_some_and(|captured| captured.truncated())
        || outcome
            .stderr()
            .is_some_and(|captured| captured.truncated());
    let disposition = match outcome.completion() {
        Completion::Exited(status) if status.success() && !capture_failed => "success".to_owned(),
        Completion::Exited(status) if capture_failed => {
            format!(
                "capture-failed after {}",
                completion_name(outcome.completion())
            )
        }
        completion => completion_name(completion),
    };
    writeln!(
        output,
        "[rust-test] stage=prebuild-terminal elapsed_ms={} disposition={}",
        started.elapsed().as_millis(),
        disposition,
    )
    .map_err(output_error)?;
    output.flush().map_err(output_error)?;
    match outcome.completion() {
        Completion::Exited(status) if status.success() && !capture_failed => Ok(()),
        completion => Err(WorkspaceTestError::new(format!(
            "{owner} ended as {}: stdout-tail={:?}; stderr-tail={:?}",
            completion_name(completion),
            captured_excerpt(outcome.stdout()),
            captured_excerpt(outcome.stderr()),
        ))),
    }
}

/// Select and run only test targets reached by staged build sources according
/// to canonical Cargo metadata. A reached library target expands to every
/// test-enabled target in that package because those targets may link the
/// library. A non-library target reaches only its root and exact `#[path]`
/// module declarations. Package manifests and build scripts reach their whole
/// package; workspace/toolchain and gate-policy authorities reach the complete
/// workspace because their effect cannot be narrowed further.
///
/// Unknown or incomplete package reach escalates to every workspace test
/// target rather than launching an unproven narrowed set. A complete report is
/// emitted only after every selected target succeeds through this coordinator.
///
/// # Errors
///
/// Returns metadata, graph-validation, launch, supervision, timeout, test, or
/// report-construction failures.
pub fn run_selected_at(
    roots: WorkspaceTestRoots<'_>,
    source: wire::Digest,
    changed_sources: &[String],
    environment: Environment,
    concurrency: usize,
    output: &mut impl Write,
) -> Result<crate::TestSelectionReport, WorkspaceTestError> {
    run_selected_at_mode(
        roots,
        source,
        changed_sources,
        environment,
        SelectedRunPolicy::Legacy(concurrency),
        output,
        false,
    )
}

/// Select and run reached test targets under one explicit fleet-aware budget.
///
/// # Errors
///
/// Returns the same bounded selection and execution failures as [`run_selected_at`].
pub fn run_selected_with_budget_at(
    roots: WorkspaceTestRoots<'_>,
    source: wire::Digest,
    changed_sources: &[String],
    environment: Environment,
    budget: WorkspaceTestBudget,
    output: &mut impl Write,
) -> Result<crate::TestSelectionReport, WorkspaceTestError> {
    run_selected_at_mode(
        roots,
        source,
        changed_sources,
        environment,
        SelectedRunPolicy::Budgeted(budget),
        output,
        false,
    )
}

/// Run every workspace target for an explicitly exhaustive change selection.
///
/// Every changed path retains an edge to every target, including paths with no
/// recognized build suffix. This is the test-side half of fail-closed
/// change-gate escalation.
///
/// # Errors
///
/// Returns the same failures as [`run_selected_at`].
pub fn run_exhaustive_selected_at(
    roots: WorkspaceTestRoots<'_>,
    source: wire::Digest,
    changed_sources: &[String],
    environment: Environment,
    concurrency: usize,
    output: &mut impl Write,
) -> Result<crate::TestSelectionReport, WorkspaceTestError> {
    run_selected_at_mode(
        roots,
        source,
        changed_sources,
        environment,
        SelectedRunPolicy::Legacy(concurrency),
        output,
        true,
    )
}

#[derive(Clone, Copy)]
enum SelectedRunPolicy {
    Legacy(usize),
    Budgeted(WorkspaceTestBudget),
}

fn run_selected_at_mode(
    roots: WorkspaceTestRoots<'_>,
    source: wire::Digest,
    changed_sources: &[String],
    environment: Environment,
    selected_policy: SelectedRunPolicy,
    output: &mut impl Write,
    exhaustive: bool,
) -> Result<crate::TestSelectionReport, WorkspaceTestError> {
    let policy = match selected_policy {
        SelectedRunPolicy::Legacy(concurrency) => RunPolicy::production(concurrency)?,
        SelectedRunPolicy::Budgeted(budget) => RunPolicy::production_budgeted(budget),
    };
    let root = roots
        .reusable
        .map_or(roots.selected, crate::RustAnalysisWorkspace::source);
    let environment = without_pre_commit_dispatch(environment)?;
    let environment = with_direct_rustc(environment)?;
    let environment = without_parent_repository(environment)?;
    let environment = match roots.reusable {
        Some(workspace) => reuse_environment(environment, workspace)?,
        None => environment,
    };
    writeln!(
        output,
        "[rust-test] stage=metadata-start changes={} exhaustive={exhaustive}",
        changed_sources.len(),
    )
    .map_err(output_error)?;
    output.flush().map_err(output_error)?;
    let metadata_started = Instant::now();
    let inventory = match acquire_inventory(root, &environment, OsStr::new("cargo")) {
        Ok(inventory) => {
            writeln!(
                output,
                "[rust-test] stage=metadata-terminal elapsed_ms={} disposition=success",
                metadata_started.elapsed().as_millis(),
            )
            .map_err(output_error)?;
            output.flush().map_err(output_error)?;
            inventory
        }
        Err(error) => {
            writeln!(
                output,
                "[rust-test] stage=metadata-terminal elapsed_ms={} disposition=failed detail={:?}",
                metadata_started.elapsed().as_millis(),
                error.to_string(),
            )
            .map_err(output_error)?;
            output.flush().map_err(output_error)?;
            return Err(error);
        }
    };
    writeln!(
        output,
        "[rust-test] stage=selection-start changes={} exhaustive={exhaustive}",
        changed_sources.len(),
    )
    .map_err(output_error)?;
    output.flush().map_err(output_error)?;
    let selection_started = Instant::now();
    let selected = if exhaustive {
        exhaustive_selection_report(&inventory, source, changed_sources)
            .map(|report| (report, Vec::new()))
    } else {
        selection_plan(&inventory, root, source, changed_sources).map(|plan| plan.into_parts())
    };
    let selected = match selected {
        Ok(selected) => selected,
        Err(error) => {
            writeln!(
                output,
                "[rust-test] stage=selection-terminal elapsed_ms={} disposition=failed detail={:?}",
                selection_started.elapsed().as_millis(),
                error.to_string(),
            )
            .map_err(output_error)?;
            output.flush().map_err(output_error)?;
            return Err(error);
        }
    };
    writeln!(
        output,
        "[rust-test] stage=selection-terminal elapsed_ms={} disposition=success tests={}",
        selection_started.elapsed().as_millis(),
        selected.0.tests().len(),
    )
    .map_err(output_error)?;
    output.flush().map_err(output_error)?;
    let (report, escalations) = selected;
    write_reach_escalations(output, &escalations)?;
    for edge in report.edges() {
        writeln!(
            output,
            "[rust-test-selection] source={:?} test={:?}",
            edge.source(),
            edge.test(),
        )
        .map_err(output_error)?;
    }
    output.flush().map_err(output_error)?;
    if !report.admits() || report.tests().is_empty() {
        return Ok(report);
    }
    let environment = inventory.execution_environment(
        environment,
        root,
        roots.writable,
        roots.reusable.is_some(),
    )?;
    let selected = inventory.selected(report.tests())?;
    let shards = partition(&selected);
    validate_partition(&selected, &shards)?;
    let policy = policy.for_shards(shards.len());
    Coordinator::new(
        shards,
        root.to_path_buf(),
        environment,
        OsString::from("cargo"),
        policy,
        output,
    )
    .run()?;
    Ok(report)
}

fn write_reach_escalations(
    output: &mut impl Write,
    escalations: &[CargoReachEscalation],
) -> Result<(), WorkspaceTestError> {
    for escalation in escalations {
        match escalation.reason().package() {
            Some(package) => writeln!(
                output,
                "[rust-test-reach-escalation] source={:?} reason={} package={package:?}",
                escalation.source(),
                escalation.reason().kind(),
            ),
            None => writeln!(
                output,
                "[rust-test-reach-escalation] source={:?} reason={}",
                escalation.source(),
                escalation.reason().kind(),
            ),
        }
        .map_err(output_error)?;
    }
    if !escalations.is_empty() {
        output.flush().map_err(output_error)?;
    }
    Ok(())
}

fn reuse_environment(
    environment: Environment,
    workspace: &crate::RustAnalysisWorkspace,
) -> Result<Environment, WorkspaceTestError> {
    let mut entries = environment
        .entries()
        .iter()
        .cloned()
        .collect::<BTreeMap<OsString, OsString>>();
    entries.insert(
        OsString::from(CARGO_TARGET_DIR),
        workspace.cargo_target().as_os_str().to_owned(),
    );
    entries.insert(
        OsString::from(CARGO_BUILD_BUILD_DIR),
        workspace.cargo_build().as_os_str().to_owned(),
    );
    Environment::new(entries).map_err(|error| WorkspaceTestError::new(error.to_string()))
}

fn without_parent_repository(environment: Environment) -> Result<Environment, WorkspaceTestError> {
    let mut entries = environment
        .entries()
        .iter()
        .cloned()
        .collect::<BTreeMap<OsString, OsString>>();
    for variable in GIT_LOCAL_ENVIRONMENT {
        entries.remove(OsStr::new(variable));
    }
    Environment::new(entries).map_err(|error| WorkspaceTestError::new(error.to_string()))
}

fn without_pre_commit_dispatch(
    environment: Environment,
) -> Result<Environment, WorkspaceTestError> {
    let mut entries = environment
        .entries()
        .iter()
        .cloned()
        .collect::<BTreeMap<OsString, OsString>>();
    entries.remove(OsStr::new(PRE_COMMIT_SOURCE));
    Environment::new(entries).map_err(|error| WorkspaceTestError::new(error.to_string()))
}

fn with_direct_rustc(environment: Environment) -> Result<Environment, WorkspaceTestError> {
    let mut entries = environment
        .entries()
        .iter()
        .cloned()
        .collect::<BTreeMap<OsString, OsString>>();
    entries.insert(OsString::from(RUSTC_WRAPPER), OsString::new());
    Environment::new(entries).map_err(|error| WorkspaceTestError::new(error.to_string()))
}

#[derive(Clone, Debug, Deserialize)]
struct Metadata {
    packages: Vec<MetadataPackage>,
    workspace_members: Vec<String>,
    #[serde(default)]
    target_directory: PathBuf,
    #[serde(default)]
    build_directory: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
struct MetadataPackage {
    id: String,
    name: String,
    manifest_path: PathBuf,
    #[serde(default)]
    dependencies: Vec<MetadataDependency>,
    targets: Vec<MetadataTarget>,
    #[serde(default)]
    features: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Deserialize)]
struct MetadataDependency {
    path: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
struct MetadataTarget {
    name: String,
    kind: Vec<String>,
    test: bool,
    src_path: PathBuf,
    #[serde(rename = "required-features", default)]
    required_features: Vec<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct BinaryOutput {
    package_id: String,
    package_name: String,
    target_name: String,
}

struct MissingBinaryOutput<'a> {
    binary: &'a BinaryOutput,
    path: PathBuf,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TargetIdentity {
    package: String,
    kind: String,
    name: String,
    required_features: Vec<String>,
    source: PathBuf,
    declared_path_sources: Vec<PathBuf>,
    source_bytes: u64,
}

impl TargetIdentity {
    fn display(&self) -> String {
        format!("{}:{}", self.kind, self.name)
    }
}

#[derive(Clone, Debug)]
struct PackageInventory {
    id: String,
    name: String,
    root: PathBuf,
    dependencies: Vec<PathBuf>,
    declared_targets: Vec<TargetIdentity>,
    targets: Vec<TargetIdentity>,
}

#[derive(Clone, Debug)]
struct Inventory {
    packages: Vec<PackageInventory>,
    binary_outputs: Vec<BinaryOutput>,
    target_directory: PathBuf,
    build_directory: PathBuf,
}

impl Inventory {
    fn execution_environment(
        &self,
        environment: Environment,
        selected_root: &Path,
        writable_root: &Path,
        allow_configured_external: bool,
    ) -> Result<Environment, WorkspaceTestError> {
        let selected_root =
            fs::canonicalize(selected_root).unwrap_or_else(|_| selected_root.to_path_buf());
        let writable_root =
            fs::canonicalize(writable_root).unwrap_or_else(|_| writable_root.to_path_buf());
        let target = writable_directory(
            &self.target_directory,
            &selected_root,
            &writable_root.join("target"),
            allow_configured_external,
        );
        let build = writable_directory(
            &self.build_directory,
            &selected_root,
            &writable_root.join("target/cargo-build"),
            allow_configured_external,
        );
        let mut entries = environment
            .entries()
            .iter()
            .cloned()
            .collect::<BTreeMap<OsString, OsString>>();
        entries.insert(OsString::from(CARGO_TARGET_DIR), target.into_os_string());
        entries.insert(
            OsString::from(CARGO_BUILD_BUILD_DIR),
            build.into_os_string(),
        );
        entries.insert(
            OsString::from(TEST_WRITABLE_ROOT),
            writable_root.into_os_string(),
        );
        Environment::new(entries).map_err(|error| WorkspaceTestError::new(error.to_string()))
    }

    fn selected(&self, tests: &[String]) -> Result<Self, WorkspaceTestError> {
        let selected = tests.iter().cloned().collect::<BTreeSet<_>>();
        let mut packages = self
            .packages
            .iter()
            .filter_map(|package| {
                let targets = package
                    .targets
                    .iter()
                    .filter(|target| {
                        selected.contains(&format!("{}::{}", package.name, target.display()))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                (!targets.is_empty()).then(|| PackageInventory {
                    id: package.id.clone(),
                    name: package.name.clone(),
                    root: package.root.clone(),
                    dependencies: package.dependencies.clone(),
                    declared_targets: package.declared_targets.clone(),
                    targets,
                })
            })
            .collect::<Vec<_>>();
        packages.sort_by(|left, right| (&left.name, &left.id).cmp(&(&right.name, &right.id)));
        let observed = packages
            .iter()
            .flat_map(|package| {
                package
                    .targets
                    .iter()
                    .map(|target| format!("{}::{}", package.name, target.display()))
            })
            .collect::<BTreeSet<_>>();
        if observed != selected {
            return Err(WorkspaceTestError::new(format!(
                "selected test inventory drifted: expected {selected:?}, observed {observed:?}"
            )));
        }
        Ok(Self {
            packages,
            binary_outputs: self.binary_outputs.clone(),
            target_directory: self.target_directory.clone(),
            build_directory: self.build_directory.clone(),
        })
    }
}

fn writable_directory(
    configured: &Path,
    selected_root: &Path,
    local: &Path,
    allow_configured_external: bool,
) -> PathBuf {
    if configured.as_os_str().is_empty()
        || configured.starts_with(selected_root)
        || !allow_configured_external
    {
        local.to_path_buf()
    } else {
        configured.to_path_buf()
    }
}

fn acquire_inventory(
    root: &Path,
    environment: &Environment,
    cargo: &OsStr,
) -> Result<Inventory, WorkspaceTestError> {
    let spec = process_spec(
        cargo,
        ["metadata", "--locked", "--format-version", "1", "--no-deps"],
        root,
        environment.clone(),
        METADATA_DEADLINE,
        Cancellation::Never,
        "workspace-test metadata",
    )?;
    let outcome = sill::spawn(spec)
        .map_err(|error| WorkspaceTestError::new(format!("metadata launch failed: {error}")))?
        .wait()
        .map_err(|error| WorkspaceTestError::new(format!("metadata wait failed: {error}")))?;
    metadata_from_outcome(outcome, root)
}

fn metadata_from_outcome(
    outcome: WaitOutcome,
    root: &Path,
) -> Result<Inventory, WorkspaceTestError> {
    match outcome.completion() {
        Completion::Exited(status) if status.success() => {}
        completion => {
            return Err(WorkspaceTestError::new(format!(
                "cargo metadata ended as {}: {}",
                completion_name(completion),
                captured_excerpt(outcome.stderr())
            )));
        }
    }
    let stdout = outcome
        .stdout()
        .ok_or_else(|| WorkspaceTestError::new("cargo metadata stdout was not captured"))?;
    if stdout.truncated() || !stdout.complete() || !outcome.outputs_drained() {
        return Err(WorkspaceTestError::new(
            "cargo metadata output exceeded or escaped its capture bound",
        ));
    }
    let metadata: Metadata = serde_json::from_slice(stdout.bytes())
        .map_err(|error| WorkspaceTestError::new(format!("cargo metadata was invalid: {error}")))?;
    inventory_from_metadata(metadata, root)
}

fn inventory_from_metadata(
    metadata: Metadata,
    root: &Path,
) -> Result<Inventory, WorkspaceTestError> {
    let target_directory = metadata.target_directory.clone();
    let build_directory = metadata.build_directory.clone();
    let members = metadata
        .workspace_members
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut binary_outputs = metadata
        .packages
        .iter()
        .filter(|package| members.contains(&package.id))
        .flat_map(package_binary_outputs)
        .collect::<Vec<_>>();
    binary_outputs.sort();
    let mut packages = metadata
        .packages
        .into_iter()
        .filter(|package| members.contains(&package.id))
        .map(|package| package_inventory(package, root))
        .collect::<Result<Vec<_>, _>>()?;
    packages.sort_by(|left, right| (&left.name, &left.id).cmp(&(&right.name, &right.id)));
    if packages.is_empty() {
        return Err(WorkspaceTestError::new(
            "cargo metadata named no workspace packages",
        ));
    }
    validate_unique_workspace_packages(&packages)?;
    validate_workspace_dependencies(&packages, root)?;
    Ok(Inventory {
        packages,
        binary_outputs,
        target_directory,
        build_directory,
    })
}

fn package_binary_outputs(package: &MetadataPackage) -> Vec<BinaryOutput> {
    let enabled = default_feature_closure(&package.features);
    package
        .targets
        .iter()
        .filter(|target| target.kind.iter().any(|kind| kind == "bin"))
        .filter(|target| {
            target
                .required_features
                .iter()
                .all(|feature| enabled.contains(feature))
        })
        .map(|target| BinaryOutput {
            package_id: package.id.clone(),
            package_name: package.name.clone(),
            target_name: target.name.clone(),
        })
        .collect()
}

fn default_feature_closure(features: &BTreeMap<String, Vec<String>>) -> BTreeSet<String> {
    let mut enabled = BTreeSet::new();
    let mut pending = features
        .contains_key("default")
        .then(|| "default".to_owned())
        .into_iter()
        .collect::<VecDeque<_>>();
    while let Some(feature) = pending.pop_front() {
        if !enabled.insert(feature.clone()) {
            continue;
        }
        for member in features.get(&feature).into_iter().flatten() {
            if features.contains_key(member) && !enabled.contains(member) {
                pending.push_back(member.clone());
            }
        }
    }
    enabled
}

fn validate_unique_workspace_packages(
    packages: &[PackageInventory],
) -> Result<(), WorkspaceTestError> {
    let mut names = BTreeSet::new();
    let mut roots = BTreeMap::new();
    for package in packages {
        if !names.insert(package.name.as_str()) {
            return Err(WorkspaceTestError::new(format!(
                "workspace package name {:?} is ambiguous for Cargo --package",
                package.name
            )));
        }
        if let Some(previous) = roots.insert(package.root.as_path(), package.name.as_str()) {
            return Err(WorkspaceTestError::new(format!(
                "workspace packages {previous:?} and {:?} share root {}",
                package.name,
                package.root.display(),
            )));
        }
    }
    Ok(())
}

fn validate_workspace_dependencies(
    packages: &[PackageInventory],
    root: &Path,
) -> Result<(), WorkspaceTestError> {
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    for package in packages {
        for dependency in &package.dependencies {
            if dependency.starts_with(&root)
                && !packages
                    .iter()
                    .any(|candidate| candidate.root == *dependency)
            {
                return Err(WorkspaceTestError::new(format!(
                    "workspace package {:?} depends on unowned local path {}",
                    package.name,
                    dependency.display(),
                )));
            }
        }
    }
    Ok(())
}

include!("workspace_tests/source_reach.rs");

fn partition(inventory: &Inventory) -> Vec<PackageInventory> {
    partition_with_target_limit(inventory, TARGETS_PER_SHARD)
}

fn partition_with_target_limit(
    inventory: &Inventory,
    targets_per_shard: usize,
) -> Vec<PackageInventory> {
    debug_assert!(targets_per_shard > 0);
    let package_target_counts = inventory
        .packages
        .iter()
        .map(|package| (package.id.clone(), package.targets.len()))
        .collect::<BTreeMap<_, _>>();
    let mut shards = inventory
        .packages
        .iter()
        .flat_map(|package| partition_package(package, targets_per_shard))
        .collect::<Vec<_>>();
    shards.sort_by(|left, right| {
        let left_startup_dominated =
            targets_per_shard != 1 && left.targets.len() <= CARGO_STARTUP_DOMINATED_TARGET_LIMIT;
        let right_startup_dominated =
            targets_per_shard != 1 && right.targets.len() <= CARGO_STARTUP_DOMINATED_TARGET_LIMIT;
        let left_package_targets = package_target_counts[&left.id];
        let right_package_targets = package_target_counts[&right.id];
        let left_source_bytes = left
            .targets
            .iter()
            .map(|target| target.source_bytes)
            .sum::<u64>();
        let right_source_bytes = right
            .targets
            .iter()
            .map(|target| target.source_bytes)
            .sum::<u64>();
        left_startup_dominated
            .cmp(&right_startup_dominated)
            .then_with(|| {
                if left_startup_dominated {
                    Ordering::Equal
                } else {
                    right_source_bytes
                        .cmp(&left_source_bytes)
                        .then_with(|| right.targets.len().cmp(&left.targets.len()))
                        .then_with(|| right_package_targets.cmp(&left_package_targets))
                }
            })
            .then_with(|| left.id.cmp(&right.id))
            .then_with(|| left.targets.cmp(&right.targets))
    });
    shards
}

fn partition_package(
    package: &PackageInventory,
    targets_per_shard: usize,
) -> Vec<PackageInventory> {
    let shard_count = package.targets.len().div_ceil(targets_per_shard);
    let mut weighted_targets = package
        .targets
        .iter()
        .map(|target| (target.source_bytes, target))
        .collect::<Vec<_>>();
    weighted_targets.sort_by(|(left_weight, left), (right_weight, right)| {
        right_weight.cmp(left_weight).then_with(|| left.cmp(right))
    });
    let mut targets_by_shard = vec![Vec::new(); shard_count];
    let mut weights_by_shard = vec![0_u64; shard_count];
    for (weight, target) in weighted_targets {
        let index = (0..shard_count)
            .filter(|index| targets_by_shard[*index].len() < targets_per_shard)
            .min_by_key(|index| {
                (
                    weights_by_shard[*index],
                    targets_by_shard[*index].len(),
                    *index,
                )
            })
            .expect("minimum shard count retains target capacity");
        targets_by_shard[index].push(target.clone());
        weights_by_shard[index] = weights_by_shard[index].saturating_add(weight);
    }
    for targets in &mut targets_by_shard {
        targets.sort();
    }
    targets_by_shard
        .into_iter()
        .map(|targets| PackageInventory {
            id: package.id.clone(),
            name: package.name.clone(),
            root: package.root.clone(),
            dependencies: package.dependencies.clone(),
            declared_targets: package.declared_targets.clone(),
            targets,
        })
        .collect()
}

fn target_source_bytes(
    source: &Path,
    declared_path_sources: &[PathBuf],
) -> Result<u64, WorkspaceTestError> {
    std::iter::once(source)
        .chain(declared_path_sources.iter().map(PathBuf::as_path))
        .try_fold(0_u64, |total, path| {
            fs::metadata(path)
                .map(|metadata| total.saturating_add(metadata.len()))
                .map_err(|error| {
                    WorkspaceTestError::new(format!(
                        "workspace test target source {} metadata failed: {error}",
                        path.display()
                    ))
                })
        })
        .map(|bytes| bytes.max(1))
}

fn validate_partition(
    inventory: &Inventory,
    shards: &[PackageInventory],
) -> Result<(), WorkspaceTestError> {
    validate_partition_with_target_limit(inventory, shards, TARGETS_PER_SHARD)
}

fn validate_partition_with_target_limit(
    inventory: &Inventory,
    shards: &[PackageInventory],
    targets_per_shard: usize,
) -> Result<(), WorkspaceTestError> {
    let expected_shards = partition_with_target_limit(inventory, targets_per_shard);
    let expected_packages = counts(expected_shards.iter().map(|package| package.id.clone()));
    let actual_packages = counts(shards.iter().map(|package| package.id.clone()));
    let expected_targets = counts(inventory.packages.iter().flat_map(|package| {
        package
            .targets
            .iter()
            .cloned()
            .map(|target| (package.id.clone(), target))
    }));
    let actual_targets = counts(shards.iter().flat_map(|package| {
        package
            .targets
            .iter()
            .cloned()
            .map(|target| (package.id.clone(), target))
    }));
    let mut differences = count_differences("package", &expected_packages, &actual_packages);
    differences.extend(count_differences(
        "target",
        &expected_targets,
        &actual_targets,
    ));
    if differences.is_empty() {
        Ok(())
    } else {
        Err(WorkspaceTestError::new(format!(
            "shard partition is not exact: {}",
            differences.join("; ")
        )))
    }
}

fn counts<T: Ord>(values: impl IntoIterator<Item = T>) -> BTreeMap<T, usize> {
    let mut counts = BTreeMap::new();
    for value in values {
        *counts.entry(value).or_default() += 1;
    }
    counts
}

fn count_differences<T: Ord + fmt::Debug>(
    kind: &str,
    expected: &BTreeMap<T, usize>,
    actual: &BTreeMap<T, usize>,
) -> Vec<String> {
    expected
        .keys()
        .chain(actual.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|key| {
            let expected = expected.get(key).copied().unwrap_or_default();
            let actual = actual.get(key).copied().unwrap_or_default();
            (expected != actual).then(|| {
                format!("{kind} {key:?} expected {expected} occurrence(s), observed {actual}")
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
