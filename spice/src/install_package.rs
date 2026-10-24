//! Verified local release-package acquisition for the installed appliance.

use std::fmt;
use std::fs::{self, File};
use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use spice::install::InstallPreview;
use spice::repository::Repository;
use wire::Outcome;
use wire::manifest::ReleaseTarget;

const MAX_PACKAGE_ENTRIES: usize = 64 + wire::manifest::renderer::MAX_FILES;
const MAX_MEMBER_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PACKAGE_BYTES: u64 = 1024 * 1024 * 1024;
const PACKAGE_DOCUMENT_PATH: &str = "release/package.json";

/// Hand the request to the verified package executable, whose own output and
/// exit status answer it.
pub(crate) fn run(
    repository: &Repository,
    source: &Path,
    apply: Option<&str>,
    json: bool,
) -> Result<ExitStatus, Error> {
    let acquired = acquire(source)?;
    let staged = StagedPackage::new(&acquired)?;
    delegate(&staged, repository.worktree_root(), apply, json)
        .status()
        .map_err(|source| launch_failure(&staged, source))
}

/// Ask the verified package executable for its typed install preview.
///
/// The staged executable alone derives and digests the plan; this front-end
/// only decodes its structured answer so the caller can render guidance that
/// names the original package source instead of the temporary staging.
pub(crate) fn preview(
    repository: &Repository,
    source: &Path,
) -> Result<Outcome<InstallPreview>, Error> {
    let acquired = acquire(source)?;
    let staged = StagedPackage::new(&acquired)?;
    let output = delegate(&staged, repository.worktree_root(), None, true)
        .output()
        .map_err(|source| launch_failure(&staged, source))?;
    answer(&output)
}

/// Decode the child's `--json` preview from the stream its outcome uses: a
/// reply on stdout, a refusal or fault on stderr with a failing status.
/// Anything the child said beside a reply, or an answer this appliance cannot
/// read, still reaches the person verbatim.
fn answer(output: &std::process::Output) -> Result<Outcome<InstallPreview>, Error> {
    let document = if output.status.success() {
        let _ = std::io::stderr().write_all(&output.stderr);
        &output.stdout
    } else {
        &output.stderr
    };
    wire::decode(document).map_err(|source| {
        if !output.status.success() {
            let _ = std::io::stderr().write_all(&output.stderr);
        }
        Error::Unreadable {
            status: output.status,
            source,
        }
    })
}

fn delegate(staged: &StagedPackage, worktree: &Path, apply: Option<&str>, json: bool) -> Command {
    let mut command = Command::new(staged.executable());
    command.current_dir(worktree).arg("install");
    if let Some(digest) = apply {
        command.arg(format!("--apply={digest}"));
    }
    if json {
        command.arg("--json");
    }
    command
}

fn launch_failure(staged: &StagedPackage, source: std::io::Error) -> Error {
    Error::environment(
        "launch verified package executable",
        staged.executable(),
        source,
    )
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
    let candidate = &admitted.document().product_version;
    let current = dist::current_product_version()
        .expect("the compiled distribution product version is canonical");
    if candidate.precedence(&current).is_le() {
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
    // Package admission already bound the optional debug member to the target
    // payload inventory, so the package document selects the exact staged path.
    let acquired = AcquiredPackage {
        executable_path: selected.executable_path.clone(),
        payload_path: format!("targets/{}/payload.json", target.as_str()),
        debug_path: selected.debug_path.clone(),
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
    debug_path: Option<String>,
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
        let file = File::open(&path)
            .map_err(|source| Error::environment("open package member", &path, source))?;
        members.push(
            proof::PackageMember::admit_file(
                relative.replace(std::path::MAIN_SEPARATOR, "/"),
                package_mode(&path)?,
                file,
                MAX_MEMBER_BYTES,
            )
            .map_err(|error| Error::input(error.to_string()))?,
        );
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
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    {
        Ok(ReleaseTarget::X86_64PcWindowsMsvc)
    }
    #[cfg(not(any(
        all(target_arch = "aarch64", target_os = "macos"),
        all(target_arch = "x86_64", target_os = "linux"),
        all(target_arch = "x86_64", target_os = "windows")
    )))]
    {
        Err(Error::input(format!(
            "this platform is not a supported release target: {}-{}",
            std::env::consts::ARCH,
            std::env::consts::OS
        )))
    }
}

