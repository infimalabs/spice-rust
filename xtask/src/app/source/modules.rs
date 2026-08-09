//! Repository guard for the browser crate's external Rust module graph.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// Every Git-tracked Rust source under `app/src` must be reachable from the
/// crate root. Attributes do not hide declarations from this walk: a module
/// compiled only for Wasm or tests is still one authored branch of the app.
fn loose_tracked_sources(root: &Path) -> Result<Vec<String>, String> {
    let tracked = tracked_app_sources(root)?;
    let reached = external_module_graph(root)?;
    Ok(tracked.difference(&reached).cloned().collect())
}

fn admit_tracked_sources(root: &Path) -> Result<(), String> {
    let loose = loose_tracked_sources(root)?;
    if loose.is_empty() {
        return Ok(());
    }
    Err(format!(
        "Git-tracked app Rust sources outside the external module graph: {}",
        loose.join(", ")
    ))
}

fn tracked_app_sources(root: &Path) -> Result<BTreeSet<String>, String> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(["ls-files", "-z", "--", "app/src"])
        .output()
        .map_err(|error| format!("git ls-files could not start: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git ls-files failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let bytes = String::from_utf8(output.stdout)
        .map_err(|error| format!("git ls-files returned non-UTF-8 paths: {error}"))?;
    Ok(bytes
        .split('\0')
        .filter(|path| path.ends_with(".rs"))
        .map(str::to_owned)
        .collect())
}

fn external_module_graph(root: &Path) -> Result<BTreeSet<String>, String> {
    let source_root = root.join("app/src");
    let mut pending = vec![source_root.join("lib.rs")];
    let mut reached = BTreeSet::new();
    while let Some(source) = pending.pop() {
        let relative = repository_spelling(root, &source)?;
        if !reached.insert(relative) {
            continue;
        }
        let text = fs::read_to_string(&source)
            .map_err(|error| format!("{} could not be read: {error}", source.display()))?;
        for declaration in declarations(&text)? {
            pending.push(resolve_module(&source, &declaration)?);
        }
    }
    Ok(reached)
}

#[derive(Debug, PartialEq, Eq)]
struct Declaration {
    name: String,
    path: Option<String>,
}

fn declarations(text: &str) -> Result<Vec<Declaration>, String> {
    let mut found = Vec::new();
    let mut explicit_path = None;
    for (index, source_line) in text.lines().enumerate() {
        let line = source_line
            .split_once("//")
            .map_or(source_line, |row| row.0)
            .trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("#[") {
            if line.starts_with("#[path") {
                explicit_path =
                    Some(path_attribute(line).ok_or_else(|| {
                        format!("line {} has a malformed path attribute", index + 1)
                    })?);
            }
            continue;
        }
        if let Some(name) = external_module_name(line) {
            found.push(Declaration {
                name,
                path: explicit_path.take(),
            });
        } else {
            explicit_path = None;
        }
    }
    Ok(found)
}

fn path_attribute(line: &str) -> Option<String> {
    let value = line.split_once('=')?.1.strip_suffix(']')?.trim();
    value
        .strip_prefix('"')?
        .strip_suffix('"')
        .map(str::to_owned)
}

fn external_module_name(line: &str) -> Option<String> {
    let statement = if let Some(rest) = line.strip_prefix("pub ") {
        rest
    } else if let Some(rest) = line.strip_prefix("pub(") {
        rest.split_once(')')?.1.trim_start()
    } else {
        line
    };
    let name = statement.strip_prefix("mod ")?.strip_suffix(';')?.trim();
    (!name.is_empty()
        && name
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric()))
    .then(|| name.to_owned())
}

fn resolve_module(source: &Path, declaration: &Declaration) -> Result<PathBuf, String> {
    let directory = module_directory(source)?;
    if let Some(path) = &declaration.path {
        return Ok(directory.join(path));
    }
    let flat = directory.join(format!("{}.rs", declaration.name));
    let nested = directory.join(&declaration.name).join("mod.rs");
    match (flat.is_file(), nested.is_file()) {
        (true, false) => Ok(flat),
        (false, true) => Ok(nested),
        (true, true) => Err(format!(
            "module {} has both {} and {}",
            declaration.name,
            flat.display(),
            nested.display()
        )),
        (false, false) => Err(format!(
            "module {} declared by {} has no external source",
            declaration.name,
            source.display()
        )),
    }
}

fn module_directory(source: &Path) -> Result<PathBuf, String> {
    let parent = source
        .parent()
        .ok_or_else(|| format!("{} has no parent", source.display()))?;
    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| format!("{} has no UTF-8 stem", source.display()))?;
    if matches!(stem, "lib" | "main" | "mod") {
        Ok(parent.to_path_buf())
    } else {
        Ok(parent.join(stem))
    }
}

fn repository_spelling(root: &Path, path: &Path) -> Result<String, String> {
    path.strip_prefix(root)
        .map_err(|_| format!("{} escaped {}", path.display(), root.display()))?
        .to_str()
        .map(|path| path.replace('\\', "/"))
        .ok_or_else(|| format!("{} is not a UTF-8 path", path.display()))
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask belongs to the workspace")
        .to_path_buf()
}

