use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sill::{Acquisition, LockFile, LockName};
use wire::CanonicalUuid;

use super::{Host, IsolationError, PreparedHome, SourceGenerationAuthority};

const SCHEMA: u64 = 2;
const MARKER: &str = "store.json";
const OWNER: &str = "owner.json";
const CONFIG: &str = "config.toml";
const MANIFEST: &str = "generation.json";
const TEMPORARY_PREFIX: &str = ".spice-tmp-";
const LOCK_WAIT: Duration = Duration::from_secs(25 * 60);
const LOCK_POLL: Duration = Duration::from_millis(25);
const RECLAIM_GRACE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Clone, Debug)]
struct Layout {
    root: PathBuf,
    packages: PathBuf,
    generations: PathBuf,
    homes: PathBuf,
    pins: PathBuf,
    locks: PathBuf,
}

impl Layout {
    fn new(host: &Host) -> Self {
        Self::from_root(host.store_root())
    }

    fn from_root(root: PathBuf) -> Self {
        Self {
            packages: root.join("packages"),
            generations: root.join("generations"),
            homes: root.join("homes"),
            pins: root.join("pins"),
            locks: root.join("locks"),
            root,
        }
    }

    fn initialize(host: &Host) -> Result<Self, IsolationError> {
        let layout = Self::new(host);
        create_private_directory(&host.bootstrap_home().join("spice"))?;
        create_private_directory(&layout.root)?;
        for directory in [
            &layout.packages,
            &layout.generations,
            &layout.homes,
            &layout.pins,
            &layout.locks,
        ] {
            create_private_directory(directory)?;
        }
        let marker = layout.root.join(MARKER);
        let expected = StoreMarker { schema: SCHEMA };
        match fs::symlink_metadata(&marker) {
            Ok(_) => {
                let observed: StoreMarker = read_json(&marker)?;
                if observed != expected {
                    return Err(IsolationError::new(
                        "cargo-isolation::store-schema-mismatch",
                        format!("expected {expected:?}, observed {observed:?}"),
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                publish_json(&marker, &expected)?;
            }
            Err(error) => {
                return Err(IsolationError::io(
                    "cargo-isolation::record-unavailable",
                    &marker,
                    error,
                ));
            }
        }
        Self::open(host)
    }

    fn open(host: &Host) -> Result<Self, IsolationError> {
        Self::open_root(host.store_root())
    }

    fn open_root(root: PathBuf) -> Result<Self, IsolationError> {
        let layout = Self::from_root(root);
        for directory in [
            &layout.root,
            &layout.packages,
            &layout.generations,
            &layout.homes,
            &layout.pins,
            &layout.locks,
        ] {
            require_directory_mode(directory, 0o700)?;
        }
        let marker = layout.root.join(MARKER);
        require_mode(&marker, 0o600)?;
        let expected = StoreMarker { schema: SCHEMA };
        let observed: StoreMarker = read_json(&marker)?;
        if observed != expected {
            return Err(IsolationError::new(
                "cargo-isolation::store-schema-mismatch",
                format!("expected {expected:?}, observed {observed:?}"),
            ));
        }
        Ok(layout)
    }
}

pub(super) fn source_generation_authority(
    home: &Path,
    lockfile: &[u8],
) -> Result<SourceGenerationAuthority, IsolationError> {
    let homes = home
        .parent()
        .filter(|path| path.file_name() == Some(OsStr::new("homes")))
        .ok_or_else(|| {
            IsolationError::new(
                "cargo-isolation::home-path-invalid",
                format!("Cargo home is outside the admitted homes directory: {home:?}"),
            )
        })?;
    let root = homes.parent().ok_or_else(|| {
        IsolationError::new(
            "cargo-isolation::home-path-invalid",
            format!("Cargo home has no source-store root: {home:?}"),
        )
    })?;
    let layout = Layout::open_root(root.to_path_buf())?;
    let generation = home_generation(home, &layout)?;
    let expected = generation_digest(lockfile);
    if generation != expected {
        return Err(IsolationError::new(
            "cargo-isolation::generation-lock-mismatch",
            format!(
                "Cargo home generation {generation} does not match selected lock generation {expected}"
            ),
        ));
    }
    validate_generation(&layout, &generation)?;
    let manifest_path = layout.generations.join(&generation).join(MANIFEST);
    let manifest = fs::read(&manifest_path).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::generation-manifest-unavailable",
            &manifest_path,
            error,
        )
    })?;
    Ok(SourceGenerationAuthority {
        generation,
        manifest: digest(b"spice-cargo-source-authority-v1", &manifest),
    })
}

pub(super) fn readmit_source_generation(
    home: &Path,
    lockfile: &[u8],
    current_worktree: &Path,
    host: &Host,
) -> Result<SourceGenerationAuthority, IsolationError> {
    let ReadmissionHome {
        retained,
        retained_root,
        owner,
        home_id,
        expected_worktree,
    } = readmission_home(home, current_worktree)?;
    if retained_root == host.store_root() {
        return source_generation_authority(home, lockfile);
    }
    let expected = generation_digest(lockfile);
    let current = Layout::open(host)?;
    let (observed_generation, already_current) = readmission_generation(home, &retained, &current)?;
    if observed_generation != expected {
        return Err(IsolationError::new(
            "cargo-isolation::generation-lock-mismatch",
            format!(
                "retained Cargo home generation {observed_generation} does not match selected lock generation {expected}"
            ),
        ));
    }
    if already_current {
        validate_generation(&current, &observed_generation)?;
    } else {
        validate_generation(&retained, &observed_generation)?;
    }
    let manifest = validate_generation(&current, &expected)?;
    let _lock = acquire_lock(&retained, &format!("home-{home_id}"))?;
    revalidate_readmission(
        home,
        &retained,
        &current,
        already_current,
        &owner,
        &expected_worktree,
        &observed_generation,
    )?;
    if !already_current {
        publish_bytes(&home.join(CONFIG), home_config(&manifest)?.as_bytes())?;
    }
    let manifest_path = current.generations.join(&expected).join(MANIFEST);
    let bytes = fs::read(&manifest_path).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::generation-manifest-unavailable",
            &manifest_path,
            error,
        )
    })?;
    Ok(SourceGenerationAuthority {
        generation: expected,
        manifest: digest(b"spice-cargo-source-authority-v1", &bytes),
    })
}

