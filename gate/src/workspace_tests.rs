//! Deterministic, bounded execution of every workspace Rust test target.

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
use syn::{Expr, Item, Lit, Meta};

use crate::WORKSPACE_TEST_CONCURRENCY_CAP;

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
    ///
    /// Complete coverage must include every executed target. It may also name
    /// unexecuted suite targets so the graph can expose a suite-seam reach edge.
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
/// library.
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
            Ok(TargetIdentity {
                package: package.id.clone(),
                kind: target.kind.join("+"),
                name: target.name,
                source,
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
                let mut direct_targets = Vec::new();
                for target in &package.targets {
                    if target_reaches_source(target, &absolute)? {
                        direct_targets.push(target);
                    }
                }
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

fn target_reaches_source(
    target: &TargetIdentity,
    source: &Path,
) -> Result<bool, WorkspaceTestError> {
    let direct_root = target_is_library(target)
        .then(|| target.source.parent())
        .flatten();
    if source == target.source || direct_root.is_some_and(|root| source.starts_with(root)) {
        return Ok(true);
    }
    let document = match fs::read_to_string(&target.source) {
        Ok(document) => document,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(WorkspaceTestError::new(format!(
                "read Cargo target source {} while resolving staged test reach: {error}",
                target.source.display()
            )));
        }
    };
    let syntax = syn::parse_file(&document).map_err(|error| {
        WorkspaceTestError::new(format!(
            "parse Cargo target source {} while resolving staged test reach: {error}",
            target.source.display()
        ))
    })?;
    let parent = target.source.parent().ok_or_else(|| {
        WorkspaceTestError::new(format!(
            "Cargo target source {} has no parent",
            target.source.display()
        ))
    })?;
    for item in syntax.items {
        let Item::Mod(module) = item else {
            continue;
        };
        for attribute in module
            .attrs
            .iter()
            .filter(|attribute| attribute.path().is_ident("path"))
        {
            let Meta::NameValue(pair) = &attribute.meta else {
                return Err(WorkspaceTestError::new(format!(
                    "Cargo target source {} has a non-value path attribute",
                    target.source.display()
                )));
            };
            let Expr::Lit(expression) = &pair.value else {
                return Err(WorkspaceTestError::new(format!(
                    "Cargo target source {} has a non-literal path attribute",
                    target.source.display()
                )));
            };
            let Lit::Str(path) = &expression.lit else {
                return Err(WorkspaceTestError::new(format!(
                    "Cargo target source {} has a non-string path attribute",
                    target.source.display()
                )));
            };
            if parent.join(path.value()) == source {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

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

#[derive(Clone, Copy, Debug)]
struct RunPolicy {
    concurrency: usize,
    shard_deadline: Duration,
    coordinator_deadline: Duration,
    progress_interval: Duration,
    output_drain: Duration,
    reap: Duration,
    grace: Duration,
}

impl RunPolicy {
    fn production(concurrency: usize) -> Result<Self, WorkspaceTestError> {
        if !(1..=WORKSPACE_TEST_CONCURRENCY_CAP).contains(&concurrency) {
            return Err(WorkspaceTestError::new(format!(
                "workspace test concurrency must be between 1 and {}, \
                 got {concurrency}",
                WORKSPACE_TEST_CONCURRENCY_CAP,
            )));
        }
        Ok(Self {
            concurrency,
            shard_deadline: SHARD_DEADLINE,
            coordinator_deadline: COORDINATOR_DEADLINE,
            progress_interval: PROGRESS_INTERVAL,
            output_drain: OUTPUT_DRAIN_DEADLINE,
            reap: REAP_DEADLINE,
            grace: TERMINATION_GRACE,
        })
    }

    fn for_shards(mut self, shards: usize) -> Self {
        self.concurrency = self.concurrency.min(if shards == 0 { 1 } else { shards });
        self
    }
}

struct ActiveShard {
    package: String,
    targets: String,
    started: Instant,
    observer: Arc<LastTest>,
    thread: thread::JoinHandle<()>,
}

struct Coordinator<'a, W> {
    pending: VecDeque<(usize, PackageInventory)>,
    active: BTreeMap<usize, ActiveShard>,
    results: Vec<ShardResult>,
    root: PathBuf,
    environment: Environment,
    cargo: OsString,
    policy: RunPolicy,
    cancellation: CancellationToken,
    sender: mpsc::Sender<ShardResult>,
    receiver: mpsc::Receiver<ShardResult>,
    output: &'a mut W,
    started: Instant,
    peak_concurrency: usize,
    failure: Option<String>,
    total: usize,
}

impl<W> Drop for Coordinator<'_, W> {
    fn drop(&mut self) {
        self.pending.clear();
        self.cancellation.cancel();
        for active in std::mem::take(&mut self.active).into_values() {
            let _ = active.thread.join();
        }
    }
}

impl<'a, W: Write> Coordinator<'a, W> {
    fn new(
        shards: Vec<PackageInventory>,
        root: PathBuf,
        environment: Environment,
        cargo: OsString,
        policy: RunPolicy,
        output: &'a mut W,
    ) -> Self {
        let total = shards.len();
        let (sender, receiver) = mpsc::channel();
        Self {
            pending: shards.into_iter().enumerate().collect(),
            active: BTreeMap::new(),
            results: Vec::new(),
            root,
            environment,
            cargo,
            policy,
            cancellation: CancellationToken::new(),
            sender,
            receiver,
            output,
            started: Instant::now(),
            peak_concurrency: 0,
            failure: None,
            total,
        }
    }

    fn run(mut self) -> Result<(), WorkspaceTestError> {
        if self.policy.concurrency == 0 || self.policy.progress_interval.is_zero() {
            return Err(WorkspaceTestError::new(
                "coordinator concurrency and progress interval must be nonzero",
            ));
        }
        self.start_available()?;
        let mut next_progress = self.started + self.policy.progress_interval;
        while !self.active.is_empty() {
            self.enforce_coordinator_deadline();
            let wait = self.next_wait(next_progress);
            match self.receiver.recv_timeout(wait) {
                Ok(result) => {
                    self.accept_result(result)?;
                    self.start_available()?;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.fail("all shard workers disconnected before reporting".to_owned());
                }
            }
            if Instant::now() >= next_progress {
                self.emit_progress()?;
                next_progress += self.policy.progress_interval;
            }
        }
        self.finish()
    }

    fn start_available(&mut self) -> Result<(), WorkspaceTestError> {
        while self.failure.is_none() && self.active.len() < self.policy.concurrency {
            let Some((index, shard)) = self.pending.pop_front() else {
                break;
            };
            let targets = target_list(&shard);
            writeln!(
                self.output,
                "[rust-test] stage=start shard={}/{} package={:?} targets={:?}",
                index + 1,
                self.total,
                shard.name,
                targets
            )
            .map_err(output_error)?;
            self.output.flush().map_err(output_error)?;
            self.launch(index, shard, targets);
        }
        self.peak_concurrency = self.peak_concurrency.max(self.active.len());
        Ok(())
    }

    fn launch(&mut self, index: usize, shard: PackageInventory, targets: String) {
        let sender = self.sender.clone();
        let root = self.root.clone();
        let environment = self.environment.clone();
        let cargo = self.cargo.clone();
        let cancellation = self.cancellation.clone();
        let policy = self.policy;
        let observer = Arc::new(LastTest::default());
        let worker_observer = Arc::clone(&observer);
        let panic_observer = Arc::clone(&observer);
        let panic_shard = shard.clone();
        let panic_targets = targets.clone();
        let package = shard.name.clone();
        let started = Instant::now();
        let handle = thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_shard(
                    index,
                    shard,
                    root,
                    environment,
                    cargo,
                    cancellation,
                    policy,
                    worker_observer,
                )
            }))
            .unwrap_or_else(|_| {
                failed_shard(
                    index,
                    panic_shard,
                    panic_targets,
                    started.elapsed(),
                    panic_observer.last(),
                    WorkspaceTestError::new("shard worker panicked"),
                )
            });
            let _ = sender.send(result);
        });
        self.active.insert(
            index,
            ActiveShard {
                package,
                targets,
                started,
                observer,
                thread: handle,
            },
        );
    }

    fn enforce_coordinator_deadline(&mut self) {
        if self.failure.is_none() && self.started.elapsed() >= self.policy.coordinator_deadline {
            self.fail(format!(
                "coordinator exceeded its {} ms deadline",
                self.policy.coordinator_deadline.as_millis()
            ));
        }
    }

    fn next_wait(&self, next_progress: Instant) -> Duration {
        let now = Instant::now();
        let progress = next_progress.saturating_duration_since(now);
        let deadline =
            (self.started + self.policy.coordinator_deadline).saturating_duration_since(now);
        progress.min(deadline).min(Duration::from_secs(1))
    }

    fn accept_result(&mut self, result: ShardResult) -> Result<(), WorkspaceTestError> {
        let active = self.active.remove(&result.index).ok_or_else(|| {
            WorkspaceTestError::new(format!("unknown shard {} reported", result.index + 1))
        })?;
        if active.thread.join().is_err() {
            self.fail(format!(
                "package {:?} shard worker panicked",
                active.package
            ));
        }
        self.emit_terminal(&result)?;
        if !result.success && self.failure.is_none() {
            self.fail(format!(
                concat!(
                    "package {:?} ended as {} at binary {:?}, last test {:?}; ",
                    "stdout-tail={:?}; stderr-tail={:?}"
                ),
                result.package,
                result.disposition,
                result.binary,
                result.last_test,
                result.stdout_tail,
                result.stderr_tail,
            ));
        }
        self.results.push(result);
        Ok(())
    }

    fn emit_terminal(&mut self, result: &ShardResult) -> Result<(), WorkspaceTestError> {
        writeln!(
            self.output,
            concat!(
                "[rust-test] stage=terminal shard={}/{} package={:?} targets={:?} ",
                "elapsed_ms={} disposition={} binary={:?} last_test={:?}"
            ),
            result.index + 1,
            self.total,
            result.package,
            result.targets,
            result.elapsed.as_millis(),
            result.disposition,
            result.binary,
            result.last_test,
        )
        .map_err(output_error)?;
        if !result.success {
            writeln!(
                self.output,
                "[rust-test] stdout-tail={:?}",
                result.stdout_tail
            )
            .map_err(output_error)?;
            writeln!(
                self.output,
                "[rust-test] stderr-tail={:?}",
                result.stderr_tail
            )
            .map_err(output_error)?;
        }
        self.output.flush().map_err(output_error)
    }

    fn emit_progress(&mut self) -> Result<(), WorkspaceTestError> {
        for (index, active) in &self.active {
            writeln!(
                self.output,
                concat!(
                    "[rust-test] stage=progress shard={}/{} package={:?} targets={:?} ",
                    "elapsed_ms={} last_test={:?}"
                ),
                index + 1,
                self.total,
                active.package,
                active.targets,
                active.started.elapsed().as_millis(),
                active.observer.last(),
            )
            .map_err(output_error)?;
        }
        self.output.flush().map_err(output_error)
    }

    fn fail(&mut self, detail: String) {
        self.failure.get_or_insert(detail);
        self.pending.clear();
        self.cancellation.cancel();
    }

    fn finish(mut self) -> Result<(), WorkspaceTestError> {
        let slowest = self.results.iter().max_by_key(|result| result.elapsed);
        let (slowest_package, slowest_elapsed) = slowest.map_or(("none", 0), |result| {
            (result.package.as_str(), result.elapsed.as_millis())
        });
        let disposition = if self.failure.is_some() {
            "failed"
        } else {
            "success"
        };
        writeln!(
            self.output,
            concat!(
                "[rust-test] coordinator disposition={} elapsed_ms={} peak_concurrency={} ",
                "slowest_package={:?} slowest_elapsed_ms={}"
            ),
            disposition,
            self.started.elapsed().as_millis(),
            self.peak_concurrency,
            slowest_package,
            slowest_elapsed,
        )
        .map_err(output_error)?;
        self.output.flush().map_err(output_error)?;
        match self.failure.take() {
            Some(detail) => Err(WorkspaceTestError::new(detail)),
            None if self.pending.is_empty() && self.results.len() == self.total => Ok(()),
            None => Err(WorkspaceTestError::new(format!(
                "coordinator covered {} of {} shards",
                self.results.len(),
                self.total
            ))),
        }
    }
}

