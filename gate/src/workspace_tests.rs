//! Deterministic bounded workspace tests; shard execution lives in `coordinator`.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use sill::{
    Argv, Cancellation, CancellationToken, Completion, Deadlines, Environment, Observation,
    ObserveError, ObservedRecord, Output, OutputStream, ProcessGroup, RecordObserver, Redaction,
    RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitOutcome, WaitPolicy, WaitStep,
};
use syn::{Expr, Item, Lit, Meta};

use crate::WORKSPACE_TEST_CONCURRENCY_CAP;

mod compile_time;
mod coordinator;
mod inventory;
mod prebuild;
mod proof;
mod selection;
mod source_reach;

use compile_time::{CompileTimeSourceAnalysis, compile_time_authorities};
use coordinator::{
    Coordinator, RunPolicy, captured_excerpt, completion_name, output_error, process_spec,
    target_arguments,
};
pub use inventory::cargo_declared_compile_time_sources_at;
use inventory::*;
pub(crate) use inventory::{RustCheckUnit, workspace_package_reach_at};
use prebuild::*;
pub use proof::{
    CargoExternalSourceAuthority, CargoGateShard, CargoGateShardKind, CargoGateShardPlan,
    CargoGateShardPlanning, CargoGateShardResult, plan_cargo_gate_shards_at,
    run_cargo_gate_shard_at, windows_typechecks_declared_at,
};
use selection::{package_inventory, selection_plan};
use source_reach::{FileMacroCrateContext, declared_path_sources};

/// Whether the compiled Cargo workspace engine applies to this selected tree.
///
/// This predicate runs before metadata or tool discovery, so an undeclared or
/// irrelevant engine cannot make Cargo a dependency of a generic repository.
#[must_use]
pub fn cargo_workspace_tests_applicable(root: &Path, changed_sources: &[String]) -> bool {
    root.join("Cargo.toml").is_file()
        && changed_sources
            .iter()
            .any(|path| selection::test_relevant_source(path))
}

/// Whether exact ownership of a removed Rust source requires the captured base tree.
///
/// # Errors
///
/// Refuses filesystem states other than a present entry or an absent path;
/// callers must not reinterpret an unreadable selected source as a deletion.
pub fn cargo_workspace_base_inventory_required(
    root: &Path,
    changed_sources: &[String],
) -> Result<bool, WorkspaceTestError> {
    for changed in changed_sources
        .iter()
        .filter(|path| Path::new(path).extension() == Some(OsStr::new("rs")))
    {
        let path = root.join(changed);
        match fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
            Err(error) => {
                return Err(WorkspaceTestError::new(format!(
                    "changed Rust source {changed:?} is unavailable at {}: {error}",
                    path.display(),
                )));
            }
        }
    }
    Ok(false)
}

/// Keep each post-prebuild Cargo invocation small enough for the full verifier's
/// bounded workspace branch to make steady progress under the absolute verifier cap.
/// Targets are source-weight balanced across the minimum shard count so one
/// cluster of larger integration targets cannot pin the coordinator tail.
const TARGETS_PER_SHARD: usize = 8;
/// A selected local run has already compiled every reached target in one shared
/// prebuild, so its remaining Cargo frontends are launch coordinators rather
/// than compiler work. Keep each package in fewer, broader groups to avoid
/// spending the enclosing gate deadline on shared build-directory lock waves.
const SELECTED_PREBUILT_TARGETS_PER_SHARD: usize = 24;
/// Proof actions are independently bounded Cargo commands, not launches over
/// the local coordinator's shared prebuild. Reuse the ordinary target cap so a
/// large package cannot put a long integration tail under one shard deadline.
const PROOF_TARGETS_PER_SHARD: usize = TARGETS_PER_SHARD;
/// Exact selected prebuild batches share one Cargo graph and output authority.
/// Admit only two package batches and divide the aggregate compiler grant
/// between their stable worker lanes. This retains package overlap without
/// recreating a four-way Cargo lock wave over their shared graph and output
/// authority.
const SELECTED_PREBUILD_CONCURRENCY: usize = 2;
/// Routine selected runs execute each fully built target once. Their target
/// budget already bounds the process count, while test-list discovery costs
/// more than the selected harnesses it would split.
const ROUTINE_PARTITIONS_PER_TARGET: usize = 1;
/// Retain finer work units inside the full verifier so a constrained two-lane
/// schedule can rebalance long test targets without paying one Cargo launch
/// for every target.
const FULL_VERIFIER_TARGETS_PER_SHARD: usize = 4;
/// Below three targets, observed Cargo startup and publication time outweighs
/// target cardinality, so these tail batches retain stable metadata order.
const CARGO_STARTUP_DOMINATED_TARGET_LIMIT: usize = 2;
/// No individual test shard may cross its local three-minute stall ceiling.
/// The finite inventory and this per-shard bound compose the coordinator bound;
/// a separate aggregate cutoff could cancel healthy shards in later waves.
const SHARD_DEADLINE: Duration = Duration::from_secs(180);
const PROGRESS_INTERVAL: Duration = Duration::from_secs(30);
const METADATA_DEADLINE: Duration = Duration::from_secs(30);
const OUTPUT_DRAIN_DEADLINE: Duration = Duration::from_secs(5);
const REAP_DEADLINE: Duration = Duration::from_secs(5);
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
const CAPTURE_LIMIT: usize = 4 * 1024 * 1024;
const FAILURE_EXCERPT_LIMIT: usize = 16 * 1024;
const RECORD_FRAME_LIMIT: usize = 256 * 1024;
const RECORD_QUEUE_CAPACITY: usize = 4_096;
const CARGO_BUILD_JOBS: &str = "CARGO_BUILD_JOBS";
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
    /// Internal classification used only to keep sibling cancellation from becoming the cause.
    synthetic_cancellation: bool,
}