pub(super) fn select_source_generation(
    home: &Path,
    lockfile: &[u8],
    current_worktree: &Path,
    host: &Host,
) -> Result<(PreparedHome, SourceGenerationAuthority), IsolationError> {
    let layout = Layout::open(host)?;
    let _store_lock = acquire_lock(&layout, "store-mutation")?;
    let (home, authority) = match readmit_source_generation(home, lockfile, current_worktree, host)
    {
        Ok(authority) => (home.to_path_buf(), authority),
        Err(error) if error.reason() == "cargo-isolation::generation-lock-mismatch" => {
            // A supervised lane retains its original generation until its seat
            // is vacant. Only its worktree's separately admitted development
            // home can supply a newer selected generation.
            let mut owner = readmission_home(home, current_worktree)?.owner;
            owner.lane = None;
            let manual = layout.homes.join(home_identity(&owner));
            let authority = readmit_source_generation(&manual, lockfile, current_worktree, host)?;
            (manual, authority)
        }
        Err(error) => return Err(error),
    };
    let pin = GenerationPin::acquire(&layout, authority.generation())?;
    Ok((
        PreparedHome {
            home,
            generation: authority.generation().to_owned(),
            store: layout.root,
            cargo: host.cargo().to_path_buf(),
            reused_generation: true,
            _pin: Some(pin),
        },
        authority,
    ))
}

struct ReadmissionHome {
    retained: Layout,
    retained_root: PathBuf,
    owner: HomeOwner,
    home_id: String,
    expected_worktree: String,
}

fn readmission_home(
    home: &Path,
    current_worktree: &Path,
) -> Result<ReadmissionHome, IsolationError> {
    let retained_homes = home
        .parent()
        .filter(|path| path.file_name() == Some(OsStr::new("homes")))
        .ok_or_else(|| {
            IsolationError::new(
                "cargo-isolation::home-path-invalid",
                format!("Cargo home is outside the admitted homes directory: {home:?}"),
            )
        })?;
    let retained_root = retained_homes.parent().ok_or_else(|| {
        IsolationError::new(
            "cargo-isolation::home-path-invalid",
            format!("Cargo home has no source-store root: {home:?}"),
        )
    })?;
    let retained_root = retained_root.to_path_buf();
    let retained = Layout::open_root(retained_root.clone())?;
    let owner: HomeOwner = read_json(&home.join(OWNER))?;
    let home_id = home_identity(&owner);
    if home.file_name() != Some(OsStr::new(&home_id)) {
        return Err(IsolationError::new(
            "cargo-isolation::home-owner-mismatch",
            format!("home path does not match observed owner {owner:?}"),
        ));
    }
    let current_worktree = fs::canonicalize(current_worktree).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::home-owner-mismatch",
            current_worktree,
            error,
        )
    })?;
    let expected_worktree = encoded_path(&current_worktree);
    if owner.worktree != expected_worktree {
        return Err(IsolationError::new(
            "cargo-isolation::home-owner-mismatch",
            format!(
                "observed home owner {owner:?} does not match current worktree {expected_worktree:?}"
            ),
        ));
    }
    Ok(ReadmissionHome {
        retained,
        retained_root,
        owner,
        home_id,
        expected_worktree,
    })
}

fn revalidate_readmission(
    home: &Path,
    retained: &Layout,
    current: &Layout,
    already_current: bool,
    owner: &HomeOwner,
    expected_worktree: &str,
    observed_generation: &str,
) -> Result<(), IsolationError> {
    let observed: HomeOwner = read_json(&home.join(OWNER))?;
    let observed_generation_after_lock = if already_current {
        home_generation(home, current)?
    } else {
        home_generation(home, retained)?
    };
    if &observed != owner
        || observed.worktree != expected_worktree
        || observed_generation_after_lock != observed_generation
    {
        return Err(IsolationError::new(
            "cargo-isolation::home-owner-mismatch",
            "retained Cargo home changed while re-admission was preparing",
        ));
    }
    Ok(())
}