fn output_error(error: io::Error) -> WorkspaceTestError {
    WorkspaceTestError::new(format!("report output failed: {error}"))
}

fn target_list(shard: &PackageInventory) -> String {
    shard
        .targets
        .iter()
        .map(TargetIdentity::display)
        .collect::<Vec<_>>()
        .join(",")
}

#[derive(Default)]
struct LastTest {
    line: Mutex<Option<String>>,
}

impl LastTest {
    fn last(&self) -> String {
        self.line
            .lock()
            .ok()
            .and_then(|line| line.clone())
            .unwrap_or_else(|| "none".to_owned())
    }

    fn observe_line(&self, line: String) -> Result<(), ObserveError> {
        if line.starts_with("test ") && !line.starts_with("test result:") {
            let mut last = self
                .line
                .lock()
                .map_err(|_| ObserveError::new("last-test observation lock poisoned"))?;
            *last = Some(line);
        }
        Ok(())
    }
}

impl RecordObserver for LastTest {
    fn observe(&self, record: ObservedRecord<'_>) -> Result<(), ObserveError> {
        let line = String::from_utf8_lossy(record.bytes())
            .trim_end()
            .to_owned();
        self.observe_line(line)
    }
}

struct ShardResult {
    index: usize,
    package: String,
    targets: String,
    elapsed: Duration,
    disposition: String,
    binary: String,
    last_test: String,
    stdout_tail: String,
    stderr_tail: String,
    success: bool,
}

