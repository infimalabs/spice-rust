//! Canonical external package index and closed package tree.

use std::fmt;
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde::de::DeserializeOwned;
use wire::manifest::{
    CANVAS_APP_FILES, ProductVersion, ProofStanding, RELEASE_MANIFEST_VERSION,
    RELEASE_PACKAGE_VERSION, ReleaseManifest, ReleasePackage, ReleasePackageTarget, ReleaseTarget,
};
use wire::{Digest, Domain};

const PACKAGE_NAME: &str = "spice";
const DATA_MODE: u32 = 0o644;
const EXECUTABLE_MODE: u32 = 0o755;
const MANIFEST_PATH: &str = "release/manifest.json";
const NOTES_PATH: &str = "release/notes.md";
const PACKAGE_PATH: &str = "release/package.json";
pub const PACKAGE_DOCUMENT_LIMIT: u64 = 8 * 1024 * 1024;
const TARGETS: [ReleaseTarget; 2] = [
    ReleaseTarget::Aarch64AppleDarwin,
    ReleaseTarget::X86_64UnknownLinuxMusl,
];

/// Exact bytes supplied for one supported target package entry.
#[derive(Clone, Debug)]
pub struct PackageTargetInput {
    target: ReleaseTarget,
    executable: PackageMember,
    payload: PackageMember,
    proof: PackageMember,
}

/// Exact bytes supplied for one member of the shared Canvas generation.
#[derive(Clone, Debug)]
pub struct PackageAppInput {
    file: String,
    member: PackageMember,
}

impl PackageAppInput {
    #[must_use]
    pub fn new(file: impl Into<String>, member: PackageMember) -> Self {
        Self {
            file: file.into(),
            member,
        }
    }
}

impl PackageTargetInput {
    #[must_use]
    pub const fn new(
        target: ReleaseTarget,
        executable: PackageMember,
        payload: PackageMember,
        proof: PackageMember,
    ) -> Self {
        Self {
            target,
            executable,
            payload,
            proof,
        }
    }
}

/// Complete already-sealed input required to form one external package tree.
#[derive(Clone, Debug)]
pub struct PackageInput<'a> {
    version: &'a ProductVersion,
    notes_identity: Digest,
    notes: PackageMember,
    manifest: PackageMember,
    app: Vec<PackageAppInput>,
    targets: Vec<PackageTargetInput>,
}

impl<'a> PackageInput<'a> {
    #[must_use]
    pub fn new(
        version: &'a ProductVersion,
        notes_identity: Digest,
        notes: PackageMember,
        manifest: PackageMember,
        app: Vec<PackageAppInput>,
        targets: Vec<PackageTargetInput>,
    ) -> Self {
        Self {
            version,
            notes_identity,
            notes,
            manifest,
            app,
            targets,
        }
    }
}

/// One exact regular file in the deterministic package tree.
#[derive(Clone, Debug)]
pub struct PackageMember {
    path: String,
    mode: u32,
    length: u64,
    digest: Digest,
    body: MemberBody,
}

