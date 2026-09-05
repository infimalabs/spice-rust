use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

use sill::Environment;

use crate::structural::scan_javascript_module_dependencies;
use crate::workspace_tests::cargo_package_input_closure_at;

const BUILD_LAUNCHER: &str = "app/browser/canvas-proof-build.mjs";
const APP_INDEX: &str = "app/index.html";

pub(super) struct BuildAuthority {
    pub(super) input_paths: Vec<String>,
    pub(super) non_cacheable_reason: Option<String>,
}

pub(super) fn authority(
    root: &Path,
    selected_paths: &[String],
    changed_paths: &[String],
    environment: &Environment,
    cargo: &OsStr,
) -> BuildAuthority {
    let selected = selected_paths.iter().cloned().collect::<BTreeSet<_>>();
    let mut authority = Authority::default();
    authority.extend(
        selected_paths
            .iter()
            .chain(changed_paths)
            .filter(|path| explicit_build_path(path))
            .cloned(),
    );
    let launchers = launcher_inputs(root, &selected, &mut authority.reasons);
    authority.extend(launchers);
    let app = app_inputs(root, &selected, &mut authority.reasons);
    authority.extend(app);

    let changed = changed_paths
        .iter()
        .filter(|path| !path.starts_with("app/browser/"))
        .cloned()
        .collect::<Vec<_>>();
    match cargo_package_input_closure_at(
        root,
        selected_paths,
        &changed,
        environment,
        cargo,
        &["xtask", "app"],
    ) {
        Ok(cargo) => {
            authority.extend(
                cargo
                    .input_paths
                    .into_iter()
                    .filter(|path| canvas_cargo_path(path)),
            );
            if let Some(reason) = cargo.non_cacheable_reason {
                authority.reasons.insert(reason);
            }
        }
        Err(error) => {
            authority.reasons.insert(format!(
                "canonical Cargo source authority is incomplete: {error}"
            ));
        }
    }
    authority.finish()
}

#[derive(Default)]
struct Authority {
    paths: BTreeSet<String>,
    reasons: BTreeSet<String>,
}

impl Authority {
    fn extend(&mut self, paths: impl IntoIterator<Item = String>) {
        self.paths.extend(paths);
    }

    fn finish(self) -> BuildAuthority {
        BuildAuthority {
            input_paths: self.paths.into_iter().collect(),
            non_cacheable_reason: (!self.reasons.is_empty())
                .then(|| self.reasons.into_iter().collect::<Vec<_>>().join("; ")),
        }
    }
}

fn explicit_build_path(path: &str) -> bool {
    !path.starts_with("app/browser/")
}

fn canvas_cargo_path(path: &str) -> bool {
    !path.starts_with("app/") || path == "app/Cargo.toml" || path.starts_with("app/src/")
}

fn launcher_inputs(
    root: &Path,
    selected: &BTreeSet<String>,
    reasons: &mut BTreeSet<String>,
) -> Vec<String> {
    let mut reached = BTreeSet::new();
    let mut pending = VecDeque::from([BUILD_LAUNCHER.to_owned()]);
    while let Some(path) = pending.pop_front() {
        if !reached.insert(path.clone()) {
            continue;
        }
        if !selected.contains(&path) {
            reasons.insert(format!(
                "Canvas build launcher {path:?} is absent from selected-tree authority"
            ));
            continue;
        }
        let source = match fs::read_to_string(root.join(&path)) {
            Ok(source) => source,
            Err(error) => {
                reasons.insert(format!(
                    "Canvas build launcher {path:?} is unreadable: {error}"
                ));
                continue;
            }
        };
        let dependencies = match scan_javascript_module_dependencies(&source) {
            Ok(dependencies) => dependencies,
            Err(error) => {
                reasons.insert(format!(
                    "Canvas build launcher {path:?} has dynamic or malformed module authority: {error}"
                ));
                continue;
            }
        };
        for specifier in dependencies.specifiers {
            if specifier.starts_with("node:") {
                continue;
            }
            if !specifier.starts_with('.') {
                reasons.insert(format!(
                    "Canvas build launcher {path:?} imports external module {specifier:?}"
                ));
                continue;
            }
            match resolve_local_module(&path, &specifier, selected) {
                Ok(dependency) => pending.push_back(dependency),
                Err(error) => {
                    reasons.insert(error);
                }
            }
        }
    }
    reached.into_iter().collect()
}

