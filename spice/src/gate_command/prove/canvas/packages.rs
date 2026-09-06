//! Immutable Playwright package closure capture and materialization.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::*;

const PLAYWRIGHT_PACKAGE: &str = "playwright";

#[derive(Deserialize)]
struct PackageLock {
    #[serde(rename = "lockfileVersion")]
    lockfile_version: u64,
    packages: BTreeMap<String, LockedPackage>,
}

#[derive(Deserialize)]
struct LockedPackage {
    version: Option<String>,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "optionalDependencies")]
    optional_dependencies: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct InstalledPackage {
    name: String,
    version: String,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "optionalDependencies")]
    optional_dependencies: BTreeMap<String, String>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct PackageSnapshotAuthority<'a> {
    packages: &'a [SnapshotPackage],
    files: Vec<SnapshotFileAuthority<'a>>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct SnapshotFileAuthority<'a> {
    path: &'a str,
    mode: u32,
    content: Digest,
}

#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
struct SnapshotPackage {
    path: String,
    name: String,
    version: String,
}

struct SnapshotFile {
    path: String,
    mode: u32,
    bytes: Vec<u8>,
}

struct PendingPackage {
    path: PathBuf,
    name: String,
}

pub(super) struct PlaywrightSnapshot {
    files: Vec<SnapshotFile>,
    pub(super) authority: Digest,
}

impl PlaywrightSnapshot {
    #[cfg(test)]
    pub(super) fn empty() -> Self {
        Self {
            files: Vec::new(),
            authority: Digest::compute(Domain::GateTool, b"empty Playwright test snapshot"),
        }
    }

    pub(super) fn capture(
        selected_root: &Path,
        installed_root: &Path,
    ) -> Result<Self, ProveFailure> {
        let first = Self::capture_once(selected_root, installed_root)?;
        let second = Self::capture_once(selected_root, installed_root)?;
        if first.authority != second.authority {
            return Err(package_refusal(
                "installed Playwright package sources moved while they were being captured",
            ));
        }
        Ok(second)
    }

    pub(super) fn capture_for_lookup(
        selected_root: &Path,
        installed_root: &Path,
    ) -> Result<Self, ProveFailure> {
        Self::capture_once(selected_root, installed_root)
    }

    fn capture_once(selected_root: &Path, installed_root: &Path) -> Result<Self, ProveFailure> {
        let lock = load_lock(selected_root)?;
        let modules = installed_root.join("node_modules");
        require_directory(&modules, "installed package root")?;
        let packages = resolve_packages(installed_root, &lock)?;
        let files = collect_closure_files(installed_root, &packages)?;
        let authority = PackageSnapshotAuthority {
            packages: &packages,
            files: files
                .iter()
                .map(|file| SnapshotFileAuthority {
                    path: &file.path,
                    mode: file.mode,
                    content: Digest::compute(Domain::GateTool, &file.bytes),
                })
                .collect(),
        };
        Ok(Self {
            authority: digest_encoded(Domain::GateTool, &authority)?,
            files,
        })
    }

    pub(super) fn materialize(&self, root: &Path) -> Result<(), ProveFailure> {
        let modules = root.join("node_modules");
        match fs::symlink_metadata(&modules) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(package_refusal(format!(
                    "snapshot destination already exists: {}",
                    modules.display(),
                )));
            }
            Err(error) => {
                return Err(package_refusal(format!(
                    "cannot inspect snapshot destination {}: {error}",
                    modules.display(),
                )));
            }
        }
        for file in &self.files {
            let destination = root.join(&file.path);
            let parent = destination.parent().ok_or_else(|| {
                package_refusal(format!("snapshot file has no parent: {}", file.path))
            })?;
            fs::create_dir_all(parent).map_err(|error| {
                package_refusal(format!("cannot create {}: {error}", parent.display()))
            })?;
            fs::write(&destination, &file.bytes).map_err(|error| {
                package_refusal(format!("cannot write {}: {error}", destination.display()))
            })?;
            set_snapshot_mode(&destination, file.mode)?;
        }
        Ok(())
    }
}