fn package_mode(path: &Path) -> Result<u32, Error> {
    #[cfg(unix)]
    {
        fs::symlink_metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o7777)
            .map_err(|source| Error::environment("read package member mode", path, source))
    }
    #[cfg(windows)]
    {
        sill::authority::owner_mode(path)
            .map(|mode| if mode & 0o100 == 0 { 0o644 } else { 0o755 })
            .map_err(|source| Error::environment("read package member mode", path, source))
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
            match sill::authority::create_private_directory(&root) {
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
            staged.publish(acquired.member(&acquired.payload_path)?)?;
            staged.publish(acquired.member(&acquired.executable_path)?)?;
            if let Some(symbols_path) = &acquired.debug_path {
                staged.publish(acquired.member(symbols_path)?)?;
            }
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
        let parent = path.parent().expect("a package member has a parent");
        sill::authority::create_private_directories(parent).map_err(|source| {
            Error::environment("create package staging directory", parent, source)
        })?;
        let mut file = sill::authority::create_private_file(&path)
            .map_err(|source| Error::environment("create staged package member", &path, source))?;
        member
            .copy_to(&mut file)
            .map_err(|error| Error::input(error.to_string()))?;
        set_staged_mode(&file, member.mode())
            .map_err(|source| Error::environment("mode staged package member", &path, source))?;
        file.sync_all()
            .map_err(|source| Error::environment("sync staged package member", &path, source))
    }
}

