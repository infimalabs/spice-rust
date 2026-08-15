//! Cargo targets whose complete compiler inputs are not statically provable.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub(super) fn compiler_dynamic_boundaries(
    root: &Path,
    paths: &BTreeSet<String>,
) -> Result<BTreeMap<String, String>, String> {
    let mut boundaries = BTreeMap::new();
    for manifest in paths.iter().filter(|path| {
        Path::new(path.as_str())
            .file_name()
            .is_some_and(|name| name == "Cargo.toml")
    }) {
        let document = fs::read_to_string(root.join(manifest))
            .map_err(|error| format!("read compiler-input manifest {manifest:?}: {error}"))?;
        let document = document
            .parse::<toml::Table>()
            .map_err(|error| format!("parse compiler-input manifest {manifest:?}: {error}"))?;
        let package_root = Path::new(manifest)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        if let Some(package) = document.get("package").and_then(toml::Value::as_table) {
            capture_build_script_boundary(package_root, package, paths, &mut boundaries)?;
        }
        if let Some(library) = document.get("lib").and_then(toml::Value::as_table) {
            capture_proc_macro_boundary(package_root, library, paths, &mut boundaries)?;
        }
    }
    Ok(boundaries)
}

fn capture_build_script_boundary(
    package_root: &Path,
    package: &toml::Table,
    paths: &BTreeSet<String>,
    boundaries: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let candidate = match package.get("build") {
        Some(toml::Value::Boolean(false)) => return Ok(()),
        Some(toml::Value::String(path)) => Some(path.as_str()),
        Some(_) => return Err("Cargo package build target is not a string or false".to_owned()),
        None => Some("build.rs"),
    };
    let Some(path) = candidate.and_then(|path| normalized_package_path(package_root, path)) else {
        return Err("Cargo package build target escapes its selected package".to_owned());
    };
    if paths.contains(&path) {
        boundaries.insert(
            path.clone(),
            format!(
                "Cargo build script {path:?} has dynamic file and environment inputs; compiler receipt reuse requires Cargo/rustc-discovered dependency evidence",
            ),
        );
    }
    Ok(())
}

fn capture_proc_macro_boundary(
    package_root: &Path,
    library: &toml::Table,
    paths: &BTreeSet<String>,
    boundaries: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    if !library
        .get("proc-macro")
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(());
    }
    let source = library
        .get("path")
        .and_then(toml::Value::as_str)
        .unwrap_or("src/lib.rs");
    let path = normalized_package_path(package_root, source)
        .ok_or_else(|| "Cargo procedural-macro target escapes its selected package".to_owned())?;
    if !paths.contains(&path) {
        return Err(format!(
            "Cargo procedural-macro source {path:?} is absent from the selected tree",
        ));
    }
    boundaries.insert(
        path.clone(),
        format!(
            "Cargo procedural-macro target {path:?} has dynamic file and environment inputs; compiler receipt reuse requires Cargo/rustc-discovered dependency evidence",
        ),
    );
    Ok(())
}

fn normalized_package_path(package_root: &Path, relative: &str) -> Option<String> {
    if Path::new(relative).is_absolute() {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in package_root.join(relative).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(component) => normalized.push(component),
            Component::ParentDir if normalized.pop() => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    normalized.to_str().map(|path| path.replace('\\', "/"))
}
