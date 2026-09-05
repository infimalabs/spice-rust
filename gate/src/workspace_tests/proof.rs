//! Stable proof actions for canonical Cargo typecheck and test shards.

use super::*;

const PROOF_CARGO_JOBS: usize = 1;
const PROOF_TEST_THREADS: usize = 1;

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
    reuse: CargoShardReuse,
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

    #[cfg(test)]
    fn compile_time_authorities(&self) -> &[CargoCompileTimeAuthority] {
        &self.compile_time_authorities
    }

    #[cfg(test)]
    fn compile_time_producers(&self) -> &[CargoCompileTimeProducer] {
        &self.compile_time_producers
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
    pub environment: Environment,
    pub cargo: &'a OsStr,
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
    let CargoGateShardPlanning {
        root,
        base,
        source,
        changed_sources,
        selected_paths,
        environment,
        cargo,
        include_tests,
    } = planning;
    let mut inventory = acquire_resolved_inventory(root, &environment, cargo)?;
    if cargo_workspace_base_inventory_required(root, changed_sources)? {
        let base = base.ok_or_else(|| {
            WorkspaceTestError::new(
                "removed Rust source requires exact base-tree Cargo ownership inventory",
            )
        })?;
        let base_inventory = acquire_resolved_inventory(base, &environment, cargo)?;
        inventory.retain_removed_target_ownership(root, &base_inventory, base, changed_sources)?;
    }

    let reach = selection::cargo_check_selections(&inventory, root, changed_sources)?;
    let typecheck_sources = typecheck_provenance(&inventory, root, changed_sources)?;
    let mut typechecks = Vec::new();
    for check in &reach {
        for target in &check.targets {
            let subcommand = if target.test_harness { "test" } else { "check" };
            let mut argv = vec![
                subcommand.to_owned(),
                "--locked".to_owned(),
                "--package".to_owned(),
                check.name.clone(),
                target.flag.to_owned(),
            ];
            if let Some(name) = &target.name {
                argv.push(name.clone());
            }
            if let Some(features) = qualified_required_features(
                target
                    .required_features
                    .iter()
                    .map(|feature| (check.name.as_str(), feature.as_str())),
            ) {
                argv.extend(["--features".to_owned(), features]);
            }
            if target.test_harness {
                argv.push("--no-run".to_owned());
            }
            argv.push("--message-format=json-render-diagnostics".to_owned());
            let targets = vec![target.identity.clone()];
            let authority = package_input_authority(
                &inventory,
                root,
                &check.name,
                selected_paths,
                typecheck_sources
                    .get(&(check.name.clone(), target.identity.clone()))
                    .map_or(&[], Vec::as_slice),
            )?;
            typechecks.push(CargoGateShard {
                kind: CargoGateShardKind::Typecheck,
                action_id: format!("cargo:typecheck:{}::{}", check.name, target.identity),
                package: check.name.clone(),
                targets,
                argv,
                input_paths: authority.input_paths,
                compile_time_producers: authority.producers,
                compile_time_authorities: authority.compile_time,
                reuse: authority.reuse,
            });
        }
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
        let test_packages = partition_with_target_limit(&selected, SELECTED_TARGETS_PER_SHARD);
        validate_partition_with_target_limit(
            &selected,
            &test_packages,
            SELECTED_TARGETS_PER_SHARD,
        )?;
        test_packages
            .iter()
            .map(|package| test_shard(&inventory, root, package, selected_paths, &test_sources))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };

    typechecks.sort_by(|left, right| left.action_id.cmp(&right.action_id));
    tests.sort_by(|left, right| left.action_id.cmp(&right.action_id));
    Ok(CargoGateShardPlan { typechecks, tests })
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
    let authority =
        package_input_authority(inventory, root, &package.name, selected_paths, &provenance)?;
    Ok(CargoGateShard {
        kind: CargoGateShardKind::Test,
        action_id: format!("cargo:test:{}::{identity}", package.name),
        package: package.name.clone(),
        targets,
        argv,
        input_paths: authority.input_paths,
        compile_time_producers: authority.producers,
        compile_time_authorities: authority.compile_time,
        reuse: authority.reuse,
    })
}