fn load_lock(selected_root: &Path) -> Result<PackageLock, ProveFailure> {
    let lock_path = selected_root.join("package-lock.json");
    require_regular_file(&lock_path, "selected package lock")?;
    let lock_bytes = fs::read(&lock_path).map_err(|error| {
        package_refusal(format!("cannot read {}: {error}", lock_path.display()))
    })?;
    let lock: PackageLock = serde_json::from_slice(&lock_bytes).map_err(|error| {
        package_refusal(format!("cannot decode {}: {error}", lock_path.display()))
    })?;
    if lock.lockfile_version != 3 {
        return Err(package_refusal(format!(
            "{} has unsupported lockfileVersion {}; expected 3",
            lock_path.display(),
            lock.lockfile_version,
        )));
    }
    Ok(lock)
}

fn resolve_packages(
    installed_root: &Path,
    lock: &PackageLock,
) -> Result<Vec<SnapshotPackage>, ProveFailure> {
    let mut pending = VecDeque::from([PendingPackage {
        path: PathBuf::from("node_modules").join(PLAYWRIGHT_PACKAGE),
        name: PLAYWRIGHT_PACKAGE.to_owned(),
    }]);
    let mut visited = BTreeMap::new();
    let mut packages = Vec::new();
    while let Some(package) = pending.pop_front() {
        validate_package_name(&package.name)?;
        let spelling = relative_path_spelling(&package.path)?;
        if let Some(prior_name) = visited.insert(spelling.clone(), package.name.clone()) {
            if prior_name != package.name {
                return Err(package_refusal(format!(
                    "package path {spelling} resolves both {prior_name:?} and {:?}",
                    package.name,
                )));
            }
            continue;
        }
        packages.push(capture_package(
            installed_root,
            lock,
            package,
            spelling,
            &mut pending,
        )?);
    }
    packages.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(packages)
}

fn capture_package(
    installed_root: &Path,
    lock: &PackageLock,
    package: PendingPackage,
    spelling: String,
    pending: &mut VecDeque<PendingPackage>,
) -> Result<SnapshotPackage, ProveFailure> {
    let locked = lock.packages.get(&spelling).ok_or_else(|| {
        package_refusal(format!("package lock is missing closure entry {spelling}"))
    })?;
    let locked_version = locked
        .version
        .as_deref()
        .ok_or_else(|| package_refusal(format!("package lock entry {spelling} has no version")))?;
    let identity = read_identity(installed_root, &package.path)?;
    if identity.name != package.name || identity.version != locked_version {
        return Err(package_refusal(format!(
            "installed package {spelling} identifies as {}@{}, but the lock requires {}@{locked_version}",
            identity.name, identity.version, package.name,
        )));
    }
    if identity.dependencies != locked.dependencies
        || identity.optional_dependencies != locked.optional_dependencies
    {
        return Err(package_refusal(format!(
            "installed package {spelling} dependency declarations disagree with the package lock",
        )));
    }

    let required = locked.dependencies.keys().map(|name| (name, false));
    let optional = locked.optional_dependencies.keys().map(|name| (name, true));
    for (name, optional) in required.chain(optional) {
        if let Some(path) = resolve_dependency(installed_root, &package.path, name, optional)? {
            pending.push_back(PendingPackage {
                path,
                name: name.clone(),
            });
        }
    }
    Ok(SnapshotPackage {
        path: spelling,
        name: identity.name,
        version: identity.version,
    })
}

fn read_identity(
    installed_root: &Path,
    package_path: &Path,
) -> Result<InstalledPackage, ProveFailure> {
    let installed_path = installed_root.join(package_path);
    require_package_directory(installed_root, package_path, "installed package")?;
    let package_json = installed_path.join("package.json");
    require_regular_file(&package_json, "installed package identity")?;
    let bytes = fs::read(&package_json).map_err(|error| {
        package_refusal(format!("cannot read {}: {error}", package_json.display()))
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        package_refusal(format!("cannot decode {}: {error}", package_json.display()))
    })
}

