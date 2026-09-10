//! Verified local release-package acquisition for the installed appliance.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use spice::repository::Repository;
use wire::manifest::{CANVAS_APP_FILES, ReleaseTarget};

const MAX_PACKAGE_ENTRIES: usize = 64;
const MAX_MEMBER_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PACKAGE_BYTES: u64 = 1024 * 1024 * 1024;
const PACKAGE_DOCUMENT_PATH: &str = "release/package.json";

pub(crate) fn run(
    repository: &Repository,
    source: &Path,
    apply: Option<&str>,
    json: bool,
) -> Result<ExitStatus, Error> {
    let acquired = acquire(source)?;
    delegate(repository.worktree_root(), &acquired, apply, json)
}

fn delegate(
    worktree: &Path,
    acquired: &AcquiredPackage,
    apply: Option<&str>,
    json: bool,
) -> Result<ExitStatus, Error> {
    let staged = StagedPackage::new(acquired)?;
    let mut command = Command::new(staged.executable());
    command.current_dir(worktree).arg("install");
    if let Some(digest) = apply {
        command.arg(format!("--apply={digest}"));
    }
    if json {
        command.arg("--json");
    }
    command.status().map_err(|source| {
        Error::environment(
            "launch verified package executable",
            staged.executable(),
            source,
        )
    })
}

fn acquire(root: &Path) -> Result<AcquiredPackage, Error> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::input(format!(
                "package root does not exist: {}",
                root.display()
            )));
        }
        Err(source) => {
            return Err(Error::environment("inspect package root", root, source));
        }
    };
    if !metadata.file_type().is_dir() {
        return Err(Error::input(format!(
            "package root is not an unlinked directory: {}",
            root.display()
        )));
    }
    let mut members = Vec::new();
    let mut bytes = 0_u64;
    let mut entries = 0_usize;
    collect(root, root, &mut members, &mut bytes, &mut entries)?;
    members.sort_by(|left, right| left.path().cmp(right.path()));
    let admitted =
        proof::admit_package_tree(&members).map_err(|error| Error::input(error.to_string()))?;
    let candidate = admitted.document().product_version.as_str();
    let current = dist::current_product_version()
        .expect("the compiled distribution product version is canonical");
    let current = current.as_str();
    if !newer(candidate, current) {
        return Err(Error::input(format!(
            "package version {candidate} is not newer than installed appliance version {current}"
        )));
    }
    let target = current_target()?;
    let selected = admitted
        .document()
        .targets
        .iter()
        .find(|candidate| candidate.target == target)
        .ok_or_else(|| Error::input(format!("package has no executable for {target}")))?;
    let acquired = AcquiredPackage {
        executable_path: selected.executable_path.clone(),
        payload_path: format!("targets/{}/payload.json", target.as_str()),
        members,
    };
    acquired.member(&acquired.executable_path)?;
    acquired.member(&acquired.payload_path)?;
    Ok(acquired)
}

#[derive(Debug)]
struct AcquiredPackage {
    executable_path: String,
    payload_path: String,
    members: Vec<proof::PackageMember>,
}

impl AcquiredPackage {
    fn member(&self, path: &str) -> Result<&proof::PackageMember, Error> {
        self.members
            .iter()
            .find(|member| member.path() == path)
            .ok_or_else(|| Error::input(format!("verified package omits staged member {path}")))
    }
}

fn newer(candidate: &str, current: &str) -> bool {
    let (candidate_core, candidate_prerelease) = version_parts(candidate);
    let (current_core, current_prerelease) = version_parts(current);
    candidate_core
        .cmp(&current_core)
        .then_with(|| match (candidate_prerelease, current_prerelease) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(candidate), Some(current)) => candidate.cmp(&current),
        })
        .is_gt()
}

fn version_parts(version: &str) -> (Vec<Number<'_>>, Option<Vec<Identifier<'_>>>) {
    let without_build = version
        .split_once('+')
        .map_or(version, |(version, _)| version);
    let (core, prerelease) = without_build
        .split_once('-')
        .map_or((without_build, None), |(core, prerelease)| {
            (core, Some(prerelease))
        });
    (
        core.split('.').map(Number).collect(),
        prerelease.map(|value| value.split('.').map(Identifier::new).collect()),
    )
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Number<'a>(&'a str);