/// How many names one fixture tries before giving up on an exclusive root.
const SCRATCH_ATTEMPTS: usize = 100;

/// A scratch repository holding one tracked module graph.
struct Fixture(PathBuf);

impl Fixture {
    /// One scratch repository under a root nothing else already holds.
    ///
    /// `create_dir` refuses a directory that exists rather than opening it, so
    /// retrying under a fresh name is what makes the root exclusive. The name
    /// carries the clock rather than the process id because ids are both reused
    /// over time and held at once by the peer lanes sharing one temporary
    /// directory. This fixture used to name itself from the id alone and then
    /// delete whatever stood there, which destroys a live directory rather than
    /// reusing a dead one; nothing here removes a directory it did not create.
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..SCRATCH_ATTEMPTS {
            let stamp = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("the clock reads after the epoch")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "spice-app-module-graph-{stamp}-{}",
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&root) {
                Ok(()) => return Self::populated(root),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("exclusive scratch root is created: {error}"),
            }
        }
        panic!("no exclusive scratch root was available in {SCRATCH_ATTEMPTS} attempts");
    }

    /// Write the module graph under test into the freshly created `root`.
    ///
    /// The fixture owns the root before the first file is written, so a refusal
    /// here unwinds through [`Fixture::drop`] and takes the directory with it.
    /// Populating before taking ownership would leave behind the one root no
    /// test ever used.
    fn populated(root: PathBuf) -> Self {
        let fixture = Self(root);
        let root = &fixture.0;
        fs::create_dir_all(root.join("app/src/browser")).expect("create module fixture");
        fs::write(
            root.join("app/src/lib.rs"),
            "#[cfg(target_arch = \"wasm32\")]\nmod browser;\n",
        )
        .expect("write fixture crate root");
        fs::write(
            root.join("app/src/browser.rs"),
            "#[cfg(any(target_arch = \"wasm32\", test))]\nmod controls;\n",
        )
        .expect("write fixture browser module");
        fs::write(
            root.join("app/src/browser/controls.rs"),
            "pub fn live() {}\n",
        )
        .expect("write reachable fixture module");
        fs::write(root.join("app/src/loose.rs"), "pub fn loose() {}\n")
            .expect("write loose fixture module");
        git(root, &["init", "--quiet"]);
        git(root, &["add", "app/src"]);
        fixture
    }

    fn root(&self) -> &Path {
        &self.0
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // A root that outlives its test is a name another run could adopt, so
        // remove it even where a panic is unwinding through here. Removal is
        // allowed to fail rather than replace the failure the test carries.
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git(root: &Path, arguments: &[&str]) {
    let status = Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(arguments)
        .status()
        .expect("fixture git command starts");
    assert!(status.success(), "fixture git command succeeds");
}

#[test]
fn every_tracked_app_rust_source_is_reachable_from_the_crate_root() {
    admit_tracked_sources(&repository_root()).expect("walk current app module graph");
}

#[test]
fn a_tracked_loose_module_fails_with_its_exact_path() {
    let fixture = Fixture::new();
    assert_eq!(
        admit_tracked_sources(fixture.root()).expect_err("tracked loose module must fail"),
        "Git-tracked app Rust sources outside the external module graph: app/src/loose.rs"
    );
}

/// Two fixtures held at once take separate roots, and neither survives its
/// owner or is emptied by the other.
///
/// The scratch name was the process id alone, and a run finding that name taken
/// deleted the directory rather than stepping around it. Both halves are tested
/// from one process because that is where the old name was guaranteed to
/// collide: the second fixture computed the first fixture's live path and
/// removed its tree. Reading the paths back after the scope closes is what
/// proves neither root survives to be adopted by the next run.
#[test]
fn two_module_fixtures_take_separate_roots_and_leave_neither_behind() {
    let identifier = std::process::id().to_string();
    let (first, second) = {
        let earlier = Fixture::new();
        let later = Fixture::new();
        assert_ne!(earlier.0, later.0, "one fixture name was issued twice");
        assert!(
            earlier.0.join("app/src/lib.rs").is_file(),
            "the later fixture removed a tree it did not create"
        );

        for root in [&earlier.0, &later.0] {
            assert!(
                root.is_dir(),
                "{} is not a live scratch root",
                root.display()
            );
            let name = root
                .file_name()
                .expect("a scratch root is named")
                .to_string_lossy()
                .into_owned();
            assert!(
                !name.split('-').any(|field| field == identifier),
                "{name} still names the process id the operating system reuses"
            );
        }
        (earlier.0.clone(), later.0.clone())
    };

    assert!(
        !first.exists(),
        "{} outlived the fixture that owns it",
        first.display()
    );
    assert!(
        !second.exists(),
        "{} outlived the fixture that owns it",
        second.display()
    );
}

#[test]
fn declarations_include_cfg_gated_external_modules_and_ignore_inline_ones() {
    assert_eq!(
        declarations("#[cfg(target_arch = \"wasm32\")]\npub(crate) mod browser;\nmod tests {\n}\n")
            .expect("parse fixture declarations"),
        [Declaration {
            name: "browser".to_owned(),
            path: None,
        }]
    );
}
