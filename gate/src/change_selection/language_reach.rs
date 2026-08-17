//! Conservative language dependency components for ordinary focused arms.
//!
//! Selection first proves which canonical arms a changed path owes. This pass
//! then parses the retained source inventory and replaces only language-arm
//! direct reach with the smallest undirected component that can alter the
//! analyzer's answer. Any graph construct the compiled resolver cannot prove
//! becomes an explicit exhaustive plan; narrowed execution never guesses.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use rustpython_ast::{Expr, Visitor};
use rustpython_parser::Parse;

use super::{
    ChangeArmReach, ChangeReachAuthority, ChangeReachEdge, ChangeReachObject, ChangeSelectionError,
    ChangeSelectionReport,
};
use crate::GateId;
use crate::structural::{
    StructuralPattern, scan_csharp_module_dependencies, scan_csharp_symbols,
    scan_javascript_module_dependencies, scan_javascript_symbols,
};

const PYTHON_ARMS: &[GateId] = &[
    GateId::PythonTypecheck,
    GateId::Reachability,
    GateId::SymbolReachability,
    GateId::PythonUnused,
    GateId::PrivateInternals,
];

/// Expand selected language arms through parsed dependency components.
///
/// `tracked_sources` is the immutable selected tree's complete regular-path
/// inventory. Package and test roots are the already-compiled policy roots
/// used to interpret Python package topology; they are inputs rather than a
/// second configuration read.
///
/// # Errors
///
/// Returns an invalid root declaration or a replacement report that no longer
/// satisfies the canonical reach/provenance schema. Source parse, read,
/// dynamic-import, unresolved-local-edge, and unsupported-construct failures
/// are retained as explicit exhaustive language-graph reach instead.
pub fn compile_language_reach(
    root: &Path,
    tracked_sources: &[String],
    package_roots: &[String],
    test_roots: &[String],
    report: ChangeSelectionReport,
) -> Result<ChangeSelectionReport, ChangeSelectionError> {
    let roots = language_roots(package_roots, test_roots)?;
    let javascript_needed = report.gate_ids().contains(&GateId::JavascriptUnused);
    let csharp_needed = report.gate_ids().contains(&GateId::CsharpUnusedCandidates);
    let python_needed = report
        .gate_ids()
        .iter()
        .any(|gate| PYTHON_ARMS.contains(gate));
    let javascript = javascript_needed.then(|| javascript_graph(root, tracked_sources));
    let csharp = csharp_needed.then(|| csharp_graph(root, tracked_sources));
    let python = python_needed.then(|| python_graph(root, tracked_sources, &roots));

    let mut reaches = Vec::with_capacity(report.reach().len());
    for direct in report.reach() {
        let gate_id = direct.gate_id();
        let sources = report
            .edges()
            .iter()
            .filter(|edge| edge.gate_id() == gate_id)
            .map(|edge| edge.source().to_owned())
            .collect::<Vec<_>>();
        let expanded = match gate_id {
            GateId::EnvNameLedger => exhaustive(
                gate_id,
                &sources,
                "the environment name ledger compares one global declaration/reference inventory",
            ),
            GateId::JavascriptUnused => expand_graph(
                gate_id,
                &sources,
                javascript
                    .as_ref()
                    .expect("selected JavaScript arm compiles its graph"),
                is_javascript,
            ),
            GateId::CsharpUnusedCandidates => expand_graph(
                gate_id,
                &sources,
                csharp.as_ref().expect("selected C# arm compiles its graph"),
                is_csharp,
            ),
            gate if PYTHON_ARMS.contains(&gate) => {
                if sources.iter().any(|source| {
                    matches!(
                        source.as_str(),
                        "pyproject.toml" | "pyrightconfig.json" | "spice-rust.toml"
                    )
                }) {
                    exhaustive(
                        gate_id,
                        &sources,
                        "Python configuration or configured-root authority changed",
                    )
                } else {
                    expand_graph(
                        gate_id,
                        &sources,
                        python
                            .as_ref()
                            .expect("selected Python arm compiles its graph"),
                        is_python,
                    )
                }
            }
            _ => direct.clone(),
        };
        reaches.push(expanded);
    }
    report.with_reach(reaches)
}