fn set_staged_mode(file: &File, mode: u32) -> std::io::Result<()> {
    sill::authority::set_file_mode(file, mode & 0o700)
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
    /// The verified executable answered a preview this appliance cannot decode.
    Unreadable {
        status: ExitStatus,
        source: serde_json::Error,
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
            Self::Unreadable { status, source } => write!(
                formatter,
                "the verified package executable exited with {status} and its preview did not decode: {source}; rerun with --json to read its own answer",
            ),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;
    use wire::manifest::{
        BuildPins, InventoryEntry, MacSymbolMetadata, PROOF_RECEIPT_VERSION, ProductVersion,
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
    }

    #[test]
    fn verified_generation_stages_only_admitted_platform_assets() {
        let root = std::env::temp_dir().join(format!(
            "spice-package-delegate-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&root).expect("delegate fixture root creates");
        let package = root.join("package");
        fs::create_dir(&package).expect("package fixture root creates");
        let tree = complete_package();
        write_package(&package, &tree);
        let acquired = acquire(&package).expect("complete package acquires");
        let symbol_path = "targets/aarch64-apple-darwin/symbols/spice.symbols";
        assert_eq!(
            acquired.member(symbol_path).unwrap().digest(),
            &Digest::compute(Domain::DistArtifact, b"symbol fixture"),
        );
        let staged = StagedPackage::new(&acquired).expect("selected package stages");
        let staged_root = staged.root.clone();
        assert_eq!(
            fs::read(staged.executable()).expect("staged executable reads"),
            platform_executable(current_target().unwrap()),
        );
        assert_eq!(sill::authority::owner_mode(&staged_root).unwrap(), 0o700,);
        assert_eq!(
            sill::authority::owner_mode(staged.executable()).unwrap(),
            acquired.member(&acquired.executable_path).unwrap().mode() & 0o700,
        );
        assert!(staged_root.join(PACKAGE_DOCUMENT_PATH).is_file());
        assert!(staged_root.join(&acquired.payload_path).is_file());
        let target = current_target().unwrap();
        let debug_path = target
            .debug_path()
            .map(|path| format!("targets/{target}/{path}"));
        assert_eq!(acquired.debug_path, debug_path);
        assert_eq!(
            debug_path
                .as_deref()
                .map(|path| staged_root.join(path).is_file()),
            target.debug_path().map(|_| true),
        );
        if target == ReleaseTarget::Aarch64AppleDarwin {
            let symbol_path = debug_path.expect("Darwin debug path");
            assert_eq!(
                fs::read(staged_root.join(&symbol_path)).unwrap(),
                b"symbol fixture"
            );
            assert_eq!(
                sill::authority::owner_mode(&staged_root.join(&symbol_path)).unwrap(),
                0o600,
            );
        }

        drop(staged);
        assert!(!staged_root.exists());
        fs::remove_dir_all(&root).expect("delegate fixture root removes");
    }

    /// Each target's executable bytes, distinct so staging proves which it took.
    fn platform_executable(target: ReleaseTarget) -> Vec<u8> {
        format!("#!/bin/sh\n# {}\n", target.as_str()).into_bytes()
    }

    fn write_package(root: &Path, tree: &proof::PackageTree) {
        for member in tree.members() {
            let path = root.join(member.path());
            fs::create_dir_all(path.parent().expect("package member has a parent"))
                .expect("package parent creates");
            let mut file =
                sill::authority::create_private_file(&path).expect("package member creates");
            member.copy_to(&mut file).expect("package member writes");
            sill::authority::set_file_mode(&file, member.mode())
                .expect("package member mode writes");
        }
    }

    fn complete_package() -> proof::PackageTree {
        const TARGETS: [ReleaseTarget; 3] = [
            ReleaseTarget::Aarch64AppleDarwin,
            ReleaseTarget::X86_64UnknownLinuxMusl,
            ReleaseTarget::X86_64PcWindowsMsvc,
        ];
        let source = Digest::compute(Domain::DistSourceCommit, b"source");
        let app_identity = Digest::compute(Domain::DistArtifact, b"app generation");
        let battery = Digest::compute(Domain::ProofBattery, b"battery");
        let environment =
            ProofEnvironment::native(Digest::compute(Domain::ProofNativeRunner, b"runner"));
        let mut executables = Vec::new();
        let mut payloads = Vec::new();
        let mut proofs = Vec::new();
        let mut manifest_targets = Vec::new();

        for target in TARGETS {
            let (executable, payload, proof, sealed) =
                target_evidence(target, &source, &app_identity, &battery, &environment);
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
            inline("release/notes.md", 0o644, notes),
            inline("release/manifest.json", 0o644, &manifest),
            TARGETS
                .into_iter()
                .enumerate()
                .map(|(index, target)| {
                    proof::PackageTargetInput::new(
                        target,
                        inline(
                            &format!("targets/{target}/{}", target.executable_path()),
                            0o755,
                            &executables[index],
                        ),
                        inline(
                            &format!("targets/{target}/payload.json"),
                            0o644,
                            &payloads[index],
                        ),
                        inline(
                            &format!("targets/{target}/proof.json"),
                            0o644,
                            &proofs[index],
                        ),
                    )
                    .with_debug(target.debug_path().map(|path| {
                        inline(
                            &format!("targets/{target}/{path}"),
                            0o644,
                            if target == ReleaseTarget::Aarch64AppleDarwin {
                                b"symbol fixture".as_slice()
                            } else {
                                b"Windows PDB fixture".as_slice()
                            },
                        )
                    }))
                })
                .collect(),
        ))
        .expect("complete package builds")
    }

    fn inline(path: &str, mode: u32, bytes: impl AsRef<[u8]>) -> proof::PackageMember {
        proof::PackageMember::admit_inline(
            path,
            mode,
            bytes.as_ref(),
            proof::PACKAGE_DOCUMENT_LIMIT,
        )
        .expect("fixture member admits")
    }

    fn target_evidence(
        target: ReleaseTarget,
        source: &Digest,
        app_identity: &Digest,
        battery: &Digest,
        environment: &ProofEnvironment,
    ) -> (Vec<u8>, Vec<u8>, Vec<u8>, ReleaseManifestTarget) {
        let executable = platform_executable(target);
        let native = Digest::compute(Domain::DistArtifact, &executable);
        let pins = BuildPins {
            rust: "rustc 1.97.1".to_owned(),
            wasm_target: "wasm32-unknown-unknown".to_owned(),
            trunk: "trunk 0.21.0".to_owned(),
            cargo_mutants: "cargo-mutants 27.0.0".to_owned(),
            proof_container: format!("spice/proof@sha256:{}", "1".repeat(64)),
        };
        let mut inventory = vec![InventoryEntry {
            path: target.executable_path().to_owned(),
            mode: 0o755,
            bytes: executable.len() as u64,
            digest: native.clone(),
        }];
        let symbols = (target == ReleaseTarget::Aarch64AppleDarwin).then(|| {
            let metadata = MacSymbolMetadata {
                path: "symbols/spice.symbols".to_owned(),
                mode: 0o644,
                bytes: 14,
                digest: Digest::compute(Domain::DistArtifact, b"symbol fixture"),
                uuid: "11111111-2222-3333-4444-555555555555".to_owned(),
                executable: native.clone(),
            };
            inventory.push(InventoryEntry {
                path: metadata.path.clone(),
                mode: metadata.mode,
                bytes: metadata.bytes,
                digest: metadata.digest.clone(),
            });
            metadata
        });
        if target == ReleaseTarget::X86_64PcWindowsMsvc {
            inventory.push(InventoryEntry {
                path: target.debug_path().expect("Windows debug path").to_owned(),
                mode: 0o644,
                bytes: b"Windows PDB fixture".len() as u64,
                digest: Digest::compute(Domain::DistArtifact, b"Windows PDB fixture"),
            });
        }
        inventory.sort_by(|left, right| left.path.cmp(&right.path));
        let manifest_inventory = inventory.clone();
        let payload = TargetPayload {
            version: TARGET_PAYLOAD_VERSION,
            target,
            source: source.clone(),
            app: app_identity.clone(),
            canvas: Digest::compute(Domain::DistArtifact, b"canvas archive"),
            native,
            symbols: symbols.clone(),
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
            payload_bytes: payload.len() as u64,
            inventory: manifest_inventory,
            symbols,
            receipt: Digest::compute(Domain::ProofReceiptV3, &proof),
            receipt_bytes: proof.len() as u64,
            battery: battery.clone(),
            replay,
            environment: environment.clone(),
            standing: ProofStanding::Clean,
        };
        (executable, payload, proof, sealed)
    }

    #[cfg(unix)]
    #[test]
    fn child_preview_is_read_from_the_stream_its_outcome_uses() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::Output;

        let preview: InstallPreview = wire::decode(
            serde_json::json!({
                "digest": Digest::compute(Domain::InstallPlan, b"child preview"),
                "plan": {"version": 1, "operation_count": 0, "operations": [], "retirements": []},
            })
            .to_string()
            .as_bytes(),
        )
        .expect("a minimal preview decodes");
        let reply = Outcome::Reply(wire::Reply {
            position: wire::ReplyPosition::Direct {},
            body: preview,
        });
        let refusal: Outcome<InstallPreview> = Outcome::Refusal(wire::Refusal::bare(
            wire::RefusalDiagnostic::new("install::canvas-refused".to_owned(), None, None, None),
        ));
        let encoded = |outcome: &Outcome<InstallPreview>| wire::encode(outcome).expect("encodes");

        let replied = Output {
            status: ExitStatusExt::from_raw(0),
            stdout: encoded(&reply),
            stderr: Vec::new(),
        };
        assert_eq!(answer(&replied).expect("reply decodes"), reply);

        let refused = Output {
            status: ExitStatusExt::from_raw(2 << 8),
            stdout: Vec::new(),
            stderr: encoded(&refusal),
        };
        assert_eq!(answer(&refused).expect("refusal decodes"), refusal);

        let silent = Output {
            status: ExitStatusExt::from_raw(2 << 8),
            stdout: encoded(&reply),
            stderr: Vec::new(),
        };
        assert!(matches!(answer(&silent), Err(Error::Unreadable { .. })));
    }
}