fn resolve_local_module(
    source: &str,
    specifier: &str,
    selected: &BTreeSet<String>,
) -> Result<String, String> {
    let parent = Path::new(source).parent().unwrap_or_else(|| Path::new(""));
    let normalized = normalize_relative(parent.join(specifier)).ok_or_else(|| {
        format!("Canvas build module {specifier:?} from {source:?} escapes the selected tree")
    })?;
    let base = path_text(&normalized)?;
    let mut candidates = BTreeSet::new();
    for candidate in [
        base.clone(),
        format!("{base}.js"),
        format!("{base}.mjs"),
        format!("{base}.cjs"),
        format!("{base}/index.js"),
        format!("{base}/index.mjs"),
        format!("{base}/index.cjs"),
    ] {
        if selected.contains(&candidate) {
            candidates.insert(candidate);
        }
    }
    if candidates.len() == 1 {
        return Ok(candidates.into_iter().next().expect("one module exists"));
    }
    Err(format!(
        "Canvas build module {specifier:?} from {source:?} resolved to {} selected files",
        candidates.len()
    ))
}

fn app_inputs(
    root: &Path,
    selected: &BTreeSet<String>,
    reasons: &mut BTreeSet<String>,
) -> Vec<String> {
    let mut paths = BTreeSet::from([APP_INDEX.to_owned()]);
    if !selected.contains(APP_INDEX) {
        reasons.insert(format!(
            "Canvas app entry {APP_INDEX:?} is absent from selected-tree authority"
        ));
        return paths.into_iter().collect();
    }
    let document = match fs::read_to_string(root.join(APP_INDEX)) {
        Ok(document) => document,
        Err(error) => {
            reasons.insert(format!(
                "Canvas app entry {APP_INDEX:?} is unreadable: {error}"
            ));
            return paths.into_iter().collect();
        }
    };
    let mut rust_entry = false;
    for line in document.lines().filter(|line| line.contains("data-trunk")) {
        let rel = match attribute(line, "rel") {
            Ok(Some(value)) => value,
            Ok(None) => {
                reasons.insert("Canvas data-trunk entry has no literal rel attribute".to_owned());
                continue;
            }
            Err(error) => {
                reasons.insert(error);
                continue;
            }
        };
        match rel.as_str() {
            "rust" => {
                rust_entry = true;
                if matches!(attribute(line, "href"), Ok(Some(_)) | Err(_)) {
                    reasons.insert(
                        "Canvas Rust data-trunk entry has unsupported source attributes".to_owned(),
                    );
                }
            }
            "css" => match attribute(line, "href") {
                Ok(Some(href)) => match app_relative_path(&href) {
                    Ok(path) => {
                        paths.insert(path.clone());
                        admit_css(root, selected, &path, reasons);
                    }
                    Err(error) => {
                        reasons.insert(error);
                    }
                },
                Ok(None) => {
                    reasons.insert("Canvas CSS data-trunk entry has no literal href".to_owned());
                }
                Err(error) => {
                    reasons.insert(error);
                }
            },
            other => {
                reasons.insert(format!(
                    "Canvas data-trunk entry uses unsupported relation {other:?}"
                ));
            }
        }
    }
    if !rust_entry {
        reasons.insert("Canvas app entry has no Rust data-trunk derivation".to_owned());
    }
    paths.into_iter().collect()
}

