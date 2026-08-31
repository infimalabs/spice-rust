use super::*;

/// Exact workspace packages reached through canonical retained Cargo metadata.
pub(crate) struct WorkspacePackageReach {
    pub(super) checks: Vec<CargoCheckSelection>,
    pub(super) roots: Vec<PathBuf>,
}

/// One package-and-target-addressed native Rust compilation unit.
pub(crate) struct RustCheckUnit {
    identity: String,
    arguments: Vec<String>,
}

impl RustCheckUnit {
    /// Stable package-qualified target identity selected from Cargo metadata.
    pub(crate) fn identity(&self) -> &str {
        &self.identity
    }

    /// Direct Cargo arguments that execute this unit without repository selection.
    pub(crate) fn arguments(&self) -> &[String] {
        &self.arguments
    }
}

impl WorkspacePackageReach {
    pub(crate) fn cargo_check_units(&self) -> Vec<RustCheckUnit> {
        let mut units = Vec::new();
        for check in &self.checks {
            for target in &check.targets {
                let subcommand = if target.test_harness { "test" } else { "check" };
                let mut arguments = vec![
                    subcommand.to_owned(),
                    "--locked".to_owned(),
                    "--package".to_owned(),
                    check.name.clone(),
                ];
                arguments.push(target.flag.to_owned());
                if let Some(name) = &target.name {
                    arguments.push(name.clone());
                }
                if let Some(features) = qualified_required_features(
                    target
                        .required_features
                        .iter()
                        .map(|feature| (check.name.as_str(), feature.as_str())),
                ) {
                    arguments.extend(["--features".to_owned(), features]);
                }
                if target.test_harness {
                    arguments.push("--no-run".to_owned());
                }
                units.push(RustCheckUnit {
                    identity: format!("{}::{}", check.name, target.identity),
                    arguments,
                });
            }
        }
        units
    }

    pub(crate) fn reaches_root(&self, root: &Path) -> bool {
        let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        self.roots.contains(&root)
    }
}

pub(crate) fn workspace_package_reach_at(
    root: &Path,
    base: Option<&Path>,
    changed_sources: &[String],
    environment: &Environment,
    cargo: &OsStr,
) -> Result<WorkspacePackageReach, WorkspaceTestError> {
    let mut inventory = acquire_inventory(root, environment, cargo)?;
    if super::cargo_workspace_base_inventory_required(root, changed_sources)? {
        let base = base.ok_or_else(|| {
            WorkspaceTestError::new(
                "removed Rust source requires exact base-tree Cargo ownership inventory",
            )
        })?;
        let base_inventory = acquire_inventory(base, environment, cargo)?;
        inventory.retain_removed_target_ownership(root, &base_inventory, base, changed_sources)?;
    }
    let checks = selection::cargo_check_selections(&inventory, root, changed_sources)?;
    Ok(WorkspacePackageReach {
        roots: checks.iter().map(|check| check.root.clone()).collect(),
        checks,
    })
}