fn language_roots(
    package_roots: &[String],
    test_roots: &[String],
) -> Result<Vec<String>, ChangeSelectionError> {
    let mut roots = if package_roots.is_empty() {
        vec!["src".to_owned()]
    } else {
        package_roots.to_vec()
    };
    if test_roots.is_empty() {
        roots.push("tests".to_owned());
    } else {
        roots.extend(test_roots.iter().cloned());
    }
    for root in &roots {
        if root.is_empty()
            || root.starts_with('/')
            || root.starts_with("./")
            || root.split('/').any(|part| part.is_empty() || part == "..")
        {
            return Err(ChangeSelectionError::new(format!(
                "language reach root {root:?} is not normalized and repository-relative"
            )));
        }
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}

fn expand_graph(
    gate_id: GateId,
    sources: &[String],
    graph: &Result<Graph, String>,
    owns: impl Fn(&str) -> bool,
) -> ChangeArmReach {
    let graph = match graph {
        Ok(graph) => graph,
        Err(reason) => return exhaustive(gate_id, sources, reason),
    };
    if let Some(source) = sources.iter().find(|source| !owns(source)) {
        return exhaustive(
            gate_id,
            sources,
            &format!(
                "{} was selected by non-language source {source:?}",
                gate_id.as_str()
            ),
        );
    }
    let mut objects = BTreeSet::new();
    let mut provenance = BTreeSet::new();
    for source in sources {
        if !graph.edges.contains_key(source) {
            return exhaustive(
                gate_id,
                sources,
                &format!("changed language source {source:?} is absent from the selected tree"),
            );
        }
        let (reached, parents) = graph.component(source);
        for path in reached {
            let object = path_object(&path);
            objects.insert(object.clone());
            if path == *source {
                provenance.insert(ChangeReachEdge::changed(source, object));
                continue;
            }
            let parent = parents
                .get(&path)
                .expect("every non-root component path has a BFS parent");
            provenance.insert(ChangeReachEdge::dependency(
                source,
                path_object(parent),
                object,
            ));
        }
    }
    ChangeArmReach::focused(
        gate_id,
        objects.into_iter().collect(),
        provenance.into_iter().collect(),
    )
}

fn exhaustive(gate_id: GateId, sources: &[String], reason: &str) -> ChangeArmReach {
    let repository = ChangeReachObject::Repository;
    let reason = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    let provenance = sources
        .iter()
        .map(|source| ChangeReachEdge::changed(source, repository.clone()))
        .collect();
    ChangeArmReach::exhaustive(
        gate_id,
        ChangeReachAuthority::LanguageGraph,
        reason,
        vec![repository],
        provenance,
    )
}

fn path_object(path: &str) -> ChangeReachObject {
    ChangeReachObject::Path {
        path: path.to_owned(),
    }
}

#[derive(Default)]
struct Graph {
    edges: BTreeMap<String, BTreeSet<String>>,
}

impl Graph {
    fn with_paths(paths: impl IntoIterator<Item = String>) -> Self {
        Self {
            edges: paths
                .into_iter()
                .map(|path| (path, BTreeSet::new()))
                .collect(),
        }
    }

    fn connect(&mut self, left: &str, right: &str) {
        if left == right {
            return;
        }
        self.edges
            .get_mut(left)
            .expect("graph edge starts at an inventoried path")
            .insert(right.to_owned());
        self.edges
            .get_mut(right)
            .expect("graph edge ends at an inventoried path")
            .insert(left.to_owned());
    }

    fn component(&self, source: &str) -> (BTreeSet<String>, BTreeMap<String, String>) {
        let mut reached = BTreeSet::new();
        let mut parents = BTreeMap::new();
        let mut pending = vec![source.to_owned()];
        while let Some(path) = pending.pop() {
            if !reached.insert(path.clone()) {
                continue;
            }
            if let Some(neighbors) = self.edges.get(&path) {
                for neighbor in neighbors.iter().rev() {
                    if !reached.contains(neighbor) && !parents.contains_key(neighbor) {
                        parents.insert(neighbor.clone(), path.clone());
                        pending.push(neighbor.clone());
                    }
                }
            }
        }
        (reached, parents)
    }
}

fn javascript_graph(root: &Path, tracked: &[String]) -> Result<Graph, String> {
    let paths = tracked
        .iter()
        .filter(|path| is_javascript(path))
        .cloned()
        .collect::<Vec<_>>();
    let mut graph = Graph::with_paths(paths.clone());
    let packages = JavascriptPackages::load(root, tracked)?;
    let mut declared: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut referenced: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for path in &paths {
        let source = read_utf8(root, path, "JavaScript")?;
        let symbols = scan_javascript_symbols(&source)
            .map_err(|error| format!("JavaScript dependency parse failed for {path}: {error}"))?;
        for matched in symbols {
            let [capture] = matched.captures() else {
                return Err(format!(
                    "JavaScript dependency parse returned malformed match for {path}"
                ));
            };
            match matched.pattern() {
                StructuralPattern::JavaScriptDeclaration => {
                    declared
                        .entry(capture.text().to_owned())
                        .or_default()
                        .insert(path.clone());
                }
                StructuralPattern::JavaScriptIdentifier
                | StructuralPattern::JavaScriptPropertyIdentifier => {
                    referenced
                        .entry(capture.text().to_owned())
                        .or_default()
                        .insert(path.clone());
                }
                _ => {}
            }
        }
        let dependencies = scan_javascript_module_dependencies(&source)
            .map_err(|error| format!("JavaScript dependency graph for {path}: {error}"))?;
        for specifier in dependencies.specifiers {
            let Some(target) = resolve_javascript(path, &specifier, &paths, &packages)? else {
                continue;
            };
            graph.connect(path, &target);
        }
    }
    connect_symbols(&mut graph, &declared, &referenced);
    Ok(graph)
}

fn csharp_graph(root: &Path, tracked: &[String]) -> Result<Graph, String> {
    let paths = tracked
        .iter()
        .filter(|path| is_csharp(path))
        .cloned()
        .collect::<Vec<_>>();
    let mut graph = Graph::with_paths(paths.clone());
    let mut declared: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut referenced: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut declared_namespaces: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut imported_namespaces: Vec<(String, String)> = Vec::new();
    for path in &paths {
        let source = read_utf8(root, path, "C#")?;
        let symbols = scan_csharp_symbols(&source)
            .map_err(|error| format!("C# dependency parse failed for {path}: {error}"))?;
        for matched in symbols {
            let [capture] = matched.captures() else {
                return Err(format!(
                    "C# dependency parse returned malformed match for {path}"
                ));
            };
            match matched.pattern() {
                StructuralPattern::CSharpDeclaration => {
                    declared
                        .entry(capture.text().to_owned())
                        .or_default()
                        .insert(path.clone());
                }
                StructuralPattern::CSharpIdentifier => {
                    referenced
                        .entry(capture.text().to_owned())
                        .or_default()
                        .insert(path.clone());
                }
                _ => {}
            }
        }
        let dependencies = scan_csharp_module_dependencies(&source)
            .map_err(|error| format!("C# dependency graph for {path}: {error}"))?;
        for namespace in dependencies.namespaces {
            declared_namespaces
                .entry(namespace)
                .or_default()
                .insert(path.clone());
        }
        imported_namespaces.extend(
            dependencies
                .imports
                .into_iter()
                .map(|namespace| (path.clone(), namespace)),
        );
        for include in dependencies.includes {
            let target = resolve_relative_source(path, &include, &paths, &["cs"])?;
            graph.connect(path, &target);
        }
    }
    connect_symbols(&mut graph, &declared, &referenced);
    for paths in declared_namespaces.values() {
        connect_all(&mut graph, paths);
    }
    for (source, imported) in imported_namespaces {
        let targets = declared_namespaces
            .iter()
            .filter(|(namespace, _)| {
                namespace.as_str() == imported
                    || namespace
                        .strip_prefix(&imported)
                        .is_some_and(|tail| tail.starts_with('.'))
            })
            .flat_map(|(_, paths)| paths)
            .cloned()
            .collect::<BTreeSet<_>>();
        for target in targets {
            graph.connect(&source, &target);
        }
    }
    Ok(graph)
}

fn python_graph(root: &Path, tracked: &[String], roots: &[String]) -> Result<Graph, String> {
    let roots = python_configured_roots(root, tracked, roots)?;
    let paths = tracked
        .iter()
        .filter(|path| is_python(path))
        .cloned()
        .collect::<Vec<_>>();
    let aliases = python_aliases(&paths, &roots)?;
    let local_heads = aliases
        .keys()
        .filter_map(|name| name.split('.').next())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let mut graph = Graph::with_paths(paths.clone());
    let mut packages: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for path in &paths {
        let source = read_utf8(root, path, "Python")?;
        let suite = rustpython_ast::Suite::parse(&source, path)
            .map_err(|error| format!("Python dependency parse failed for {path}: {error}"))?;
        let current = preferred_python_alias(path, &roots);
        let initializer = path.ends_with("/__init__.py") || path.ends_with("/__init__.pyi");
        let mut collector = PythonImportCollector::new(current.clone(), initializer);
        for statement in suite {
            collector.visit_stmt(statement);
        }
        if let Some(dynamic) = collector.dynamic {
            return Err(format!("Python dependency graph for {path}: {dynamic}"));
        }
        for import in collector.exact_imports {
            let candidates = aliases.get(&import).cloned().unwrap_or_default();
            if candidates.len() > 1 {
                return Err(format!(
                    "Python import {import:?} in {path} resolves to multiple selected sources"
                ));
            }
            if let Some(target) = candidates.into_iter().next() {
                graph.connect(path, &target);
                continue;
            }
            if import
                .split('.')
                .next()
                .is_some_and(|head| local_heads.contains(head))
            {
                return Err(format!(
                    "Python import {import:?} in {path} does not resolve in the selected tree"
                ));
            }
        }
        for import in collector.imports {
            let candidates = resolve_python_import(&import, &aliases);
            if candidates.len() > 1 {
                return Err(format!(
                    "Python import {import:?} in {path} resolves to multiple selected sources"
                ));
            }
            if let Some(target) = candidates.into_iter().next() {
                graph.connect(path, &target);
                continue;
            }
            if import
                .split('.')
                .next()
                .is_some_and(|head| local_heads.contains(head))
                || import.starts_with('.')
            {
                return Err(format!(
                    "Python import {import:?} in {path} does not resolve in the selected tree"
                ));
            }
        }
        if let Some(package) = python_package(path, &roots) {
            packages.entry(package).or_default().insert(path.clone());
        }
    }
    for paths in packages.values() {
        connect_all(&mut graph, paths);
    }
    Ok(graph)
}

fn python_configured_roots(
    root: &Path,
    tracked: &[String],
    roots: &[String],
) -> Result<Vec<String>, String> {
    let mut configured = roots.to_vec();
    if tracked.iter().any(|path| path == "pyrightconfig.json") {
        return Err(
            "Python dependency graph does not narrow pyrightconfig.json precedence".to_owned(),
        );
    }
    if !tracked.iter().any(|path| path == "pyproject.toml") {
        return Ok(configured);
    }
    let source = read_utf8(root, "pyproject.toml", "Python configuration")?;
    let document: toml::Value = toml::from_str(&source)
        .map_err(|error| format!("Python configuration pyproject.toml: {error}"))?;
    let pyright = document
        .get("tool")
        .and_then(|tool| tool.get("pyright"))
        .and_then(toml::Value::as_table);
    let Some(pyright) = pyright else {
        return Ok(configured);
    };
    if pyright.contains_key("extends") {
        return Err(
            "Python dependency graph does not narrow inherited tool.pyright extends configuration"
                .to_owned(),
        );
    }
    if let Some(extra_paths) = pyright.get("extraPaths") {
        extend_python_roots(&mut configured, extra_paths, "tool.pyright.extraPaths")?;
    }
    for field in ["stubPath", "typeshedPath"] {
        let Some(value) = pyright.get(field) else {
            continue;
        };
        let root = value
            .as_str()
            .ok_or_else(|| format!("Python configuration tool.pyright.{field} is not text"))?;
        push_python_root(&mut configured, root, &format!("tool.pyright.{field}"))?;
    }
    if let Some(environments) = pyright.get("executionEnvironments") {
        let environments = environments.as_array().ok_or_else(|| {
            "Python configuration tool.pyright.executionEnvironments is not an array".to_owned()
        })?;
        for (index, environment) in environments.iter().enumerate() {
            let environment = environment.as_table().ok_or_else(|| {
                format!(
                    "Python configuration tool.pyright.executionEnvironments[{index}] is not a table"
                )
            })?;
            if let Some(root) = environment.get("root") {
                let root = root.as_str().ok_or_else(|| {
                    format!(
                        "Python configuration tool.pyright.executionEnvironments[{index}].root is not text"
                    )
                })?;
                push_python_root(
                    &mut configured,
                    root,
                    &format!("tool.pyright.executionEnvironments[{index}].root"),
                )?;
            }
            if let Some(extra_paths) = environment.get("extraPaths") {
                extend_python_roots(
                    &mut configured,
                    extra_paths,
                    &format!("tool.pyright.executionEnvironments[{index}].extraPaths"),
                )?;
            }
        }
    }
    configured.sort();
    configured.dedup();
    Ok(configured)
}

fn extend_python_roots(
    roots: &mut Vec<String>,
    value: &toml::Value,
    field: &str,
) -> Result<(), String> {
    let values = value
        .as_array()
        .ok_or_else(|| format!("Python configuration {field} is not an array"))?;
    for (index, value) in values.iter().enumerate() {
        let root = value
            .as_str()
            .ok_or_else(|| format!("Python configuration {field}[{index}] is not text"))?;
        push_python_root(roots, root, &format!("{field}[{index}]"))?;
    }
    Ok(())
}

fn push_python_root(roots: &mut Vec<String>, root: &str, field: &str) -> Result<(), String> {
    let root = root
        .strip_prefix("./")
        .unwrap_or(root)
        .trim_end_matches('/');
    if root.is_empty() || root == "." {
        return Ok(());
    }
    if root.starts_with('/')
        || root.split('/').any(|part| part.is_empty() || part == "..")
        || root.contains(['\\', '*', '?', '[', ']', '{', '}', '$', '\0', '\r', '\n'])
    {
        return Err(format!(
            "Python configuration {field} root {root:?} is not normalized and repository-relative"
        ));
    }
    roots.push(root.to_owned());
    Ok(())
}

fn connect_symbols(
    graph: &mut Graph,
    declarations: &BTreeMap<String, BTreeSet<String>>,
    references: &BTreeMap<String, BTreeSet<String>>,
) {
    for (symbol, declaring_paths) in declarations {
        connect_all(graph, declaring_paths);
        if let Some(referencing_paths) = references.get(symbol) {
            for declaration in declaring_paths {
                for reference in referencing_paths {
                    graph.connect(declaration, reference);
                }
            }
        }
    }
}

fn connect_all(graph: &mut Graph, paths: &BTreeSet<String>) {
    let paths = paths.iter().collect::<Vec<_>>();
    for pair in paths.windows(2) {
        graph.connect(pair[0], pair[1]);
    }
}

fn resolve_javascript(
    source: &str,
    specifier: &str,
    paths: &[String],
    packages: &JavascriptPackages,
) -> Result<Option<String>, String> {
    if specifier.starts_with("node:") {
        return Ok(None);
    }
    if !specifier.starts_with('.') {
        return packages.external(source, specifier).map(|()| None);
    }
    resolve_relative_source(source, specifier, paths, &["js", "mjs", "cjs"]).map(Some)
}

#[derive(Default)]
struct JavascriptPackages {
    packages: Vec<JavascriptPackage>,
}

struct JavascriptPackage {
    directory: String,
    name: Option<String>,
    dependencies: BTreeSet<String>,
}

impl JavascriptPackages {
    fn load(root: &Path, tracked: &[String]) -> Result<Self, String> {
        let mut packages = Vec::new();
        for path in tracked.iter().filter(|path| path.ends_with("package.json")) {
            let source = read_utf8(root, path, "JavaScript package")?;
            let document: serde_json::Value = serde_json::from_str(&source)
                .map_err(|error| format!("JavaScript package authority {path}: {error}"))?;
            let object = document.as_object().ok_or_else(|| {
                format!("JavaScript package authority {path} is not a JSON object")
            })?;
            let name = object
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            let mut dependencies = BTreeSet::new();
            for field in [
                "dependencies",
                "devDependencies",
                "peerDependencies",
                "optionalDependencies",
            ] {
                let Some(value) = object.get(field) else {
                    continue;
                };
                let entries = value.as_object().ok_or_else(|| {
                    format!("JavaScript package authority {path} field {field} is not an object")
                })?;
                dependencies.extend(entries.keys().cloned());
            }
            let directory = Path::new(path)
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .to_string_lossy()
                .replace('\\', "/");
            packages.push(JavascriptPackage {
                directory,
                name,
                dependencies,
            });
        }
        packages.sort_by(|left, right| {
            right
                .directory
                .len()
                .cmp(&left.directory.len())
                .then_with(|| left.directory.cmp(&right.directory))
        });
        Ok(Self { packages })
    }

    fn external(&self, source: &str, specifier: &str) -> Result<(), String> {
        if specifier.starts_with('/') || specifier.starts_with('#') {
            return Err(format!(
                "JavaScript module {specifier:?} in {source} uses an unresolved configured-root specifier"
            ));
        }
        let package_name = javascript_package_name(specifier).ok_or_else(|| {
            format!("JavaScript module {specifier:?} in {source} has no package identity")
        })?;
        let authority = self.packages.iter().find(|package| {
            package.directory.is_empty()
                || source
                    .strip_prefix(&package.directory)
                    .is_some_and(|tail| tail.starts_with('/'))
        });
        let Some(authority) = authority else {
            return Err(format!(
                "JavaScript bare import {specifier:?} in {source} has no package authority"
            ));
        };
        if authority.name.as_deref() == Some(package_name) {
            return Err(format!(
                "JavaScript package self-import {specifier:?} in {source} is unsupported"
            ));
        }
        if authority.dependencies.contains(package_name) {
            Ok(())
        } else {
            Err(format!(
                "JavaScript bare import {specifier:?} in {source} is absent from its package authority"
            ))
        }
    }
}

fn javascript_package_name(specifier: &str) -> Option<&str> {
    if specifier.starts_with('@') {
        let slash = specifier.find('/')?;
        let tail = &specifier[slash + 1..];
        let end = tail
            .find('/')
            .map_or(specifier.len(), |index| slash + 1 + index);
        return specifier.get(..end);
    }
    specifier.split('/').next()
}

fn resolve_relative_source(
    source: &str,
    specifier: &str,
    paths: &[String],
    extensions: &[&str],
) -> Result<String, String> {
    let parent = Path::new(source).parent().unwrap_or_else(|| Path::new(""));
    let normalized = normalize_relative(parent.join(specifier)).ok_or_else(|| {
        format!("dependency {specifier:?} from {source} escapes the selected tree")
    })?;
    let base = normalized.to_string_lossy().replace('\\', "/");
    let mut candidates = BTreeSet::new();
    if paths.iter().any(|path| path == &base) {
        candidates.insert(base.clone());
    }
    for extension in extensions {
        let path = format!("{base}.{extension}");
        if paths.contains(&path) {
            candidates.insert(path);
        }
        let index = format!("{base}/index.{extension}");
        if paths.contains(&index) {
            candidates.insert(index);
        }
    }
    if candidates.len() == 1 {
        return Ok(candidates.into_iter().next().expect("one candidate exists"));
    }
    Err(format!(
        "dependency {specifier:?} from {source} resolved to {} selected sources",
        candidates.len()
    ))
}

fn normalize_relative(path: PathBuf) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            Component::Prefix(_) | Component::RootDir => return None,
        }
    }
    Some(normalized)
}