#[derive(Clone, Debug)]
enum MemberBody {
    Inline(Arc<[u8]>),
    File {
        file: Arc<Mutex<File>>,
        identity: MemberFileIdentity,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MemberFileIdentity {
    length: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl PackageMember {
    /// Construct one observed package-tree member.
    ///
    /// The role is derived from the path so callers re-reading a staged tree
    /// cannot choose a digest domain independently from the package grammar.
    #[must_use]
    fn inline(path: impl Into<String>, mode: u32, bytes: impl Into<Vec<u8>>) -> Self {
        let path = path.into();
        let bytes = bytes.into();
        let digest = Digest::compute(member_domain(&path), &bytes);
        Self {
            path,
            mode,
            length: bytes.len() as u64,
            digest,
            body: MemberBody::Inline(bytes.into()),
        }
    }

    /// Admit one complete inline document under an explicit positive ceiling.
    ///
    /// # Errors
    ///
    /// Refuses zero or a body larger than `limit` before retaining it.
    pub fn admit_inline(
        path: impl Into<String>,
        mode: u32,
        bytes: &[u8],
        limit: u64,
    ) -> Result<Self, PackageError> {
        if limit == 0 || limit > PACKAGE_DOCUMENT_LIMIT || bytes.len() as u64 > limit {
            return Err(member_error(
                &path.into(),
                &format!(
                    "length={} requested={limit} maximum={PACKAGE_DOCUMENT_LIMIT}",
                    bytes.len()
                ),
            ));
        }
        Ok(Self::inline(path, mode, bytes))
    }

    /// Admit one already-confined regular file without retaining its body.
    ///
    /// # Errors
    ///
    /// Refuses a zero ceiling, non-regular input, overflow, read failure, or
    /// movement while the exact domain digest is streamed.
    pub fn admit_file(
        path: impl Into<String>,
        mode: u32,
        file: File,
        limit: u64,
    ) -> Result<Self, PackageError> {
        let path = path.into();
        let identity = MemberFileIdentity::capture(&file, &path)?;
        let (length, digest) = digest_file(&file, &path, limit)?;
        if length != identity.length || MemberFileIdentity::capture(&file, &path)? != identity {
            return Err(member_error(
                &path,
                "file moved while its digest was admitted",
            ));
        }
        Ok(Self {
            path,
            mode,
            length,
            digest,
            body: MemberBody::File {
                file: Arc::new(Mutex::new(file)),
                identity,
            },
        })
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub const fn mode(&self) -> u32 {
        self.mode
    }

    #[must_use]
    pub const fn length(&self) -> u64 {
        self.length
    }

    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }

    fn inline_identity_holds(&self) -> bool {
        match &self.body {
            MemberBody::Inline(bytes) => {
                Digest::compute(member_domain(&self.path), bytes) == self.digest
            }
            MemberBody::File { .. } => true,
        }
    }

    /// Read a canonical document only after its complete size is admitted.
    ///
    /// # Errors
    ///
    /// Refuses zero/insufficient bounds, read failures, or file movement.
    pub fn read_bounded(&self, limit: u64) -> Result<Vec<u8>, PackageError> {
        if limit == 0 || self.length > limit {
            return Err(member_error(
                &self.path,
                &format!("length={} maximum={limit}", self.length),
            ));
        }
        match &self.body {
            MemberBody::Inline(bytes) => Ok(bytes.to_vec()),
            MemberBody::File { file, identity } => {
                let mut file = file
                    .lock()
                    .expect("package member file lock is not poisoned");
                require_file_identity(&file, &self.path, identity)?;
                file.seek(SeekFrom::Start(0))
                    .map_err(|error| member_error(&self.path, &error.to_string()))?;
                let mut bytes = Vec::with_capacity(
                    usize::try_from(self.length).expect("bounded package member fits usize"),
                );
                Read::by_ref(&mut *file)
                    .take(limit.saturating_add(1))
                    .read_to_end(&mut bytes)
                    .map_err(|error| member_error(&self.path, &error.to_string()))?;
                require_file_identity(&file, &self.path, identity)?;
                if bytes.len() as u64 != self.length
                    || Digest::compute(member_domain(&self.path), &bytes) != self.digest
                {
                    return Err(member_error(&self.path, "file moved while it was read"));
                }
                Ok(bytes)
            }
        }
    }

    /// Copy one verified member body without retaining another full body.
    ///
    /// # Errors
    ///
    /// Refuses destination failures or file movement before or after the copy.
    pub fn copy_to(&self, writer: &mut dyn Write) -> Result<u64, PackageError> {
        match &self.body {
            MemberBody::Inline(bytes) => {
                writer
                    .write_all(bytes)
                    .map_err(|error| member_error(&self.path, &error.to_string()))?;
            }
            MemberBody::File { file, identity } => {
                let mut guard = file
                    .lock()
                    .expect("package member file lock is not poisoned");
                require_file_identity(&guard, &self.path, identity)?;
                guard
                    .seek(SeekFrom::Start(0))
                    .map_err(|error| member_error(&self.path, &error.to_string()))?;
                let (copied, digest) = Digest::copy_reader(
                    member_domain(&self.path),
                    &mut *guard,
                    writer,
                    self.length,
                )
                .map_err(|error| member_error(&self.path, &error.to_string()))?;
                require_file_identity(&guard, &self.path, identity)?;
                if copied != self.length || digest != self.digest {
                    return Err(member_error(
                        &self.path,
                        "copied bytes differ from the admitted member",
                    ));
                }
            }
        }
        Ok(self.length)
    }

    fn revalidate(&self) -> Result<(), PackageError> {
        self.copy_to(&mut std::io::sink()).map(|_| ())
    }
}

impl PartialEq for PackageMember {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
            && self.mode == other.mode
            && self.length == other.length
            && self.digest == other.digest
    }
}

impl Eq for PackageMember {}

impl MemberFileIdentity {
    fn capture(file: &File, path: &str) -> Result<Self, PackageError> {
        let metadata = file
            .metadata()
            .map_err(|error| member_error(path, &error.to_string()))?;
        if !metadata.is_file() {
            return Err(member_error(path, "source is not a regular file"));
        }
        Ok(Self::from_metadata(&metadata))
    }

    fn from_metadata(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt as _;

        Self {
            length: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        }
    }
}

fn digest_file(file: &File, path: &str, limit: u64) -> Result<(u64, Digest), PackageError> {
    let mut file = file
        .try_clone()
        .map_err(|error| member_error(path, &error.to_string()))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| member_error(path, &error.to_string()))?;
    Digest::compute_reader(member_domain(path), file, limit)
        .map_err(|error| member_error(path, &error.to_string()))
}

fn require_file_identity(
    file: &File,
    path: &str,
    identity: &MemberFileIdentity,
) -> Result<(), PackageError> {
    if MemberFileIdentity::capture(file, path)? == *identity {
        Ok(())
    } else {
        Err(member_error(path, "file identity moved after admission"))
    }
}

fn member_error(path: &str, detail: &str) -> PackageError {
    PackageError::Member {
        path: path.to_owned(),
        detail: detail.to_owned(),
    }
}

/// One canonical package index and its exact domain identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedPackage {
    document: ReleasePackage,
    bytes: Vec<u8>,
    identity: Digest,
}