#[derive(Clone, Debug)]
pub(super) struct CargoCheckSelection {
    pub(super) name: String,
    pub(super) root: PathBuf,
    pub(super) targets: Vec<CargoTargetSelection>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct CargoTargetSelection {
    pub(super) identity: String,
    pub(super) flag: &'static str,
    pub(super) name: Option<String>,
    pub(super) required_features: Vec<String>,
    pub(super) test_harness: bool,
}

pub(super) fn qualified_required_features<'a>(
    features: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<String> {
    let features = features
        .into_iter()
        .map(|(package, feature)| format!("{package}/{feature}"))
        .collect::<BTreeSet<_>>();
    (!features.is_empty()).then(|| features.into_iter().collect::<Vec<_>>().join(","))
}

#[derive(Deserialize)]
pub(super) struct Metadata {
    pub(super) packages: Vec<MetadataPackage>,
    pub(super) workspace_members: Vec<String>,
    #[serde(default)]
    pub(super) target_directory: PathBuf,
    #[serde(default)]
    pub(super) build_directory: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct MetadataPackage {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) manifest_path: PathBuf,
    #[serde(default)]
    pub(super) dependencies: Vec<MetadataDependency>,
    pub(super) targets: Vec<MetadataTarget>,
    #[serde(default)]
    pub(super) features: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct MetadataDependency {
    pub(super) path: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct MetadataTarget {
    pub(super) name: String,
    pub(super) kind: Vec<String>,
    pub(super) test: bool,
    pub(super) src_path: PathBuf,
    #[serde(rename = "required-features", default)]
    pub(super) required_features: Vec<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct BinaryOutput {
    pub(super) package_id: String,
    pub(super) package_name: String,
    pub(super) target_name: String,
}

pub(super) struct MissingBinaryOutput<'a> {
    pub(super) binary: &'a BinaryOutput,
    pub(super) path: PathBuf,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct DeclaredPathSource {
    pub(super) path: PathBuf,
    pub(super) test_only: bool,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct TargetIdentity {
    pub(super) package: String,
    pub(super) kind: String,
    pub(super) name: String,
    pub(super) required_features: Vec<String>,
    pub(super) source: PathBuf,
    pub(super) declared_path_sources: Vec<DeclaredPathSource>,
    pub(super) source_bytes: u64,
}

impl TargetIdentity {
    pub(super) fn display(&self) -> String {
        format!("{}:{}", self.kind, self.name)
    }
}

#[derive(Clone, Debug)]
pub(super) struct PackageInventory {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) root: PathBuf,
    pub(super) dependencies: Vec<PathBuf>,
    pub(super) declared_targets: Vec<TargetIdentity>,
    pub(super) removed_targets: Vec<TargetIdentity>,
    pub(super) targets: Vec<TargetIdentity>,
}

#[derive(Clone, Debug)]
pub(super) struct Inventory {
    pub(super) packages: Vec<PackageInventory>,
    pub(super) binary_outputs: Vec<BinaryOutput>,
    pub(super) target_directory: PathBuf,
    pub(super) build_directory: PathBuf,
}

impl Inventory {
    pub(super) fn retain_removed_target_ownership(
        &mut self,
        root: &Path,
        base: &Self,
        base_root: &Path,
        changed_sources: &[String],
    ) -> Result<(), WorkspaceTestError> {
        let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let base_root = fs::canonicalize(base_root).unwrap_or_else(|_| base_root.to_path_buf());
        for changed in changed_sources
            .iter()
            .filter(|path| Path::new(path).extension() == Some(OsStr::new("rs")))
        {
            let selected_source = root.join(changed);
            match fs::symlink_metadata(&selected_source) {
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(WorkspaceTestError::new(format!(
                        "changed Rust source {changed:?} is unavailable at {}: {error}",
                        selected_source.display(),
                    )));
                }
            }
            let base_source = base_root.join(changed);
            let mut owners = Vec::new();
            for package in &base.packages {
                for target in &package.targets {
                    if target.source == base_source {
                        owners.push((package, target));
                    }
                }
            }
            let [(base_package, base_target)] = owners.as_slice() else {
                if owners.is_empty() {
                    continue;
                }
                return Err(WorkspaceTestError::new(format!(
                    "removed Rust source {changed:?} has ambiguous base-tree Cargo target ownership",
                )));
            };
            let relative_package = base_package.root.strip_prefix(&base_root).map_err(|_| {
                WorkspaceTestError::new(format!(
                    "base-tree Cargo package {:?} root {} escapes base root {}",
                    base_package.name,
                    base_package.root.display(),
                    base_root.display(),
                ))
            })?;
            let current_package = self
                .packages
                .iter_mut()
                .find(|package| {
                    package.name == base_package.name
                        && package.root == root.join(relative_package)
                })
                .ok_or_else(|| {
                    WorkspaceTestError::new(format!(
                        "removed Rust source {changed:?} belonged to base-tree Cargo package {:?}, which has no matching current package",
                        base_package.name,
                    ))
                })?;
            let mut target = (*base_target).clone();
            target.package.clone_from(&current_package.id);
            target.source = rebase_path(&target.source, &base_root, &root, changed)?;
            for declared in &mut target.declared_path_sources {
                declared.path = rebase_path(&declared.path, &base_root, &root, changed)?;
            }
            current_package.removed_targets.push(target);
            current_package.removed_targets.sort();
            current_package.removed_targets.dedup();
        }
        Ok(())
    }

    pub(super) fn execution_environment(
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

    pub(super) fn selected(&self, tests: &[String]) -> Result<Self, WorkspaceTestError> {
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
                    removed_targets: package.removed_targets.clone(),
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
        let selected_packages = packages
            .iter()
            .map(|package| package.id.as_str())
            .collect::<BTreeSet<_>>();
        let binary_outputs = self
            .binary_outputs
            .iter()
            .filter(|binary| selected_packages.contains(binary.package_id.as_str()))
            .cloned()
            .collect();
        Ok(Self {
            packages,
            binary_outputs,
            target_directory: self.target_directory.clone(),
            build_directory: self.build_directory.clone(),
        })
    }
}

fn rebase_path(
    path: &Path,
    base_root: &Path,
    root: &Path,
    changed: &str,
) -> Result<PathBuf, WorkspaceTestError> {
    let relative = path.strip_prefix(base_root).map_err(|_| {
        WorkspaceTestError::new(format!(
            "base-tree Cargo ownership for {changed:?} names path {} outside base root {}",
            path.display(),
            base_root.display(),
        ))
    })?;
    Ok(root.join(relative))
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

pub(super) fn acquire_inventory(
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

pub(super) fn inventory_from_metadata(
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