fn readmission_generation(
    home: &Path,
    retained: &Layout,
    current: &Layout,
) -> Result<(String, bool), IsolationError> {
    match home_generation(home, retained) {
        Ok(generation) => Ok((generation, false)),
        Err(retained_error) => match home_generation(home, current) {
            Ok(generation) => Ok((generation, true)),
            Err(_) => Err(retained_error),
        },
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StoreMarker {
    schema: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct HomeOwner {
    schema: u64,
    worktree: String,
    lane: Option<String>,
}

fn home_identity(owner: &HomeOwner) -> String {
    digest(
        b"spice-cargo-home-v1",
        &serde_json::to_vec(owner).expect("home owner is serializable"),
    )
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct GenerationManifest {
    schema: u64,
    generation: String,
    lock_digest: String,
    source_config: toml::Value,
    packages: Vec<PackageRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
struct PackageRecord {
    directory: String,
    identity: String,
    name: String,
    version: String,
    source: String,
    checksum: Option<String>,
    tree_digest: String,
}

#[derive(Clone, Debug)]
struct LockedPackage {
    name: String,
    version: String,
    source: String,
    checksum: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CargoChecksum {
    #[serde(rename = "$comment")]
    _comment: Option<String>,
    files: BTreeMap<String, String>,
    package: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Inspection {
    store: String,
    generations: usize,
    packages: usize,
    homes: usize,
    logical_bytes: u64,
    physical_bytes: u64,
    issues: Vec<String>,
}

impl Inspection {
    #[must_use]
    pub fn store(&self) -> &str {
        &self.store
    }

    #[must_use]
    pub const fn generations(&self) -> usize {
        self.generations
    }

    #[must_use]
    pub const fn packages(&self) -> usize {
        self.packages
    }

    #[must_use]
    pub const fn homes(&self) -> usize {
        self.homes
    }

    #[must_use]
    pub const fn logical_bytes(&self) -> u64 {
        self.logical_bytes
    }

    #[must_use]
    pub const fn physical_bytes(&self) -> u64 {
        self.physical_bytes
    }

    #[must_use]
    pub fn issues(&self) -> &[String] {
        &self.issues
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CleanupReport {
    removed_temporaries: usize,
    removed_pins: usize,
    removed_generations: usize,
    removed_packages: usize,
    retained_uncertain: Vec<String>,
    after: Inspection,
}

impl CleanupReport {
    #[must_use]
    pub const fn removed_temporaries(&self) -> usize {
        self.removed_temporaries
    }

    #[must_use]
    pub const fn removed_pins(&self) -> usize {
        self.removed_pins
    }

    #[must_use]
    pub const fn removed_generations(&self) -> usize {
        self.removed_generations
    }

    #[must_use]
    pub const fn removed_packages(&self) -> usize {
        self.removed_packages
    }

    #[must_use]
    pub fn retained_uncertain(&self) -> &[String] {
        &self.retained_uncertain
    }

    #[must_use]
    pub const fn after(&self) -> &Inspection {
        &self.after
    }
}

pub(super) fn prepare(
    worktree: &Path,
    lane: Option<CanonicalUuid>,
    host: &Host,
) -> Result<PreparedHome, IsolationError> {
    let layout = Layout::initialize(host)?;
    let _store_lock = acquire_lock(&layout, "store-mutation")?;
    let worktree = fs::canonicalize(worktree).map_err(|error| {
        IsolationError::io("cargo-isolation::worktree-unavailable", worktree, error)
    })?;
    let lock_path = worktree.join("Cargo.lock");
    let lock_bytes = fs::read(&lock_path).map_err(|error| {
        IsolationError::io("cargo-isolation::lockfile-unavailable", &lock_path, error)
    })?;
    let generation = generation_digest(&lock_bytes);
    let (manifest, reused_generation) =
        ensure_generation(&layout, &worktree, &lock_bytes, &generation, host)?;
    let home = ensure_home(&layout, &worktree, lane, &manifest)?;
    let pin = if lane.is_none() {
        Some(pins::GenerationPin::acquire(&layout, &generation)?)
    } else {
        None
    };
    Ok(PreparedHome {
        home,
        generation,
        store: layout.root,
        cargo: host.cargo().to_path_buf(),
        reused_generation,
        _pin: pin,
    })
}

fn ensure_generation(
    layout: &Layout,
    worktree: &Path,
    lock_bytes: &[u8],
    generation: &str,
    host: &Host,
) -> Result<(GenerationManifest, bool), IsolationError> {
    let final_path = layout.generations.join(generation);
    if final_path.exists() {
        return validate_generation(layout, generation).map(|manifest| (manifest, true));
    }
    let _lock = acquire_lock(layout, &format!("generation-{generation}"))?;
    if final_path.exists() {
        return validate_generation(layout, generation).map(|manifest| (manifest, true));
    }
    let temporary = TemporaryDirectory::create(&layout.generations, "generation")?;
    let vendor = temporary.path().join("vendor");
    fs::create_dir(&vendor)
        .map_err(|error| IsolationError::io("cargo-isolation::seed-unavailable", &vendor, error))?;
    let locked = locked_packages(lock_bytes)?;
    let final_vendor = final_path.join("vendor");
    let (source_config, packages) = if locked.is_empty() {
        (empty_source_config(&final_vendor)?, Vec::new())
    } else {
        let source_config = seed::run_vendor(worktree, &vendor, host)?;
        let packages = publish_packages(layout, &vendor, &locked)?;
        (
            rebind_source_config(source_config, &vendor, &final_vendor)?,
            packages,
        )
    };
    let manifest = GenerationManifest {
        schema: SCHEMA,
        generation: generation.to_owned(),
        lock_digest: digest(b"spice-cargo-lock-v2", lock_bytes),
        source_config,
        packages,
    };
    publish_json(&temporary.path().join(MANIFEST), &manifest)?;
    make_generation_read_only(temporary.path())?;
    temporary.publish(&final_path)?;
    validate_generation(layout, generation).map(|manifest| (manifest, false))
}

fn empty_source_config(final_vendor: &Path) -> Result<toml::Value, IsolationError> {
    let mut vendored = toml::Table::new();
    vendored.insert(
        "directory".to_owned(),
        toml::Value::String(path_text(final_vendor)?),
    );
    let mut sources = toml::Table::new();
    sources.insert("vendored-sources".to_owned(), toml::Value::Table(vendored));
    let mut root = toml::Table::new();
    root.insert("source".to_owned(), toml::Value::Table(sources));
    Ok(toml::Value::Table(root))
}

fn locked_packages(bytes: &[u8]) -> Result<Vec<LockedPackage>, IsolationError> {
    let document: toml::Value = toml::from_str(std::str::from_utf8(bytes).map_err(|error| {
        IsolationError::new("cargo-isolation::lockfile-invalid", error.to_string())
    })?)
    .map_err(|error| IsolationError::new("cargo-isolation::lockfile-invalid", error.to_string()))?;
    let packages = document
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| {
            IsolationError::new(
                "cargo-isolation::lockfile-invalid",
                "Cargo.lock has no package array",
            )
        })?;
    let mut locked = Vec::new();
    for package in packages {
        let Some(source) = package.get("source").and_then(toml::Value::as_str) else {
            continue;
        };
        locked.push(LockedPackage {
            name: lock_text(package, "name")?,
            version: lock_text(package, "version")?,
            source: source.to_owned(),
            checksum: package
                .get("checksum")
                .and_then(toml::Value::as_str)
                .map(str::to_owned),
        });
    }
    Ok(locked)
}

fn lock_text(package: &toml::Value, key: &str) -> Result<String, IsolationError> {
    package
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            IsolationError::new(
                "cargo-isolation::lockfile-invalid",
                format!("external package lacks {key}"),
            )
        })
}

fn publish_packages(
    layout: &Layout,
    vendor: &Path,
    locked: &[LockedPackage],
) -> Result<Vec<PackageRecord>, IsolationError> {
    let mut directories = read_directory(vendor)?;
    directories.sort_by(|left, right| {
        left.file_name()
            .as_bytes()
            .cmp(right.file_name().as_bytes())
    });
    let mut records = Vec::with_capacity(directories.len());
    for directory in directories {
        if !directory
            .file_type()
            .map_err(|error| {
                IsolationError::io("cargo-isolation::seed-invalid", &directory.path(), error)
            })?
            .is_dir()
        {
            return Err(IsolationError::new(
                "cargo-isolation::seed-invalid",
                format!(
                    "vendor root contains a non-directory: {:?}",
                    directory.path()
                ),
            ));
        }
        records.push(publish_package(layout, directory.path(), locked)?);
    }
    records.sort();
    if records.len() != locked.len() {
        return Err(IsolationError::new(
            "cargo-isolation::seed-invalid",
            format!(
                "cargo vendor published {} packages for {} external lock rows",
                records.len(),
                locked.len()
            ),
        ));
    }
    Ok(records)
}

fn publish_package(
    layout: &Layout,
    source: PathBuf,
    locked: &[LockedPackage],
) -> Result<PackageRecord, IsolationError> {
    let directory = normal_utf8_name(&source)?;
    let manifest: toml::Value = toml::from_str(
        &fs::read_to_string(source.join("Cargo.toml")).map_err(|error| {
            IsolationError::io("cargo-isolation::package-invalid", &source, error)
        })?,
    )
    .map_err(|error| IsolationError::new("cargo-isolation::package-invalid", error.to_string()))?;
    let package = manifest.get("package").ok_or_else(|| {
        IsolationError::new(
            "cargo-isolation::package-invalid",
            "vendored Cargo.toml lacks package",
        )
    })?;
    let name = lock_text(package, "name")?;
    let version = lock_text(package, "version")?;
    let checksums: CargoChecksum = read_json(&source.join(".cargo-checksum.json"))?;
    verify_checksum_files(&source, &checksums)?;
    let locked = match_locked_package(&name, &version, checksums.package.as_deref(), locked)?;
    let source_digest = tree_digest(&source)?;
    let identity = package_identity(locked, &source_digest)?;
    let package_temp = layout.packages.join(temporary_name("package"));
    fs::rename(&source, &package_temp).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::package-publication-failed",
            &source,
            error,
        )
    })?;
    seal_package_tree(&package_temp)?;
    let final_path = layout.packages.join(&identity);
    let _lock = acquire_lock(layout, &format!("package-{identity}"))?;
    if final_path.exists() {
        let observed = tree_digest(&final_path)?;
        if observed != source_digest {
            return Err(IsolationError::new(
                "cargo-isolation::package-digest-mismatch",
                format!("package {identity} expected {source_digest}, observed {observed}"),
            ));
        }
        remove_read_only_tree(&package_temp)?;
    } else {
        rename_noreplace(&package_temp, &final_path)?;
    }
    symlink(PathBuf::from("../../../packages").join(&identity), source).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::generation-link-failed",
            &final_path,
            error,
        )
    })?;
    Ok(PackageRecord {
        directory,
        identity,
        name,
        version,
        source: locked.source.clone(),
        checksum: locked.checksum.clone(),
        tree_digest: source_digest,
    })
}

