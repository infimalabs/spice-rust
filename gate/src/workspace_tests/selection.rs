//! Pure staged build-source reach over one canonical Cargo inventory.

use super::*;

pub(super) fn package_inventory(
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
            let source_bytes = target_source_bytes(&source, &declared_path_sources)?;
            Ok(TargetIdentity {
                package: package.id.clone(),
                kind: target.kind.join("+"),
                name: target.name,
                source,
                declared_path_sources,
                source_bytes,
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

pub(super) fn selection_report(
    inventory: &Inventory,
    root: &Path,
    source: wire::Digest,
    changed_sources: &[String],
) -> Result<crate::TestSelectionReport, WorkspaceTestError> {
    let selected_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut build_sources = changed_sources
        .iter()
        .filter(|path| test_relevant_source(path))
        .cloned()
        .collect::<Vec<_>>();
    build_sources.sort();
    build_sources.dedup();
    let mut unmapped = Vec::new();
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
                    select_package_targets(package, changed, &mut tests, &mut edges)?;
                    continue;
                }
                let direct_targets = package
                    .targets
                    .iter()
                    .filter(|target| target_reaches_source(target, &absolute))
                    .collect::<Vec<_>>();
                if direct_targets.is_empty() {
                    unmapped.push(changed.clone());
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
                }
            }
            None => unmapped.push(changed.clone()),
        }
    }
    for changed in &unmapped {
        for package in &inventory.packages {
            select_package_targets(package, changed, &mut tests, &mut edges)?;
        }
    }
    crate::TestEvidenceGraph::select_tests(
        source,
        build_sources,
        tests.into_iter().collect(),
        edges.into_iter().collect(),
        crate::CoverageCompleteness::Complete,
    )
    .map_err(|error| WorkspaceTestError::new(error.to_string()))
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
        "Cargo.toml"
            | "Cargo.lock"
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
    for target in &package.targets {
        let test = format!("{}::{}", package.name, target.display());
        tests.insert(test.clone());
        edges.insert(
            crate::TestSelectionEdge::new(changed.to_owned(), test)
                .map_err(|error| WorkspaceTestError::new(error.to_string()))?,
        );
    }
    Ok(())
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