impl WorkspaceTestError {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
            synthetic_cancellation: false,
        }
    }

    fn synthetic_cancellation(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
            synthetic_cancellation: true,
        }
    }

    fn is_synthetic_cancellation(&self) -> bool {
        self.synthetic_cancellation
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
    base: Option<&'a Path>,
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

    fn prebuild_job_grants(self, active_batches: usize) -> Vec<usize> {
        let lanes = active_batches
            .min(self.concurrency)
            .min(SELECTED_PREBUILD_CONCURRENCY);
        if lanes == 0 {
            return Vec::new();
        }
        let aggregate = self.concurrency.saturating_mul(self.cargo_jobs);
        let jobs_per_lane = aggregate / lanes;
        let remainder = aggregate % lanes;
        (0..lanes)
            .map(|lane| jobs_per_lane + usize::from(lane < remainder))
            .collect()
    }
}

impl<'a> WorkspaceTestRoots<'a> {
    /// Bind an immutable selected root to its live worktree's output owner.
    #[must_use]
    pub const fn new(selected: &'a Path, writable: &'a Path) -> Self {
        Self {
            selected,
            writable,
            base: None,
            reusable: None,
        }
    }

    /// Bind the exact committed base retained beside a staged selection.
    #[must_use]
    pub const fn with_base(mut self, base: &'a Path) -> Self {
        self.base = Some(base);
        self
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
/// then reuses that completed native build cache without repeating setup
/// inside the test clock.
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
    prebuild_app_launcher(root, &environment, OsStr::new("cargo"), output)
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

/// Select and run only test targets reached by staged build sources according
/// to canonical Cargo metadata. An ordinary library change reaches its own
/// unit harness and the library/binary unit harnesses of reverse dependents;
/// process-heavy integration targets run when changed directly or through the
/// full verifier. A non-library target reaches only its root and exact `#[path]`
/// module declarations. Package manifests and build scripts reach their whole
/// package; workspace/toolchain and gate-policy authorities reach the complete
/// workspace because their effect cannot be narrowed further.
///
/// Present Rust sources must resolve to a declared Cargo target. Removed,
/// unclaimed inputs conservatively reach their containing package because they
/// may have been runtime fixture data rather than modules. Sources outside any
/// package or with no proven test-enabled reach refuse before a compiler or test
/// process starts. A complete report is emitted only after every selected target
/// succeeds through this coordinator.
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
) -> Result<crate::TestSelectionReport, WorkspaceTestError> {
    let (policy, prebuild_budget) = match selected_policy {
        SelectedRunPolicy::Legacy(concurrency) => (RunPolicy::production(concurrency)?, None),
        SelectedRunPolicy::Budgeted(budget) => {
            (RunPolicy::production_budgeted(budget), Some(budget))
        }
    };
    let root = roots
        .reusable
        .map_or(roots.selected, crate::RustAnalysisWorkspace::source);
    let environment = selected_run_environment(roots, environment)?;
    let inventory =
        acquire_selected_inventory(root, roots.base, &environment, changed_sources, output)?;
    let report = selected_report(&inventory, root, source, changed_sources, output)?;
    let routine = selection::routine_selection(changed_sources, report.tests().len());
    let (selection_class, target_limit, shard_limit) = if routine {
        let limit = selection::ROUTINE_TEST_TARGET_LIMIT.to_string();
        ("routine", limit.clone(), limit)
    } else {
        ("authority", "exempt".to_owned(), "exempt".to_owned())
    };
    writeln!(
        output,
        "[rust-test-selection] detail=canonical-report class={} sources={} tests={} edges={} target_limit={} shard_limit={}",
        selection_class,
        report.changed_sources().len(),
        report.tests().len(),
        report.edges().len(),
        target_limit,
        shard_limit,
    )
    .map_err(output_error)?;
    output.flush().map_err(output_error)?;
    if report.admits() && !report.tests().is_empty() {
        execute_selected_tests(
            roots,
            root,
            environment,
            &inventory,
            report.tests(),
            SelectedExecutionPolicy {
                routine,
                run: policy,
                prebuild_budget,
            },
            output,
        )?;
    }
    Ok(report)
}

fn selected_run_environment(
    roots: WorkspaceTestRoots<'_>,
    environment: Environment,
) -> Result<Environment, WorkspaceTestError> {
    let environment = without_pre_commit_dispatch(environment)?;
    let environment = with_direct_rustc(environment)?;
    let environment = without_parent_repository(environment)?;
    Ok(match roots.reusable {
        Some(workspace) => reuse_environment(environment, workspace)?,
        None => environment,
    })
}

fn acquire_selected_inventory(
    root: &Path,
    base: Option<&Path>,
    environment: &Environment,
    changed_sources: &[String],
    output: &mut impl Write,
) -> Result<Inventory, WorkspaceTestError> {
    writeln!(
        output,
        "[rust-test] stage=metadata-start changes={}",
        changed_sources.len(),
    )
    .map_err(output_error)?;
    output.flush().map_err(output_error)?;
    let metadata_started = Instant::now();
    let mut inventory = match acquire_inventory(root, environment, OsStr::new("cargo")) {
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
    if cargo_workspace_base_inventory_required(root, changed_sources)? {
        let base = base.ok_or_else(|| {
            WorkspaceTestError::new(
                "removed Rust source requires exact base-tree Cargo ownership inventory",
            )
        })?;
        let base_inventory = acquire_inventory(base, environment, OsStr::new("cargo"))?;
        inventory.retain_removed_target_ownership(root, &base_inventory, base, changed_sources)?;
    }
    Ok(inventory)
}

fn selected_report(
    inventory: &Inventory,
    root: &Path,
    source: wire::Digest,
    changed_sources: &[String],
    output: &mut impl Write,
) -> Result<crate::TestSelectionReport, WorkspaceTestError> {
    writeln!(
        output,
        "[rust-test] stage=selection-start changes={}",
        changed_sources.len(),
    )
    .map_err(output_error)?;
    output.flush().map_err(output_error)?;
    let selection_started = Instant::now();
    let selected = selection_plan(inventory, root, source, changed_sources)
        .map(selection::SelectionPlan::into_report);
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
        selected.tests().len(),
    )
    .map_err(output_error)?;
    output.flush().map_err(output_error)?;
    Ok(selected)
}

/// How one selected test run is driven: whether the selection is routine, the
/// run policy it starts under, and the prebuild budget that may replace it.
struct SelectedExecutionPolicy {
    routine: bool,
    run: RunPolicy,
    prebuild_budget: Option<WorkspaceTestBudget>,
}

fn execute_selected_tests(
    roots: WorkspaceTestRoots<'_>,
    root: &Path,
    environment: Environment,
    inventory: &Inventory,
    tests: &[String],
    policy: SelectedExecutionPolicy,
    output: &mut impl Write,
) -> Result<(), WorkspaceTestError> {
    let SelectedExecutionPolicy {
        routine,
        run: mut policy,
        prebuild_budget,
    } = policy;
    let execution_started = Instant::now();
    let (environment, selected, shards) =
        prepare_selected_execution(roots, root, environment, inventory, tests, output)?;
    let prebuilt = if let Some(reusable) = roots.reusable {
        let started = Instant::now();
        writeln!(
            output,
            "[rust-test] stage=selected-prebuild targets={} batches=0 lanes=0 source=rust-policy",
            tests.len(),
        )
        .map_err(output_error)?;
        let prebuilt = prebuilt_test_plan(&selected, reusable.cargo_artifacts())?;
        writeln!(
            output,
            "[rust-test] stage=selected-prebuild-terminal elapsed_ms={} disposition=reused source=rust-policy",
            started.elapsed().as_millis(),
        )
        .map_err(output_error)?;
        output.flush().map_err(output_error)?;
        Some(prebuilt)
    } else if shards.len() > 1 {
        Some(prebuild_selected_tests(
            root,
            &environment,
            OsStr::new("cargo"),
            &selected,
            prebuild_budget,
            policy.concurrency.min(SELECTED_PREBUILD_CONCURRENCY),
            output,
        )?)
    } else {
        None
    };
    let prebuilt = if let Some(prebuilt) = prebuilt {
        if let Some(budget) = prebuild_budget {
            policy = RunPolicy::production_prebuilt_budgeted(budget);
        } else {
            policy.shared_outputs = true;
        }
        let partitions_per_target = selected_partitions_per_target(routine, policy.concurrency);
        let prebuilt = partition_largest_prebuilt_tests(
            prebuilt,
            &environment,
            partitions_per_target,
            policy.concurrency,
            output,
        )?;
        Some(prebuilt)
    } else {
        None
    };
    run_selected_shards(prebuilt, shards, root, environment, policy, output)?;
    writeln!(
        output,
        "[rust-test] stage=selected-execution-terminal elapsed_ms={} disposition=success",
        execution_started.elapsed().as_millis(),
    )
    .map_err(output_error)?;
    output.flush().map_err(output_error)?;
    Ok(())
}

fn run_selected_shards(
    prebuilt: Option<Vec<PrebuiltTest>>,
    shards: Vec<PackageInventory>,
    root: &Path,
    environment: Environment,
    policy: RunPolicy,
    output: &mut impl Write,
) -> Result<(), WorkspaceTestError> {
    let execution_shards = prebuilt.as_ref().map_or(shards.len(), Vec::len);
    let policy = policy.for_shards(execution_shards);
    if let Some(prebuilt) = prebuilt {
        Coordinator::new_prebuilt(
            prebuilt,
            root.to_path_buf(),
            environment,
            OsString::from("cargo"),
            policy,
            output,
        )
        .run()?;
    } else {
        Coordinator::new(
            shards,
            root.to_path_buf(),
            environment,
            OsString::from("cargo"),
            policy,
            output,
        )
        .run()?;
    }
    Ok(())
}

fn prepare_selected_execution(
    roots: WorkspaceTestRoots<'_>,
    root: &Path,
    environment: Environment,
    inventory: &Inventory,
    tests: &[String],
    output: &mut impl Write,
) -> Result<(Environment, Inventory, Vec<PackageInventory>), WorkspaceTestError> {
    let setup_started = Instant::now();
    let environment = inventory.execution_environment(
        environment,
        root,
        roots.writable,
        roots.reusable.is_some(),
    )?;
    writeln!(
        output,
        "[rust-test] stage=selected-environment-terminal elapsed_ms={} disposition=success",
        setup_started.elapsed().as_millis(),
    )
    .map_err(output_error)?;
    let inventory_started = Instant::now();
    let selected = inventory.selected(tests)?;
    writeln!(
        output,
        "[rust-test] stage=selected-inventory-terminal elapsed_ms={} disposition=success",
        inventory_started.elapsed().as_millis(),
    )
    .map_err(output_error)?;
    let units_started = Instant::now();
    write_selected_test_units(&selected, output)?;
    writeln!(
        output,
        "[rust-test] stage=selected-units-terminal elapsed_ms={} disposition=success",
        units_started.elapsed().as_millis(),
    )
    .map_err(output_error)?;
    let target_partition_started = Instant::now();
    let shards = partition_with_target_limit(&selected, SELECTED_PREBUILT_TARGETS_PER_SHARD);
    validate_partition_with_target_limit(&selected, &shards, SELECTED_PREBUILT_TARGETS_PER_SHARD)?;
    writeln!(
        output,
        "[rust-test] stage=selected-target-partition-terminal elapsed_ms={} disposition=success shards={}",
        target_partition_started.elapsed().as_millis(),
        shards.len(),
    )
    .map_err(output_error)?;
    output.flush().map_err(output_error)?;
    Ok((environment, selected, shards))
}

fn selected_partitions_per_target(routine: bool, execution_concurrency: usize) -> usize {
    if routine {
        ROUTINE_PARTITIONS_PER_TARGET
    } else {
        execution_concurrency
    }
}

fn write_selected_test_units(
    inventory: &Inventory,
    output: &mut impl Write,
) -> Result<(), WorkspaceTestError> {
    for package in &inventory.packages {
        for target in &package.targets {
            let mut argv = vec![
                "cargo".to_owned(),
                "test".to_owned(),
                "--locked".to_owned(),
                "--package".to_owned(),
                package.id.clone(),
            ];
            argv.extend(
                target_arguments(target)?
                    .into_iter()
                    .map(|argument| {
                        argument.into_string().map_err(|argument| {
                            WorkspaceTestError::new(format!(
                                "selected test target {:?} rendered non-UTF-8 argument {argument:?}",
                                target.display(),
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            );
            if let Some(features) = qualified_required_features(
                target
                    .required_features
                    .iter()
                    .map(|feature| (package.name.as_str(), feature.as_str())),
            ) {
                argv.extend(["--features".to_owned(), features]);
            }
            let identity = format!("{}::{}", package.name, target.display());
            let argv = serde_json::to_string(&argv).map_err(|error| {
                WorkspaceTestError::new(format!(
                    "selected test unit {identity:?} command could not be encoded: {error}"
                ))
            })?;
            writeln!(output, "[rust-test-unit] identity={identity:?} argv={argv}",)
                .map_err(output_error)?;
        }
    }
    output.flush().map_err(output_error)?;
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

fn with_cargo_build_jobs(
    environment: &Environment,
    jobs: usize,
) -> Result<Environment, WorkspaceTestError> {
    let mut entries = environment
        .entries()
        .iter()
        .cloned()
        .collect::<BTreeMap<OsString, OsString>>();
    entries.insert(
        OsString::from(CARGO_BUILD_JOBS),
        OsString::from(jobs.to_string()),
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
            namespace_context: package.namespace_context.clone(),
            compile_time_authority: package.compile_time_authority.clone(),
            declared_targets: package.declared_targets.clone(),
            removed_targets: package.removed_targets.clone(),
            targets,
            data_roots: package.data_roots.clone(),
        })
        .collect()
}

fn target_source_bytes(
    source: &Path,
    declared_path_sources: &[DeclaredPathSource],
) -> Result<u64, WorkspaceTestError> {
    std::iter::once(source)
        .chain(
            declared_path_sources
                .iter()
                .map(|declared| declared.path.as_path()),
        )
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
mod partition_tests {
    use super::*;

    #[test]
    fn proof_target_cap_splits_one_large_package_without_losing_determinism_or_targets() {
        let target_count = PROOF_TARGETS_PER_SHARD * 2 + 1;
        let targets = (0..target_count)
            .map(|index| TargetIdentity {
                package: "fixture".to_owned(),
                kind: "test".to_owned(),
                name: format!("target-{index:02}"),
                required_features: Vec::new(),
                source: PathBuf::from(format!("target-{index:02}.rs")),
                declared_path_sources: Vec::new(),
                source_bytes: u64::try_from(index + 1).expect("fixture weight fits u64"),
            })
            .collect::<Vec<_>>();
        let inventory = Inventory {
            packages: vec![PackageInventory {
                id: "fixture 0.0.0".to_owned(),
                name: "fixture".to_owned(),
                root: PathBuf::from("fixture"),
                dependencies: Vec::new(),
                namespace_context: FileMacroCrateContext::from_metadata("fixture", "2024", &[])
                    .expect("fixture namespace is current"),
                compile_time_authority: None,
                declared_targets: targets.clone(),
                removed_targets: Vec::new(),
                targets,
                data_roots: Vec::new(),
            }],
            moved_package_roots: BTreeMap::new(),
            resolution: CargoResolution::default(),
            binary_outputs: Vec::new(),
            target_directory: PathBuf::from("target"),
            build_directory: PathBuf::from("target/cargo-build"),
        };

        let shards = partition_with_target_limit(&inventory, PROOF_TARGETS_PER_SHARD);
        let repeated = partition_with_target_limit(&inventory, PROOF_TARGETS_PER_SHARD);

        assert_eq!(shards.len(), 3);
        assert!(
            shards
                .iter()
                .all(|shard| shard.targets.len() <= PROOF_TARGETS_PER_SHARD)
        );
        let projection = |shards: &[PackageInventory]| {
            shards
                .iter()
                .map(|shard| {
                    shard
                        .targets
                        .iter()
                        .map(|target| target.name.clone())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(projection(&shards), projection(&repeated));
        let observed = shards
            .iter()
            .flat_map(|shard| shard.targets.iter().map(|target| target.name.clone()))
            .collect::<BTreeSet<_>>();
        let expected = inventory.packages[0]
            .targets
            .iter()
            .map(|target| target.name.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(observed, expected);
    }
}