impl Ord for Number<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.cmp(other.0))
    }
}

impl PartialOrd for Number<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Identifier<'a> {
    Numeric(Number<'a>),
    Text(&'a str),
}

impl<'a> Identifier<'a> {
    fn new(value: &'a str) -> Self {
        if value.bytes().all(|byte| byte.is_ascii_digit()) {
            Self::Numeric(Number(value))
        } else {
            Self::Text(value)
        }
    }
}

impl Ord for Identifier<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Numeric(left), Self::Numeric(right)) => left.cmp(right),
            (Self::Numeric(_), Self::Text(_)) => std::cmp::Ordering::Less,
            (Self::Text(_), Self::Numeric(_)) => std::cmp::Ordering::Greater,
            (Self::Text(left), Self::Text(right)) => left.cmp(right),
        }
    }
}

impl PartialOrd for Identifier<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn collect(
    root: &Path,
    directory: &Path,
    members: &mut Vec<proof::PackageMember>,
    total: &mut u64,
    visited: &mut usize,
) -> Result<(), Error> {
    let entries = fs::read_dir(directory)
        .map_err(|source| Error::environment("read package directory", directory, source))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| Error::environment("read package entry", directory, source))?;
    for entry in entries {
        if *visited == MAX_PACKAGE_ENTRIES {
            return Err(Error::input(format!(
                "package has more than {MAX_PACKAGE_ENTRIES} filesystem entries"
            )));
        }
        *visited += 1;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|source| Error::environment("inspect package member", &path, source))?;
        if metadata.file_type().is_dir() {
            collect(root, &path, members, total, visited)?;
            continue;
        }
        if !metadata.file_type().is_file() {
            return Err(Error::input(format!(
                "package member is not an unlinked regular file: {}",
                path.display()
            )));
        }
        if metadata.len() > MAX_MEMBER_BYTES {
            return Err(Error::input(format!(
                "package member exceeds {MAX_MEMBER_BYTES} bytes: {}",
                path.display()
            )));
        }
        *total = total
            .checked_add(metadata.len())
            .filter(|total| *total <= MAX_PACKAGE_BYTES)
            .ok_or_else(|| {
                Error::input(format!(
                    "package regular files exceed {MAX_PACKAGE_BYTES} bytes"
                ))
            })?;
        let relative = path
            .strip_prefix(root)
            .expect("a traversed package member remains below its root");
        let relative = relative.to_str().ok_or_else(|| {
            Error::input(format!("package member path is not UTF-8: {relative:?}"))
        })?;
        let mut file = File::open(&path)
            .map_err(|source| Error::environment("open package member", &path, source))?;
        let mut content = Vec::with_capacity(
            usize::try_from(metadata.len()).expect("bounded member length fits usize"),
        );
        file.read_to_end(&mut content)
            .map_err(|source| Error::environment("read package member", &path, source))?;
        if content.len() as u64 != metadata.len() {
            return Err(Error::input(format!(
                "package member changed while read: {}",
                path.display()
            )));
        }
        members.push(proof::PackageMember::new(
            relative.replace(std::path::MAIN_SEPARATOR, "/"),
            metadata.permissions().mode() & 0o7777,
            content,
        ));
    }
    Ok(())
}

fn current_target() -> Result<ReleaseTarget, Error> {
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    {
        Ok(ReleaseTarget::Aarch64AppleDarwin)
    }
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    {
        Ok(ReleaseTarget::X86_64UnknownLinuxMusl)
    }
    #[cfg(not(any(
        all(target_arch = "aarch64", target_os = "macos"),
        all(target_arch = "x86_64", target_os = "linux")
    )))]
    {
        Err(Error::input(format!(
            "this platform is not a supported release target: {}-{}",
            std::env::consts::ARCH,
            std::env::consts::OS
        )))
    }
}

struct StagedPackage {
    root: PathBuf,
    executable: PathBuf,
}

