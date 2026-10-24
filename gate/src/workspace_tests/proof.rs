//! Stable proof actions for canonical Cargo typecheck and test shards.

use super::*;

mod input_provenance;

use input_provenance::selected_package_input_paths;

const PROOF_CARGO_JOBS: usize = 1;
const PROOF_TEST_THREADS: usize = 1;
const DIGEST_HEX_LENGTH: usize = 64;

const WINDOWS_TARGET: &str = "x86_64-pc-windows-msvc";
const WINDOWS_TOOLCHAIN_PIN: &str = "dist/windows-msvc.toml";
const WINDOWS_TYPECHECKS: [WindowsTypecheck; 3] = [
    WindowsTypecheck {
        package: "spice",
        identity: "windows-x86_64-pc-windows-msvc-bin:spice",
        selectors: &["--bin", "spice"],
    },
    WindowsTypecheck {
        package: "spice",
        identity: "windows-x86_64-pc-windows-msvc-tests",
        selectors: &["--tests"],
    },
    WindowsTypecheck {
        package: "sill",
        identity: "windows-x86_64-pc-windows-msvc-tests",
        selectors: &["--tests"],
    },
];

struct WindowsTypecheck {
    package: &'static str,
    identity: &'static str,
    selectors: &'static [&'static str],
}
/// Whether one complete staged tree declares the repository Windows toolchain.
#[must_use]
pub fn windows_typechecks_declared_at(root: &Path) -> bool {
    root.join(WINDOWS_TOOLCHAIN_PIN).is_file()
}

/// Logical Cargo work represented by one independently reusable receipt.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CargoGateShardKind {
    Typecheck,
    Test,
}

/// One stable canonical Cargo command and its complete selected-tree closure.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct CargoGateShard {
    kind: CargoGateShardKind,
    action_id: String,
    package: String,
    targets: Vec<String>,
    argv: Vec<String>,
    input_paths: Vec<String>,
    compile_time_producers: Vec<CargoCompileTimeProducer>,
    compile_time_authorities: Vec<CargoCompileTimeAuthority>,
    #[serde(skip_serializing_if = "Option::is_none")]
    external_source: Option<CargoExternalSourceAuthority>,
    reuse: CargoShardReuse,
}

/// Immutable Cargo source-store generation covering external package producers.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct CargoExternalSourceAuthority {
    version: u64,
    generation: String,
    manifest: String,
}

impl CargoExternalSourceAuthority {
    pub fn new(generation: String, manifest: String) -> Result<Self, WorkspaceTestError> {
        if !canonical_digest(&generation) || !canonical_digest(&manifest) {
            return Err(WorkspaceTestError::new(
                "Cargo external source authority requires canonical generation and manifest digests",
            ));
        }
        Ok(Self {
            version: 1,
            generation,
            manifest,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum CargoShardReuse {
    Reusable,
    NonCacheable { reason: String },
}

/// One executable compile-time target in the shard's resolved Cargo graph.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CargoCompileTimeProducer {
    package_id: String,
    package: String,
    target: String,
    kind: String,
    external: bool,
    selected_tree_authority: bool,
    source_generation_eligible: bool,
}

/// Closed executable compile-time derivation reached by one Cargo shard.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CargoCompileTimeAuthority {
    pub(super) package: String,
    pub(super) target: String,
    pub(super) kind: String,
    pub(super) input_paths: Vec<String>,
    pub(super) environment: Vec<String>,
    pub(super) generated_outputs: Vec<String>,
}

impl CargoGateShard {
    #[must_use]
    pub const fn kind(&self) -> CargoGateShardKind {
        self.kind
    }

    #[must_use]
    pub fn action_id(&self) -> &str {
        &self.action_id
    }

    #[must_use]
    pub fn package(&self) -> &str {
        &self.package
    }

    #[must_use]
    pub fn targets(&self) -> &[String] {
        &self.targets
    }

    #[must_use]
    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    #[must_use]
    pub fn input_paths(&self) -> &[String] {
        &self.input_paths
    }

    /// Why this shard must not derive or consult reusable proof identity.
    #[must_use]
    pub fn non_cacheable_reason(&self) -> Option<&str> {
        match &self.reuse {
            CargoShardReuse::Reusable => None,
            CargoShardReuse::NonCacheable { reason } => Some(reason),
        }
    }
}

/// Canonical Cargo actions selected from one immutable source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CargoGateShardPlan {
    typechecks: Vec<CargoGateShard>,
    tests: Vec<CargoGateShard>,
}

/// Authorities required to derive canonical Cargo proof shards.
pub struct CargoGateShardPlanning<'a> {
    pub root: &'a Path,
    pub base: Option<&'a Path>,
    pub source: wire::Digest,
    pub changed_sources: &'a [String],
    pub selected_paths: &'a [String],
    pub include_windows_typechecks: bool,
    pub environment: Environment,
    pub cargo: &'a OsStr,
    pub external_source: Option<CargoExternalSourceAuthority>,
    pub include_tests: bool,
}