fn match_locked_package<'a>(
    name: &str,
    version: &str,
    package_checksum: Option<&str>,
    locked: &'a [LockedPackage],
) -> Result<&'a LockedPackage, IsolationError> {
    let candidates = locked
        .iter()
        .filter(|package| package.name == name && package.version == version)
        .filter(|package| match package_checksum {
            Some(checksum) => package.checksum.as_deref() == Some(checksum),
            None => package.source.starts_with("git+") && package.checksum.is_none(),
        })
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [package] => Ok(*package),
        [] => Err(IsolationError::new(
            "cargo-isolation::package-lock-mismatch",
            format!("vendored {name} {version} is absent from the exact external lock rows"),
        )),
        _ => Err(IsolationError::new(
            "cargo-isolation::package-lock-ambiguous",
            format!("vendored {name} {version} matches more than one external lock row"),
        )),
    }
}

fn package_identity(package: &LockedPackage, tree_digest: &str) -> Result<String, IsolationError> {
    if let Some(checksum) = &package.checksum {
        if checksum.len() != 64 || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(IsolationError::new(
                "cargo-isolation::registry-checksum-invalid",
                format!("registry lock checksum is not 64 hexadecimal bytes: {checksum:?}"),
            ));
        }
        return Ok(format!("registry-{checksum}"));
    }
    let revision = package
        .source
        .rsplit_once('#')
        .map(|(_, revision)| revision)
        .filter(|revision| {
            matches!(revision.len(), 40 | 64)
                && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .ok_or_else(|| {
            IsolationError::new(
                "cargo-isolation::git-revision-unpinned",
                format!(
                    "Git lock source has no exact object identity: {:?}",
                    package.source
                ),
            )
        })?;
    Ok(format!("git-{revision}-{tree_digest}"))
}

fn verify_checksum_files(root: &Path, checksum: &CargoChecksum) -> Result<(), IsolationError> {
    let expected = checksum.files.keys().cloned().collect::<BTreeSet<_>>();
    let observed = package_file_set(root)?;
    if observed != expected {
        return Err(IsolationError::new(
            "cargo-isolation::package-file-set-mismatch",
            format!("{root:?}: expected {expected:?}, observed {observed:?}"),
        ));
    }
    for (relative, expected) in &checksum.files {
        let path = root.join(relative);
        let bytes = fs::read(&path).map_err(|error| {
            IsolationError::io(
                "cargo-isolation::package-checksum-unavailable",
                &path,
                error,
            )
        })?;
        let observed = sha256_hex(&bytes);
        if &observed != expected {
            return Err(IsolationError::new(
                "cargo-isolation::package-checksum-mismatch",
                format!("{path:?}: expected {expected}, observed {observed}"),
            ));
        }
    }
    Ok(())
}

fn package_file_set(root: &Path) -> Result<BTreeSet<String>, IsolationError> {
    let mut files = BTreeSet::new();
    collect_package_files(root, root, &mut files)?;
    Ok(files)
}

fn collect_package_files(
    root: &Path,
    directory: &Path,
    files: &mut BTreeSet<String>,
) -> Result<(), IsolationError> {
    for entry in read_directory(directory)? {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            IsolationError::io("cargo-isolation::package-unreadable", &path, error)
        })?;
        if file_type.is_dir() {
            collect_package_files(root, &path, files)?;
        } else if file_type.is_file() {
            let relative = path.strip_prefix(root).map_err(|error| {
                IsolationError::new("cargo-isolation::package-path-invalid", error.to_string())
            })?;
            if relative != Path::new(".cargo-checksum.json") {
                files.insert(path_text(relative)?);
            }
        } else {
            return Err(IsolationError::new(
                "cargo-isolation::package-file-type-invalid",
                format!("vendored package contains unsupported entry: {path:?}"),
            ));
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};

    format!("{:x}", Sha256::digest(bytes))
}