struct CargoPackageInputAuthority {
    input_paths: Vec<String>,
    producers: Vec<CargoCompileTimeProducer>,
    compile_time: Vec<CargoCompileTimeAuthority>,
    reuse: CargoShardReuse,
}

fn package_input_authority(
    inventory: &Inventory,
    root: &Path,
    package_name: &str,
    selected_paths: &[String],
    changed_sources: &[String],
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
    let mut paths = selected_package_input_paths(
        inventory,
        &canonical_root,
        &roots,
        selected_paths,
        changed_sources,
    )?;
    let producers = resolved_compile_time_producers(inventory, &package.id)?;
    let (compile_time, reuse) = compile_time_reuse(
        inventory,
        &canonical_root,
        &roots,
        selected_paths,
        &mut paths,
        &producers,
        &external_local_dependencies,
    );
    Ok(CargoPackageInputAuthority {
        input_paths: paths.into_iter().collect(),
        producers,
        compile_time,
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

fn selected_package_input_paths(
    inventory: &Inventory,
    canonical_root: &Path,
    roots: &BTreeSet<PathBuf>,
    selected_paths: &[String],
    changed_sources: &[String],
) -> Result<BTreeSet<String>, WorkspaceTestError> {
    let relative_roots = roots
        .iter()
        .map(|path| {
            path.strip_prefix(canonical_root)
                .map(Path::to_path_buf)
                .map_err(|_| {
                    WorkspaceTestError::new(format!(
                        "Cargo package root {} escapes selected root {}",
                        path.display(),
                        canonical_root.display(),
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut paths = selected_paths
        .iter()
        .filter(|path| {
            let candidate = Path::new(path.as_str());
            relative_roots
                .iter()
                .any(|root| candidate.starts_with(root))
                || workspace_cargo_authority(candidate)
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    paths.extend(changed_sources.iter().cloned());
    for package in inventory
        .packages
        .iter()
        .filter(|package| roots.contains(&package.root))
    {
        for target in package
            .declared_targets
            .iter()
            .chain(&package.removed_targets)
        {
            for declared in &target.declared_path_sources {
                let relative = declared.path.strip_prefix(canonical_root).map_err(|_| {
                    WorkspaceTestError::new(format!(
                        "declared Cargo input {} escapes selected root {}",
                        declared.path.display(),
                        canonical_root.display(),
                    ))
                })?;
                paths.insert(path_text(relative)?);
            }
        }
    }
    Ok(paths)
}

fn compile_time_reuse(
    inventory: &Inventory,
    canonical_root: &Path,
    roots: &BTreeSet<PathBuf>,
    selected_paths: &[String],
    paths: &mut BTreeSet<String>,
    producers: &[CargoCompileTimeProducer],
    external_local_dependencies: &BTreeSet<PathBuf>,
) -> (Vec<CargoCompileTimeAuthority>, CargoShardReuse) {
    let mut unsafe_authorities = producers
        .iter()
        .filter(|producer| producer.external || !producer.selected_tree_authority)
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
    unsafe_authorities.extend(external_local_dependencies.iter().map(|dependency| {
        format!(
            "external local dependency {}",
            dependency
                .file_name()
                .unwrap_or(dependency.as_os_str())
                .to_string_lossy(),
        )
    }));
    if unsafe_authorities.is_empty() {
        match compile_time_authorities(inventory, canonical_root, roots, selected_paths, paths) {
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

fn workspace_cargo_authority(path: &Path) -> bool {
    matches!(
        path.to_str(),
        Some("Cargo.toml" | "Cargo.lock" | "rust-toolchain" | "rust-toolchain.toml")
    ) || path.starts_with(".cargo")
}

pub(super) fn path_text(path: &Path) -> Result<String, WorkspaceTestError> {
    path.to_str()
        .map(|path| path.replace(std::path::MAIN_SEPARATOR, "/"))
        .ok_or_else(|| {
            WorkspaceTestError::new(format!("Cargo input path {} is not UTF-8", path.display()))
        })
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
    let spec = process_spec(
        cargo,
        shard.argv.iter().map(String::as_str),
        root,
        environment,
        SHARD_DEADLINE,
        Cancellation::Never,
        shard.action_id(),
    )?;
    let outcome = sill::spawn(spec)
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
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn equivalent_roots_plan_identical_path_independent_shards() {
        let first = Fixture::new("first");
        let second = Fixture::new("second");
        let first_plan = first.plan(true);
        let second_plan = second.plan(true);

        assert_eq!(first_plan, second_plan);
        assert!(!first_plan.typechecks().is_empty());
        assert!(!first_plan.tests().is_empty());
        for shard in first_plan.typechecks().iter().chain(first_plan.tests()) {
            assert!(shard.action_id().starts_with("cargo:"));
            assert_eq!(shard.argv()[2], "--package");
            assert_eq!(shard.argv()[3], shard.package());
            assert!(shard.input_paths().contains(&"Cargo.toml".to_owned()));
            assert!(shard.input_paths().contains(&"Cargo.lock".to_owned()));
            assert_eq!(
                shard
                    .input_paths()
                    .contains(&"proof-authority.txt".to_owned()),
                shard.package() == "fixture",
            );
            assert_eq!(
                shard.compile_time_authorities().len(),
                usize::from(shard.package() == "fixture") * 2,
            );
            assert_eq!(
                shard.compile_time_producers().len(),
                usize::from(shard.package() == "fixture") * 2,
            );
            assert!(
                shard
                    .compile_time_producers()
                    .iter()
                    .all(|producer| !producer.external)
            );
            if let Some(authority) = shard.compile_time_authorities().first() {
                assert_eq!(authority.package, "fixture");
                assert_eq!(authority.kind, "custom-build");
                assert_eq!(authority.input_paths, ["proof-authority.txt"]);
                assert_eq!(authority.environment, ["FIXTURE_BUILD_INPUT"]);
                assert!(authority.generated_outputs.is_empty());
                let proc_macro = &shard.compile_time_authorities()[1];
                assert_eq!(proc_macro.package, "fixture-macro");
                assert_eq!(proc_macro.kind, "proc-macro");
                assert!(proc_macro.input_paths.is_empty());
                assert!(proc_macro.environment.is_empty());
                assert!(proc_macro.generated_outputs.is_empty());
            }
            let own_source = format!("{}/src/lib.rs", shard.package());
            let peer = if shard.package() == "fixture" {
                "peer/src/lib.rs"
            } else {
                "fixture/src/lib.rs"
            };
            assert!(shard.input_paths().contains(&own_source));
            assert!(!shard.input_paths().contains(&peer.to_owned()));
            assert!(!shard.input_paths().contains(&"notes.txt".to_owned()));
            assert!(
                !shard
                    .action_id()
                    .contains(first.root.to_string_lossy().as_ref())
            );
            assert!(
                shard
                    .argv()
                    .iter()
                    .all(|argument| !argument.contains(first.root.to_string_lossy().as_ref()))
            );
        }
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the repository graph regression uses the active test environment"
    )]
    fn repository_fold_graph_reaches_external_libsqlite3_sys_build_script() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("gate crate has a workspace parent");
        let environment =
            Environment::new(std::env::vars_os()).expect("ambient test environment is valid");
        let inventory = acquire_resolved_inventory(root, &environment, OsStr::new("cargo"))
            .expect("acquire repository Cargo graph");
        let fold = inventory
            .packages
            .iter()
            .find(|package| package.name == "fold")
            .expect("repository contains fold");
        let producers = resolved_compile_time_producers(&inventory, &fold.id)
            .expect("classify fold compile-time producers");
        let sqlite = producers
            .iter()
            .find(|producer| {
                producer.package == "libsqlite3-sys"
                    && producer.kind == "custom-build"
                    && producer.target == "build-script-build"
            })
            .expect("fold reaches libsqlite3-sys build script through rusqlite");
        assert!(sqlite.external);
        assert!(!sqlite.selected_tree_authority);
        assert!(sqlite.package_id.starts_with("registry+"));
        let serde_derive = producers
            .iter()
            .find(|producer| producer.package == "serde_derive" && producer.kind == "proc-macro")
            .expect("fold reaches the external serde_derive proc macro");
        assert!(serde_derive.external);
        assert!(!serde_derive.selected_tree_authority);
        assert!(serde_derive.package_id.starts_with("registry+"));
    }

    #[test]
    fn external_path_build_script_makes_only_the_dependent_shard_non_cacheable() {
        let fixture = Fixture::new("external-path-producer");
        let external = fixture.root.with_extension("external-build");
        fs::create_dir_all(external.join("src")).expect("create external package source");
        fs::write(
            external.join("Cargo.toml"),
            concat!(
                "[package]\n",
                "name = \"external-build\"\n",
                "version = \"0.1.0\"\n",
                "edition = \"2024\"\n",
                "build = \"build.rs\"\n",
            ),
        )
        .expect("write external package manifest");
        fs::write(external.join("build.rs"), "fn main() {}\n")
            .expect("write external build script");
        fs::write(external.join("src/lib.rs"), "pub const EXTERNAL: u8 = 1;\n")
            .expect("write external package source");
        fs::write(
            fixture.root.join("fixture/Cargo.toml"),
            format!(
                concat!(
                    "[package]\n",
                    "name = \"fixture\"\n",
                    "version = \"0.1.0\"\n",
                    "edition = \"2024\"\n",
                    "build = \"build.rs\"\n\n",
                    "[package.metadata.spice.cargo-proof]\n",
                    "version = 1\n",
                    "complete = true\n\n",
                    "[dependencies]\n",
                    "fixture-macro = {{ path = \"../fixture-macro\" }}\n",
                    "external-build = {{ path = {:?} }}\n",
                ),
                external,
            ),
        )
        .expect("add external build dependency");
        fs::write(
            fixture.root.join("Cargo.lock"),
            concat!(
                "# This file is automatically @generated by Cargo.\n",
                "version = 4\n\n",
                "[[package]]\n",
                "name = \"external-build\"\n",
                "version = \"0.1.0\"\n\n",
                "[[package]]\n",
                "name = \"fixture\"\n",
                "version = \"0.1.0\"\n",
                "dependencies = [\"external-build\", \"fixture-macro\"]\n\n",
                "[[package]]\n",
                "name = \"fixture-macro\"\n",
                "version = \"0.1.0\"\n\n",
                "[[package]]\n",
                "name = \"peer\"\n",
                "version = \"0.1.0\"\n",
            ),
        )
        .expect("add external build dependency to lockfile");

        let plan = fixture.plan(true);
        let shard = plan
            .typechecks()
            .iter()
            .find(|shard| shard.package() == "fixture")
            .expect("fixture typecheck shard is planned");
        let external_producer = shard
            .compile_time_producers()
            .iter()
            .find(|producer| producer.package == "external-build")
            .expect("external build script is classified");
        assert!(external_producer.external);
        assert!(!external_producer.selected_tree_authority);
        assert!(
            shard
                .non_cacheable_reason()
                .is_some_and(|reason| reason.contains("external external-build"))
        );
        assert!(
            plan.typechecks()
                .iter()
                .find(|shard| shard.package() == "peer")
                .expect("peer typecheck shard is planned")
                .non_cacheable_reason()
                .is_none()
        );
        fs::remove_dir_all(external).expect("remove external package");
    }

    #[test]
    fn shard_execution_returns_success_only_for_a_passing_command() {
        let fixture = Fixture::new("execute");
        let plan = fixture.plan(false);
        let shard = plan
            .typechecks()
            .first()
            .expect("fixture selects a typecheck shard");
        let mut output = Vec::new();
        let result = run_cargo_gate_shard_at(
            &fixture.root,
            &fixture.outputs,
            fixture.environment(),
            OsStr::new("cargo"),
            shard,
            &mut output,
        )
        .unwrap_or_else(|error| {
            panic!(
                "valid source passes its planned shard: {error}\n{}",
                String::from_utf8_lossy(&output),
            )
        });
        assert_eq!(result.action_id, shard.action_id());
        assert_eq!(result.completion, "passed");

        fs::write(fixture.root.join("fixture/src/lib.rs"), "pub fn broken(\n")
            .expect("write invalid Rust source");
        let error = run_cargo_gate_shard_at(
            &fixture.root,
            &fixture.outputs,
            fixture.environment(),
            OsStr::new("cargo"),
            shard,
            &mut Vec::new(),
        )
        .expect_err("failed Cargo execution cannot produce a successful result");
        assert!(error.to_string().contains("ended as exit code="), "{error}");
    }

    #[test]
    fn incomplete_file_macro_authority_refuses_before_a_shard_can_be_planned() {
        let fixture = Fixture::new("incomplete-file-macro");
        fs::write(
            fixture.root.join("fixture/src/lib.rs"),
            "pub const AUTHORITY: &str = include_str!(\"../../missing.txt\");\n",
        )
        .expect("write incomplete file-macro source");

        let error = fixture
            .try_plan(true)
            .expect_err("incomplete file-macro closure cannot produce Cargo shards");
        assert!(
            error
                .to_string()
                .contains("is not readable selected-tree authority"),
            "unexpected refusal: {error}",
        );
    }

    #[test]
    fn executable_compile_time_code_requires_a_closed_declaration() {
        let fixture = Fixture::new("missing-build-authority");
        fs::write(
            fixture.root.join("fixture/Cargo.toml"),
            concat!(
                "[package]\n",
                "name = \"fixture\"\n",
                "version = \"0.1.0\"\n",
                "edition = \"2024\"\n",
                "build = \"build.rs\"\n\n",
                "[dependencies]\n",
                "fixture-macro = { path = \"../fixture-macro\" }\n",
            ),
        )
        .expect("remove compile-time authority declaration");

        let reason = fixture.fixture_non_cacheable_reason();
        assert!(
            reason.contains("Cargo shard is non-cacheable")
                && reason.contains("no complete package.metadata.spice.cargo-proof"),
            "unexpected refusal: {reason}",
        );
    }

    #[test]
    fn out_dir_input_without_a_build_script_refuses_before_planning() {
        let fixture = Fixture::new("missing-out-dir-producer");
        fs::write(
            fixture.root.join("fixture/Cargo.toml"),
            concat!(
                "[package]\n",
                "name = \"fixture\"\n",
                "version = \"0.1.0\"\n",
                "edition = \"2024\"\n\n",
                "[dependencies]\n",
                "fixture-macro = { path = \"../fixture-macro\" }\n",
            ),
        )
        .expect("remove OUT_DIR producer");
        fs::remove_file(fixture.root.join("fixture/build.rs"))
            .expect("remove auto-discovered build script");
        fs::write(
            fixture.root.join("fixture/src/lib.rs"),
            "std::include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\n",
        )
        .expect("write OUT_DIR consumer");

        let reason = fixture.fixture_non_cacheable_reason();
        assert!(
            reason.contains("consumes OUT_DIR generated inputs without a custom-build producer"),
            "unexpected refusal: {reason}",
        );
    }

    #[test]
    fn dynamic_rerun_directive_refuses_before_a_shard_can_be_planned() {
        let fixture = Fixture::new("dynamic-rerun");
        fs::write(
            fixture.root.join("fixture/build.rs"),
            "fn main() { let path = \"../proof-authority.txt\"; println!(\"cargo::rerun-if-changed={path}\"); }\n",
        )
        .expect("write dynamic build-script directive");

        let reason = fixture.fixture_non_cacheable_reason();
        assert!(
            reason.contains("dynamic or multiline compile-time output"),
            "unexpected refusal: {reason}",
        );
    }

    #[test]
    fn positional_interpolation_cannot_hide_a_cargo_directive() {
        let fixture = Fixture::new("positional-output");
        fs::write(
            fixture.root.join("fixture/build.rs"),
            "fn main() { println!(\"{}\", \"cargo::rerun-if-changed=../proof-authority.txt\"); }\n",
        )
        .expect("write positionally interpolated build-script output");

        let reason = fixture.fixture_non_cacheable_reason();
        assert!(
            reason.contains("computed or interpolated compile-time output"),
            "unexpected refusal: {reason}",
        );
    }

    #[test]
    fn malformed_rerun_directive_refuses_before_a_shard_can_be_planned() {
        let fixture = Fixture::new("malformed-rerun");
        fs::write(
            fixture.root.join("fixture/build.rs"),
            "fn main() { println!(\"cargo::rerun-if-changed\"); }\n",
        )
        .expect("write malformed build-script directive");

        let reason = fixture.fixture_non_cacheable_reason();
        assert!(
            reason.contains("malformed Cargo directive"),
            "unexpected refusal: {reason}",
        );
    }

    #[test]
    fn linker_directive_is_non_cacheable_without_external_input_authority() {
        let fixture = Fixture::new("linker-directive");
        fs::write(
            fixture.root.join("fixture/build.rs"),
            "fn main() { println!(\"cargo::rustc-link-search=native=../native\"); }\n",
        )
        .expect("write native linker build-script directive");

        let reason = fixture.fixture_non_cacheable_reason();
        assert!(
            reason.contains("unsupported Cargo directive \"rustc-link-search\""),
            "unexpected refusal: {reason}",
        );
    }

    #[test]
    fn undeclared_ambient_environment_refuses_before_planning() {
        let fixture = Fixture::new("ambient-environment");
        fs::write(
            fixture.root.join("fixture/build.rs"),
            "fn main() { let _ = std::env::var(\"AMBIENT_BUILD_INPUT\"); }\n",
        )
        .expect("write ambient build-script read");

        let reason = fixture.fixture_non_cacheable_reason();
        assert!(
            reason.contains("reads undeclared ambient environment \"AMBIENT_BUILD_INPUT\""),
            "unexpected refusal: {reason}",
        );
    }

    #[test]
    fn undeclared_filesystem_read_refuses_before_planning() {
        let fixture = Fixture::new("ambient-file");
        fs::write(
            fixture.root.join("fixture/build.rs"),
            "fn main() { let _ = std::fs::read_to_string(\"../notes.txt\"); }\n",
        )
        .expect("write undeclared build-script file read");

        let reason = fixture.fixture_non_cacheable_reason();
        assert!(
            reason.contains("reads undeclared compile-time file \"../notes.txt\""),
            "unexpected refusal: {reason}",
        );
    }

    #[test]
    fn process_dependency_refuses_before_planning() {
        let fixture = Fixture::new("process-dependency");
        fs::write(
            fixture.root.join("fixture/build.rs"),
            "fn main() { let _ = std::process::Command::new(\"git\").status(); }\n",
        )
        .expect("write process-dependent build script");

        let reason = fixture.fixture_non_cacheable_reason();
        assert!(
            reason.contains("unbounded method dispatch .status()"),
            "unexpected refusal: {reason}",
        );
    }

    #[test]
    fn renamed_process_constructor_is_non_cacheable() {
        let fixture = Fixture::new("renamed-process-constructor");
        fs::write(
            fixture.root.join("fixture/build.rs"),
            "use std::process::Command as C;\nfn main() { let _ = C::new(\"git\"); }\n",
        )
        .expect("write renamed process constructor");

        let reason = fixture.fixture_non_cacheable_reason();
        assert!(
            reason.contains("unrecognized executable call C::new"),
            "unexpected refusal: {reason}",
        );
    }

    #[test]
    fn helper_wrapper_call_is_non_cacheable() {
        let fixture = Fixture::new("helper-wrapper");
        fs::write(
            fixture.root.join("fixture/build.rs"),
            concat!(
                "fn main() { helper(); }\n",
                "fn helper() { let _ = std::process::Command::new(\"git\"); }\n",
            ),
        )
        .expect("write process helper wrapper");

        let reason = fixture.fixture_non_cacheable_reason();
        assert!(
            reason.contains("unrecognized executable call helper"),
            "unexpected refusal: {reason}",
        );
    }

    #[test]
    fn method_dispatch_is_non_cacheable() {
        let fixture = Fixture::new("method-dispatch");
        fs::write(
            fixture.root.join("fixture/build.rs"),
            "fn main() { let _ = std::env::var(\"HOME\").unwrap(); }\n",
        )
        .expect("write method-based environment access");

        let reason = fixture.fixture_non_cacheable_reason();
        assert!(
            reason.contains("unbounded method dispatch .unwrap()"),
            "unexpected refusal: {reason}",
        );
    }

    #[test]
    fn computed_compile_time_output_is_non_cacheable() {
        let fixture = Fixture::new("computed-output");
        fs::write(
            fixture.root.join("fixture/build.rs"),
            "fn main() { let message = \"cargo::rerun-if-changed=../proof-authority.txt\"; println!(message); }\n",
        )
        .expect("write computed build-script output");

        let reason = fixture.fixture_non_cacheable_reason();
        assert!(
            reason.contains("computed or interpolated compile-time output"),
            "unexpected refusal: {reason}",
        );
    }

    #[test]
    fn filesystem_output_is_non_cacheable() {
        let fixture = Fixture::new("filesystem-output");
        fs::write(
            fixture.root.join("fixture/build.rs"),
            "fn main() { let _ = std::fs::write(\"generated.rs\", \"generated\"); }\n",
        )
        .expect("write filesystem output");

        let reason = fixture.fixture_non_cacheable_reason();
        assert!(
            reason.contains("unbounded compile-time filesystem output"),
            "unexpected refusal: {reason}",
        );
    }

    #[test]
    fn escaping_rerun_path_refuses_before_planning() {
        let fixture = Fixture::new("escaping-rerun");
        let external = fixture.root.with_extension("external-authority");
        fs::write(&external, "outside selected tree\n").expect("write external authority");
        fs::write(
            fixture.root.join("fixture/build.rs"),
            format!(
                "fn main() {{ println!(\"cargo::rerun-if-changed=../../{}\"); }}\n",
                external.file_name().unwrap().to_string_lossy(),
            ),
        )
        .expect("write escaping build-script directive");

        let reason = fixture.fixture_non_cacheable_reason();
        assert!(
            reason.contains("escapes selected root"),
            "unexpected refusal: {reason}",
        );
        fs::remove_file(external).expect("remove external authority");
    }

    struct Fixture {
        root: PathBuf,
        outputs: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "gate-cargo-proof-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            let outputs = root.with_extension("outputs");
            fs::create_dir_all(root.join("fixture/src")).expect("create fixture source");
            fs::create_dir_all(root.join("fixture/tests")).expect("create fixture tests");
            fs::create_dir_all(root.join("fixture-macro/src")).expect("create macro source");
            fs::create_dir_all(root.join("peer/src")).expect("create peer source");
            fs::write(
                root.join("Cargo.toml"),
                "[workspace]\nresolver = \"2\"\nmembers = [\"fixture\", \"fixture-macro\", \"peer\"]\n",
            )
            .expect("write workspace manifest");
            fs::write(
                root.join("Cargo.lock"),
                concat!(
                    "# This file is automatically @generated by Cargo.\n",
                    "version = 4\n\n",
                    "[[package]]\n",
                    "name = \"fixture\"\n",
                    "version = \"0.1.0\"\n",
                    "dependencies = [\"fixture-macro\"]\n\n",
                    "[[package]]\n",
                    "name = \"fixture-macro\"\n",
                    "version = \"0.1.0\"\n\n",
                    "[[package]]\n",
                    "name = \"peer\"\n",
                    "version = \"0.1.0\"\n",
                ),
            )
            .expect("write lockfile");
            fs::write(
                root.join("fixture/Cargo.toml"),
                concat!(
                    "[package]\n",
                    "name = \"fixture\"\n",
                    "version = \"0.1.0\"\n",
                    "edition = \"2024\"\n",
                    "build = \"build.rs\"\n\n",
                    "[package.metadata.spice.cargo-proof]\n",
                    "version = 1\n",
                    "complete = true\n",
                    "\n[dependencies]\n",
                    "fixture-macro = { path = \"../fixture-macro\" }\n",
                ),
            )
            .expect("write package manifest");
            fs::write(
                root.join("fixture/build.rs"),
                concat!(
                    "fn main() {\n",
                    "    println!(\"cargo::rerun-if-changed=../proof-authority.txt\");\n",
                    "    println!(\"cargo::rerun-if-env-changed=FIXTURE_BUILD_INPUT\");\n",
                    "}\n",
                ),
            )
            .expect("write package build script");
            fs::write(
                root.join("fixture/src/lib.rs"),
                concat!(
                    "pub const VALUE: u8 = 1;\n",
                    "pub const AUTHORITY: &str = std::r#include_str!(\"../../proof-authority.txt\");\n",
                    "#[fixture_macro::passthrough]\n",
                    "pub fn expanded() {}\n",
                ),
            )
            .expect("write library source");
            fs::write(
                root.join("fixture-macro/Cargo.toml"),
                concat!(
                    "[package]\n",
                    "name = \"fixture-macro\"\n",
                    "version = \"0.1.0\"\n",
                    "edition = \"2024\"\n\n",
                    "[lib]\n",
                    "proc-macro = true\n\n",
                    "[package.metadata.spice.cargo-proof]\n",
                    "version = 1\n",
                    "complete = true\n",
                ),
            )
            .expect("write proc-macro package manifest");
            fs::write(
                root.join("fixture-macro/src/lib.rs"),
                concat!(
                    "extern crate proc_macro;\n",
                    "use proc_macro::TokenStream;\n",
                    "#[proc_macro_attribute]\n",
                    "pub fn passthrough(_: TokenStream, item: TokenStream) -> TokenStream { item }\n",
                ),
            )
            .expect("write proc-macro source");
            fs::write(
                root.join("fixture/tests/integration.rs"),
                "#[test]\nfn value_is_one() { assert_eq!(fixture::VALUE, 1); }\n",
            )
            .expect("write integration source");
            fs::write(
                root.join("peer/Cargo.toml"),
                "[package]\nname = \"peer\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
            )
            .expect("write peer manifest");
            fs::write(root.join("peer/src/lib.rs"), "pub const PEER: u8 = 2;\n")
                .expect("write peer source");
            fs::write(root.join("notes.txt"), "not a Cargo input\n")
                .expect("write unrelated source");
            fs::write(root.join("proof-authority.txt"), "root Cargo input\n")
                .expect("write root Cargo authority");
            Self { root, outputs }
        }

        #[expect(
            clippy::disallowed_methods,
            reason = "the process-execution fixture captures its own test environment"
        )]
        fn environment(&self) -> Environment {
            Environment::new(std::env::vars_os()).expect("ambient test environment is valid")
        }

        fn plan(&self, include_tests: bool) -> CargoGateShardPlan {
            self.try_plan(include_tests)
                .expect("plan fixture Cargo shards")
        }

        fn try_plan(&self, include_tests: bool) -> Result<CargoGateShardPlan, WorkspaceTestError> {
            let changed_sources = [
                "fixture/src/lib.rs".to_owned(),
                "peer/src/lib.rs".to_owned(),
            ];
            let selected_paths = [
                "Cargo.lock".to_owned(),
                "Cargo.toml".to_owned(),
                "fixture/Cargo.toml".to_owned(),
                "fixture/build.rs".to_owned(),
                "fixture/src/lib.rs".to_owned(),
                "fixture/tests/integration.rs".to_owned(),
                "fixture-macro/Cargo.toml".to_owned(),
                "fixture-macro/src/lib.rs".to_owned(),
                "notes.txt".to_owned(),
                "peer/Cargo.toml".to_owned(),
                "peer/src/lib.rs".to_owned(),
                "proof-authority.txt".to_owned(),
            ];
            plan_cargo_gate_shards_at(CargoGateShardPlanning {
                root: &self.root,
                base: None,
                source: wire::Digest::compute(wire::Domain::GateTree, b"fixture"),
                changed_sources: &changed_sources,
                selected_paths: &selected_paths,
                environment: self.environment(),
                cargo: OsStr::new("cargo"),
                include_tests,
            })
        }

        fn fixture_non_cacheable_reason(&self) -> String {
            let plan = self.plan(true);
            let fixture = plan
                .typechecks()
                .iter()
                .find(|shard| shard.package() == "fixture")
                .expect("fixture typecheck shard is planned");
            let peer = plan
                .typechecks()
                .iter()
                .find(|shard| shard.package() == "peer")
                .expect("peer typecheck shard is planned");
            assert!(
                peer.non_cacheable_reason().is_none(),
                "unrelated peer must remain reusable"
            );
            fixture
                .non_cacheable_reason()
                .expect("fixture shard must be non-cacheable")
                .to_owned()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).expect("remove Cargo proof fixture");
            if self.outputs.exists() {
                fs::remove_dir_all(&self.outputs).expect("remove Cargo proof outputs");
            }
        }
    }
}