fn collect_closure_files(
    installed_root: &Path,
    packages: &[SnapshotPackage],
) -> Result<Vec<SnapshotFile>, ProveFailure> {
    let package_roots = packages
        .iter()
        .map(|package| package.path.clone())
        .collect::<BTreeSet<_>>();
    let mut files = Vec::new();
    for package in packages {
        require_package_directory(
            installed_root,
            Path::new(&package.path),
            "installed package",
        )?;
        collect_snapshot_files(
            installed_root,
            &installed_root.join(&package.path),
            &package_roots,
            &mut files,
        )?;
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    if files.windows(2).any(|pair| pair[0].path == pair[1].path) {
        return Err(package_refusal(
            "installed Playwright closure contains duplicate file paths",
        ));
    }
    Ok(files)
}

fn resolve_dependency(
    installed_root: &Path,
    requiring: &Path,
    name: &str,
    optional: bool,
) -> Result<Option<PathBuf>, ProveFailure> {
    validate_package_name(name)?;
    for candidate in dependency_candidates(requiring, name)? {
        if package_directory_exists(
            installed_root,
            &candidate,
            &format!("installed dependency {name}"),
        )? {
            return Ok(Some(candidate));
        }
    }
    if optional {
        return Ok(None);
    }
    Err(package_refusal(format!(
        "installed dependency {name:?} required by {} is absent",
        relative_path_spelling(requiring)?,
    )))
}

fn dependency_candidates(requiring: &Path, name: &str) -> Result<Vec<PathBuf>, ProveFailure> {
    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = Some(requiring);
    while let Some(directory) = current {
        if directory.file_name() != Some(OsStr::new("node_modules")) {
            let candidate = directory.join("node_modules").join(name);
            relative_path_spelling(&candidate)?;
            if seen.insert(candidate.clone()) {
                candidates.push(candidate);
            }
        }
        current = directory.parent();
    }
    Ok(candidates)
}

fn collect_snapshot_files(
    installed_root: &Path,
    directory: &Path,
    package_roots: &BTreeSet<String>,
    files: &mut Vec<SnapshotFile>,
) -> Result<(), ProveFailure> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| package_refusal(format!("cannot read {}: {error}", directory.display())))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            package_refusal(format!("cannot read {}: {error}", directory.display()))
        })?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            package_refusal(format!("cannot inspect {}: {error}", path.display()))
        })?;
        let relative = path.strip_prefix(installed_root).map_err(|_| {
            package_refusal(format!(
                "installed package member escapes its root: {}",
                path.display(),
            ))
        })?;
        let spelling = relative_path_spelling(relative)?;
        if metadata.file_type().is_dir() {
            if package_roots.contains(&spelling) {
                continue;
            }
            collect_snapshot_files(installed_root, &path, package_roots, files)?;
            continue;
        }
        if !metadata.file_type().is_file() {
            return Err(package_refusal(format!(
                "installed package member {} is not a regular file",
                path.display(),
            )));
        }
        files.push(SnapshotFile {
            path: spelling,
            mode: snapshot_mode(&metadata),
            bytes: fs::read(&path).map_err(|error| {
                package_refusal(format!("cannot read {}: {error}", path.display()))
            })?,
        });
    }
    Ok(())
}

fn validate_package_name(name: &str) -> Result<(), ProveFailure> {
    let path = Path::new(name);
    let components = path.components().collect::<Vec<_>>();
    let scoped = name.starts_with('@');
    let expected = if scoped { 2 } else { 1 };
    if components.len() != expected
        || components
            .iter()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        || name.contains('\\')
    {
        return Err(package_refusal(format!(
            "package name is not a confined npm path: {name:?}",
        )));
    }
    Ok(())
}

fn relative_path_spelling(path: &Path) -> Result<String, ProveFailure> {
    path.components()
        .map(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .map(|parts| parts.join("/"))
        .ok_or_else(|| {
            package_refusal(format!(
                "package snapshot path is not confined UTF-8: {}",
                path.display(),
            ))
        })
}

fn require_directory(path: &Path, description: &str) -> Result<(), ProveFailure> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| package_refusal(format!("cannot inspect {}: {error}", path.display())))?;
    if !metadata.file_type().is_dir() {
        return Err(package_refusal(format!(
            "{description} is not a directory: {}",
            path.display(),
        )));
    }
    Ok(())
}

fn require_package_directory(
    installed_root: &Path,
    relative: &Path,
    description: &str,
) -> Result<(), ProveFailure> {
    if package_directory_exists(installed_root, relative, description)? {
        return Ok(());
    }
    Err(package_refusal(format!(
        "{description} is absent: {}",
        installed_root.join(relative).display(),
    )))
}

fn package_directory_exists(
    installed_root: &Path,
    relative: &Path,
    description: &str,
) -> Result<bool, ProveFailure> {
    let mut current = installed_root.to_path_buf();
    let mut components = relative.components();
    if components.next() != Some(std::path::Component::Normal(OsStr::new("node_modules"))) {
        return Err(package_refusal(format!(
            "{description} is not below the installed package root: {}",
            installed_root.join(relative).display(),
        )));
    }
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(package_refusal(format!(
                "{description} has an unconfined component: {}",
                installed_root.join(relative).display(),
            )));
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(package_refusal(format!(
                    "{description} path component is not a directory: {}",
                    current.display(),
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(package_refusal(format!(
                    "cannot inspect {description} path component {}: {error}",
                    current.display(),
                )));
            }
        }
    }
    Ok(true)
}