fn rebind_source_config(
    mut config: toml::Value,
    staged: &Path,
    final_vendor: &Path,
) -> Result<toml::Value, IsolationError> {
    let sources = config
        .get_mut("source")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| {
            IsolationError::new(
                "cargo-isolation::seed-output-invalid",
                "cargo vendor emitted no source table",
            )
        })?;
    let mut rebound = false;
    for (_, source) in sources.iter_mut() {
        let Some(table) = source.as_table_mut() else {
            continue;
        };
        if table.get("directory").and_then(toml::Value::as_str) == staged.to_str() {
            table.insert(
                "directory".to_owned(),
                toml::Value::String(path_text(final_vendor)?),
            );
            rebound = true;
        }
    }
    if !rebound {
        return Err(IsolationError::new(
            "cargo-isolation::seed-output-invalid",
            "cargo vendor output does not name its exact staged directory",
        ));
    }
    Ok(config)
}

fn ensure_home(
    layout: &Layout,
    worktree: &Path,
    lane: Option<CanonicalUuid>,
    generation: &GenerationManifest,
) -> Result<PathBuf, IsolationError> {
    let owner = HomeOwner {
        schema: SCHEMA,
        worktree: encoded_path(worktree),
        lane: lane.map(|lane| lane.to_string()),
    };
    let home_id = home_identity(&owner);
    let _lock = acquire_lock(layout, &format!("home-{home_id}"))?;
    let lock_path = worktree.join("Cargo.lock");
    let current_lock = fs::read(&lock_path).map_err(|error| {
        IsolationError::io("cargo-isolation::lockfile-unavailable", &lock_path, error)
    })?;
    let current_digest = digest(b"spice-cargo-lock-v2", &current_lock);
    if current_digest != generation.lock_digest {
        return Err(IsolationError::new(
            "cargo-isolation::lockfile-changed",
            "Cargo.lock changed while its generation was preparing; rerun the original command through `spice dev cargo -- ...`",
        ));
    }
    let home = layout.homes.join(home_id);
    let existing = match fs::symlink_metadata(&home) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(IsolationError::io(
                "cargo-isolation::home-unavailable",
                &home,
                error,
            ));
        }
    };
    let config = home_config(generation)?;
    if existing {
        let previous = home_generation(&home, layout)?;
        validate_generation(layout, &previous)?;
        let owner_path = home.join(OWNER);
        let observed: HomeOwner = read_json(&owner_path)?;
        if observed != owner {
            return Err(IsolationError::new(
                "cargo-isolation::home-owner-mismatch",
                format!("expected {owner:?}, observed {observed:?}"),
            ));
        }
        publish_bytes(&home.join(CONFIG), config.as_bytes())?;
    } else {
        let temporary = TemporaryDirectory::create(&layout.homes, "home")?;
        publish_json(&temporary.path().join(OWNER), &owner)?;
        publish_bytes(&temporary.path().join(CONFIG), config.as_bytes())?;
        temporary.publish(&home)?;
    }
    Ok(home)
}