fn python_aliases(
    paths: &[String],
    roots: &[String],
) -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    let mut aliases: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for path in paths {
        let mut names = BTreeSet::from([python_module_name(path)]);
        for root in roots {
            if let Some(relative) = path
                .strip_prefix(root)
                .and_then(|tail| tail.strip_prefix('/'))
            {
                names.insert(python_module_name(relative));
            }
        }
        for name in names.into_iter().filter(|name| !name.is_empty()) {
            aliases.entry(name).or_default().insert(path.clone());
        }
    }
    if let Some((name, paths)) = aliases.iter().find(|(_, paths)| paths.len() > 2) {
        return Err(format!(
            "Python module alias {name:?} resolves to {} selected sources",
            paths.len()
        ));
    }
    Ok(aliases)
}

fn preferred_python_alias(path: &str, roots: &[String]) -> String {
    roots
        .iter()
        .find_map(|root| {
            path.strip_prefix(root)
                .and_then(|tail| tail.strip_prefix('/'))
                .map(python_module_name)
        })
        .unwrap_or_else(|| python_module_name(path))
}

fn python_module_name(path: &str) -> String {
    let without_extension = path
        .strip_suffix(".pyi")
        .or_else(|| path.strip_suffix(".py"))
        .unwrap_or(path);
    let mut components = without_extension.split('/').collect::<Vec<_>>();
    if components.last() == Some(&"__init__") {
        components.pop();
    }
    components.join(".")
}

