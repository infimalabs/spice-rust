//! Pure staged build-source reach over one canonical Cargo inventory.

use super::*;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum CargoReachEscalationReason {
    OutsidePackage,
    UnreachedInPackage { package: String },
}

impl CargoReachEscalationReason {
    pub(super) const fn kind(&self) -> &'static str {
        match self {
            Self::OutsidePackage => "outside-package",
            Self::UnreachedInPackage { .. } => "unreached-in-package",
        }
    }

    pub(super) fn package(&self) -> Option<&str> {
        match self {
            Self::OutsidePackage => None,
            Self::UnreachedInPackage { package } => Some(package),
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct CargoReachEscalation {
    source: String,
    reason: CargoReachEscalationReason,
}

impl CargoReachEscalation {
    fn outside_package(source: &str) -> Self {
        Self {
            source: source.to_owned(),
            reason: CargoReachEscalationReason::OutsidePackage,
        }
    }

    fn unreached_in_package(source: &str, package: &str) -> Self {
        Self {
            source: source.to_owned(),
            reason: CargoReachEscalationReason::UnreachedInPackage {
                package: package.to_owned(),
            },
        }
    }

    pub(super) fn source(&self) -> &str {
        &self.source
    }

    pub(super) const fn reason(&self) -> &CargoReachEscalationReason {
        &self.reason
    }
}

pub(super) struct SelectionPlan {
    report: crate::TestSelectionReport,
    escalations: Vec<CargoReachEscalation>,
}

impl SelectionPlan {
    #[cfg(test)]
    pub(super) fn report(&self) -> &crate::TestSelectionReport {
        &self.report
    }

    #[cfg(test)]
    pub(super) fn escalations(&self) -> &[CargoReachEscalation] {
        &self.escalations
    }

    pub(super) fn into_parts(self) -> (crate::TestSelectionReport, Vec<CargoReachEscalation>) {
        (self.report, self.escalations)
    }
}

pub(super) fn package_inventory(
    package: MetadataPackage,
    root: &Path,
) -> Result<PackageInventory, WorkspaceTestError> {
    let selected_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    if !package.manifest_path.is_absolute() {
        return Err(WorkspaceTestError::new(format!(
            "workspace package {:?} metadata named relative manifest {}",
            package.name,
            package.manifest_path.display(),
        )));
    }
    let manifest = package.manifest_path.clone();
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
    let mut dependencies = package
        .dependencies
        .into_iter()
        .filter_map(|dependency| dependency.path)
        .map(|path| {
            if !path.is_absolute() {
                return Err(WorkspaceTestError::new(format!(
                    "workspace package {:?} metadata named relative dependency {}",
                    package.name,
                    path.display(),
                )));
            }
            Ok(fs::canonicalize(&path).unwrap_or(path))
        })
        .collect::<Result<Vec<_>, _>>()?;
    dependencies.sort();
    dependencies.dedup();
    let mut declared_targets = package
        .targets
        .into_iter()
        .map(|target| {
            let test = target.test;
            let mut required_features = target.required_features;
            required_features.sort();
            required_features.dedup();
            if let Some(feature) = required_features
                .iter()
                .find(|feature| !package.features.contains_key(feature.as_str()))
            {
                return Err(WorkspaceTestError::new(format!(
                    "workspace package {:?} target {:?} requires undeclared feature {:?}",
                    package.name, target.name, feature,
                )));
            }
            if !target.src_path.is_absolute() {
                return Err(WorkspaceTestError::new(format!(
                    "workspace package {:?} target {:?} metadata named relative source {}",
                    package.name,
                    target.name,
                    target.src_path.display(),
                )));
            }
            let source = target.src_path;
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
            let source_bytes = target_source_bytes(&source, &declared_path_sources)?;
            Ok((
                test,
                TargetIdentity {
                    package: package.id.clone(),
                    kind: target.kind.join("+"),
                    name: target.name,
                    required_features,
                    source,
                    declared_path_sources,
                    source_bytes,
                },
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    declared_targets.sort_by(|left, right| left.1.cmp(&right.1));
    let targets = declared_targets
        .iter()
        .filter(|(test, _)| *test)
        .map(|(_, target)| target.clone())
        .collect();
    let declared_targets = declared_targets
        .into_iter()
        .map(|(_, target)| target)
        .collect();
    Ok(PackageInventory {
        id: package.id,
        name: package.name,
        root: package_root.to_path_buf(),
        dependencies,
        declared_targets,
        targets,
    })
}

pub(super) fn selection_plan(
    inventory: &Inventory,
    root: &Path,
    source: wire::Digest,
    changed_sources: &[String],
) -> Result<SelectionPlan, WorkspaceTestError> {
    let selected_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut build_sources = changed_sources
        .iter()
        .filter(|path| test_relevant_source(path))
        .cloned()
        .collect::<Vec<_>>();
    build_sources.sort();
    build_sources.dedup();
    let mut escalations = BTreeSet::new();
    let mut tests = BTreeSet::new();
    let mut edges = BTreeSet::new();
    for changed in &build_sources {
        let absolute = selected_root.join(changed);
        if workspace_wide_source(changed) {
            for package in &inventory.packages {
                select_package_targets(package, changed, &mut tests, &mut edges)?;
            }
            continue;
        }
        let owner = inventory
            .packages
            .iter()
            .filter(|package| absolute.starts_with(&package.root))
            .max_by_key(|package| package.root.components().count());
        match owner {
            Some(package) => {
                if package_wide_source(changed, package, &absolute) {
                    select_reached_package_targets(
                        inventory, package, changed, &mut tests, &mut edges,
                    )?;
                    continue;
                }
                let direct_targets = package
                    .declared_targets
                    .iter()
                    .filter(|target| target_reaches_source(target, &absolute))
                    .collect::<Vec<_>>();
                if direct_targets.is_empty() {
                    escalations.insert(CargoReachEscalation::unreached_in_package(
                        changed,
                        &package.name,
                    ));
                } else {
                    let reaches_dependents = direct_targets
                        .iter()
                        .any(|target| target_reaches_dependents(target));
                    if reaches_dependents {
                        select_reached_package_targets(
                            inventory, package, changed, &mut tests, &mut edges,
                        )?;
                    } else {
                        let direct_tests = package
                            .targets
                            .iter()
                            .filter(|target| target_reaches_source(target, &absolute))
                            .collect::<Vec<_>>();
                        if direct_tests.is_empty() {
                            escalations.insert(CargoReachEscalation::unreached_in_package(
                                changed,
                                &package.name,
                            ));
                        } else {
                            select_targets(package, direct_tests, changed, &mut tests, &mut edges)?;
                        }
                    }
                }
            }
            None => {
                escalations.insert(CargoReachEscalation::outside_package(changed));
            }
        }
    }
    for escalation in &escalations {
        for package in &inventory.packages {
            select_package_targets(package, escalation.source(), &mut tests, &mut edges)?;
        }
    }
    let report = crate::TestEvidenceGraph::select_tests(
        source,
        build_sources,
        tests.into_iter().collect(),
        edges.into_iter().collect(),
        crate::CoverageCompleteness::Complete,
    )
    .map_err(|error| WorkspaceTestError::new(error.to_string()))?;
    Ok(SelectionPlan {
        report,
        escalations: escalations.into_iter().collect(),
    })
}

#[cfg(test)]
pub(super) fn selection_report(
    inventory: &Inventory,
    root: &Path,
    source: wire::Digest,
    changed_sources: &[String],
) -> Result<crate::TestSelectionReport, WorkspaceTestError> {
    selection_plan(inventory, root, source, changed_sources).map(|plan| plan.into_parts().0)
}

pub(super) fn cargo_check_selections(
    inventory: &Inventory,
    root: &Path,
    changed_sources: &[String],
) -> Result<Vec<CargoCheckSelection>, WorkspaceTestError> {
    let selected_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut reached = BTreeMap::<&str, Option<BTreeSet<CargoTargetSelection>>>::new();
    if changed_sources.is_empty() {
        select_all_package_checks(inventory.packages.iter(), &mut reached);
    }
    for changed in changed_sources
        .iter()
        .filter(|path| test_relevant_source(path))
    {
        if workspace_wide_source(changed) {
            select_all_package_checks(inventory.packages.iter(), &mut reached);
            continue;
        }
        let absolute = selected_root.join(changed);
        let owner = inventory
            .packages
            .iter()
            .filter(|package| absolute.starts_with(&package.root))
            .max_by_key(|package| package.root.components().count());
        let Some(package) = owner else {
            select_all_package_checks(inventory.packages.iter(), &mut reached);
            continue;
        };
        let direct_targets = package
            .declared_targets
            .iter()
            .filter(|target| target_reaches_source(target, &absolute))
            .collect::<Vec<_>>();
        if package_wide_source(changed, package, &absolute)
            || direct_targets
                .iter()
                .any(|target| target_reaches_dependents(target))
        {
            select_all_package_checks(reverse_dependency_closure(inventory, package), &mut reached);
        } else if direct_targets.is_empty() {
            select_all_package_checks(inventory.packages.iter(), &mut reached);
        } else {
            let targets = reached
                .entry(package.id.as_str())
                .or_insert_with(|| Some(BTreeSet::new()));
            if let Some(targets) = targets {
                for target in direct_targets {
                    targets.insert(cargo_target_selection(target)?);
                }
            }
        }
    }
    Ok(inventory
        .packages
        .iter()
        .filter_map(|package| {
            reached
                .get(package.id.as_str())
                .map(|targets| CargoCheckSelection {
                    name: package.name.clone(),
                    root: package.root.clone(),
                    all_targets: targets.is_none(),
                    required_features: targets
                        .as_ref()
                        .map(|targets| {
                            targets
                                .iter()
                                .flat_map(|target| target.required_features.iter().cloned())
                                .collect::<BTreeSet<_>>()
                        })
                        .unwrap_or_else(|| {
                            package
                                .declared_targets
                                .iter()
                                .flat_map(|target| target.required_features.iter().cloned())
                                .collect()
                        })
                        .into_iter()
                        .collect(),
                    targets: targets
                        .as_ref()
                        .map(|targets| targets.iter().cloned().collect())
                        .unwrap_or_default(),
                })
        })
        .collect())
}

fn select_all_package_checks<'a>(
    packages: impl IntoIterator<Item = &'a PackageInventory>,
    reached: &mut BTreeMap<&'a str, Option<BTreeSet<CargoTargetSelection>>>,
) {
    for package in packages {
        reached.insert(package.id.as_str(), None);
    }
}

fn cargo_target_selection(
    target: &TargetIdentity,
) -> Result<CargoTargetSelection, WorkspaceTestError> {
    let flag = target
        .kind
        .split('+')
        .find_map(|kind| match kind {
            "bin" => Some("--bin"),
            "test" => Some("--test"),
            "example" => Some("--example"),
            "bench" => Some("--bench"),
            _ => None,
        })
        .ok_or_else(|| {
            WorkspaceTestError::new(format!(
                "workspace target {} has no exact Cargo check selector",
                target.display()
            ))
        })?;
    Ok(CargoTargetSelection {
        flag,
        name: target.name.clone(),
        required_features: target.required_features.clone(),
    })
}

fn select_reached_package_targets(
    inventory: &Inventory,
    package: &PackageInventory,
    changed: &str,
    tests: &mut BTreeSet<String>,
    edges: &mut BTreeSet<crate::TestSelectionEdge>,
) -> Result<(), WorkspaceTestError> {
    for reached in reverse_dependency_closure(inventory, package) {
        select_package_targets(reached, changed, tests, edges)?;
    }
    Ok(())
}

fn reverse_dependency_closure<'a>(
    inventory: &'a Inventory,
    package: &'a PackageInventory,
) -> Vec<&'a PackageInventory> {
    let mut reached = BTreeSet::from([package.id.as_str()]);
    let mut pending = VecDeque::from([package.root.as_path()]);
    while let Some(root) = pending.pop_front() {
        for dependent in &inventory.packages {
            if dependent
                .dependencies
                .iter()
                .any(|dependency| dependency == root)
                && reached.insert(dependent.id.as_str())
            {
                pending.push_back(&dependent.root);
            }
        }
    }
    inventory
        .packages
        .iter()
        .filter(|candidate| reached.contains(candidate.id.as_str()))
        .collect()
}

fn select_targets<'a>(
    package: &PackageInventory,
    targets: impl IntoIterator<Item = &'a TargetIdentity>,
    changed: &str,
    tests: &mut BTreeSet<String>,
    edges: &mut BTreeSet<crate::TestSelectionEdge>,
) -> Result<(), WorkspaceTestError> {
    for target in targets {
        let test = format!("{}::{}", package.name, target.display());
        tests.insert(test.clone());
        edges.insert(
            crate::TestSelectionEdge::new(changed.to_owned(), test)
                .map_err(|error| WorkspaceTestError::new(error.to_string()))?,
        );
    }
    Ok(())
}

pub(super) fn exhaustive_selection_report(
    inventory: &Inventory,
    source: wire::Digest,
    changed_sources: &[String],
) -> Result<crate::TestSelectionReport, WorkspaceTestError> {
    let mut changed_sources = changed_sources.to_vec();
    changed_sources.sort();
    changed_sources.dedup();
    let mut tests = BTreeSet::new();
    let mut edges = BTreeSet::new();
    for changed in &changed_sources {
        for package in &inventory.packages {
            select_package_targets(package, changed, &mut tests, &mut edges)?;
        }
    }
    crate::TestEvidenceGraph::select_tests(
        source,
        changed_sources,
        tests.into_iter().collect(),
        edges.into_iter().collect(),
        crate::CoverageCompleteness::Complete,
    )
    .map_err(|error| WorkspaceTestError::new(error.to_string()))
}

pub(super) fn test_relevant_source(path: &str) -> bool {
    Path::new(path).extension() == Some(OsStr::new("rs"))
        || Path::new(path).file_name() == Some(OsStr::new("Cargo.toml"))
        || workspace_wide_source(path)
}

fn workspace_wide_source(path: &str) -> bool {
    matches!(
        path,
        ".cargo/config"
            | ".cargo/config.toml"
            | "Cargo.toml"
            | "Cargo.lock"
            | "rust-toolchain"
            | "rust-toolchain.toml"
            | "spice-rust.toml"
            | "baseline.toml"
            | "gate/src/battery/manifest.toml"
    )
}

fn package_wide_source(changed: &str, package: &PackageInventory, absolute: &Path) -> bool {
    absolute == package.root.join("Cargo.toml")
        || (Path::new(changed).file_name() == Some(OsStr::new("build.rs"))
            && absolute.parent() == Some(package.root.as_path()))
}

fn select_package_targets(
    package: &PackageInventory,
    changed: &str,
    tests: &mut BTreeSet<String>,
    edges: &mut BTreeSet<crate::TestSelectionEdge>,
) -> Result<(), WorkspaceTestError> {
    select_targets(package, &package.targets, changed, tests, edges)
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

fn target_is_library(target: &TargetIdentity) -> bool {
    target.kind.split('+').any(|kind| {
        matches!(
            kind,
            "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro"
        )
    })
}

fn target_reaches_dependents(target: &TargetIdentity) -> bool {
    target_is_library(target) || target.kind.split('+').any(|kind| kind == "custom-build")
}