fn home_config(generation: &GenerationManifest) -> Result<String, IsolationError> {
    let mut document = generation.source_config.clone();
    let table = document.as_table_mut().ok_or_else(|| {
        IsolationError::new(
            "cargo-isolation::generation-invalid",
            "source config is not a table",
        )
    })?;
    let mut net = toml::Table::new();
    net.insert("offline".to_owned(), toml::Value::Boolean(true));
    table.insert("net".to_owned(), toml::Value::Table(net));
    let mut cache = toml::Table::new();
    cache.insert(
        "auto-clean-frequency".to_owned(),
        toml::Value::String("never".to_owned()),
    );
    table.insert("cache".to_owned(), toml::Value::Table(cache));
    toml::to_string(&document).map_err(|error| {
        IsolationError::new("cargo-isolation::home-config-invalid", error.to_string())
    })
}

fn home_generation(home: &Path, layout: &Layout) -> Result<String, IsolationError> {
    require_mode(home, 0o700)?;
    require_mode(&home.join(OWNER), 0o600)?;
    require_mode(&home.join(CONFIG), 0o600)?;
    let owner: HomeOwner = read_json(&home.join(OWNER))?;
    if owner.schema != SCHEMA {
        return Err(IsolationError::new(
            "cargo-isolation::home-owner-mismatch",
            format!("home carries schema {} instead of {SCHEMA}", owner.schema),
        ));
    }
    let expected = digest(
        b"spice-cargo-home-v1",
        &serde_json::to_vec(&owner).expect("home owner is serializable"),
    );
    if home.file_name().and_then(OsStr::to_str) != Some(expected.as_str()) {
        return Err(IsolationError::new(
            "cargo-isolation::home-owner-mismatch",
            format!("home path does not match its owner identity: {home:?}"),
        ));
    }
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(home.join(CONFIG)).map_err(|error| {
            IsolationError::io("cargo-isolation::home-config-invalid", home, error)
        })?)
        .map_err(|error| {
            IsolationError::new("cargo-isolation::home-config-invalid", error.to_string())
        })?;
    let directory = config
        .get("source")
        .and_then(|source| source.get("vendored-sources"))
        .and_then(|source| source.get("directory"))
        .and_then(toml::Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| {
            IsolationError::new(
                "cargo-isolation::home-config-invalid",
                "home config has no vendored-sources directory",
            )
        })?;
    let generation = directory
        .parent()
        .filter(|parent| parent.parent() == Some(layout.generations.as_path()))
        .and_then(Path::file_name)
        .and_then(OsStr::to_str)
        .ok_or_else(|| {
            IsolationError::new(
                "cargo-isolation::home-config-invalid",
                format!("source directory escapes generation store: {directory:?}"),
            )
        })?;
    Ok(generation.to_owned())
}

fn validate_generation(
    layout: &Layout,
    generation: &str,
) -> Result<GenerationManifest, IsolationError> {
    let root = layout.generations.join(generation);
    require_mode(&root, 0o500)?;
    require_mode(&root.join("vendor"), 0o500)?;
    require_mode(&root.join(MANIFEST), 0o400)?;
    let manifest: GenerationManifest = read_json(&root.join(MANIFEST))?;
    if manifest.schema != SCHEMA || manifest.generation != generation {
        return Err(IsolationError::new(
            "cargo-isolation::generation-invalid",
            format!("generation {generation} carries incompatible identity"),
        ));
    }
    for package in &manifest.packages {
        let link = root.join("vendor").join(&package.directory);
        let target = fs::read_link(&link).map_err(|error| {
            IsolationError::io("cargo-isolation::generation-link-invalid", &link, error)
        })?;
        let expected = PathBuf::from("../../../packages").join(&package.identity);
        let package_root = layout.packages.join(&package.identity);
        if target != expected || !package_root.is_dir() {
            return Err(IsolationError::new(
                "cargo-isolation::generation-link-invalid",
                format!("{link:?}: expected {expected:?}, observed {target:?}"),
            ));
        }
        require_mode(&package_root, 0o500)?;
    }
    Ok(manifest)
}

fn require_mode(path: &Path, expected: u32) -> Result<(), IsolationError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        IsolationError::io("cargo-isolation::permission-unavailable", path, error)
    })?;
    if metadata.file_type().is_symlink()
        || (!metadata.is_dir() && !metadata.is_file())
        || metadata.permissions().mode() & 0o777 != expected
    {
        return Err(IsolationError::new(
            "cargo-isolation::permission-mismatch",
            format!(
                "{path:?}: expected mode {expected:04o} on a real file or directory, observed {:04o}",
                metadata.permissions().mode() & 0o777,
            ),
        ));
    }
    Ok(())
}