impl AdmittedPackage {
    #[must_use]
    pub const fn document(&self) -> &ReleasePackage {
        &self.document
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub const fn identity(&self) -> &Digest {
        &self.identity
    }
}

/// Complete deterministic regular-file closure for one release package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageTree {
    package: AdmittedPackage,
    members: Vec<PackageMember>,
}

impl PackageTree {
    #[must_use]
    pub const fn package(&self) -> &AdmittedPackage {
        &self.package
    }

    #[must_use]
    pub fn members(&self) -> &[PackageMember] {
        &self.members
    }
}

/// Build and admit one exact external package tree.
///
/// # Errors
///
/// Refuses noncanonical or stale evidence, target order or binding drift, a
/// non-clean proof, or executable bytes that differ from the sealed payload.
pub fn build_package(input: PackageInput<'_>) -> Result<PackageTree, PackageError> {
    assemble_package(input)
}

/// Build and admit only the canonical external package index.
///
/// This verifies the same manifest, payload, proof, executable, target-order,
/// and notes bindings as [`build_package`] without cloning the (potentially
/// large) executable members into a package tree. Release preview uses this
/// consequence-free form because it needs the exact package identity, while
/// the later rebuild/upload operation owns materializing the tree.
///
/// # Errors
///
/// Returns the same evidence and binding refusals as [`build_package`].
pub fn build_package_index(input: PackageInput<'_>) -> Result<AdmittedPackage, PackageError> {
    assemble_package_index(&input)
}