fn attribute(line: &str, name: &str) -> Result<Option<String>, String> {
    let marker = format!("{name}=\"");
    let mut values = line.match_indices(&marker);
    let Some((first, _)) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(format!(
            "Canvas data-trunk entry repeats attribute {name:?}"
        ));
    }
    let tail = &line[first + marker.len()..];
    let Some(end) = tail.find('"') else {
        return Err(format!(
            "Canvas data-trunk entry has unterminated attribute {name:?}"
        ));
    };
    let value = &tail[..end];
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(format!(
            "Canvas data-trunk entry has invalid attribute {name:?}"
        ));
    }
    Ok(Some(value.to_owned()))
}

fn app_relative_path(href: &str) -> Result<String, String> {
    let path = Path::new(href);
    if path.extension() != Some(OsStr::new("css")) {
        return Err(format!(
            "Canvas app stylesheet {href:?} is not a CSS selected-tree path"
        ));
    }
    let normalized = normalize_relative(Path::new("app").join(path))
        .ok_or_else(|| format!("Canvas app stylesheet {href:?} escapes the selected tree"))?;
    let path = path_text(&normalized)?;
    if !path.starts_with("app/") {
        return Err(format!(
            "Canvas app stylesheet {href:?} escapes the app source root"
        ));
    }
    Ok(path)
}

