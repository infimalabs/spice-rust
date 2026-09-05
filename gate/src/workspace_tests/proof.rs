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
    let mut inventory = acquire_inventory(root, &environment, cargo)?;
    if cargo_workspace_base_inventory_required(root, changed_sources)? {
        let base = base.ok_or_else(|| {
            WorkspaceTestError::new(
                "removed Rust source requires exact base-tree Cargo ownership inventory",
            )
        })?;
        let base_inventory = acquire_inventory(base, &environment, cargo)?;
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
            typechecks.push(CargoGateShard {
                kind: CargoGateShardKind::Typecheck,
                action_id: format!("cargo:typecheck:{}::{}", check.name, target.identity),
                package: check.name.clone(),
                targets,
                argv,
                input_paths: package_input_paths(
                    &inventory,
                    root,
                    &check.name,
                    selected_paths,
                    typecheck_sources
                        .get(&(check.name.clone(), target.identity.clone()))
                        .map_or(&[], Vec::as_slice),
                )?,
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
    Ok(CargoGateShard {
        kind: CargoGateShardKind::Test,
        action_id: format!("cargo:test:{}::{identity}", package.name),
        package: package.name.clone(),
        targets,
        argv,
        input_paths: package_input_paths(
            inventory,
            root,
            &package.name,
            selected_paths,
            &provenance,
        )?,
    })
}

fn package_input_paths(
    inventory: &Inventory,
    root: &Path,
    package_name: &str,
    selected_paths: &[String],
    changed_sources: &[String],
) -> Result<Vec<String>, WorkspaceTestError> {
    let canonical_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut roots = BTreeSet::new();
    let mut pending = VecDeque::new();
    let package = inventory
        .packages
        .iter()
        .find(|package| package.name == package_name)
        .ok_or_else(|| {
            WorkspaceTestError::new(format!("Cargo package {package_name:?} vanished"))
        })?;
    pending.push_back(package.root.clone());
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
        if let Some(external) = package
            .dependencies
            .iter()
            .find(|path| !path.starts_with(&canonical_root))
        {
            return Err(WorkspaceTestError::new(format!(
                "Cargo package {:?} has external local dependency {}; reusable proof requires a selected-tree closure",
                package.name,
                external.display(),
            )));
        }
        pending.extend(package.dependencies.iter().cloned());
    }

    let relative_roots = roots
        .iter()
        .map(|path| {
            path.strip_prefix(&canonical_root)
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
                let relative = declared.path.strip_prefix(&canonical_root).map_err(|_| {
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
    Ok(paths.into_iter().collect())
}

fn workspace_cargo_authority(path: &Path) -> bool {
    matches!(
        path.to_str(),
        Some("Cargo.toml" | "Cargo.lock" | "rust-toolchain" | "rust-toolchain.toml")
    ) || path.starts_with(".cargo")
}

fn path_text(path: &Path) -> Result<String, WorkspaceTestError> {
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
        .expect("valid source passes its planned shard");
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
            fs::create_dir_all(root.join("peer/src")).expect("create peer source");
            fs::write(
                root.join("Cargo.toml"),
                "[workspace]\nresolver = \"2\"\nmembers = [\"fixture\", \"peer\"]\n",
            )
            .expect("write workspace manifest");
            fs::write(
                root.join("Cargo.lock"),
                "# This file is automatically @generated by Cargo.\nversion = 4\n\n[[package]]\nname = \"fixture\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"peer\"\nversion = \"0.1.0\"\n",
            )
            .expect("write lockfile");
            fs::write(
                root.join("fixture/Cargo.toml"),
                "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
            )
            .expect("write package manifest");
            fs::write(
                root.join("fixture/src/lib.rs"),
                "pub const VALUE: u8 = 1;\n",
            )
            .expect("write library source");
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
            let changed_sources = [
                "fixture/src/lib.rs".to_owned(),
                "peer/src/lib.rs".to_owned(),
            ];
            let selected_paths = [
                "Cargo.lock".to_owned(),
                "Cargo.toml".to_owned(),
                "fixture/Cargo.toml".to_owned(),
                "fixture/src/lib.rs".to_owned(),
                "fixture/tests/integration.rs".to_owned(),
                "notes.txt".to_owned(),
                "peer/Cargo.toml".to_owned(),
                "peer/src/lib.rs".to_owned(),
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
            .expect("plan fixture Cargo shards")
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