#[allow(clippy::too_many_arguments)]
fn run_shard(
    index: usize,
    shard: PackageInventory,
    root: PathBuf,
    environment: Environment,
    cargo: OsString,
    cancellation: CancellationToken,
    policy: RunPolicy,
    observer: Arc<LastTest>,
) -> ShardResult {
    let started = Instant::now();
    let targets = target_list(&shard);
    let owner = format!("workspace test package {}", shard.name);
    let spec = shard_spec(
        &cargo,
        &shard,
        &root,
        environment,
        cancellation,
        policy,
        &owner,
    );
    let outcome = spec.and_then(|spec| {
        let observation = Observation::new(
            OutputStream::Stdout,
            RECORD_FRAME_LIMIT,
            RECORD_QUEUE_CAPACITY,
            Arc::clone(&observer) as Arc<dyn RecordObserver>,
        )
        .map_err(|error| WorkspaceTestError::new(error.to_string()))?;
        let running = sill::spawn_observed(spec, observation)
            .map_err(|error| WorkspaceTestError::new(format!("shard launch failed: {error}")))?;
        running
            .wait()
            .map_err(|error| WorkspaceTestError::new(format!("shard wait failed: {error}")))
    });
    shard_result(
        index,
        shard,
        targets,
        started.elapsed(),
        observer.last(),
        outcome,
    )
}