/// Re-admit a package tree from only its regular-file closure.
///
/// # Errors
///
/// Refuses extra, missing, duplicated, reordered, mis-moded, noncanonical, or
/// digest-mismatched members and any evidence binding drift.
pub fn admit_package_tree(members: &[PackageMember]) -> Result<AdmittedPackage, PackageError> {
    if members.iter().any(|member| !member.inline_identity_holds()) {
        return Err(PackageError::Closure);
    }
    let package_member = exact_member(members, PACKAGE_PATH, DATA_MODE)?;
    let package_bytes = package_member.read_bounded(PACKAGE_DOCUMENT_LIMIT)?;
    let document: ReleasePackage = canonical(&package_bytes, "release package")?;
    admit_package_document(&document)?;
    let version = document.product_version.clone();
    let manifest = exact_member(members, MANIFEST_PATH, DATA_MODE)?;
    let notes = exact_member(members, NOTES_PATH, DATA_MODE)?;
    let mut targets = Vec::with_capacity(TARGETS.len());
    for target in TARGETS {
        targets.push(PackageTargetInput::new(
            target,
            exact_member(members, &executable_path(target), EXECUTABLE_MODE)?.clone(),
            exact_member(members, &payload_path(target), DATA_MODE)?.clone(),
            exact_member(members, &proof_path(target), DATA_MODE)?.clone(),
        ));
    }
    let mut app = CANVAS_APP_FILES
        .iter()
        .map(|file| {
            exact_member(members, &app_path(file), DATA_MODE)
                .map(|member| PackageAppInput::new(*file, member.clone()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    app.sort_by(|left, right| left.file.cmp(&right.file));
    let rebuilt = assemble_package(PackageInput::new(
        &version,
        document.notes.clone(),
        notes.clone(),
        manifest.clone(),
        app,
        targets,
    ))?;
    if rebuilt.members != members || rebuilt.package.document != document {
        return Err(PackageError::Closure);
    }
    Ok(rebuilt.package)
}

fn assemble_package(input: PackageInput<'_>) -> Result<PackageTree, PackageError> {
    let package = assemble_package_index(&input)?;
    let mut members = vec![
        input.manifest.clone(),
        input.notes.clone(),
        member(
            PACKAGE_PATH,
            DATA_MODE,
            package.bytes(),
            Domain::DistReleasePackage,
        ),
    ];
    for target in &input.targets {
        members.extend([
            target.executable.clone(),
            target.payload.clone(),
            target.proof.clone(),
        ]);
    }
    for asset in &input.app {
        members.push(asset.member.clone());
    }
    members.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(PackageTree { package, members })
}

fn assemble_package_index(input: &PackageInput<'_>) -> Result<AdmittedPackage, PackageError> {
    require_member(&input.manifest, MANIFEST_PATH, DATA_MODE)?;
    require_member(&input.notes, NOTES_PATH, DATA_MODE)?;
    input.notes.revalidate()?;
    let manifest_bytes = input.manifest.read_bounded(PACKAGE_DOCUMENT_LIMIT)?;
    let manifest: ReleaseManifest = canonical(&manifest_bytes, "release manifest")?;
    let manifest_identity = input.manifest.digest.clone();
    admit_manifest_shape(&manifest)?;
    if input.targets.len() != TARGETS.len() {
        return Err(PackageError::TargetCount(input.targets.len()));
    }
    let mut package_targets = Vec::with_capacity(TARGETS.len());
    let mut first_app = None;
    for (index, ((target, expected), sealed)) in input
        .targets
        .iter()
        .zip(TARGETS)
        .zip(&manifest.targets)
        .enumerate()
    {
        if target.target != expected || sealed.target != expected {
            return Err(PackageError::TargetOrder {
                index,
                expected,
                input: target.target,
                manifest: sealed.target,
            });
        }
        let admitted = admit_target(target, &input.app, sealed, &manifest.source, index)?;
        if let Some((generation, inventory)) = &first_app {
            if generation != &admitted.app || inventory != &admitted.app_inventory {
                return Err(PackageError::Binding {
                    index,
                    field: "shared Canvas generation",
                });
            }
        } else {
            first_app = Some((admitted.app.clone(), admitted.app_inventory.clone()));
        }
        package_targets.push(ReleasePackageTarget {
            target: expected,
            executable_path: executable_path(expected),
            executable: admitted.executable,
            payload: admitted.payload,
            proof: admitted.proof,
        });
    }
    let document = ReleasePackage {
        schema: RELEASE_PACKAGE_VERSION,
        package: PACKAGE_NAME.to_owned(),
        product_version: input.version.clone(),
        source: manifest.source.clone(),
        manifest: manifest_identity,
        notes: input.notes_identity.clone(),
        notes_content: input.notes.digest.clone(),
        targets: package_targets,
    };
    let bytes = wire::encode(&document).map_err(|error| PackageError::Encode {
        label: "release package",
        detail: error.to_string(),
    })?;
    Ok(AdmittedPackage {
        identity: Digest::compute(Domain::DistReleasePackage, &bytes),
        document,
        bytes,
    })
}

#[derive(Debug)]
struct TargetEvidence {
    app: Digest,
    app_inventory: Vec<wire::manifest::InventoryEntry>,
    executable: Digest,
    payload: Digest,
    proof: Digest,
}

fn admit_target(
    input: &PackageTargetInput,
    app: &[PackageAppInput],
    sealed: &wire::manifest::ReleaseManifestTarget,
    source: &Digest,
    index: usize,
) -> Result<TargetEvidence, PackageError> {
    require_member(
        &input.executable,
        &executable_path(input.target),
        EXECUTABLE_MODE,
    )?;
    input.executable.revalidate()?;
    require_member(&input.payload, &payload_path(input.target), DATA_MODE)?;
    require_member(&input.proof, &proof_path(input.target), DATA_MODE)?;
    let payload_bytes = input.payload.read_bounded(PACKAGE_DOCUMENT_LIMIT)?;
    let proof_bytes = input.proof.read_bounded(PACKAGE_DOCUMENT_LIMIT)?;
    let admitted_payload = crate::payload::admit(&payload_bytes).map_err(PackageError::Payload)?;
    let payload = admitted_payload.value();
    let admitted_proof =
        crate::evidence::admit_receipt(&proof_bytes).map_err(PackageError::Proof)?;
    let proof = admitted_proof.value();
    let payload_identity = admitted_payload.digest().clone();
    let proof_identity = admitted_proof.digest().clone();
    let executable = input.executable.digest.clone();
    admit_app(app, payload, index)?;
    let executable_inventory = payload
        .inventory
        .iter()
        .find(|entry| entry.path == "bin/spice")
        .ok_or(PackageError::Binding {
            index,
            field: "executable inventory",
        })?;
    let bindings = [
        (payload.target == input.target, "payload target"),
        (payload.source == *source, "payload source"),
        (payload.native == executable, "payload executable"),
        (
            executable_inventory.mode == EXECUTABLE_MODE,
            "executable mode",
        ),
        (
            executable_inventory.digest == executable,
            "inventory executable",
        ),
        (sealed.payload == payload_identity, "manifest payload"),
        (sealed.receipt == proof_identity, "manifest proof"),
        (proof.target == input.target, "proof target"),
        (proof.payload == payload_identity, "proof payload"),
        (proof.battery == sealed.battery, "proof battery"),
        (proof.replay == sealed.replay, "proof replay"),
        (proof.environment == sealed.environment, "proof environment"),
        (proof.standing == ProofStanding::Clean, "proof standing"),
        (sealed.standing == ProofStanding::Clean, "manifest standing"),
    ];
    if let Some((_, field)) = bindings.into_iter().find(|(matches, _)| !matches) {
        return Err(PackageError::Binding { index, field });
    }
    Ok(TargetEvidence {
        app: payload.app.clone(),
        app_inventory: payload
            .inventory
            .iter()
            .filter(|entry| entry.path.starts_with("app/"))
            .cloned()
            .collect(),
        executable,
        payload: payload_identity,
        proof: proof_identity,
    })
}

fn admit_app(
    app: &[PackageAppInput],
    payload: &wire::manifest::TargetPayload,
    index: usize,
) -> Result<(), PackageError> {
    let mut expected = CANVAS_APP_FILES.to_vec();
    expected.sort_unstable();
    if app
        .iter()
        .map(|asset| asset.file.as_str())
        .collect::<Vec<_>>()
        != expected
    {
        return Err(PackageError::Binding {
            index,
            field: "Canvas package inventory",
        });
    }
    for asset in app {
        let path = format!("app/{}", asset.file);
        require_member(&asset.member, &path, DATA_MODE)?;
        asset.member.revalidate()?;
        let Some(entry) = payload.inventory.iter().find(|entry| entry.path == path) else {
            return Err(PackageError::Binding {
                index,
                field: "Canvas payload inventory",
            });
        };
        if entry.mode != DATA_MODE
            || entry.bytes != asset.member.length
            || entry.digest != asset.member.digest
        {
            return Err(PackageError::Binding {
                index,
                field: "Canvas asset",
            });
        }
    }
    Ok(())
}

fn admit_manifest_shape(manifest: &ReleaseManifest) -> Result<(), PackageError> {
    if manifest.version != RELEASE_MANIFEST_VERSION {
        return Err(PackageError::ManifestVersion(manifest.version));
    }
    if manifest.targets.len() != TARGETS.len() {
        return Err(PackageError::TargetCount(manifest.targets.len()));
    }
    Ok(())
}

fn admit_package_document(document: &ReleasePackage) -> Result<(), PackageError> {
    if document.schema != RELEASE_PACKAGE_VERSION {
        return Err(PackageError::PackageVersion(document.schema));
    }
    if document.package != PACKAGE_NAME {
        return Err(PackageError::PackageName(document.package.clone()));
    }
    if document.targets.len() != TARGETS.len() {
        return Err(PackageError::TargetCount(document.targets.len()));
    }
    for (index, (entry, expected)) in document.targets.iter().zip(TARGETS).enumerate() {
        if entry.target != expected || entry.executable_path != executable_path(expected) {
            return Err(PackageError::PackageTarget { index });
        }
    }
    Ok(())
}

fn canonical<T>(bytes: &[u8], label: &'static str) -> Result<T, PackageError>
where
    T: DeserializeOwned + Serialize,
{
    let value = wire::decode(bytes).map_err(|error| PackageError::Decode {
        label,
        detail: error.to_string(),
    })?;
    let canonical = wire::encode(&value).map_err(|error| PackageError::Encode {
        label,
        detail: error.to_string(),
    })?;
    if canonical == bytes {
        Ok(value)
    } else {
        Err(PackageError::NonCanonical(label))
    }
}

fn member(path: &str, mode: u32, bytes: &[u8], domain: Domain) -> PackageMember {
    let member = PackageMember::inline(path, mode, bytes);
    debug_assert_eq!(member.digest, Digest::compute(domain, bytes));
    member
}

fn require_member(member: &PackageMember, path: &str, mode: u32) -> Result<(), PackageError> {
    if member.path == path && member.mode == mode {
        Ok(())
    } else {
        Err(PackageError::Closure)
    }
}

fn member_domain(path: &str) -> Domain {
    match path {
        MANIFEST_PATH => Domain::DistReleaseManifest,
        NOTES_PATH => Domain::DistReleaseNotesContent,
        PACKAGE_PATH => Domain::DistReleasePackage,
        path if path.ends_with("/payload.json") => Domain::DistTargetPayload,
        path if path.ends_with("/proof.json") => Domain::ProofReceiptV3,
        _ => Domain::DistArtifact,
    }
}

fn exact_member<'a>(
    members: &'a [PackageMember],
    path: &str,
    mode: u32,
) -> Result<&'a PackageMember, PackageError> {
    let mut found = members.iter().filter(|member| member.path == path);
    let member = found
        .next()
        .ok_or_else(|| PackageError::Missing(path.to_owned()))?;
    if found.next().is_some() {
        return Err(PackageError::Duplicate(path.to_owned()));
    }
    if member.mode != mode {
        return Err(PackageError::Mode {
            path: path.to_owned(),
            expected: mode,
            observed: member.mode,
        });
    }
    Ok(member)
}

fn executable_path(target: ReleaseTarget) -> String {
    format!("targets/{target}/bin/spice")
}

fn payload_path(target: ReleaseTarget) -> String {
    format!("targets/{target}/payload.json")
}

fn proof_path(target: ReleaseTarget) -> String {
    format!("targets/{target}/proof.json")
}

fn app_path(file: &str) -> String {
    format!("app/{file}")
}

/// Why an external release package was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum PackageError {
    Member {
        path: String,
        detail: String,
    },
    Decode {
        label: &'static str,
        detail: String,
    },
    Encode {
        label: &'static str,
        detail: String,
    },
    NonCanonical(&'static str),
    Payload(crate::PayloadError),
    Proof(crate::EvidenceError),
    ManifestVersion(u16),
    PackageVersion(u16),
    PackageName(String),
    TargetCount(usize),
    TargetOrder {
        index: usize,
        expected: ReleaseTarget,
        input: ReleaseTarget,
        manifest: ReleaseTarget,
    },
    Binding {
        index: usize,
        field: &'static str,
    },
    PackageTarget {
        index: usize,
    },
    Missing(String),
    Duplicate(String),
    Mode {
        path: String,
        expected: u32,
        observed: u32,
    },
    Closure,
}

impl fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Member { path, detail } => {
                write!(formatter, "release package member {path} refuses: {detail}")
            }
            Self::Decode { label, detail } => {
                write!(formatter, "{label} does not decode: {detail}")
            }
            Self::Encode { label, detail } => {
                write!(formatter, "{label} does not encode: {detail}")
            }
            Self::NonCanonical(label) => {
                write!(formatter, "{label} is not canonical current wire JSON")
            }
            Self::Payload(error) => write!(formatter, "target payload refuses: {error}"),
            Self::Proof(error) => write!(formatter, "proof receipt refuses: {error}"),
            Self::ManifestVersion(version) => {
                write!(
                    formatter,
                    "release manifest schema {version} is unsupported"
                )
            }
            Self::PackageVersion(version) => {
                write!(formatter, "release package schema {version} is unsupported")
            }
            Self::PackageName(name) => {
                write!(formatter, "release package name is unsupported: {name}")
            }
            Self::TargetCount(count) => {
                write!(
                    formatter,
                    "release package has {count} targets, expected two"
                )
            }
            Self::TargetOrder {
                index,
                expected,
                input,
                manifest,
            } => write!(
                formatter,
                "target {index} order moved: expected {expected}, input {input}, manifest {manifest}"
            ),
            Self::Binding { index, field } => {
                write!(formatter, "release target {index} binding moved at {field}")
            }
            Self::PackageTarget { index } => write!(
                formatter,
                "release package target {index} has a noncanonical target or executable path"
            ),
            Self::Missing(path) => write!(formatter, "release package member is missing: {path}"),
            Self::Duplicate(path) => {
                write!(formatter, "release package member is duplicated: {path}")
            }
            Self::Mode {
                path,
                expected,
                observed,
            } => write!(
                formatter,
                "release package member {path} mode is {observed:o}, expected {expected:o}"
            ),
            Self::Closure => {
                formatter.write_str("release package tree is not the exact canonical closure")
            }
        }
    }
}