fn require_directory_mode(path: &Path, expected: u32) -> Result<(), IsolationError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        IsolationError::io("cargo-isolation::directory-unavailable", path, error)
    })?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o777 != expected
    {
        return Err(IsolationError::new(
            "cargo-isolation::directory-permission-mismatch",
            format!(
                "{path:?}: expected a real mode {expected:04o} directory, observed mode {:04o}",
                metadata.permissions().mode() & 0o777,
            ),
        ));
    }
    Ok(())
}

fn tree_digest(root: &Path) -> Result<String, IsolationError> {
    let mut hasher = blake3::Hasher::new();
    hash_directory(root, Path::new(""), &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
}

fn hash_directory(
    root: &Path,
    relative: &Path,
    hasher: &mut blake3::Hasher,
) -> Result<(), IsolationError> {
    let directory = root.join(relative);
    let mut entries = read_directory(&directory)?;
    entries.sort_by(|left, right| {
        left.file_name()
            .as_bytes()
            .cmp(right.file_name().as_bytes())
    });
    for entry in entries {
        let name = entry.file_name();
        let child = relative.join(&name);
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
            IsolationError::io("cargo-isolation::package-unreadable", &entry.path(), error)
        })?;
        hasher.update(&(name.as_bytes().len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        if metadata.is_dir() {
            hasher.update(b"d");
            hash_directory(root, &child, hasher)?;
        } else if metadata.is_file() {
            hasher.update(b"f");
            let mut file = File::open(entry.path()).map_err(|error| {
                IsolationError::io("cargo-isolation::package-unreadable", &entry.path(), error)
            })?;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let count = file.read(&mut buffer).map_err(|error| {
                    IsolationError::io("cargo-isolation::package-unreadable", &entry.path(), error)
                })?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
            }
        } else {
            return Err(IsolationError::new(
                "cargo-isolation::package-file-type-invalid",
                format!(
                    "vendored package contains unsupported entry: {:?}",
                    entry.path()
                ),
            ));
        }
    }
    Ok(())
}

fn make_generation_read_only(root: &Path) -> Result<(), IsolationError> {
    change_tree_permissions(root, false)
}

fn seal_package_tree(root: &Path) -> Result<(), IsolationError> {
    let metadata = fs::symlink_metadata(root).map_err(|error| {
        IsolationError::io("cargo-isolation::permission-unavailable", root, error)
    })?;
    if metadata.file_type().is_symlink() {
        return Err(IsolationError::new(
            "cargo-isolation::file-type-invalid",
            format!("immutable package contains a link: {root:?}"),
        ));
    }
    if metadata.is_dir() {
        for entry in read_directory(root)? {
            seal_package_tree(&entry.path())?;
        }
        return set_mode(root, 0o500);
    }
    if metadata.is_file() {
        let executable = metadata.permissions().mode() & 0o111 != 0;
        return set_mode(root, if executable { 0o700 } else { 0o600 });
    }
    Err(IsolationError::new(
        "cargo-isolation::file-type-invalid",
        format!("unsupported store entry: {root:?}"),
    ))
}

fn change_tree_permissions(root: &Path, writable: bool) -> Result<(), IsolationError> {
    let metadata = fs::symlink_metadata(root).map_err(|error| {
        IsolationError::io("cargo-isolation::permission-unavailable", root, error)
    })?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in read_directory(root)? {
            change_tree_permissions(&entry.path(), writable)?;
        }
        set_mode(root, if writable { 0o700 } else { 0o500 })
    } else if metadata.is_file() {
        let executable = metadata.permissions().mode() & 0o111 != 0;
        set_mode(
            root,
            if writable {
                if executable { 0o700 } else { 0o600 }
            } else if executable {
                0o500
            } else {
                0o400
            },
        )
    } else {
        Err(IsolationError::new(
            "cargo-isolation::file-type-invalid",
            format!("unsupported store entry: {root:?}"),
        ))
    }
}

fn set_mode(path: &Path, mode: u32) -> Result<(), IsolationError> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| IsolationError::io("cargo-isolation::permission-unavailable", path, error))
}

fn create_private_directory(path: &Path) -> Result<(), IsolationError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            if metadata.permissions().mode() & 0o777 != 0o700 {
                return Err(IsolationError::new(
                    "cargo-isolation::directory-permission-mismatch",
                    format!("{path:?} is not mode 0700"),
                ));
            }
            Ok(())
        }
        Ok(_) => Err(IsolationError::new(
            "cargo-isolation::directory-type-mismatch",
            format!("{path:?} is not an owned directory"),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(path) {
                Ok(()) => create_private_directory(path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    create_private_directory(path)
                }
                Err(error) => Err(IsolationError::io(
                    "cargo-isolation::directory-unavailable",
                    path,
                    error,
                )),
            }
        }
        Err(error) => Err(IsolationError::io(
            "cargo-isolation::directory-unavailable",
            path,
            error,
        )),
    }
}