fn shard_spec(
    cargo: &OsStr,
    shard: &PackageInventory,
    root: &Path,
    environment: Environment,
    cancellation: CancellationToken,
    policy: RunPolicy,
    owner: &str,
) -> Result<Spec, WorkspaceTestError> {
    let mut arguments = vec![
        OsString::from("test"),
        OsString::from("--locked"),
        OsString::from("--package"),
        OsString::from(&shard.id),
    ];
    for target in &shard.targets {
        arguments.extend(target_arguments(target)?);
    }
    let mut spec = process_spec(
        cargo,
        arguments,
        root,
        environment,
        policy.shard_deadline,
        Cancellation::Token(cancellation),
        owner,
    )?;
    spec.deadlines = Deadlines::new(policy.shard_deadline, policy.output_drain, policy.reap)
        .map_err(|error| WorkspaceTestError::new(error.to_string()))?;
    spec.grace = policy.grace;
    Ok(spec)
}

fn target_arguments(target: &TargetIdentity) -> Result<Vec<OsString>, WorkspaceTestError> {
    let (selector, named) = match target.kind.as_str() {
        "bin" => ("--bin", true),
        "test" => ("--test", true),
        "example" => ("--example", true),
        "bench" => ("--bench", true),
        "lib" | "cdylib+rlib" | "proc-macro" => ("--lib", false),
        kind => {
            return Err(WorkspaceTestError::new(format!(
                "package {:?} target {:?} has unsupported Cargo kind {kind:?}",
                target.package, target.name
            )));
        }
    };
    let mut arguments = vec![OsString::from(selector)];
    if named {
        arguments.push(OsString::from(&target.name));
    }
    Ok(arguments)
}