impl StagedPackage {
    fn new(acquired: &AcquiredPackage) -> Result<Self, Error> {
        for _ in 0..32 {
            let root = std::env::temp_dir().join(format!(
                "spice-package-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&root) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(Error::environment(
                        "create verified package staging root",
                        &root,
                        source,
                    ));
                }
            }
            let staged = Self {
                executable: root.join(&acquired.executable_path),
                root,
            };
            staged.publish(acquired.member(PACKAGE_DOCUMENT_PATH)?)?;
            for file in CANVAS_APP_FILES {
                staged.publish(acquired.member(&format!("app/{file}"))?)?;
            }
            staged.publish(acquired.member(&acquired.payload_path)?)?;
            staged.publish(acquired.member(&acquired.executable_path)?)?;
            return Ok(staged);
        }
        Err(Error::input(
            "could not allocate a unique verified package staging root",
        ))
    }

    fn executable(&self) -> &Path {
        &self.executable
    }

    fn publish(&self, member: &proof::PackageMember) -> Result<(), Error> {
        let path = self.root.join(member.path());
        fs::create_dir_all(path.parent().expect("a package member has a parent")).map_err(
            |source| Error::environment("create package staging directory", &path, source),
        )?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(member.mode())
            .open(&path)
            .map_err(|source| Error::environment("create staged package member", &path, source))?;
        file.write_all(member.bytes())
            .map_err(|source| Error::environment("write staged package member", &path, source))?;
        file.set_permissions(fs::Permissions::from_mode(member.mode()))
            .map_err(|source| Error::environment("mode staged package member", &path, source))?;
        file.sync_all()
            .map_err(|source| Error::environment("sync staged package member", &path, source))
    }
}

impl Drop for StagedPackage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Debug)]
pub(crate) enum Error {
    Input(String),
    Environment {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
}

impl Error {
    pub(crate) fn input(detail: impl Into<String>) -> Self {
        Self::Input(detail.into())
    }

    fn environment(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self::Environment {
            operation,
            path: path.to_path_buf(),
            source,
        }
    }