fn require_regular_file(path: &Path, description: &str) -> Result<(), ProveFailure> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| package_refusal(format!("cannot inspect {}: {error}", path.display())))?;
    if !metadata.file_type().is_file() {
        return Err(package_refusal(format!(
            "{description} is not a regular file: {}",
            path.display(),
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn snapshot_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn snapshot_mode(metadata: &fs::Metadata) -> u32 {
    u32::from(metadata.permissions().readonly())
}

#[cfg(unix)]
fn set_snapshot_mode(path: &Path, mode: u32) -> Result<(), ProveFailure> {
    use std::os::unix::fs::PermissionsExt as _;

    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| package_refusal(format!("cannot set mode on {}: {error}", path.display())))
}

#[cfg(not(unix))]
fn set_snapshot_mode(path: &Path, mode: u32) -> Result<(), ProveFailure> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| package_refusal(format!("cannot inspect {}: {error}", path.display())))?
        .permissions();
    permissions.set_readonly(mode != 0);
    fs::set_permissions(path, permissions)
        .map_err(|error| package_refusal(format!("cannot set mode on {}: {error}", path.display())))
}

fn package_refusal(detail: impl ToString) -> ProveFailure {
    ProveFailure::refusal("gate::prove-canvas-package-refused", detail)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::{Map, Value, json};

    use super::*;

    struct PackageFixture {
        root: PathBuf,
        selected: PathBuf,
        installed: PathBuf,
    }

    impl PackageFixture {
        fn empty() -> Self {
            static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "spice-canvas-package-test-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
            ));
            let selected = root.join("selected");
            let installed = root.join("installed");
            fs::create_dir_all(&selected).expect("create selected fixture root");
            fs::create_dir_all(installed.join("node_modules"))
                .expect("create installed fixture root");
            Self {
                root,
                selected,
                installed,
            }
        }

        fn new() -> Self {
            let fixture = Self::empty();
            fixture.write_lock(json!({
                "": {},
                "node_modules/playwright": {
                    "version": "1.0.0",
                    "dependencies": {"playwright-core": "1.0.0"},
                    "optionalDependencies": {"fsevents": "1.0.0"}
                },
                "node_modules/playwright-core": {"version": "1.0.0"},
                "node_modules/fsevents": {"version": "1.0.0"}
            }));
            fixture.write_package(
                "node_modules/playwright",
                "playwright",
                "1.0.0",
                &[("playwright-core", "1.0.0")],
                &[("fsevents", "1.0.0")],
            );
            fixture.write_package(
                "node_modules/playwright-core",
                "playwright-core",
                "1.0.0",
                &[],
                &[],
            );
            fixture.write_package("node_modules/fsevents", "fsevents", "1.0.0", &[], &[]);
            fixture
        }

        fn write_lock(&self, packages: Value) {
            fs::write(
                self.selected.join("package-lock.json"),
                serde_json::to_vec(&json!({
                    "lockfileVersion": 3,
                    "packages": packages,
                }))
                .expect("encode package lock"),
            )
            .expect("write package lock");
        }

        fn write_package(
            &self,
            relative: &str,
            name: &str,
            version: &str,
            dependencies: &[(&str, &str)],
            optional_dependencies: &[(&str, &str)],
        ) {
            let package = self.installed.join(relative);
            fs::create_dir_all(&package).expect("create package fixture");
            self.write_identity(relative, name, version, dependencies, optional_dependencies);
            fs::write(
                package.join("index.js"),
                format!("export const value = '{name}@{version}';"),
            )
            .expect("write package source");
        }

        fn write_identity(
            &self,
            relative: &str,
            name: &str,
            version: &str,
            dependencies: &[(&str, &str)],
            optional_dependencies: &[(&str, &str)],
        ) {
            let map = |rows: &[(&str, &str)]| {
                rows.iter()
                    .map(|(name, version)| {
                        ((*name).to_owned(), Value::String((*version).to_owned()))
                    })
                    .collect::<Map<_, _>>()
            };
            fs::write(
                self.installed.join(relative).join("package.json"),
                serde_json::to_vec(&json!({
                    "name": name,
                    "version": version,
                    "dependencies": map(dependencies),
                    "optionalDependencies": map(optional_dependencies),
                }))
                .expect("encode package identity"),
            )
            .expect("write package identity");
        }

        fn source(&self, relative: &str) -> PathBuf {
            self.installed.join(relative).join("index.js")
        }
    }

    impl Drop for PackageFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn failure(result: Result<PlaywrightSnapshot, ProveFailure>) -> (String, String) {
        match result {
            Err(ProveFailure::Refusal { reason, detail }) => (reason, detail),
            Err(ProveFailure::Fault { reason, detail }) => {
                panic!("expected refusal, got fault {reason}: {detail}")
            }
            Ok(_) => panic!("package capture must refuse"),
        }
    }

    fn success<T>(result: Result<T, ProveFailure>) -> T {
        match result {
            Ok(value) => value,
            Err(ProveFailure::Refusal { reason, detail })
            | Err(ProveFailure::Fault { reason, detail }) => {
                panic!("unexpected failure {reason}: {detail}")
            }
        }
    }

    fn nested_fixture() -> PackageFixture {
        let fixture = PackageFixture::empty();
        fixture.write_lock(json!({
            "": {},
            "node_modules/playwright": {
                "version": "1.0.0",
                "dependencies": {"dependency": "1.0.0"}
            },
            "node_modules/playwright/node_modules/dependency": {"version": "1.0.0"},
            "node_modules/dependency": {"version": "2.0.0"}
        }));
        fixture.write_package(
            "node_modules/playwright",
            "playwright",
            "1.0.0",
            &[("dependency", "1.0.0")],
            &[],
        );
        fixture.write_package(
            "node_modules/playwright/node_modules/dependency",
            "dependency",
            "1.0.0",
            &[],
            &[],
        );
        fixture.write_package("node_modules/dependency", "dependency", "2.0.0", &[], &[]);
        fixture
    }

    #[test]
    fn package_snapshot_authority_is_root_independent_and_content_sensitive() {
        let first = PackageFixture::new();
        let second = PackageFixture::new();
        let original = success(PlaywrightSnapshot::capture(
            &first.selected,
            &first.installed,
        ));
        let equivalent = success(PlaywrightSnapshot::capture(
            &second.selected,
            &second.installed,
        ));
        assert_eq!(original.authority, equivalent.authority);

        fs::write(
            second.source("node_modules/playwright"),
            b"same version, changed bytes",
        )
        .expect("mutate package source");
        let changed = success(PlaywrightSnapshot::capture(
            &second.selected,
            &second.installed,
        ));
        assert_ne!(original.authority, changed.authority);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let source = first.source("node_modules/playwright-core");
            fs::set_permissions(&source, fs::Permissions::from_mode(0o755))
                .expect("change package source mode");
            let changed_mode = success(PlaywrightSnapshot::capture(
                &first.selected,
                &first.installed,
            ));
            assert_ne!(original.authority, changed_mode.authority);
        }
    }

    #[test]
    fn package_snapshot_materializes_only_captured_bytes() {
        let fixture = PackageFixture::new();
        let snapshot = success(PlaywrightSnapshot::capture(
            &fixture.selected,
            &fixture.installed,
        ));
        fs::write(fixture.source("node_modules/playwright"), b"live mutation")
            .expect("mutate live package source");
        let materialized = fixture.root.join("materialized");
        fs::create_dir(&materialized).expect("create materialization root");
        success(snapshot.materialize(&materialized));
        assert_eq!(
            fs::read(materialized.join("node_modules/playwright/index.js"))
                .expect("read materialized source"),
            b"export const value = 'playwright@1.0.0';",
        );
        assert_eq!(
            fs::read(fixture.source("node_modules/playwright")).expect("read live source"),
            b"live mutation",
        );
    }

    #[test]
    fn nested_dependency_shadows_an_unrelated_hoisted_version() {
        let fixture = nested_fixture();
        let original = success(PlaywrightSnapshot::capture(
            &fixture.selected,
            &fixture.installed,
        ));
        let paths = original
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<BTreeSet<_>>();
        assert!(paths.contains("node_modules/playwright/node_modules/dependency/index.js"));
        assert!(!paths.contains("node_modules/dependency/index.js"));
        let materialized = fixture.root.join("nested-materialized");
        fs::create_dir(&materialized).expect("create nested materialization root");
        success(original.materialize(&materialized));
        assert!(
            materialized
                .join("node_modules/playwright/node_modules/dependency/index.js")
                .is_file()
        );
        assert!(!materialized.join("node_modules/dependency").exists());

        fs::write(
            fixture.source("node_modules/dependency"),
            b"unrelated hoisted mutation",
        )
        .expect("mutate unrelated hoisted package");
        let unchanged = success(PlaywrightSnapshot::capture(
            &fixture.selected,
            &fixture.installed,
        ));
        assert_eq!(original.authority, unchanged.authority);

        fs::write(
            fixture.source("node_modules/playwright/node_modules/dependency"),
            b"executed nested mutation",
        )
        .expect("mutate nested package");
        let changed = success(PlaywrightSnapshot::capture(
            &fixture.selected,
            &fixture.installed,
        ));
        assert_ne!(original.authority, changed.authority);
    }

    #[test]
    fn hoisted_scoped_dependency_can_resolve_its_own_nested_dependency() {
        let fixture = PackageFixture::empty();
        fixture.write_lock(json!({
            "": {},
            "node_modules/playwright": {
                "version": "1.0.0",
                "dependencies": {"@scope/bridge": "1.0.0"}
            },
            "node_modules/@scope/bridge": {
                "version": "1.0.0",
                "dependencies": {"leaf": "1.0.0"}
            },
            "node_modules/@scope/bridge/node_modules/leaf": {"version": "1.0.0"}
        }));
        fixture.write_package(
            "node_modules/playwright",
            "playwright",
            "1.0.0",
            &[("@scope/bridge", "1.0.0")],
            &[],
        );
        fixture.write_package(
            "node_modules/@scope/bridge",
            "@scope/bridge",
            "1.0.0",
            &[("leaf", "1.0.0")],
            &[],
        );
        fixture.write_package(
            "node_modules/@scope/bridge/node_modules/leaf",
            "leaf",
            "1.0.0",
            &[],
            &[],
        );

        let snapshot = success(PlaywrightSnapshot::capture(
            &fixture.selected,
            &fixture.installed,
        ));
        assert!(
            snapshot.files.iter().any(|file| {
                file.path == "node_modules/@scope/bridge/node_modules/leaf/index.js"
            })
        );
    }

    #[test]
    fn absent_optional_dependency_is_skipped_but_a_present_one_is_bound() {
        let fixture = PackageFixture::new();
        fs::remove_dir_all(fixture.installed.join("node_modules/fsevents"))
            .expect("remove optional package");
        let absent = success(PlaywrightSnapshot::capture(
            &fixture.selected,
            &fixture.installed,
        ));
        assert!(
            absent
                .files
                .iter()
                .all(|file| !file.path.contains("fsevents"))
        );

        fixture.write_package("node_modules/fsevents", "fsevents", "1.0.0", &[], &[]);
        let present = success(PlaywrightSnapshot::capture(
            &fixture.selected,
            &fixture.installed,
        ));
        assert_ne!(absent.authority, present.authority);
    }

    #[test]
    fn existing_nested_dependency_requires_its_exact_lock_entry() {
        let fixture = nested_fixture();
        fixture.write_lock(json!({
            "": {},
            "node_modules/playwright": {
                "version": "1.0.0",
                "dependencies": {"dependency": "1.0.0"}
            },
            "node_modules/dependency": {"version": "2.0.0"}
        }));
        let (reason, detail) = failure(PlaywrightSnapshot::capture(
            &fixture.selected,
            &fixture.installed,
        ));
        assert_eq!(reason, "gate::prove-canvas-package-refused");
        assert!(
            detail.contains("node_modules/playwright/node_modules/dependency"),
            "{detail}"
        );
    }

    #[test]
    fn nested_identity_and_dependency_disagreement_refuse_at_the_resolved_path() {
        for (name, version, dependencies) in [
            ("other", "1.0.0", Vec::new()),
            ("dependency", "2.0.0", Vec::new()),
            ("dependency", "1.0.0", vec![("undeclared", "1.0.0")]),
        ] {
            let fixture = nested_fixture();
            fixture.write_identity(
                "node_modules/playwright/node_modules/dependency",
                name,
                version,
                &dependencies,
                &[],
            );
            let (reason, detail) = failure(PlaywrightSnapshot::capture(
                &fixture.selected,
                &fixture.installed,
            ));
            assert_eq!(reason, "gate::prove-canvas-package-refused");
            assert!(
                detail.contains("node_modules/playwright/node_modules/dependency"),
                "{detail}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn package_snapshot_refuses_scoped_package_ancestor_links() {
        let fixture = PackageFixture::empty();
        fixture.write_lock(json!({
            "": {},
            "node_modules/playwright": {
                "version": "1.0.0",
                "optionalDependencies": {"@scope/bridge": "1.0.0"}
            },
            "node_modules/@scope/bridge": {"version": "1.0.0"}
        }));
        fixture.write_package(
            "node_modules/playwright",
            "playwright",
            "1.0.0",
            &[],
            &[("@scope/bridge", "1.0.0")],
        );
        fixture.write_package("external-scope/bridge", "@scope/bridge", "1.0.0", &[], &[]);
        let external = fixture.root.join("external-scope");
        fs::rename(fixture.installed.join("external-scope"), &external)
            .expect("move scoped package outside installed root");
        let scope = fixture.installed.join("node_modules/@scope");
        std::os::unix::fs::symlink(&external, &scope).expect("link package scope outside root");

        let (reason, detail) = failure(PlaywrightSnapshot::capture(
            &fixture.selected,
            &fixture.installed,
        ));
        assert_eq!(reason, "gate::prove-canvas-package-refused");
        assert!(
            detail.contains(scope.to_string_lossy().as_ref()),
            "{detail}"
        );
        assert!(
            detail.contains("path component is not a directory"),
            "{detail}"
        );

        fs::remove_file(&scope).expect("remove external scope link");
        std::os::unix::fs::symlink(fixture.root.join("missing-scope"), &scope)
            .expect("create broken package scope link");
        let (_, detail) = failure(PlaywrightSnapshot::capture(
            &fixture.selected,
            &fixture.installed,
        ));
        assert!(
            detail.contains(scope.to_string_lossy().as_ref()),
            "{detail}"
        );
        assert!(
            detail.contains("path component is not a directory"),
            "{detail}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn package_snapshot_refuses_nested_scoped_package_ancestor_links() {
        let fixture = PackageFixture::empty();
        fixture.write_lock(json!({
            "": {},
            "node_modules/playwright": {
                "version": "1.0.0",
                "dependencies": {"parent": "1.0.0"}
            },
            "node_modules/playwright/node_modules/parent": {
                "version": "1.0.0",
                "dependencies": {"@scope/leaf": "1.0.0"}
            },
            "node_modules/playwright/node_modules/parent/node_modules/@scope/leaf": {
                "version": "1.0.0"
            }
        }));
        fixture.write_package(
            "node_modules/playwright",
            "playwright",
            "1.0.0",
            &[("parent", "1.0.0")],
            &[],
        );
        fixture.write_package(
            "node_modules/playwright/node_modules/parent",
            "parent",
            "1.0.0",
            &[("@scope/leaf", "1.0.0")],
            &[],
        );
        fixture.write_package(
            "external-nested-scope/leaf",
            "@scope/leaf",
            "1.0.0",
            &[],
            &[],
        );
        let external = fixture.root.join("external-nested-scope");
        fs::rename(fixture.installed.join("external-nested-scope"), &external)
            .expect("move nested scope outside installed root");
        let scope = fixture
            .installed
            .join("node_modules/playwright/node_modules/parent/node_modules/@scope");
        fs::create_dir_all(scope.parent().expect("scope has a parent"))
            .expect("create nested package modules");
        std::os::unix::fs::symlink(&external, &scope)
            .expect("link nested package scope outside root");

        let (reason, detail) = failure(PlaywrightSnapshot::capture(
            &fixture.selected,
            &fixture.installed,
        ));
        assert_eq!(reason, "gate::prove-canvas-package-refused");
        assert!(
            detail.contains(scope.to_string_lossy().as_ref()),
            "{detail}"
        );
        assert!(
            detail.contains("path component is not a directory"),
            "{detail}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn package_snapshot_refuses_symlinked_members() {
        let fixture = PackageFixture::new();
        let source = fixture.source("node_modules/playwright");
        fs::remove_file(&source).expect("remove package source");
        std::os::unix::fs::symlink("package.json", &source).expect("symlink package source");
        let (reason, _) = failure(PlaywrightSnapshot::capture(
            &fixture.selected,
            &fixture.installed,
        ));
        assert_eq!(reason, "gate::prove-canvas-package-refused");
    }
}
