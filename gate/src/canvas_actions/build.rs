use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

use sill::Environment;

use crate::CargoExternalSourceAuthority;
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
    environment: &Environment,
    cargo: &OsStr,
    external_source: Option<&CargoExternalSourceAuthority>,
) -> BuildAuthority {
    let selected = selected_paths.iter().cloned().collect::<BTreeSet<_>>();
    let mut authority = Authority::default();
    let launchers = launcher_inputs(root, &selected, &mut authority.reasons);
    authority.extend(launchers);
    let app = app_inputs(root, &selected, &mut authority.reasons);
    authority.extend(app);

    match cargo_package_input_closure_at(
        root,
        selected_paths,
        // The Canvas build owns fixed package roots. Changed-path selection
        // cannot add execution inputs beyond their resolved Cargo closure.
        &[],
        environment,
        cargo,
        external_source,
        &["xtask", "app"],
    ) {
        Ok(cargo) => {
            authority.extend(
                cargo
                    .input_paths
                    .into_iter()
                    .filter(|path| canvas_cargo_path(path)),
            );
            authority.extend(cargo.target_source_paths);
            authority.extend(cargo.file_macro_paths);
            authority.extend(cargo.compile_time_paths);
            authority.extend(cargo.workspace_configuration_paths);
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
    use crate::{
        ActionKey, ActionKeyAuthority, ActionNonFileInput, ActionNonFileKind,
        ActionPolicyAuthority, ActionReuseClass, ActionSpec, ActionSpecFields, Selector,
        compile_explicit_action_input, derive_authority_bound_action_key, test_repository,
    };
    use wire::{Digest, Domain};

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

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the Cargo closure fixture captures its own compiler environment"
    )]
    fn cargo_reached_browser_inputs_survive_without_admitting_browser_peers() {
        let fixture = cargo_browser_fixture("cargo-browser-reach");
        let selected = fixture.selected().into_iter().collect::<Vec<_>>();
        let environment = Environment::new(std::env::vars_os()).expect("test environment");
        let cargo = cargo_package_input_closure_at(
            &fixture.0,
            &selected,
            &[],
            &environment,
            OsStr::new("cargo"),
            None,
            &["xtask", "app"],
        )
        .expect("resolve fixture Cargo closure");

        assert!(
            cargo
                .target_source_paths
                .contains(&"app/browser/reached-module.rs".to_owned())
        );
        for path in [
            "app/browser/reached-include.rs",
            "app/browser/reached.txt",
            "app/browser/reached.bin",
        ] {
            assert!(cargo.file_macro_paths.contains(&path.to_owned()));
        }
        assert!(
            cargo
                .compile_time_paths
                .contains(&"app/browser/build-input.txt".to_owned())
        );
        assert!(
            cargo
                .workspace_configuration_paths
                .contains(&"Cargo.lock".to_owned())
        );
        assert!(
            cargo
                .input_paths
                .contains(&"app/browser/unreached.rs".to_owned())
        );

        let build = authority(
            &fixture.0,
            &selected,
            &environment,
            OsStr::new("cargo"),
            None,
        );
        assert!(
            build.non_cacheable_reason.is_none(),
            "unexpected refusal: {:?}",
            build.non_cacheable_reason
        );
        for path in [
            "app/browser/reached-module.rs",
            "app/browser/reached-include.rs",
            "app/browser/reached.txt",
            "app/browser/reached.bin",
            "app/browser/build-input.txt",
        ] {
            assert!(
                build.input_paths.contains(&path.to_owned()),
                "Canvas build closure omitted {path}"
            );
        }
        for path in ["app/browser/scenario.mjs", "app/browser/unreached.rs"] {
            assert!(
                !build.input_paths.contains(&path.to_owned()),
                "Canvas build closure admitted browser-only {path}"
            );
        }
    }

    #[test]
    fn cargo_reached_browser_input_moves_build_and_bound_shard_identity() {
        let fixture = cargo_browser_fixture("cargo-browser-identity");
        test_repository::initialize(&fixture.0);
        test_repository::stage_all(&fixture.0);
        test_repository::commit(&fixture.0, "seed Canvas browser authority");
        let original = canvas_identities(&fixture.0, Selector::revision("HEAD"))
            .expect("derive original Canvas identities");

        let linked = fixture.0.with_extension("linked");
        test_repository::create_linked_worktree(&fixture.0, &linked, "canvas-browser-peer");
        assert_eq!(
            canvas_identities(&linked, Selector::revision("HEAD"))
                .expect("derive equivalent-root Canvas identities"),
            original,
            "equivalent roots must preserve every Canvas identity",
        );
        test_repository::remove_linked_worktree(&fixture.0, &linked);

        fs::write(
            fixture.0.join("app/browser/scenario.mjs"),
            "export const SCENARIO = false;\n",
        )
        .expect("change browser-only source");
        test_repository::stage(&fixture.0, &["app/browser/scenario.mjs"]);
        let browser_only = canvas_identities(&fixture.0, Selector::staged())
            .expect("derive browser-only Canvas identities");
        assert_eq!(browser_only.0, original.0);
        assert_eq!(browser_only.1, original.1);
        assert_ne!(
            browser_only.2, original.2,
            "a browser-only source must move its shard without moving canvas:build",
        );

        fs::write(
            fixture.0.join("app/browser/scenario.mjs"),
            "export const SCENARIO = true;\n",
        )
        .expect("restore browser-only source");
        test_repository::stage(&fixture.0, &["app/browser/scenario.mjs"]);
        fs::write(
            fixture.0.join("spice-rust.toml"),
            "[taste.fixture]\nwords = [\"changed\"]\nmessage = \"Changed.\"\n",
        )
        .expect("change unrelated tracked policy");
        test_repository::stage(&fixture.0, &["spice-rust.toml"]);
        let policy_only = canvas_identities(&fixture.0, Selector::staged())
            .expect("derive unrelated-policy Canvas identities");
        assert_eq!(
            policy_only, original,
            "selection-only policy bytes outside the build closure move no Canvas identity",
        );

        fs::write(
            fixture.0.join("spice-rust.toml"),
            "[taste.fixture]\nwords = [\"fixture\"]\nmessage = \"Fixture.\"\n",
        )
        .expect("restore unrelated tracked policy");
        test_repository::stage(&fixture.0, &["spice-rust.toml"]);
        fs::write(fixture.0.join("app/browser/reached.txt"), "changed text\n")
            .expect("change Cargo-reached browser input");
        test_repository::stage(&fixture.0, &["app/browser/reached.txt"]);
        let changed = canvas_identities(&fixture.0, Selector::staged())
            .expect("derive changed Canvas identities");
        assert_ne!(changed.0, original.0, "content must move the manifest");
        assert_ne!(changed.1, original.1, "content must move canvas:build");
        assert_ne!(
            changed.2, original.2,
            "content must move every build-bound shard",
        );

        #[cfg(target_family = "unix")]
        {
            set_executable(&fixture.0.join("app/browser/reached.txt"), true);
            test_repository::stage(&fixture.0, &["app/browser/reached.txt"]);
            let changed_mode = canvas_identities(&fixture.0, Selector::staged())
                .expect("derive mode-changed Canvas identities");
            assert_ne!(changed_mode.0, changed.0, "mode must move the manifest");
            assert_ne!(changed_mode.1, changed.1, "mode must move canvas:build");
            assert_ne!(
                changed_mode.2, changed.2,
                "mode must move every build-bound shard",
            );
        }

        fs::remove_file(fixture.0.join("app/browser/reached.txt"))
            .expect("remove Cargo-reached browser input");
        test_repository::stage(&fixture.0, &["app/browser/reached.txt"]);
        let refusal = canvas_identities(&fixture.0, Selector::staged())
            .expect_err("a missing Cargo-reached input must refuse before identity");
        assert!(
            refusal.contains("is not readable selected-tree authority"),
            "unexpected refusal: {refusal}",
        );
    }

    fn cargo_browser_fixture(name: &str) -> Fixture {
        Fixture::new(
            name,
            &[
                (
                    "Cargo.toml",
                    "[workspace]\nmembers = [\"app\", \"xtask\"]\nresolver = \"3\"\n",
                ),
                (
                    "Cargo.lock",
                    concat!(
                        "# This file is automatically @generated by Cargo.\n",
                        "version = 4\n\n",
                        "[[package]]\nname = \"app\"\nversion = \"0.0.0\"\n\n",
                        "[[package]]\nname = \"xtask\"\nversion = \"0.0.0\"\n",
                    ),
                ),
                (
                    "spice-rust.toml",
                    "[taste.fixture]\nwords = [\"fixture\"]\nmessage = \"Fixture.\"\n",
                ),
                (
                    "app/Cargo.toml",
                    concat!(
                        "[package]\nname = \"app\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
                        "build = \"build.rs\"\n\n",
                        "[package.metadata.spice.cargo-proof]\nversion = 1\ncomplete = true\n",
                    ),
                ),
                (
                    "app/build.rs",
                    "fn main() { println!(\"cargo::rerun-if-changed=browser/build-input.txt\"); }\n",
                ),
                (
                    "app/src/lib.rs",
                    concat!(
                        "#[path = \"../browser/reached-module.rs\"]\nmod reached_module;\n",
                        "include!(\"../browser/reached-include.rs\");\n",
                        "const TEXT: &str = include_str!(\"../browser/reached.txt\");\n",
                        "const BYTES: &[u8] = include_bytes!(\"../browser/reached.bin\");\n",
                    ),
                ),
                (
                    "app/browser/canvas-proof-build.mjs",
                    "import './canvas-build.mjs';\n",
                ),
                (
                    "app/browser/canvas-build.mjs",
                    "export const BUILD = true;\n",
                ),
                (
                    "app/browser/reached-module.rs",
                    "pub const MODULE: u8 = 1;\n",
                ),
                (
                    "app/browser/reached-include.rs",
                    "const INCLUDED: u8 = 1;\n",
                ),
                ("app/browser/reached.txt", "text\n"),
                ("app/browser/reached.bin", "bytes\n"),
                ("app/browser/build-input.txt", "build input\n"),
                (
                    "app/browser/scenario.mjs",
                    "export const SCENARIO = true;\n",
                ),
                ("app/browser/unreached.rs", "pub const UNREACHED: u8 = 1;\n"),
                (APP_INDEX, "<link data-trunk rel=\"rust\">\n"),
                (
                    "xtask/Cargo.toml",
                    "[package]\nname = \"xtask\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
                ),
                ("xtask/src/main.rs", "fn main() {}\n"),
            ],
        )
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "the Canvas identity fixture captures its own compiler environment"
    )]
    fn canvas_identities(
        repository: &Path,
        selector: Selector,
    ) -> Result<(Digest, ActionKey, ActionKey), String> {
        let tree = selector
            .materialize(repository)
            .map_err(|error| error.to_string())?;
        let environment =
            Environment::new(std::env::vars_os()).map_err(|error| error.to_string())?;
        let build = authority(
            tree.root(),
            tree.paths(),
            &environment,
            OsStr::new("cargo"),
            None,
        );
        if let Some(reason) = build.non_cacheable_reason {
            return Err(reason);
        }
        let build_manifest = compile_explicit_action_input(&tree, build.input_paths, Vec::new())
            .map_err(|error| error.to_string())?
            .digest()
            .map_err(|error| error.to_string())?;
        let build_key = canvas_action_key("canvas:build", build_manifest.clone());
        let build_binding = ActionNonFileInput::new(
            ActionNonFileKind::Declaration,
            "canvas-build-proof",
            Digest::compute(Domain::GateActionKey, build_key.as_str().as_bytes()),
        )
        .map_err(|error| error.to_string())?;
        let shard_manifest = compile_explicit_action_input(
            &tree,
            vec!["app/browser/scenario.mjs".to_owned()],
            vec![build_binding],
        )
        .map_err(|error| error.to_string())?
        .digest()
        .map_err(|error| error.to_string())?;
        let shard_key = canvas_action_key("canvas:shard:fixture", shard_manifest);
        Ok((build_manifest, build_key, shard_key))
    }

    fn canvas_action_key(action: &str, input_manifest: Digest) -> ActionKey {
        let spec = ActionSpec::new(ActionSpecFields {
            action_id: action.to_owned(),
            runner: "canvas-proof-fixture".to_owned(),
            runner_version: 1,
            declaration: "canvas-proof-fixture-v1".to_owned(),
            argv: vec!["node".to_owned(), "fixture.mjs".to_owned()],
            cwd: ".".to_owned(),
            environment_names: Vec::new(),
            tools: Vec::new(),
            input_strategy: "canvas-fixture-v1".to_owned(),
            reuse: ActionReuseClass::Cacheable,
        })
        .expect("construct Canvas fixture action specification");
        derive_authority_bound_action_key(
            &spec,
            &ActionKeyAuthority {
                input_manifest,
                policy_authority: ActionPolicyAuthority::ResolvedExecution,
                gate_set: canvas_authority_digest(b"gate-set"),
                tools: canvas_authority_digest(b"tools"),
                environment: canvas_authority_digest(b"environment"),
                baseline: None,
            },
        )
        .expect("derive Canvas fixture action key")
    }

    fn canvas_authority_digest(bytes: &[u8]) -> Digest {
        Digest::compute(Domain::GateActionKey, bytes)
    }

    #[cfg(target_family = "unix")]
    fn set_executable(path: &Path, executable: bool) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path).expect("read fixture mode").permissions();
        let mode = if executable { 0o755 } else { 0o644 };
        permissions.set_mode(mode);
        fs::set_permissions(path, permissions).expect("set fixture mode");
    }
}