fn admit_css(root: &Path, selected: &BTreeSet<String>, path: &str, reasons: &mut BTreeSet<String>) {
    if !selected.contains(path) {
        reasons.insert(format!(
            "Canvas app stylesheet {path:?} is absent from selected-tree authority"
        ));
        return;
    }
    match fs::read_to_string(root.join(path)) {
        Ok(source) if source.contains("@import") || source.contains("url(") => {
            reasons.insert(format!(
                "Canvas app stylesheet {path:?} has unsupported external or generated inputs"
            ));
        }
        Ok(_) => {}
        Err(error) => {
            reasons.insert(format!(
                "Canvas app stylesheet {path:?} is unreadable: {error}"
            ));
        }
    }
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

fn path_text(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(|path| path.replace(std::path::MAIN_SEPARATOR, "/"))
        .ok_or_else(|| format!("Canvas build input {} is not UTF-8", path.display()))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new(name: &str, files: &[(&str, &str)]) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "gate-canvas-build-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir(&root).expect("create Canvas build fixture");
            for (relative, contents) in files {
                let path = root.join(relative);
                fs::create_dir_all(path.parent().expect("fixture file has a parent"))
                    .expect("create Canvas build fixture parent");
                fs::write(path, contents).expect("write Canvas build fixture file");
            }
            Self(root)
        }

        fn selected(&self) -> BTreeSet<String> {
            let mut paths = BTreeSet::new();
            collect(&self.0, &self.0, &mut paths);
            paths
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("remove Canvas build fixture");
        }
    }

    fn collect(root: &Path, directory: &Path, paths: &mut BTreeSet<String>) {
        for entry in fs::read_dir(directory).expect("read fixture directory") {
            let entry = entry.expect("read fixture entry");
            if entry.file_type().expect("read fixture file type").is_dir() {
                collect(root, &entry.path(), paths);
            } else {
                let entry_path = entry.path();
                let relative = entry_path
                    .strip_prefix(root)
                    .expect("fixture entry stays under its root");
                paths.insert(path_text(relative).expect("fixture path is UTF-8"));
            }
        }
    }

    #[test]
    fn launcher_graph_reaches_only_static_local_modules() {
        let fixture = Fixture::new(
            "launchers",
            &[
                (
                    BUILD_LAUNCHER,
                    "import assert from 'node:assert';\nimport './canvas-build.mjs';\n",
                ),
                (
                    "app/browser/canvas-build.mjs",
                    "import './nested.js';\nexport const BUILD = true;\n",
                ),
                ("app/browser/nested.js", "export const NESTED = true;\n"),
                ("app/browser/peer.mjs", "export const PEER = true;\n"),
            ],
        );
        let selected = fixture.selected();
        let mut reasons = BTreeSet::new();

        let paths = launcher_inputs(&fixture.0, &selected, &mut reasons);

        assert!(reasons.is_empty(), "unexpected reasons: {reasons:?}");
        assert_eq!(
            paths,
            [
                "app/browser/canvas-build.mjs",
                "app/browser/canvas-proof-build.mjs",
                "app/browser/nested.js",
            ]
        );
        assert!(!explicit_build_path("app/browser/peer.mjs"));
    }

    #[test]
    fn dynamic_external_escaping_and_missing_launcher_inputs_are_non_cacheable() {
        for (name, source, expected) in [
            (
                "dynamic",
                "const path = './next.mjs'; import(path);\n",
                "dynamic or malformed module authority",
            ),
            (
                "external",
                "import value from 'external-package';\n",
                "imports external module",
            ),
            (
                "escaping",
                "import '../../../outside.mjs';\n",
                "escapes the selected tree",
            ),
            (
                "missing",
                "import './missing.mjs';\n",
                "resolved to 0 selected files",
            ),
        ] {
            let fixture = Fixture::new(name, &[(BUILD_LAUNCHER, source)]);
            let mut reasons = BTreeSet::new();
            launcher_inputs(&fixture.0, &fixture.selected(), &mut reasons);
            assert!(
                reasons.iter().any(|reason| reason.contains(expected)),
                "{name} did not produce {expected:?}: {reasons:?}"
            );
        }
    }

    #[test]
    fn unreadable_launcher_input_is_non_cacheable() {
        let fixture = Fixture::new(
            "unreadable-launcher",
            &[(BUILD_LAUNCHER, "export const BUILD = true;\n")],
        );
        fs::write(fixture.0.join(BUILD_LAUNCHER), [0xff]).expect("write non-UTF-8 launcher input");
        let mut reasons = BTreeSet::new();

        launcher_inputs(&fixture.0, &fixture.selected(), &mut reasons);

        assert!(
            reasons.iter().any(|reason| reason.contains("unreadable")),
            "non-UTF-8 launcher did not produce an unreadable reason: {reasons:?}"
        );
    }

    #[test]
    fn app_derivation_captures_styles_and_refuses_unbounded_css_inputs() {
        let fixture = Fixture::new(
            "app",
            &[
                (
                    APP_INDEX,
                    concat!(
                        "<link data-trunk rel=\"css\" href=\"style.css\">\n",
                        "<link data-trunk rel=\"rust\">\n",
                    ),
                ),
                ("app/style.css", "body { color: black; }\n"),
            ],
        );
        let mut reasons = BTreeSet::new();
        assert_eq!(
            app_inputs(&fixture.0, &fixture.selected(), &mut reasons),
            ["app/index.html", "app/style.css"]
        );
        assert!(reasons.is_empty(), "unexpected reasons: {reasons:?}");

        fs::write(
            fixture.0.join(APP_INDEX),
            concat!(
                "<link data-trunk rel=\"css\" href=\"style.css\">\n",
                "<link data-trunk rel=\"css\" href=\"added.css\">\n",
                "<link data-trunk rel=\"rust\">\n",
            ),
        )
        .expect("add declared stylesheet derivation");
        fs::write(fixture.0.join("app/added.css"), "body { color: blue; }\n")
            .expect("write declared stylesheet");
        let changed = app_inputs(&fixture.0, &fixture.selected(), &mut reasons);
        assert_eq!(
            changed,
            ["app/added.css", "app/index.html", "app/style.css"]
        );

        fs::write(
            fixture.0.join("app/style.css"),
            "body { background: url(image.png); }\n",
        )
        .expect("write generated CSS input");
        app_inputs(&fixture.0, &fixture.selected(), &mut reasons);
        assert!(
            reasons
                .iter()
                .any(|reason| reason.contains("external or generated inputs"))
        );
    }
}