fn acquire_lock(layout: &Layout, name: &str) -> Result<sill::Possession, IsolationError> {
    let name = LockName::new(name)
        .map_err(|error| IsolationError::new("cargo-isolation::lock-invalid", error.to_string()))?;
    let path = layout.locks.join(format!("{}.lock", name.as_str()));
    let lock = LockFile::new(name, path);
    let started = std::time::Instant::now();
    loop {
        match lock.try_acquire().map_err(|error| {
            IsolationError::io("cargo-isolation::lock-unavailable", lock.path(), error)
        })? {
            Acquisition::Acquired(possession) => return Ok(possession),
            Acquisition::Contended if started.elapsed() < LOCK_WAIT => thread::sleep(LOCK_POLL),
            Acquisition::Contended => {
                return Err(IsolationError::new(
                    "cargo-isolation::lock-deadline-elapsed",
                    format!("timed out waiting for {}", lock.name().as_str()),
                ));
            }
        }
    }
}

fn publish_json(path: &Path, value: &impl Serialize) -> Result<(), IsolationError> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        IsolationError::new("cargo-isolation::record-invalid", error.to_string())
    })?;
    publish_bytes(path, &bytes)
}

fn publish_bytes(path: &Path, bytes: &[u8]) -> Result<(), IsolationError> {
    let destination = atomic::Destination::new(path).map_err(|error| {
        IsolationError::new(
            "cargo-isolation::record-publication-failed",
            error.to_string(),
        )
    })?;
    destination
        .publish(|file| std::io::Write::write_all(file, bytes))
        .map_err(|error| {
            IsolationError::new(
                "cargo-isolation::record-publication-failed",
                error.to_string(),
            )
        })?;
    set_mode(path, 0o600)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, IsolationError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| IsolationError::io("cargo-isolation::record-unavailable", path, error))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(IsolationError::new(
            "cargo-isolation::record-type-mismatch",
            format!("{path:?} is not a regular file"),
        ));
    }
    serde_json::from_slice(
        &fs::read(path).map_err(|error| {
            IsolationError::io("cargo-isolation::record-unavailable", path, error)
        })?,
    )
    .map_err(|error| IsolationError::new("cargo-isolation::record-invalid", error.to_string()))
}

fn read_directory(path: &Path) -> Result<Vec<fs::DirEntry>, IsolationError> {
    fs::read_dir(path)
        .map_err(|error| IsolationError::io("cargo-isolation::directory-unavailable", path, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| IsolationError::io("cargo-isolation::directory-unavailable", path, error))
}

fn normal_utf8_name(path: &Path) -> Result<String, IsolationError> {
    let name = path.file_name().and_then(OsStr::to_str).ok_or_else(|| {
        IsolationError::new(
            "cargo-isolation::package-name-invalid",
            format!("vendored package name is not UTF-8: {path:?}"),
        )
    })?;
    if name.is_empty()
        || matches!(name, "." | "..")
        || name
            .as_bytes()
            .iter()
            .any(|byte| matches!(byte, b'/' | b'\\'))
    {
        return Err(IsolationError::new(
            "cargo-isolation::package-name-invalid",
            format!("vendored package name is not one component: {name:?}"),
        ));
    }
    Ok(name.to_owned())
}

fn path_text(path: &Path) -> Result<String, IsolationError> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        IsolationError::new(
            "cargo-isolation::store-path-non-utf8",
            format!("Cargo configuration cannot encode store path {path:?}"),
        )
    })
}

pub(super) fn encoded_path(path: &Path) -> String {
    path.as_os_str()
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn digest(domain: &[u8], bytes: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(&[0]);
    hasher.update(bytes);
    hasher.finalize().to_hex().to_string()
}

fn generation_digest(lock_bytes: &[u8]) -> String {
    digest(b"spice-cargo-generation-v2", lock_bytes)
}

fn temporary_name(label: &str) -> OsString {
    OsString::from(format!(
        "{TEMPORARY_PREFIX}{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ))
}

struct TemporaryDirectory {
    path: PathBuf,
    armed: bool,
}

impl TemporaryDirectory {
    fn create(parent: &Path, label: &str) -> Result<Self, IsolationError> {
        let path = parent.join(temporary_name(label));
        fs::create_dir(&path).map_err(|error| {
            IsolationError::io("cargo-isolation::temporary-unavailable", &path, error)
        })?;
        set_mode(&path, 0o700)?;
        Ok(Self { path, armed: true })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn publish(mut self, final_path: &Path) -> Result<(), IsolationError> {
        rename_noreplace(&self.path, final_path)?;
        self.armed = false;
        Ok(())
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        if self.armed {
            let _ = remove_read_only_tree(&self.path);
        }
    }
}

fn rename_noreplace(source: &Path, destination: &Path) -> Result<(), IsolationError> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        source,
        rustix::fs::CWD,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(|error| {
        IsolationError::new(
            "cargo-isolation::atomic-publication-failed",
            format!("{source:?} -> {destination:?}: {error}"),
        )
    })
}

fn remove_read_only_tree(path: &Path) -> Result<(), IsolationError> {
    if !path.exists() {
        return Ok(());
    }
    change_tree_permissions(path, true)?;
    fs::remove_dir_all(path)
        .map_err(|error| IsolationError::io("cargo-isolation::cleanup-failed", path, error))
}

mod maintenance;
mod pins;
mod seed;

pub(super) use pins::GenerationPin;

pub(super) fn inspect(host: &Host) -> Result<Inspection, IsolationError> {
    maintenance::inspect(host)
}

pub(super) fn cleanup(host: &Host) -> Result<CleanupReport, IsolationError> {
    maintenance::cleanup(host)
}

#[cfg(test)]
pub(super) fn cleanup_with_grace(
    host: &Host,
    grace: Duration,
) -> Result<CleanupReport, IsolationError> {
    maintenance::cleanup_with_grace(host, grace)
}