fn python_package(path: &str, roots: &[String]) -> Option<String> {
    let alias = preferred_python_alias(path, roots);
    alias
        .split('.')
        .next()
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
}

fn resolve_python_import(
    import: &str,
    aliases: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut candidate = import;
    loop {
        if let Some(paths) = aliases.get(candidate) {
            return paths.clone();
        }
        let Some((prefix, _)) = candidate.rsplit_once('.') else {
            return BTreeSet::new();
        };
        candidate = prefix;
    }
}

struct PythonImportCollector {
    current: String,
    initializer: bool,
    exact_imports: BTreeSet<String>,
    imports: BTreeSet<String>,
    dynamic: Option<String>,
}

impl PythonImportCollector {
    fn new(current: String, initializer: bool) -> Self {
        Self {
            current,
            initializer,
            exact_imports: BTreeSet::new(),
            imports: BTreeSet::new(),
            dynamic: None,
        }
    }
}

impl Visitor for PythonImportCollector {
    fn visit_stmt_import(&mut self, node: rustpython_ast::StmtImport) {
        self.exact_imports
            .extend(node.names.iter().map(|alias| alias.name.to_string()));
        self.generic_visit_stmt_import(node);
    }

    fn visit_stmt_import_from(&mut self, node: rustpython_ast::StmtImportFrom) {
        let module = resolve_python_relative(
            &self.current,
            self.initializer,
            node.level.as_ref().map_or(0, rustpython_ast::Int::to_usize),
            node.module.as_ref().map(ToString::to_string),
        );
        if module.is_empty() {
            self.dynamic = Some("relative import escapes its package root".to_owned());
        } else {
            self.imports.insert(module.clone());
            self.imports.extend(
                node.names
                    .iter()
                    .map(|alias| format!("{module}.{}", alias.name)),
            );
        }
        self.generic_visit_stmt_import_from(node);
    }