impl std::error::Error for PackageError {}

#[cfg(test)]
mod tests {
    use wire::manifest::{
        BuildPins, InventoryEntry, PROOF_RECEIPT_VERSION, ProofEnvironment, ProofReceipt,
        ReleaseManifestTarget, TARGET_PAYLOAD_VERSION, TargetPayload,
    };

    use super::*;

    struct Fixture {
        version: ProductVersion,
        notes_identity: Digest,
        notes: Vec<u8>,
        manifest: Vec<u8>,
        app: Vec<(String, Vec<u8>)>,
        executables: Vec<Vec<u8>>,
        payloads: Vec<Vec<u8>>,
        proofs: Vec<Vec<u8>>,
    }

    impl Fixture {
        fn input(&self) -> PackageInput<'_> {
            PackageInput::new(
                &self.version,
                self.notes_identity.clone(),
                member(
                    NOTES_PATH,
                    DATA_MODE,
                    &self.notes,
                    Domain::DistReleaseNotesContent,
                ),
                member(
                    MANIFEST_PATH,
                    DATA_MODE,
                    &self.manifest,
                    Domain::DistReleaseManifest,
                ),
                self.app
                    .iter()
                    .map(|(file, bytes)| {
                        PackageAppInput::new(
                            file,
                            member(&app_path(file), DATA_MODE, bytes, Domain::DistArtifact),
                        )
                    })
                    .collect(),
                TARGETS
                    .into_iter()
                    .enumerate()
                    .map(|(index, target)| {
                        PackageTargetInput::new(
                            target,
                            member(
                                &executable_path(target),
                                EXECUTABLE_MODE,
                                &self.executables[index],
                                Domain::DistArtifact,
                            ),
                            member(
                                &payload_path(target),
                                DATA_MODE,
                                &self.payloads[index],
                                Domain::DistTargetPayload,
                            ),
                            member(
                                &proof_path(target),
                                DATA_MODE,
                                &self.proofs[index],
                                Domain::ProofReceiptV3,
                            ),
                        )
                    })
                    .collect(),
            )
        }
    }

    #[test]
    fn canonical_package_tree_round_trips_as_one_closed_identity() {
        let inline_fixture = fixture();
        let index = build_package_index(inline_fixture.input()).expect("fixture index builds");
        let tree = build_package(inline_fixture.input()).expect("fixture package builds");
        assert_eq!(index, *tree.package());
        assert_eq!(tree.members().len(), 17);
        assert!(
            tree.members()
                .is_sorted_by(|left, right| left.path() < right.path())
        );
        let admitted = admit_package_tree(tree.members()).expect("package tree re-admits");
        assert_eq!(admitted, *tree.package());
        assert_eq!(admitted.document().product_version.as_str(), "1.2.3");
        assert_eq!(admitted.document().targets.len(), 2);
        for executable in [true, false] {
            let fixture = fixture();
            let (member_path, mode, body) = if executable {
                (
                    executable_path(TARGETS[0]),
                    EXECUTABLE_MODE,
                    fixture.executables[0].clone(),
                )
            } else {
                (
                    app_path(&fixture.app[0].0),
                    DATA_MODE,
                    fixture.app[0].1.clone(),
                )
            };
            let path = std::env::temp_dir().join(format!(
                "proof-package-member-{}-{executable}",
                std::process::id()
            ));
            std::fs::write(&path, &body).expect("file member writes");
            let member = PackageMember::admit_file(
                member_path,
                mode,
                File::open(&path).expect("file member opens"),
                body.len() as u64,
            )
            .expect("file member admits");
            let mut input = fixture.input();
            if executable {
                input.targets[0].executable = member;
            } else {
                input.app[0].member = member;
            }
            let tree = build_package(input.clone()).expect("file-backed package builds");
            let mut moved = body;
            moved[0] ^= 1;
            std::fs::write(&path, moved).expect("file member moves");
            assert!(build_package_index(input.clone()).is_err());
            assert!(build_package(input).is_err());
            assert!(admit_package_tree(tree.members()).is_err());
            std::fs::remove_file(path).expect("file member removes");
        }
        assert!(PackageMember::admit_inline("release/notes.md", DATA_MODE, b"x", 0).is_err());
        let oversized = vec![0; PACKAGE_DOCUMENT_LIMIT as usize + 1];
        assert!(
            PackageMember::admit_inline(
                "release/notes.md",
                DATA_MODE,
                &oversized,
                PACKAGE_DOCUMENT_LIMIT,
            )
            .is_err()
        );
    }

    #[test]
    fn every_external_identity_input_moves_or_refuses_the_package() {
        let baseline_fixture = fixture();
        let baseline = build_package(baseline_fixture.input())
            .expect("baseline")
            .package
            .identity;

        let mut notes = fixture();
        notes.notes.push(b'!');
        assert_ne!(
            build_package(notes.input())
                .expect("notes move")
                .package
                .identity,
            baseline
        );

        let mut version = fixture();
        version.version = ProductVersion::parse("1.2.4").expect("version");
        assert_ne!(
            build_package(version.input())
                .expect("version moves")
                .package
                .identity,
            baseline
        );

        let mut executable = fixture();
        executable.executables[0].push(0);
        assert!(matches!(
            build_package(executable.input()),
            Err(PackageError::Binding {
                field: "payload executable",
                ..
            })
        ));

        let mut app = fixture();
        app.app[0].1.push(0);
        assert!(matches!(
            build_package(app.input()),
            Err(PackageError::Binding {
                field: "Canvas asset",
                ..
            })
        ));
    }

    #[test]
    fn missing_extra_reordered_and_mismoded_members_refuse() {
        let tree = build_package(fixture().input()).expect("fixture package");

        let mut missing = tree.members.clone();
        missing.retain(|member| member.path() != "app/style.css");
        assert_eq!(
            admit_package_tree(&missing),
            Err(PackageError::Missing("app/style.css".to_owned()))
        );

        let mut extra = tree.members.clone();
        extra.push(member("extra", DATA_MODE, b"extra", Domain::DistArtifact));
        assert_eq!(admit_package_tree(&extra), Err(PackageError::Closure));

        let mut reordered = tree.members.clone();
        reordered.swap(0, 1);
        assert_eq!(admit_package_tree(&reordered), Err(PackageError::Closure));

        let mut mismoded = tree.members.clone();
        let executable = mismoded
            .iter_mut()
            .find(|member| member.path.ends_with("/bin/spice"))
            .expect("executable member");
        executable.mode = DATA_MODE;
        assert!(matches!(
            admit_package_tree(&mismoded),
            Err(PackageError::Mode { .. })
        ));

        let mut digest_moved = tree.members.clone();
        digest_moved[0].digest = digest(Domain::DistArtifact, b"moved");
        assert_eq!(
            admit_package_tree(&digest_moved),
            Err(PackageError::Closure)
        );
    }

    #[test]
    fn noncanonical_or_moved_sealed_evidence_refuses_before_packaging() {
        let mut payload = fixture();
        payload.payloads[0].push(b'\n');
        assert_eq!(
            build_package(payload.input()),
            Err(PackageError::Payload(crate::PayloadError::NonCanonical))
        );

        let mut proof = fixture();
        let mut receipt: ProofReceipt = wire::decode(&proof.proofs[0]).expect("receipt");
        receipt.standing = ProofStanding::Refused;
        proof.proofs[0] = wire::encode(&receipt).expect("moved receipt");
        assert!(matches!(
            build_package(proof.input()),
            Err(PackageError::Binding {
                field: "manifest proof",
                ..
            })
        ));
    }

    fn fixture() -> Fixture {
        let source = digest(Domain::DistSourceCommit, b"source");
        let app_identity = digest(Domain::DistArtifact, b"app generation");
        let mut app = CANVAS_APP_FILES
            .iter()
            .map(|file| ((*file).to_owned(), file.as_bytes().to_vec()))
            .collect::<Vec<_>>();
        app.sort_by(|left, right| left.0.cmp(&right.0));
        let battery = digest(Domain::ProofBattery, b"battery");
        let environment = ProofEnvironment::native(digest(Domain::ProofNativeRunner, b"runner"));
        let mut executables = Vec::new();
        let mut payloads = Vec::new();
        let mut proofs = Vec::new();
        let mut manifest_targets = Vec::new();
        for (index, target) in TARGETS.into_iter().enumerate() {
            let executable = format!("executable-{index}").into_bytes();
            let native = Digest::compute(Domain::DistArtifact, &executable);
            let pins = BuildPins {
                rust: "rustc 1.97.1".to_owned(),
                wasm_target: "wasm32-unknown-unknown".to_owned(),
                trunk: "trunk 0.21.14".to_owned(),
                cargo_mutants: "cargo-mutants 27.1.0".to_owned(),
                proof_container: format!("registry.example/proof@sha256:{}", "1".repeat(64)),
            };
            let mut inventory = app
                .iter()
                .map(|(file, bytes)| InventoryEntry {
                    path: format!("app/{file}"),
                    mode: DATA_MODE,
                    bytes: bytes.len() as u64,
                    digest: Digest::compute(Domain::DistArtifact, bytes),
                })
                .collect::<Vec<_>>();
            inventory.push(InventoryEntry {
                path: "bin/spice".to_owned(),
                mode: EXECUTABLE_MODE,
                bytes: executable.len() as u64,
                digest: native.clone(),
            });
            inventory.sort_by(|left, right| left.path.cmp(&right.path));
            let payload = TargetPayload {
                version: TARGET_PAYLOAD_VERSION,
                target,
                source: source.clone(),
                app: app_identity.clone(),
                native: native.clone(),
                pins_digest: Digest::compute(Domain::DistPins, &wire::encode(&pins).expect("pins")),
                inventory_digest: Digest::compute(
                    Domain::DistInventory,
                    &wire::encode(&inventory).expect("inventory"),
                ),
                pins,
                inventory,
            };
            let payload_bytes = wire::encode(&payload).expect("payload");
            let payload_identity = Digest::compute(Domain::DistTargetPayload, &payload_bytes);
            let replay = digest(Domain::ProofReplayV3, format!("replay-{index}").as_bytes());
            let proof = ProofReceipt {
                version: PROOF_RECEIPT_VERSION,
                target,
                payload: payload_identity.clone(),
                battery: battery.clone(),
                replay: replay.clone(),
                environment: environment.clone(),
                standing: ProofStanding::Clean,
            };
            let proof_bytes = wire::encode(&proof).expect("proof");
            let proof_identity = Digest::compute(Domain::ProofReceiptV3, &proof_bytes);
            manifest_targets.push(ReleaseManifestTarget {
                target,
                payload: payload_identity,
                receipt: proof_identity,
                battery: battery.clone(),
                replay,
                environment: environment.clone(),
                standing: ProofStanding::Clean,
            });
            executables.push(executable);
            payloads.push(payload_bytes);
            proofs.push(proof_bytes);
        }
        let manifest = ReleaseManifest {
            version: RELEASE_MANIFEST_VERSION,
            source,
            targets: manifest_targets,
        };
        Fixture {
            version: ProductVersion::parse("1.2.3").expect("version"),
            notes_identity: digest(Domain::DistReleaseNotes, b"notes evidence"),
            notes: b"Curated release notes\n".to_vec(),
            manifest: wire::encode(&manifest).expect("manifest"),
            app,
            executables,
            payloads,
            proofs,
        }
    }

    fn digest(domain: Domain, bytes: &[u8]) -> Digest {
        Digest::compute(domain, bytes)
    }
}
