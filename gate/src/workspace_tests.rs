//! Deterministic, bounded execution of every workspace Rust test target.
//! Inventory and selection stay here; shard execution lives in `coordinator`.

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

use coordinator::{
    Coordinator, RunPolicy, captured_excerpt, completion_name, output_error, process_spec,
};
#[cfg(test)]
use coordinator::{LastTest, isolate_shard_target, target_arguments};

const TARGETS_PER_SHARD: usize = 12;
const SHARD_DEADLINE: Duration = Duration::from_secs(180);
const COORDINATOR_DEADLINE: Duration = Duration::from_secs(180);
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
const TEST_WRITABLE_ROOT: &str = "SPICE_WORKSPACE_TEST_WRITABLE_ROOT"; // env-policy: allow
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
    let policy = RunPolicy::production(concurrency)?;
    let inventory = acquire_inventory(root, &environment, OsStr::new("cargo"))?;
    let environment = inventory.execution_environment(environment, root, root)?;
    let execution = WorkspaceTestExecution::from_inventory(&inventory);
    let shards = partition(&inventory);
    validate_partition(&inventory, &shards)?;
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

/// Select and run only test targets reached by staged Rust sources according to
/// canonical Cargo target roots. A reached library target expands to every
/// test-enabled target in that package because those targets may link the
/// library. A non-library target reaches only its root and exact `#[path]`
/// module declarations.
///
/// Unknown or incomplete package reach returns a non-admitting typed report
/// without launching a narrowed test set. A complete report is emitted only
/// after every selected target succeeds through this same coordinator.
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
    let policy = RunPolicy::production(concurrency)?;
    let root = roots
        .reusable
        .map_or(roots.selected, crate::RustAnalysisWorkspace::source);
    let environment = without_parent_repository(environment)?;
    let environment = match roots.reusable {
        Some(workspace) => reuse_environment(environment, workspace)?,
        None => environment,
    };
    let inventory = acquire_inventory(root, &environment, OsStr::new("cargo"))?;
    let report = selection_report(&inventory, root, source, changed_sources)?;
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
    let environment = inventory.execution_environment(environment, root, roots.writable)?;
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
    targets: Vec<MetadataTarget>,
}

#[derive(Clone, Debug, Deserialize)]
struct MetadataTarget {
    name: String,
    kind: Vec<String>,
    test: bool,
    src_path: PathBuf,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TargetIdentity {
    package: String,
    kind: String,
    name: String,
    source: PathBuf,
    declared_path_sources: Vec<PathBuf>,
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
    targets: Vec<TargetIdentity>,
}

#[derive(Clone, Debug)]
struct Inventory {
    packages: Vec<PackageInventory>,
    target_directory: PathBuf,
    build_directory: PathBuf,
}

impl Inventory {
    fn execution_environment(
        &self,
        environment: Environment,
        selected_root: &Path,
        writable_root: &Path,
    ) -> Result<Environment, WorkspaceTestError> {
        let selected_root =
            fs::canonicalize(selected_root).unwrap_or_else(|_| selected_root.to_path_buf());
        let writable_root =
            fs::canonicalize(writable_root).unwrap_or_else(|_| writable_root.to_path_buf());
        let target = writable_directory(
            &self.target_directory,
            &selected_root,
            &writable_root.join("target"),
        );
        let build = writable_directory(
            &self.build_directory,
            &selected_root,
            &writable_root.join("target/cargo-build"),
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
            target_directory: self.target_directory.clone(),
            build_directory: self.build_directory.clone(),
        })
    }
}