fn process_spec<I, S>(
    program: &OsStr,
    arguments: I,
    root: &Path,
    environment: Environment,
    runtime: Duration,
    cancellation: Cancellation,
    owner: &str,
) -> Result<Spec, WorkspaceTestError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    Ok(Spec {
        argv: Argv::new(program, arguments)
            .map_err(|error| WorkspaceTestError::new(error.to_string()))?,
        cwd: root.to_owned(),
        environment,
        group: ProcessGroup::New,
        deadlines: Deadlines::new(runtime, OUTPUT_DRAIN_DEADLINE, REAP_DEADLINE)
            .map_err(|error| WorkspaceTestError::new(error.to_string()))?,
        wait: WaitPolicy::new(owner, cancellation, RestartDisposition::Never)
            .map_err(|error| WorkspaceTestError::new(error.to_string()))?,
        output: Output::new(
            StdinPolicy::Inherit,
            StreamPolicy::Capture {
                limit: CAPTURE_LIMIT,
            },
            StreamPolicy::Capture {
                limit: CAPTURE_LIMIT,
            },
        ),
        redaction: Redaction::None,
        grace: TERMINATION_GRACE,
    })
}

fn shard_result(
    index: usize,
    shard: PackageInventory,
    targets: String,
    elapsed: Duration,
    last_test: String,
    outcome: Result<WaitOutcome, WorkspaceTestError>,
) -> ShardResult {
    let Ok(outcome) = outcome else {
        return failed_shard(
            index,
            shard,
            targets,
            elapsed,
            last_test,
            outcome.unwrap_err(),
        );
    };
    let stdout_tail = captured_excerpt(outcome.stdout());
    let stderr_tail = captured_excerpt(outcome.stderr());
    let binary = last_binary(outcome.stderr());
    let capture_failed = outcome.observation_failure().is_some()
        || !outcome.outputs_drained()
        || outcome.stdout().is_some_and(|output| output.truncated())
        || outcome.stderr().is_some_and(|output| output.truncated());
    let (disposition, success) = match outcome.completion() {
        Completion::Exited(status) if status.success() && !capture_failed => {
            ("success".to_owned(), true)
        }
        Completion::Exited(status) if capture_failed => (
            format!("capture-failed after {}", exit_name(*status)),
            false,
        ),
        Completion::Exited(status) => (exit_name(*status), false),
        Completion::TimedOut(_) => ("timeout".to_owned(), false),
        Completion::Cancelled(_) => ("cancelled".to_owned(), false),
    };
    ShardResult {
        index,
        package: shard.name,
        targets,
        elapsed,
        disposition,
        binary,
        last_test,
        stdout_tail,
        stderr_tail,
        success,
    }
}

fn failed_shard(
    index: usize,
    shard: PackageInventory,
    targets: String,
    elapsed: Duration,
    last_test: String,
    error: WorkspaceTestError,
) -> ShardResult {
    ShardResult {
        index,
        package: shard.name,
        targets,
        elapsed,
        disposition: format!("supervision-error: {error}"),
        binary: "not-observed".to_owned(),
        last_test,
        stdout_tail: String::new(),
        stderr_tail: String::new(),
        success: false,
    }
}

fn captured_excerpt(output: Option<&sill::CapturedOutput>) -> String {
    let Some(output) = output else {
        return "not-captured".to_owned();
    };
    let bytes = output.bytes();
    let start = bytes.len().saturating_sub(FAILURE_EXCERPT_LIMIT);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

fn last_binary(stderr: Option<&sill::CapturedOutput>) -> String {
    let Some(stderr) = stderr else {
        return "not-observed".to_owned();
    };
    String::from_utf8_lossy(stderr.bytes())
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("Running ").map(str::to_owned))
        .unwrap_or_else(|| "not-observed".to_owned())
}

fn completion_name(completion: &Completion) -> String {
    match completion {
        Completion::Exited(status) => exit_name(*status),
        Completion::TimedOut(_) => "timeout".to_owned(),
        Completion::Cancelled(_) => "cancelled".to_owned(),
    }
}

fn exit_name(status: std::process::ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "signal".to_owned(), |code| format!("exit code={code}"))
}

#[cfg(test)]
mod tests;