    pub(crate) const fn is_environment(&self) -> bool {
        matches!(self, Self::Environment { .. })
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input(detail) => formatter.write_str(detail),
            Self::Environment {
                operation,
                path,
                source,
            } => write!(formatter, "{operation} at {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;
    use wire::manifest::{
        BuildPins, CANVAS_APP_FILES, InventoryEntry, PROOF_RECEIPT_VERSION, ProductVersion,
        ProofEnvironment, ProofReceipt, ProofStanding, RELEASE_MANIFEST_VERSION, ReleaseManifest,
        ReleaseManifestTarget, TARGET_PAYLOAD_VERSION, TargetPayload,
    };
    use wire::{Digest, Domain};

    #[test]
    fn package_root_must_be_an_unlinked_directory() {
        let path = std::env::temp_dir().join(format!(
            "spice-package-root-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::write(&path, b"not a package directory").expect("fixture file writes");
        let error = acquire(&path).expect_err("regular file cannot be a package root");
        fs::remove_file(&path).expect("fixture file removes");
        assert!(matches!(error, Error::Input(_)));
        assert!(error.to_string().contains("unlinked directory"));
    }

    #[test]
    fn verified_generation_receives_only_the_existing_install_transaction() {
        let root = std::env::temp_dir().join(format!(
            "spice-package-delegate-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&root).expect("delegate fixture root creates");
        let observed = root.join("arguments");
        let package = root.join("package");
        fs::create_dir(&package).expect("package fixture root creates");
        let tree = complete_package(&observed);
        write_package(&package, &tree);
        let acquired = acquire(&package).expect("complete package acquires");
        let staged = StagedPackage::new(&acquired).expect("selected package stages");
        let staged_root = staged.root.clone();
        assert!(staged.executable().is_file());
        assert_eq!(
            fs::metadata(&staged_root).unwrap().permissions().mode() & 0o777,
            0o700,
        );
        assert_eq!(
            fs::metadata(staged.executable())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            acquired.member(&acquired.executable_path).unwrap().mode(),
        );
        assert!(staged_root.join(PACKAGE_DOCUMENT_PATH).is_file());
        assert!(staged_root.join(&acquired.payload_path).is_file());
        for file in CANVAS_APP_FILES {
            assert_eq!(
                fs::read(staged_root.join("app").join(file)).unwrap(),
                acquired.member(&format!("app/{file}")).unwrap().bytes(),
            );
        }
        drop(staged);
        assert!(!staged_root.exists());
        fs::remove_dir_all(&package).expect("remove admitted source package");

        let status =
            delegate(&root, &acquired, Some("abc123"), true).expect("verified generation launches");
        assert!(status.success());
        assert_eq!(
            fs::read_to_string(&observed).expect("delegated argv reads"),
            "install --apply=abc123 --json\n"
        );
        fs::remove_dir_all(&root).expect("delegate fixture root removes");
    }

    #[test]
    fn package_version_must_advance_semantic_precedence() {
        for (candidate, current) in [
            ("1.2.4", "1.2.3"),
            ("1.10.0", "1.9.9"),
            ("2.0.0", "2.0.0-rc.1"),
            ("2.0.0-rc.10", "2.0.0-rc.2"),
        ] {
            assert!(
                newer(candidate, current),
                "{candidate} must advance {current}"
            );
        }
        for (candidate, current) in [
            ("1.2.3", "1.2.3"),
            ("1.2.3+new", "1.2.3+old"),
            ("1.9.9", "1.10.0"),
            ("2.0.0-rc.1", "2.0.0"),
        ] {
            assert!(
                !newer(candidate, current),
                "{candidate} must not advance {current}"
            );
        }
    }

    #[test]
    fn current_complete_package_acquires_and_delegates_its_platform_executable() {
        let root = std::env::temp_dir().join(format!(
            "spice-complete-package-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&root).expect("package fixture root creates");
        let marker = root.join("selected-executable-ran");
        let tree = complete_package(&marker);
        write_package(&root, &tree);

        let selected = acquire(&root).expect("complete package acquires");
        let status =
            delegate(&root, &selected, None, false).expect("selected executable delegates");
        assert!(status.success());
        assert_eq!(
            fs::read_to_string(&marker).expect("delegate marker reads"),
            "install\n"
        );
        fs::remove_dir_all(&root).expect("package fixture removes");
    }

    fn write_package(root: &Path, tree: &proof::PackageTree) {
        for member in tree.members() {
            let path = root.join(member.path());
            fs::create_dir_all(path.parent().expect("package member has a parent"))
                .expect("package parent creates");
            fs::write(&path, member.bytes()).expect("package member writes");
            fs::set_permissions(&path, fs::Permissions::from_mode(member.mode()))
                .expect("package member mode writes");
        }
    }

    fn complete_package(marker: &Path) -> proof::PackageTree {
        const TARGETS: [ReleaseTarget; 2] = [
            ReleaseTarget::Aarch64AppleDarwin,
            ReleaseTarget::X86_64UnknownLinuxMusl,
        ];
        let source = Digest::compute(Domain::DistSourceCommit, b"source");
        let app_identity = Digest::compute(Domain::DistArtifact, b"canvas generation");
        let mut app = CANVAS_APP_FILES
            .iter()
            .map(|file| ((*file).to_owned(), file.as_bytes().to_vec()))
            .collect::<Vec<_>>();
        app.sort_by(|left, right| left.0.cmp(&right.0));
        let battery = Digest::compute(Domain::ProofBattery, b"battery");
        let environment =
            ProofEnvironment::native(Digest::compute(Domain::ProofNativeRunner, b"runner"));
        let mut executables = Vec::new();
        let mut payloads = Vec::new();
        let mut proofs = Vec::new();
        let mut manifest_targets = Vec::new();

        for target in TARGETS {
            let (executable, payload, proof, sealed) = target_evidence(
                target,
                marker,
                &source,
                &app_identity,
                &app,
                &battery,
                &environment,
            );
            executables.push(executable);
            payloads.push(payload);
            proofs.push(proof);
            manifest_targets.push(sealed);
        }
        let manifest = wire::encode(&ReleaseManifest {
            version: RELEASE_MANIFEST_VERSION,
            source,
            targets: manifest_targets,
        })
        .expect("manifest encodes");
        let current = dist::current_product_version().expect("current product version");
        let major = current
            .as_str()
            .split('.')
            .next()
            .expect("canonical version has a major component");
        let version = ProductVersion::parse(&format!("1{major}.0.0"))
            .expect("synthetic package advances the current major");
        let notes = b"Release notes\n";
        let notes_identity = Digest::compute(Domain::DistReleaseNotes, b"notes evidence");
        proof::build_package(proof::PackageInput::new(
            &version,
            notes_identity,
            notes,
            &manifest,
            app.iter()
                .map(|(file, bytes)| proof::PackageAppInput::new(file, bytes))
                .collect(),
            TARGETS
                .into_iter()
                .enumerate()
                .map(|(index, target)| {
                    proof::PackageTargetInput::new(
                        target,
                        &executables[index],
                        &payloads[index],
                        &proofs[index],
                    )
                })
                .collect(),
        ))
        .expect("complete package builds")
    }

    #[allow(clippy::too_many_arguments)]
    fn target_evidence(
        target: ReleaseTarget,
        marker: &Path,
        source: &Digest,
        app_identity: &Digest,
        app: &[(String, Vec<u8>)],
        battery: &Digest,
        environment: &ProofEnvironment,
    ) -> (Vec<u8>, Vec<u8>, Vec<u8>, ReleaseManifestTarget) {
        let executable = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\n",
            marker.display()
        )
        .into_bytes();
        let native = Digest::compute(Domain::DistArtifact, &executable);
        let pins = BuildPins {
            rust: "rustc 1.97.1".to_owned(),
            wasm_target: "wasm32-unknown-unknown".to_owned(),
            trunk: "trunk 0.21.0".to_owned(),
            cargo_mutants: "cargo-mutants 27.0.0".to_owned(),
            proof_container: format!("spice/proof@sha256:{}", "1".repeat(64)),
        };
        let mut inventory = app
            .iter()
            .map(|(file, bytes)| InventoryEntry {
                path: format!("app/{file}"),
                mode: 0o644,
                bytes: bytes.len() as u64,
                digest: Digest::compute(Domain::DistArtifact, bytes),
            })
            .collect::<Vec<_>>();
        inventory.push(InventoryEntry {
            path: "bin/spice".to_owned(),
            mode: 0o755,
            bytes: executable.len() as u64,
            digest: native.clone(),
        });
        inventory.sort_by(|left, right| left.path.cmp(&right.path));
        let payload = TargetPayload {
            version: TARGET_PAYLOAD_VERSION,
            target,
            source: source.clone(),
            app: app_identity.clone(),
            native,
            pins_digest: Digest::compute(
                Domain::DistPins,
                &wire::encode(&pins).expect("pins encode"),
            ),
            inventory_digest: Digest::compute(
                Domain::DistInventory,
                &wire::encode(&inventory).expect("inventory encodes"),
            ),
            pins,
            inventory,
        };
        let payload = wire::encode(&payload).expect("payload encodes");
        let payload_identity = Digest::compute(Domain::DistTargetPayload, &payload);
        let replay = Digest::compute(Domain::ProofReplayV3, target.as_str().as_bytes());
        let proof = ProofReceipt {
            version: PROOF_RECEIPT_VERSION,
            target,
            payload: payload_identity.clone(),
            battery: battery.clone(),
            replay: replay.clone(),
            environment: environment.clone(),
            standing: ProofStanding::Clean,
        };
        let proof = wire::encode(&proof).expect("proof receipt encodes");
        let sealed = ReleaseManifestTarget {
            target,
            payload: payload_identity,
            receipt: Digest::compute(Domain::ProofReceiptV3, &proof),
            battery: battery.clone(),
            replay,
            environment: environment.clone(),
            standing: ProofStanding::Clean,
        };
        (executable, payload, proof, sealed)
    }
}