impl CargoGateShardPlan {
    #[must_use]
    pub fn typechecks(&self) -> &[CargoGateShard] {
        &self.typechecks
    }

    #[must_use]
    pub fn tests(&self) -> &[CargoGateShard] {
        &self.tests
    }
}

/// Typed successful result committed to one Cargo shard receipt.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct CargoGateShardResult {
    action_id: String,
    completion: String,
}

/// Plan package-and-target Cargo proof actions from canonical metadata and reach.
///
/// `selected_paths` must be the complete regular-file inventory of `root`.
/// Package closures include the package, transitive local workspace dependencies,
/// workspace Cargo authorities, declared compile-time path inputs, and every
/// changed path whose provenance selected the shard.
pub fn plan_cargo_gate_shards_at(
    planning: CargoGateShardPlanning<'_>,
) -> Result<CargoGateShardPlan, WorkspaceTestError> {
    let environment = planning.environment.clone();
    let cargo = planning.cargo;
    plan_cargo_gate_shards(planning, |root| {
        acquire_resolved_inventory(root, &environment, cargo)
    })
}

/// Plan Cargo proof shards from the resolved inventory `acquire` reports for
/// the selected root and, when removed Rust source requires it, the base root.
fn plan_cargo_gate_shards(
    planning: CargoGateShardPlanning<'_>,
    mut acquire: impl FnMut(&Path) -> Result<Inventory, WorkspaceTestError>,
) -> Result<CargoGateShardPlan, WorkspaceTestError> {
    let CargoGateShardPlanning {
        root,
        base,
        source,
        changed_sources,
        selected_paths,
        include_windows_typechecks,
        environment: _,
        cargo: _,
        external_source,
        include_tests,
    } = planning;
    let mut inventory = acquire(root)?;
    if cargo_workspace_base_inventory_required(root, changed_sources)? {
        let base = base.ok_or_else(|| {
            WorkspaceTestError::new(
                "removed Rust source requires exact base-tree Cargo ownership inventory",
            )
        })?;
        let base_inventory = acquire(base)?;
        inventory.retain_removed_target_ownership(root, &base_inventory, base, changed_sources)?;
    }

    let mut source_analysis = CompileTimeSourceAnalysis::default();
    let mut typechecks = typecheck_shards(
        &inventory,
        root,
        changed_sources,
        selected_paths,
        &mut source_analysis,
        external_source.as_ref(),
    )?;
    if include_windows_typechecks {
        typechecks.extend(windows_typecheck_shards(
            &inventory,
            root,
            changed_sources,
            selected_paths,
            &mut source_analysis,
            external_source.as_ref(),
        )?);
    }

    let mut tests = if include_tests {
        let report = selection_plan(&inventory, root, source, changed_sources)?.into_report();
        let selected = inventory.selected(report.tests())?;
        let test_sources = report.edges().iter().fold(
            BTreeMap::<String, Vec<String>>::new(),
            |mut sources, edge| {
                sources
                    .entry(edge.test().to_owned())
                    .or_default()
                    .push(edge.source().to_owned());
                sources
            },
        );
        let test_packages = partition_with_target_limit(&selected, PROOF_TARGETS_PER_SHARD);
        validate_partition_with_target_limit(&selected, &test_packages, PROOF_TARGETS_PER_SHARD)?;
        test_packages
            .iter()
            .map(|package| {
                test_shard(
                    &inventory,
                    root,
                    package,
                    selected_paths,
                    &test_sources,
                    &mut source_analysis,
                    external_source.as_ref(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };

    typechecks.sort_by(|left, right| left.action_id.cmp(&right.action_id));
    tests.sort_by(|left, right| left.action_id.cmp(&right.action_id));
    Ok(CargoGateShardPlan { typechecks, tests })
}

fn typecheck_shards(
    inventory: &Inventory,
    root: &Path,
    changed_sources: &[String],
    selected_paths: &[String],
    source_analysis: &mut CompileTimeSourceAnalysis,
    external_source: Option<&CargoExternalSourceAuthority>,
) -> Result<Vec<CargoGateShard>, WorkspaceTestError> {
    let reach = selection::cargo_check_selections(inventory, root, changed_sources)?;
    let sources = typecheck_provenance(inventory, root, changed_sources)?;
    let mut shards = Vec::new();
    for check in &reach {
        for target in &check.targets {
            let argv = target.clippy_arguments(&check.name);
            let authority = package_input_authority(
                inventory,
                root,
                &check.name,
                selected_paths,
                sources
                    .get(&(check.name.clone(), target.identity.clone()))
                    .map_or(&[], Vec::as_slice),
                source_analysis,
                external_source,
            )?;
            shards.push(CargoGateShard {
                kind: CargoGateShardKind::Typecheck,
                action_id: format!("cargo:typecheck:{}::{}", check.name, target.identity),
                package: check.name.clone(),
                targets: vec![target.identity.clone()],
                argv,
                input_paths: authority.input_paths,
                compile_time_producers: authority.producers,
                compile_time_authorities: authority.compile_time,
                external_source: authority.external_source,
                reuse: authority.reuse,
            });
        }
    }
    Ok(shards)
}

fn windows_typecheck_shards(
    inventory: &Inventory,
    root: &Path,
    changed_sources: &[String],
    selected_paths: &[String],
    source_analysis: &mut CompileTimeSourceAnalysis,
    external_source: Option<&CargoExternalSourceAuthority>,
) -> Result<Vec<CargoGateShard>, WorkspaceTestError> {
    WINDOWS_TYPECHECKS
        .iter()
        .map(|check| {
            let mut argv = vec![
                "clippy".to_owned(),
                "--locked".to_owned(),
                "--package".to_owned(),
                check.package.to_owned(),
            ];
            argv.extend(
                check
                    .selectors
                    .iter()
                    .map(|selector| (*selector).to_owned()),
            );
            argv.extend([
                "--target".to_owned(),
                WINDOWS_TARGET.to_owned(),
                "--message-format=json-render-diagnostics".to_owned(),
                "--".to_owned(),
                "-D".to_owned(),
                "warnings".to_owned(),
            ]);
            let mut authority = package_input_authority(
                inventory,
                root,
                check.package,
                selected_paths,
                changed_sources,
                source_analysis,
                external_source,
            )?;
            authority.input_paths.push(WINDOWS_TOOLCHAIN_PIN.to_owned());
            authority.input_paths.sort();
            authority.input_paths.dedup();
            Ok(CargoGateShard {
                kind: CargoGateShardKind::Typecheck,
                action_id: format!("cargo:typecheck:{}::{}", check.package, check.identity),
                package: check.package.to_owned(),
                targets: vec![check.identity.to_owned()],
                argv,
                input_paths: authority.input_paths,
                compile_time_producers: authority.producers,
                compile_time_authorities: authority.compile_time,
                external_source: authority.external_source,
                reuse: authority.reuse,
            })
        })
        .collect()
}

fn typecheck_provenance(
    inventory: &Inventory,
    root: &Path,
    changed_sources: &[String],
) -> Result<BTreeMap<(String, String), Vec<String>>, WorkspaceTestError> {
    let mut provenance = BTreeMap::<(String, String), Vec<String>>::new();
    for source in changed_sources {
        for check in
            selection::cargo_check_selections(inventory, root, std::slice::from_ref(source))?
        {
            for target in check.targets {
                provenance
                    .entry((check.name.clone(), target.identity))
                    .or_default()
                    .push(source.clone());
            }
        }
    }
    Ok(provenance)
}

fn test_shard(
    inventory: &Inventory,
    root: &Path,
    package: &PackageInventory,
    selected_paths: &[String],
    test_sources: &BTreeMap<String, Vec<String>>,
    source_analysis: &mut CompileTimeSourceAnalysis,
    external_source: Option<&CargoExternalSourceAuthority>,
) -> Result<CargoGateShard, WorkspaceTestError> {
    let mut argv = vec![
        "test".to_owned(),
        "--locked".to_owned(),
        "--package".to_owned(),
        package.name.clone(),
        "--jobs".to_owned(),
        PROOF_CARGO_JOBS.to_string(),
    ];
    let mut targets = Vec::new();
    for target in &package.targets {
        targets.push(target.display());
        argv.extend(
            target_arguments(target)?
                .into_iter()
                .map(|argument| argument.to_string_lossy().into_owned()),
        );
    }
    if let Some(features) = qualified_required_features(package.targets.iter().flat_map(|target| {
        target
            .required_features
            .iter()
            .map(|feature| (package.name.as_str(), feature.as_str()))
    })) {
        argv.extend(["--features".to_owned(), features]);
    }
    argv.extend([
        "--".to_owned(),
        format!("--test-threads={PROOF_TEST_THREADS}"),
    ]);
    let identity = targets.join(",");
    let provenance = targets
        .iter()
        .flat_map(|target| {
            test_sources
                .get(&format!("{}::{target}", package.name))
                .into_iter()
                .flatten()
                .cloned()
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let authority = package_input_authority(
        inventory,
        root,
        &package.name,
        selected_paths,
        &provenance,
        source_analysis,
        external_source,
    )?;
    Ok(CargoGateShard {
        kind: CargoGateShardKind::Test,
        action_id: format!("cargo:test:{}::{identity}", package.name),
        package: package.name.clone(),
        targets,
        argv,
        input_paths: authority.input_paths,
        compile_time_producers: authority.producers,
        compile_time_authorities: authority.compile_time,
        external_source: authority.external_source,
        reuse: authority.reuse,
    })
}

struct CargoPackageInputAuthority {
    input_paths: Vec<String>,
    producers: Vec<CargoCompileTimeProducer>,
    compile_time: Vec<CargoCompileTimeAuthority>,
    external_source: Option<CargoExternalSourceAuthority>,
    reuse: CargoShardReuse,
}

fn package_input_authority(
    inventory: &Inventory,
    root: &Path,
    package_name: &str,
    selected_paths: &[String],
    changed_sources: &[String],
    source_analysis: &mut CompileTimeSourceAnalysis,
    external_source: Option<&CargoExternalSourceAuthority>,
) -> Result<CargoPackageInputAuthority, WorkspaceTestError> {
    let canonical_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let package = inventory
        .packages
        .iter()
        .find(|package| package.name == package_name)
        .ok_or_else(|| {
            WorkspaceTestError::new(format!("Cargo package {package_name:?} vanished"))
        })?;
    let (roots, external_local_dependencies) =
        reached_local_package_roots(inventory, &canonical_root, package);
    let mut inputs = selected_package_input_paths(
        inventory,
        &canonical_root,
        &roots,
        selected_paths,
        changed_sources,
    )?;
    let producers = resolved_compile_time_producers(inventory, &package.id)?;
    let unsafe_authorities = unsafe_compile_time_authorities(
        &producers,
        CompileTimeExternal {
            local_dependencies: &external_local_dependencies,
            source: external_source,
        },
    );
    let (compile_time, reuse) = compile_time_reuse(&unsafe_authorities, || {
        compile_time_authorities(
            inventory,
            &canonical_root,
            &roots,
            selected_paths,
            &mut inputs,
            source_analysis,
        )
    });
    let uses_external_source = producers
        .iter()
        .any(|producer| producer.source_generation_eligible);
    Ok(CargoPackageInputAuthority {
        input_paths: inputs.into_iter().collect(),
        producers,
        compile_time,
        external_source: uses_external_source
            .then(|| external_source.cloned())
            .flatten(),
        reuse,
    })
}

fn reached_local_package_roots(
    inventory: &Inventory,
    canonical_root: &Path,
    package: &PackageInventory,
) -> (BTreeSet<PathBuf>, BTreeSet<PathBuf>) {
    let mut roots = BTreeSet::new();
    let mut pending = VecDeque::from([package.root.clone()]);
    let mut external = BTreeSet::new();
    while let Some(package_root) = pending.pop_front() {
        if !roots.insert(package_root.clone()) {
            continue;
        }
        let Some(package) = inventory
            .packages
            .iter()
            .find(|candidate| candidate.root == package_root)
        else {
            continue;
        };
        for dependency in &package.dependencies {
            if dependency.starts_with(canonical_root) {
                pending.push_back(dependency.clone());
            } else {
                external.insert(dependency.clone());
            }
        }
    }
    (roots, external)
}

struct CompileTimeExternal<'a> {
    local_dependencies: &'a BTreeSet<PathBuf>,
    source: Option<&'a CargoExternalSourceAuthority>,
}

/// Every producer or dependency that denies compile-time reuse, named for
/// the refusal.
fn unsafe_compile_time_authorities(
    producers: &[CargoCompileTimeProducer],
    external: CompileTimeExternal<'_>,
) -> Vec<String> {
    let mut unsafe_authorities = producers
        .iter()
        .filter(|producer| {
            (!producer.selected_tree_authority && !producer.source_generation_eligible)
                || (producer.source_generation_eligible && external.source.is_none())
        })
        .map(|producer| {
            format!(
                "{} {}::{} ({})",
                if producer.external {
                    "external"
                } else {
                    "non-workspace"
                },
                producer.package,
                producer.target,
                producer.kind,
            )
        })
        .collect::<Vec<_>>();
    unsafe_authorities.extend(external.local_dependencies.iter().map(|dependency| {
        format!(
            "external local dependency {}",
            dependency
                .file_name()
                .unwrap_or(dependency.as_os_str())
                .to_string_lossy(),
        )
    }));
    unsafe_authorities
}

/// Decide shard reuse: an empty `unsafe_authorities` list admits the
/// compile-time authorities `authorities` derives; anything else, or a
/// derivation failure, makes the shard non-cacheable with that reason.
fn compile_time_reuse(
    unsafe_authorities: &[String],
    authorities: impl FnOnce() -> Result<Vec<CargoCompileTimeAuthority>, WorkspaceTestError>,
) -> (Vec<CargoCompileTimeAuthority>, CargoShardReuse) {
    if unsafe_authorities.is_empty() {
        match authorities() {
            Ok(authorities) => (authorities, CargoShardReuse::Reusable),
            Err(error) => (
                Vec::new(),
                CargoShardReuse::NonCacheable {
                    reason: error.to_string(),
                },
            ),
        }
    } else {
        (
            Vec::new(),
            CargoShardReuse::NonCacheable {
                reason: format!(
                    "resolved Cargo graph reaches dependencies or executable compile-time producers without bounded selected-tree authority: {}",
                    unsafe_authorities.join(", "),
                ),
            },
        )
    }
}

fn resolved_compile_time_producers(
    inventory: &Inventory,
    package_id: &str,
) -> Result<Vec<CargoCompileTimeProducer>, WorkspaceTestError> {
    let mut pending = VecDeque::from([package_id.to_owned()]);
    let mut visited = BTreeSet::new();
    let mut producers = Vec::new();
    while let Some(id) = pending.pop_front() {
        if !visited.insert(id.clone()) {
            continue;
        }
        let package = inventory.resolution.packages.get(&id).ok_or_else(|| {
            WorkspaceTestError::new(format!(
                "Cargo shard root or dependency {id:?} is absent from resolved package inventory",
            ))
        })?;
        for target in &package.compile_time_targets {
            producers.push(CargoCompileTimeProducer {
                package_id: package.authority_id.clone(),
                package: package.name.clone(),
                target: target.name.clone(),
                kind: target.kind.clone(),
                external: package.external,
                selected_tree_authority: inventory
                    .packages
                    .iter()
                    .any(|workspace| workspace.id == package.id),
                source_generation_eligible: package.external
                    && !package.authority_id.starts_with("external-path#"),
            });
        }
        let dependencies = inventory.resolution.edges.get(&id).ok_or_else(|| {
            WorkspaceTestError::new(format!(
                "Cargo shard package {id:?} is absent from the resolved dependency graph",
            ))
        })?;
        pending.extend(dependencies.iter().cloned());
    }
    producers.sort();
    producers.dedup();
    Ok(producers)
}

pub(super) fn path_text(path: &Path) -> Result<String, WorkspaceTestError> {
    path.to_str()
        .map(|path| path.replace(std::path::MAIN_SEPARATOR, "/"))
        .ok_or_else(|| {
            WorkspaceTestError::new(format!("Cargo input path {} is not UTF-8", path.display()))
        })
}

fn canonical_digest(value: &str) -> bool {
    value.len() == DIGEST_HEX_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// Execute one planned Cargo shard with fixed proof-runner bounds.
pub fn run_cargo_gate_shard_at(
    root: &Path,
    writable_root: &Path,
    environment: Environment,
    cargo: &OsStr,
    shard: &CargoGateShard,
    output: &mut impl Write,
) -> Result<CargoGateShardResult, WorkspaceTestError> {
    let mut entries = environment
        .entries()
        .iter()
        .cloned()
        .collect::<BTreeMap<OsString, OsString>>();
    entries.insert(
        OsString::from(CARGO_TARGET_DIR),
        writable_root.join("target").into_os_string(),
    );
    entries.insert(
        OsString::from(CARGO_BUILD_BUILD_DIR),
        writable_root.join("target/cargo-build").into_os_string(),
    );
    entries.insert(OsString::from(RUSTC_WRAPPER), OsString::new());
    entries.insert(OsString::from("RUSTC_WORKSPACE_WRAPPER"), OsString::new());
    let environment =
        Environment::new(entries).map_err(|error| WorkspaceTestError::new(error.to_string()))?;
    if shard.kind == CargoGateShardKind::Test {
        // Match the full verifier: compile the exact selected targets before
        // starting their execution clock. A fresh staged path can invalidate
        // Cargo fingerprints even when the selected source bytes are unchanged.
        let arguments = shard
            .argv
            .iter()
            .map(String::as_str)
            .take_while(|argument| *argument != "--")
            .chain(std::iter::once("--no-run"));
        prebuild::run_prebuild_command(
            root,
            &environment,
            cargo,
            output,
            "cargo test --no-run (proof shard)",
            arguments,
            shard.action_id(),
        )?;
    }
    let spec = process_spec(
        cargo,
        shard.argv.iter().map(String::as_str),
        root,
        environment,
        SHARD_DEADLINE,
        Cancellation::Never,
        shard.action_id(),
    )?;
    let outcome = crate::measure::spawn(spec)
        .map_err(|error| {
            WorkspaceTestError::new(format!("{} launch failed: {error}", shard.action_id))
        })?
        .wait()
        .map_err(|error| {
            WorkspaceTestError::new(format!("{} wait failed: {error}", shard.action_id))
        })?;
    for captured in [outcome.stdout(), outcome.stderr()].into_iter().flatten() {
        output.write_all(captured.bytes()).map_err(output_error)?;
        if captured.truncated() || !captured.complete() {
            return Err(WorkspaceTestError::new(format!(
                "{} output exceeded its capture bound",
                shard.action_id,
            )));
        }
    }
    output.flush().map_err(output_error)?;
    if !outcome.outputs_drained() {
        return Err(WorkspaceTestError::new(format!(
            "{} output did not drain",
            shard.action_id,
        )));
    }
    match outcome.completion() {
        Completion::Exited(status) if status.success() => Ok(CargoGateShardResult {
            action_id: shard.action_id.clone(),
            completion: "passed".to_owned(),
        }),
        completion => Err(WorkspaceTestError::new(format!(
            "{} ended as {}",
            shard.action_id,
            completion_name(completion),
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generated_outputs(plan: &CargoGateShardPlan) -> BTreeSet<String> {
        plan.typechecks()
            .iter()
            .flat_map(|shard| &shard.compile_time_authorities)
            .flat_map(|authority| &authority.generated_outputs)
            .cloned()
            .collect()
    }

    /// The `cargo metadata` document Cargo reports for the planning fixture.
    fn fixture_metadata(root: &Path) -> serde_json::Value {
        let path = |relative: &str| root.join(relative);
        let id = format!("path+file://{}#plan-source-analysis@0.1.0", root.display());
        serde_json::json!({
            "packages": [{
                "id": id,
                "name": "plan-source-analysis",
                "version": "0.1.0",
                "edition": "2024",
                "source": null,
                "dependencies": [],
                "targets": [
                    {"kind": ["lib"], "name": "plan_source_analysis", "test": true,
                     "src_path": path("src/lib.rs")},
                    {"kind": ["bin"], "name": "plan-source-analysis", "test": true,
                     "src_path": path("src/main.rs")},
                    {"kind": ["custom-build"], "name": "build-script-build", "test": false,
                     "src_path": path("build.rs")},
                ],
                "features": {},
                "manifest_path": path("Cargo.toml"),
                "metadata": {"spice": {"cargo-proof": {"version": 1, "complete": true}}},
            }],
            "workspace_members": [id],
            "resolve": {"nodes": [{"id": id, "dependencies": []}]},
            "target_directory": path("target"),
            "build_directory": path("target"),
        })
    }

    /// The manifest and lock file that metadata describes.
    fn write_manifest(fixture: &crate::test_repository::Fixture) {
        fixture.write(
            "Cargo.toml",
            concat!(
                "[package]\n",
                "name = \"plan-source-analysis\"\n",
                "version = \"0.1.0\"\n",
                "edition = \"2024\"\n",
                "build = \"build.rs\"\n\n",
                "[[bin]]\n",
                "name = \"plan-source-analysis\"\n",
                "path = \"src/main.rs\"\n\n",
                "[package.metadata.spice.cargo-proof]\n",
                "version = 1\n",
                "complete = true\n",
            ),
        );
        fixture.write(
            "Cargo.lock",
            concat!(
                "# This file is automatically @generated by Cargo.\n",
                "version = 4\n\n",
                "[[package]]\n",
                "name = \"plan-source-analysis\"\n",
                "version = \"0.1.0\"\n",
            ),
        );
    }

    fn windows_fixture_metadata(root: &Path) -> serde_json::Value {
        let path = |relative: &str| root.join(relative);
        let spice = format!("path+file://{}/spice#spice@0.0.0", root.display());
        let sill = format!("path+file://{}/sill#sill@0.0.0", root.display());
        serde_json::json!({
            "packages": [
                {
                    "id": spice,
                    "name": "spice",
                    "version": "0.0.0",
                    "edition": "2024",
                    "source": null,
                    "dependencies": [],
                    "targets": [
                        {"kind": ["lib"], "name": "spice", "test": true,
                         "src_path": path("spice/src/lib.rs")},
                        {"kind": ["bin"], "name": "spice", "test": true,
                         "src_path": path("spice/src/main.rs")},
                    ],
                    "features": {},
                    "manifest_path": path("spice/Cargo.toml"),
                    "metadata": {"spice": {"cargo-proof": {"version": 1, "complete": true}}},
                },
                {
                    "id": sill,
                    "name": "sill",
                    "version": "0.0.0",
                    "edition": "2024",
                    "source": null,
                    "dependencies": [],
                    "targets": [
                        {"kind": ["lib"], "name": "sill", "test": true,
                         "src_path": path("sill/src/lib.rs")},
                    ],
                    "features": {},
                    "manifest_path": path("sill/Cargo.toml"),
                    "metadata": {"spice": {"cargo-proof": {"version": 1, "complete": true}}},
                },
            ],
            "workspace_members": [spice, sill],
            "resolve": {"nodes": [
                {"id": spice, "dependencies": []},
                {"id": sill, "dependencies": []},
            ]},
            "target_directory": path("target"),
            "build_directory": path("target"),
        })
    }

    fn assert_windows_typecheck_contract(shards: &[CargoGateShard]) {
        assert_eq!(shards.len(), 3);
        assert_eq!(
            shards[0].action_id(),
            "cargo:typecheck:spice::windows-x86_64-pc-windows-msvc-bin:spice"
        );
        assert_eq!(
            shards[0].argv(),
            [
                "clippy",
                "--locked",
                "--package",
                "spice",
                "--bin",
                "spice",
                "--target",
                "x86_64-pc-windows-msvc",
                "--message-format=json-render-diagnostics",
                "--",
                "-D",
                "warnings",
            ]
        );
        assert_eq!(
            shards[1].action_id(),
            "cargo:typecheck:spice::windows-x86_64-pc-windows-msvc-tests"
        );
        assert_eq!(
            shards[1].argv(),
            [
                "clippy",
                "--locked",
                "--package",
                "spice",
                "--tests",
                "--target",
                "x86_64-pc-windows-msvc",
                "--message-format=json-render-diagnostics",
                "--",
                "-D",
                "warnings",
            ]
        );
        assert_eq!(
            shards[2].action_id(),
            "cargo:typecheck:sill::windows-x86_64-pc-windows-msvc-tests"
        );
        assert_eq!(
            shards[2].argv(),
            [
                "clippy",
                "--locked",
                "--package",
                "sill",
                "--tests",
                "--target",
                "x86_64-pc-windows-msvc",
                "--message-format=json-render-diagnostics",
                "--",
                "-D",
                "warnings",
            ]
        );
        assert!(shards.iter().all(|shard| {
            shard
                .input_paths()
                .iter()
                .any(|path| path == WINDOWS_TOOLCHAIN_PIN)
                && shard
                    .input_paths()
                    .iter()
                    .any(|path| path == "spice/src/lib.rs")
        }));
    }

    #[test]
    fn rust_reach_adds_complete_appliance_cross_checks() {
        let fixture = crate::test_repository::Fixture::repository("cargo-windows-typecheck");
        fixture.write(
            "Cargo.toml",
            "[workspace]\nmembers = [\"sill\", \"spice\"]\nresolver = \"3\"\n",
        );
        fixture.write("Cargo.lock", "version = 4\n");
        fixture.write(WINDOWS_TOOLCHAIN_PIN, "version = 1\n");
        fixture.write(
            "spice/Cargo.toml",
            "[package]\nname = \"spice\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        );
        fixture.write("spice/src/lib.rs", "pub fn library() {}\n");
        fixture.write("spice/src/main.rs", "fn main() {}\n");
        fixture.write(
            "sill/Cargo.toml",
            "[package]\nname = \"sill\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        );
        fixture.write("sill/src/lib.rs", "pub fn library() {}\n");
        let selected_paths = [
            "Cargo.lock",
            "Cargo.toml",
            WINDOWS_TOOLCHAIN_PIN,
            "sill/Cargo.toml",
            "sill/src/lib.rs",
            "spice/Cargo.toml",
            "spice/src/lib.rs",
            "spice/src/main.rs",
        ]
        .map(str::to_owned);
        let changed_sources = ["spice/src/lib.rs".to_owned()];
        let root = fs::canonicalize(fixture.path()).expect("canonical fixture root");
        let metadata = serde_json::from_value(windows_fixture_metadata(&root))
            .expect("fixture metadata has Cargo's shape");
        let inventory =
            inventory_from_metadata(metadata, &root, true).expect("resolve fixture inventory");
        let plan = |paths: &[String]| {
            windows_typecheck_shards(
                &inventory,
                &root,
                &changed_sources,
                paths,
                &mut CompileTimeSourceAnalysis::default(),
                None,
            )
        };

        let without_pin = selected_paths
            .iter()
            .filter(|path| path.as_str() != WINDOWS_TOOLCHAIN_PIN)
            .cloned()
            .collect::<Vec<_>>();
        assert!(windows_typechecks_declared_at(&root));
        let shards = plan(&without_pin).expect("plan ordinary Rust cross-target checks");
        assert_windows_typecheck_contract(&shards);

        let pin_shards = plan(&selected_paths).expect("plan Windows pin cross-target checks");
        assert_eq!(pin_shards, shards);
    }

    #[test]
    fn plan_source_analysis_is_unique_equivalent_and_source_version_bound() {
        let fixture = crate::test_repository::Fixture::repository("cargo-plan-source-analysis");
        write_manifest(&fixture);
        fixture.write(
            "build.rs",
            "fn main() { println!(\"cargo::rerun-if-changed=build.rs\"); }\n",
        );
        fixture.write(
            "src/lib.rs",
            "include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\n",
        );
        fixture.write("src/main.rs", "fn main() {}\n");
        let selected_paths = [
            "Cargo.lock",
            "Cargo.toml",
            "build.rs",
            "src/lib.rs",
            "src/main.rs",
        ]
        .map(str::to_owned);
        let changed_sources = ["src/lib.rs".to_owned(), "src/main.rs".to_owned()];
        let root = fs::canonicalize(fixture.path()).expect("canonical fixture root");
        let metadata = fixture_metadata(&root);
        let environment = Environment::new(std::iter::empty::<(OsString, OsString)>())
            .expect("empty environment is valid");
        let plan = || {
            plan_cargo_gate_shards(
                CargoGateShardPlanning {
                    root: &root,
                    base: None,
                    source: wire::Digest::compute(wire::Domain::GateTree, b"source-a"),
                    changed_sources: &changed_sources,
                    selected_paths: &selected_paths,
                    environment: environment.clone(),
                    cargo: OsStr::new("cargo"),
                    external_source: None,
                    include_tests: false,
                    include_windows_typechecks: false,
                },
                |selected| {
                    let metadata = serde_json::from_value(metadata.clone())
                        .expect("fixture metadata has Cargo's shape");
                    inventory_from_metadata(metadata, selected, true)
                },
            )
        };

        super::super::compile_time::reset_source_parse_count();
        let first = plan().expect("plan fixture Cargo shards");
        let first_parses = super::super::compile_time::source_parse_count();
        let unique_sources = selected_paths
            .iter()
            .filter(|path| path.ends_with(".rs"))
            .count();
        let repeated_traversal_parses = first.typechecks().len() * unique_sources;

        assert_eq!(first.typechecks().len(), 2);
        assert!(first.tests().is_empty());
        assert_eq!(first_parses, unique_sources);
        assert_eq!(repeated_traversal_parses, 6);

        super::super::compile_time::reset_source_parse_count();
        let equivalent = plan().expect("replan equivalent Cargo shards");
        assert_eq!(
            super::super::compile_time::source_parse_count(),
            unique_sources
        );
        assert_eq!(equivalent, first);

        fixture.write(
            "src/lib.rs",
            "include!(concat!(env!(\"OUT_DIR\"), \"/changed.rs\"));\n",
        );
        super::super::compile_time::reset_source_parse_count();
        let changed = plan().expect("replan changed Cargo source");
        assert_eq!(
            super::super::compile_time::source_parse_count(),
            unique_sources
        );
        assert_eq!(
            generated_outputs(&first),
            BTreeSet::from(["generated.rs".to_owned()])
        );
        assert_eq!(
            generated_outputs(&changed),
            BTreeSet::from(["changed.rs".to_owned()])
        );

        fixture.write("src/lib.rs", "pub fn malformed( {\n");
        let malformed = plan().expect_err("malformed changed source must be refused");
        let malformed = malformed.to_string();
        assert!(
            malformed.contains("parse") && malformed.contains("src/lib.rs"),
            "unexpected malformed-source refusal: {malformed}"
        );
    }
}