    fn visit_expr_call(&mut self, node: rustpython_ast::ExprCall) {
        if expression_path(&node.func)
            .is_some_and(|path| matches!(path.as_str(), "__import__" | "importlib.import_module"))
        {
            self.dynamic = Some("dynamic Python import is unsupported".to_owned());
        }
        self.generic_visit_expr_call(node);
    }
}

fn expression_path(expression: &Expr) -> Option<String> {
    match expression {
        Expr::Name(node) => Some(node.id.to_string()),
        Expr::Attribute(node) => {
            expression_path(&node.value).map(|base| format!("{base}.{}", node.attr))
        }
        _ => None,
    }
}

fn resolve_python_relative(
    current: &str,
    initializer: bool,
    level: usize,
    module: Option<String>,
) -> String {
    if level == 0 {
        return module.unwrap_or_default();
    }
    let mut parts = current.split('.').map(str::to_owned).collect::<Vec<_>>();
    if !initializer && !parts.is_empty() {
        parts.pop();
    }
    for _ in 1..level {
        parts.pop();
    }
    if let Some(module) = module {
        parts.extend(module.split('.').map(str::to_owned));
    }
    parts.join(".")
}

fn read_utf8(root: &Path, path: &str, language: &str) -> Result<String, String> {
    fs::read_to_string(root.join(path))
        .map_err(|error| format!("{language} dependency source {path} is unavailable: {error}"))
}