fn writable_directory(configured: &Path, selected_root: &Path, local: &Path) -> PathBuf {
    if configured.as_os_str().is_empty() || configured.starts_with(selected_root) {
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
    Ok(Inventory {
        packages,
        target_directory,
        build_directory,
    })
}

fn package_inventory(
    package: MetadataPackage,
    root: &Path,
) -> Result<PackageInventory, WorkspaceTestError> {
    let selected_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let manifest = if package.manifest_path.is_absolute() {
        package.manifest_path.clone()
    } else {
        selected_root.join(&package.manifest_path)
    };
    let manifest = fs::canonicalize(&manifest).unwrap_or(manifest);
    let package_root = manifest.parent().ok_or_else(|| {
        WorkspaceTestError::new(format!(
            "workspace package {:?} has no manifest parent",
            package.name
        ))
    })?;
    if !package_root.starts_with(&selected_root) {
        return Err(WorkspaceTestError::new(format!(
            "workspace package {:?} manifest {} escapes selected root {}",
            package.name,
            manifest.display(),
            selected_root.display(),
        )));
    }
    let mut targets = package
        .targets
        .into_iter()
        .filter(|target| target.test)
        .map(|target| {
            let source = if target.src_path.is_absolute() {
                target.src_path
            } else {
                selected_root.join(target.src_path)
            };
            let source = fs::canonicalize(&source).unwrap_or(source);
            if !source.starts_with(&selected_root) {
                return Err(WorkspaceTestError::new(format!(
                    "workspace package {:?} target source {} escapes selected root {}",
                    package.name,
                    source.display(),
                    selected_root.display(),
                )));
            }
            let declared_path_sources = declared_path_sources(&source)?;
            Ok(TargetIdentity {
                package: package.id.clone(),
                kind: target.kind.join("+"),
                name: target.name,
                source,
                declared_path_sources,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    targets.sort();
    Ok(PackageInventory {
        id: package.id,
        name: package.name,
        root: package_root.to_path_buf(),
        targets,
    })
}

fn selection_report(
    inventory: &Inventory,
    root: &Path,
    source: wire::Digest,
    changed_sources: &[String],
) -> Result<crate::TestSelectionReport, WorkspaceTestError> {
    let selected_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut rust_sources = changed_sources
        .iter()
        .filter(|path| Path::new(path.as_str()).extension() == Some(OsStr::new("rs")))
        .cloned()
        .collect::<Vec<_>>();
    rust_sources.sort();
    rust_sources.dedup();
    let mut mapped = 0usize;
    let mut gaps = Vec::new();
    let mut tests = BTreeSet::new();
    let mut edges = BTreeSet::new();
    for changed in &rust_sources {
        let absolute = selected_root.join(changed);
        let owner = inventory
            .packages
            .iter()
            .filter(|package| absolute.starts_with(&package.root))
            .max_by_key(|package| package.root.components().count());
        match owner {
            Some(package) => {
                let direct_targets = package
                    .targets
                    .iter()
                    .filter(|target| target_reaches_source(target, &absolute))
                    .collect::<Vec<_>>();
                if direct_targets.is_empty() {
                    gaps.push(format!(
                        "changed Rust source {changed:?} reaches no direct Cargo test target"
                    ));
                } else {
                    let reaches_library = direct_targets
                        .iter()
                        .any(|target| target_is_library(target));
                    let reached_targets = if reaches_library {
                        package.targets.iter().collect::<Vec<_>>()
                    } else {
                        direct_targets
                    };
                    for target in reached_targets {
                        let test = format!("{}::{}", package.name, target.display());
                        tests.insert(test.clone());
                        edges.insert(
                            crate::TestSelectionEdge::new(changed.clone(), test)
                                .map_err(|error| WorkspaceTestError::new(error.to_string()))?,
                        );
                    }
                    mapped += 1;
                }
            }
            None => gaps.push(format!(
                "changed Rust source {changed:?} is outside every workspace package"
            )),
        }
    }
    let coverage = if gaps.is_empty() {
        crate::CoverageCompleteness::Complete
    } else if mapped == 0 {
        crate::CoverageCompleteness::unknown(gaps.join("; "))
            .map_err(|error| WorkspaceTestError::new(error.to_string()))?
    } else {
        crate::CoverageCompleteness::incomplete(gaps.join("; "))
            .map_err(|error| WorkspaceTestError::new(error.to_string()))?
    };
    crate::TestEvidenceGraph::select_tests(
        source,
        rust_sources,
        tests.into_iter().collect(),
        edges.into_iter().collect(),
        coverage,
    )
    .map_err(|error| WorkspaceTestError::new(error.to_string()))
}

/// Resolve only reach that the Cargo target root or an exact Rust `#[path]`
/// module declaration establishes; ordinary directory proximity is not reach.
fn target_reaches_source(target: &TargetIdentity, source: &Path) -> bool {
    if source == target.source {
        return true;
    }
    let direct_root = target_is_library(target)
        .then(|| target.source.parent())
        .flatten();
    direct_root.is_some_and(|root| source.starts_with(root))
        || target
            .declared_path_sources
            .iter()
            .any(|declared| declared == source)
}

include!("workspace_tests/source_reach.rs");

fn target_is_library(target: &TargetIdentity) -> bool {
    target.kind.split('+').any(|kind| {
        matches!(
            kind,
            "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro"
        )
    })
}

fn partition(inventory: &Inventory) -> Vec<PackageInventory> {
    inventory
        .packages
        .iter()
        .flat_map(|package| {
            package
                .targets
                .chunks(TARGETS_PER_SHARD)
                .map(|targets| PackageInventory {
                    id: package.id.clone(),
                    name: package.name.clone(),
                    root: package.root.clone(),
                    targets: targets.to_vec(),
                })
        })
        .collect()
}

fn validate_partition(
    inventory: &Inventory,
    shards: &[PackageInventory],
) -> Result<(), WorkspaceTestError> {
    let expected_shards = partition(inventory);
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