fn is_javascript(path: &str) -> bool {
    matches!(extension(path), Some("js" | "mjs" | "cjs"))
}

fn is_csharp(path: &str) -> bool {
    extension(path) == Some("cs")
}

fn is_python(path: &str) -> bool {
    matches!(extension(path), Some("py" | "pyi"))
}

fn extension(path: &str) -> Option<&str> {
    path.rsplit_once('.').map(|(_, extension)| extension)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use wire::{Digest, Domain};

    use super::*;
    use crate::{ChangeReachMode, select_current_source_change_gates};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn javascript_reach_keeps_import_and_symbol_dependents_but_not_a_peer_component() {
        let fixture = Fixture::new("javascript-component");
        fixture.write(
            "package.json",
            "{\"name\":\"fixture\",\"dependencies\":{\"external\":\"1.0.0\"}}\n",
        );
        fixture.write(
            "src/entry.js",
            concat!(
                "import assert from 'node:assert';\n",
                "import external from 'external';\n",
                "import { used } from './dep.js';\n",
                "assert(external);\n",
                "console.log(used);\n",
            ),
        );
        fixture.write("src/dep.js", "export const used = 1;\n");
        fixture.write("tests/consumer.js", "console.log(used);\n");
        fixture.write("other/isolated.js", "const lonely = 1;\n");
        let tracked = fixture.paths();
        let selection = select_current_source_change_gates(source(), &["src/entry.js".to_owned()])
            .expect("JavaScript change selects its arms");

        let expanded = compile_language_reach(
            &fixture.root,
            &tracked,
            &["src".to_owned()],
            &["tests".to_owned()],
            selection,
        )
        .expect("JavaScript graph compiles");
        assert_eq!(
            reached_paths(&expanded, GateId::JavascriptUnused),
            BTreeSet::from([
                "src/dep.js".to_owned(),
                "src/entry.js".to_owned(),
                "tests/consumer.js".to_owned(),
            ])
        );
        assert_eq!(
            expanded
                .reach_for(GateId::EnvNameLedger)
                .expect("ledger selected")
                .mode(),
            &ChangeReachMode::Exhaustive {
                authority: ChangeReachAuthority::LanguageGraph,
                reason: "the environment name ledger compares one global declaration/reference inventory"
                    .to_owned(),
            }
        );
    }

    #[test]
    fn csharp_reach_keeps_namespace_and_inheritance_dependents_only() {
        let fixture = Fixture::new("csharp-component");
        fixture.write(
            "src/Widget.cs",
            "namespace Product.Core;\nclass Widget { private int unused; }\n",
        );
        fixture.write(
            "src/Child.cs",
            "using Product.Core;\nclass Child : Widget { }\n",
        );
        fixture.write("other/Other.cs", "namespace Other;\nclass OtherType { }\n");
        let tracked = fixture.paths();
        let selection = select_current_source_change_gates(source(), &["src/Widget.cs".to_owned()])
            .expect("C# change selects its arms");

        let expanded = compile_language_reach(&fixture.root, &tracked, &[], &[], selection)
            .expect("C# graph compiles");
        assert_eq!(
            reached_paths(&expanded, GateId::CsharpUnusedCandidates),
            BTreeSet::from(["src/Child.cs".to_owned(), "src/Widget.cs".to_owned()])
        );
    }

    #[test]
    fn python_reach_keeps_transitive_package_component_only() {
        let fixture = Fixture::new("python-component");
        fixture.write("src/pkg/__init__.py", "\n");
        fixture.write("src/pkg/a.py", "from pkg import b\nVALUE = b.VALUE\n");
        fixture.write("src/pkg/b.py", "from pkg import c\nVALUE = c.VALUE\n");
        fixture.write("src/pkg/c.py", "VALUE = 1\n");
        fixture.write("src/other/alone.py", "VALUE = 2\n");
        fixture.write("tests/test_pkg.py", "from pkg import a\nassert a.VALUE\n");
        let tracked = fixture.paths();
        let selection = select_current_source_change_gates(source(), &["src/pkg/b.py".to_owned()])
            .expect("Python change selects its arms");

        let expanded = compile_language_reach(
            &fixture.root,
            &tracked,
            &["src".to_owned()],
            &["tests".to_owned()],
            selection,
        )
        .expect("Python graph compiles");
        let expected = BTreeSet::from([
            "src/pkg/__init__.py".to_owned(),
            "src/pkg/a.py".to_owned(),
            "src/pkg/b.py".to_owned(),
            "src/pkg/c.py".to_owned(),
            "tests/test_pkg.py".to_owned(),
        ]);
        for gate in PYTHON_ARMS {
            assert_eq!(reached_paths(&expanded, *gate), expected, "{gate}");
        }
    }

    #[test]
    fn malformed_peer_source_escalates_before_narrowed_execution() {
        let fixture = Fixture::new("malformed-javascript");
        fixture.write("src/changed.js", "const changed = 1;\n");
        fixture.write("other/broken.js", "const = ;\n");
        let tracked = fixture.paths();
        let selection =
            select_current_source_change_gates(source(), &["src/changed.js".to_owned()])
                .expect("JavaScript change selects its arms");

        let expanded = compile_language_reach(&fixture.root, &tracked, &[], &[], selection)
            .expect("malformed graph is explicit evidence");
        let reach = expanded
            .reach_for(GateId::JavascriptUnused)
            .expect("JavaScript arm remains selected");
        assert!(matches!(
            reach.mode(),
            ChangeReachMode::Exhaustive {
                authority: ChangeReachAuthority::LanguageGraph,
                reason,
            } if reason.contains("other/broken.js")
        ));
        assert_eq!(reach.objects(), [ChangeReachObject::Repository]);
    }

    #[test]
    fn dynamic_javascript_import_explicitly_escalates_before_narrowed_execution() {
        let fixture = Fixture::new("dynamic-javascript");
        fixture.write(
            "src/changed.js",
            "const target = './dependency.js';\nimport(target);\n",
        );
        fixture.write("src/dependency.js", "export const VALUE = 1;\n");
        let tracked = fixture.paths();
        let selection =
            select_current_source_change_gates(source(), &["src/changed.js".to_owned()])
                .expect("JavaScript change selects its arms");

        let expanded = compile_language_reach(&fixture.root, &tracked, &[], &[], selection)
            .expect("dynamic import becomes explicit reach");
        let reach = expanded
            .reach_for(GateId::JavascriptUnused)
            .expect("JavaScript arm remains selected");
        assert!(matches!(
            reach.mode(),
            ChangeReachMode::Exhaustive {
                authority: ChangeReachAuthority::LanguageGraph,
                reason,
            } if reason.contains("dynamic or unsupported specifier")
        ));
        assert_eq!(reach.objects(), [ChangeReachObject::Repository]);
    }

    #[test]
    fn unresolved_local_python_import_explicitly_escalates() {
        let fixture = Fixture::new("unresolved-python");
        fixture.write("src/pkg/__init__.py", "\n");
        fixture.write("src/pkg/changed.py", "import pkg.missing\n");
        let tracked = fixture.paths();
        let selection =
            select_current_source_change_gates(source(), &["src/pkg/changed.py".to_owned()])
                .expect("Python change selects its arms");

        let expanded =
            compile_language_reach(&fixture.root, &tracked, &["src".to_owned()], &[], selection)
                .expect("unresolved import becomes explicit reach");
        let reach = expanded
            .reach_for(GateId::PythonTypecheck)
            .expect("Python typecheck remains selected");
        assert!(matches!(
            reach.mode(),
            ChangeReachMode::Exhaustive {
                authority: ChangeReachAuthority::LanguageGraph,
                reason,
            } if reason.contains("pkg.missing") && reason.contains("does not resolve")
        ));
        assert_eq!(reach.objects(), [ChangeReachObject::Repository]);
    }

    #[test]
    fn unsupported_csharp_reference_directive_explicitly_escalates() {
        let fixture = Fixture::new("unsupported-csharp-reference");
        fixture.write("src/Changed.cs", "#r \"external.dll\"\nclass Changed { }\n");
        let tracked = fixture.paths();
        let selection =
            select_current_source_change_gates(source(), &["src/Changed.cs".to_owned()])
                .expect("C# change selects its arms");

        let expanded = compile_language_reach(&fixture.root, &tracked, &[], &[], selection)
            .expect("unsupported reference becomes explicit reach");
        let reach = expanded
            .reach_for(GateId::CsharpUnusedCandidates)
            .expect("C# arm remains selected");
        assert!(matches!(
            reach.mode(),
            ChangeReachMode::Exhaustive {
                authority: ChangeReachAuthority::LanguageGraph,
                reason,
            } if reason.contains("C#")
        ));
        assert_eq!(reach.objects(), [ChangeReachObject::Repository]);
    }

    #[test]
    fn python_configuration_change_explicitly_escalates_every_graph_arm() {
        let fixture = Fixture::new("python-configuration");
        fixture.write("pyproject.toml", "[project]\nname = 'fixture'\n");
        fixture.write("src/pkg/module.py", "VALUE = 1\n");
        let tracked = fixture.paths();
        let selection =
            select_current_source_change_gates(source(), &["pyproject.toml".to_owned()])
                .expect("Python configuration selects graph arms");

        let expanded = compile_language_reach(
            &fixture.root,
            &tracked,
            &["src".to_owned()],
            &["tests".to_owned()],
            selection,
        )
        .expect("Python configuration reach compiles");

        for gate in PYTHON_ARMS {
            let reach = expanded.reach_for(*gate).expect("graph arm selected");
            assert!(matches!(
                reach.mode(),
                ChangeReachMode::Exhaustive {
                    authority: ChangeReachAuthority::LanguageGraph,
                    reason,
                } if reason == "Python configuration or configured-root authority changed"
            ));
            assert_eq!(reach.objects(), [ChangeReachObject::Repository]);
        }
    }

    #[test]
    fn pyright_extra_path_defines_package_topology_without_joining_peer_packages() {
        let fixture = Fixture::new("python-extra-path");
        fixture.write("pyproject.toml", "[tool.pyright]\nextraPaths = ['lib']\n");
        fixture.write("lib/pkg/__init__.py", "\n");
        fixture.write("lib/pkg/a.py", "from pkg import b\nVALUE = b.VALUE\n");
        fixture.write("lib/pkg/b.py", "VALUE = 1\n");
        fixture.write("lib/other/alone.py", "VALUE = 2\n");
        fixture.write("tests/test_pkg.py", "from pkg import a\nassert a.VALUE\n");
        let tracked = fixture.paths();
        let selection = select_current_source_change_gates(source(), &["lib/pkg/a.py".to_owned()])
            .expect("configured-root Python change selects graph arms");

        let expanded = compile_language_reach(
            &fixture.root,
            &tracked,
            &[],
            &["tests".to_owned()],
            selection,
        )
        .expect("configured Pyright root compiles");

        let reach = expanded
            .reach_for(GateId::PythonTypecheck)
            .expect("Python typecheck remains selected");
        assert_eq!(reach.mode(), &ChangeReachMode::Focused);
        assert_eq!(
            reached_paths(&expanded, GateId::PythonTypecheck),
            BTreeSet::from([
                "lib/pkg/__init__.py".to_owned(),
                "lib/pkg/a.py".to_owned(),
                "lib/pkg/b.py".to_owned(),
                "tests/test_pkg.py".to_owned(),
            ])
        );
    }

    #[test]
    fn javascript_package_authority_change_explicitly_escalates() {
        let fixture = Fixture::new("javascript-configuration");
        fixture.write(
            "package.json",
            "{\"name\":\"fixture\",\"dependencies\":{\"external\":\"1.0.0\"}}\n",
        );
        fixture.write("src/module.js", "import external from 'external';\n");
        let tracked = fixture.paths();
        let selection = select_current_source_change_gates(source(), &["package.json".to_owned()])
            .expect("JavaScript package authority selects its graph arm");

        let expanded = compile_language_reach(&fixture.root, &tracked, &[], &[], selection)
            .expect("JavaScript package authority reach compiles");
        let reach = expanded
            .reach_for(GateId::JavascriptUnused)
            .expect("JavaScript graph arm selected");
        assert!(matches!(
            reach.mode(),
            ChangeReachMode::Exhaustive {
                authority: ChangeReachAuthority::LanguageGraph,
                reason,
            } if reason.contains("non-language source")
        ));
        assert_eq!(reach.objects(), [ChangeReachObject::Repository]);
    }

    #[test]
    fn inherited_pyright_configuration_escalates_before_narrowing() {
        let fixture = Fixture::new("python-extends");
        fixture.write("pyproject.toml", "[tool.pyright]\nextends = 'base.toml'\n");
        fixture.write("src/pkg/module.py", "VALUE = 1\n");
        let tracked = fixture.paths();
        let selection =
            select_current_source_change_gates(source(), &["src/pkg/module.py".to_owned()])
                .expect("Python source selects graph arms");

        let expanded = compile_language_reach(&fixture.root, &tracked, &[], &[], selection)
            .expect("inherited configuration becomes explicit reach");
        let reach = expanded
            .reach_for(GateId::PythonTypecheck)
            .expect("Python typecheck remains selected");
        assert!(matches!(
            reach.mode(),
            ChangeReachMode::Exhaustive {
                authority: ChangeReachAuthority::LanguageGraph,
                reason,
            } if reason.contains("inherited tool.pyright extends")
        ));
    }

    fn reached_paths(report: &ChangeSelectionReport, gate: GateId) -> BTreeSet<String> {
        report
            .reach_for(gate)
            .expect("selected arm has reach")
            .objects()
            .iter()
            .filter_map(|object| match object {
                ChangeReachObject::Path { path } => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    fn source() -> Digest {
        Digest::compute(Domain::GateTree, b"language reach fixture")
    }

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let next = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "gate-language-reach-{}-{name}-{next}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("create language reach fixture");
            Self { root }
        }

        fn write(&self, path: &str, source: &str) {
            let absolute = self.root.join(path);
            fs::create_dir_all(absolute.parent().expect("fixture file has parent"))
                .expect("create fixture parent");
            fs::write(absolute, source).expect("write language reach fixture");
        }

        fn paths(&self) -> Vec<String> {
            fn collect(root: &Path, directory: &Path, paths: &mut Vec<String>) {
                for entry in fs::read_dir(directory).expect("read fixture directory") {
                    let entry = entry.expect("read fixture entry");
                    let path = entry.path();
                    if path.is_dir() {
                        collect(root, &path, paths);
                    } else {
                        paths.push(
                            path.strip_prefix(root)
                                .expect("fixture path is beneath root")
                                .to_string_lossy()
                                .replace('\\', "/"),
                        );
                    }
                }
            }
            let mut paths = Vec::new();
            collect(&self.root, &self.root, &mut paths);
            paths.sort();
            paths
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).expect("remove language reach fixture");
        }
    }
}
