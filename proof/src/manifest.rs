//! Verification, composition, and manifest-last release publication.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Component, Path, PathBuf};

use wire::manifest::{
    ProofEnvironment, ProofStanding, RELEASE_MANIFEST_VERSION, ReleaseManifest,
    ReleaseManifestTarget, ReleaseTarget, TargetPayload,
};
use wire::{Digest, Domain};

use crate::evidence::admit_receipt;
use crate::{Battery, EvidenceError, PayloadError, ReplayVerdict, admit, verify};

const DATA_MODE: u32 = 0o644;
const MANIFEST_POINTER: &str = "release/manifest.json";
const TARGETS: [ReleaseTarget; 2] = [
    ReleaseTarget::Aarch64AppleDarwin,
    ReleaseTarget::X86_64UnknownLinuxMusl,
];

/// One exact payload, detached receipt, and replay submitted for release.
#[derive(Clone, Copy, Debug)]
pub struct ManifestInput<'a> {
    payload: &'a [u8],
    receipt: &'a [u8],
    replay: &'a ReplayVerdict,
}

impl<'a> ManifestInput<'a> {
    #[must_use]
    pub const fn new(payload: &'a [u8], receipt: &'a [u8], replay: &'a ReplayVerdict) -> Self {
        Self {
            payload,
            receipt,
            replay,
        }
    }
}

/// A complete canonical release and the exact bytes published for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedManifest {
    pub manifest: ReleaseManifest,
    pub bytes: Vec<u8>,
    pub digest: Digest,
    pub pointer: PathBuf,
}

/// Durable publication seam used by the filesystem and interruption fixtures.
pub trait ManifestPublisher {
    fn publish(&mut self, path: &Path, mode: u32, bytes: &[u8])
    -> Result<(), ManifestPublishError>;
}

/// Atomic filesystem publication rooted at one exact release directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileManifestPublisher {
    root: PathBuf,
}

impl FileManifestPublisher {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl ManifestPublisher for FileManifestPublisher {
    fn publish(
        &mut self,
        path: &Path,
        mode: u32,
        bytes: &[u8],
    ) -> Result<(), ManifestPublishError> {
        validate_relative(path)?;
        let destination = self.root.join(path);
        let parent = destination
            .parent()
            .ok_or_else(|| ManifestPublishError::InvalidPath(path.to_path_buf()))?;
        fs::create_dir_all(parent).map_err(|source| ManifestPublishError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let mut staged = atomic::Destination::new(&destination)?.stage()?;
        staged
            .write_all(bytes)
            .map_err(|source| ManifestPublishError::Io {
                path: staged.temporary_path().to_path_buf(),
                source,
            })?;
        set_mode(staged.temporary_path(), mode)?;
        staged.commit()?;
        Ok(())
    }
}

/// Admit the current fixed manifest pointer and every object it names.
///
/// The caller supplies the executable identities observed at the binary rim.
/// Both target payloads must carry the embedded app, and the payload matching
/// `native_target` must carry the invoking executable itself.
///
/// # Errors
///
/// Refuses missing, non-regular, symlinked, noncanonical, stale, partial, or
/// mixed-target evidence. Every content-addressed object is re-read and its
/// digest and typed bindings are checked before a manifest is returned.
pub fn admit_manifest(
    root: &Path,
    embedded_app: &Digest,
    native_target: ReleaseTarget,
    native: &Digest,
) -> Result<PublishedManifest, ManifestAdmissionError> {
    let root = admit_stage_root(root)?;
    let published = admit_pointer(&root)?;
    let battery = Battery::current();
    let mut first_payload = None;
    for (index, (entry, expected_target)) in
        published.manifest.targets.iter().zip(TARGETS).enumerate()
    {
        let payload = admit_target(
            &root,
            &published.manifest,
            entry,
            expected_target,
            index,
            embedded_app,
            native_target,
            native,
            battery.digest(),
        )?;
        admit_shared_payload(first_payload.as_ref(), &payload)?;
        first_payload.get_or_insert(payload);
    }
    Ok(published)
}

fn admit_stage_root(root: &Path) -> Result<PathBuf, ManifestAdmissionError> {
    let stage_metadata =
        fs::symlink_metadata(root).map_err(|source| ManifestAdmissionError::Input {
            label: "release stage",
            path: root.to_path_buf(),
            source,
        })?;
    if stage_metadata.file_type().is_symlink() {
        return Err(ManifestAdmissionError::StageSymlink(root.to_path_buf()));
    }
    if !stage_metadata.is_dir() {
        return Err(ManifestAdmissionError::StageNotDirectory(
            root.to_path_buf(),
        ));
    }
    fs::canonicalize(root).map_err(|source| ManifestAdmissionError::Input {
        label: "release stage",
        path: root.to_path_buf(),
        source,
    })
}

fn admit_pointer(root: &Path) -> Result<PublishedManifest, ManifestAdmissionError> {
    let pointer = PathBuf::from(MANIFEST_POINTER);
    let bytes = read_regular(root, &pointer, "release manifest pointer")?;
    let manifest: ReleaseManifest = wire::decode(&bytes)
        .map_err(|source| ManifestAdmissionError::ManifestDecode(source.to_string()))?;
    let canonical = wire::encode(&manifest)
        .map_err(|source| ManifestAdmissionError::ManifestEncode(source.to_string()))?;
    if canonical != bytes {
        return Err(ManifestAdmissionError::ManifestNonCanonical);
    }
    if manifest.version != RELEASE_MANIFEST_VERSION {
        return Err(ManifestAdmissionError::ManifestVersion(manifest.version));
    }
    if manifest.targets.len() != TARGETS.len() {
        return Err(ManifestAdmissionError::EntryCount(manifest.targets.len()));
    }

    let digest = Digest::compute(Domain::DistReleaseManifest, &bytes);
    let object = object_path("manifest", &digest);
    if read_regular(root, &object, "release manifest object")? != bytes {
        return Err(ManifestAdmissionError::ManifestObject);
    }
    Ok(PublishedManifest {
        manifest,
        bytes,
        digest,
        pointer,
    })
}

#[allow(clippy::too_many_arguments)]
fn admit_target(
    root: &Path,
    manifest: &ReleaseManifest,
    entry: &ReleaseManifestTarget,
    expected_target: ReleaseTarget,
    index: usize,
    embedded_app: &Digest,
    native_target: ReleaseTarget,
    native: &Digest,
    battery: &Digest,
) -> Result<TargetPayload, ManifestAdmissionError> {
    if entry.target != expected_target {
        return Err(ManifestAdmissionError::TargetOrder {
            index,
            expected: expected_target,
            observed: entry.target,
        });
    }
    let payload = admit_target_payload(
        root,
        manifest,
        entry,
        index,
        embedded_app,
        native_target,
        native,
    )?;
    admit_target_receipt(root, entry, index, &payload, battery)?;
    Ok(payload)
}

#[allow(clippy::too_many_arguments)]
fn admit_target_payload(
    root: &Path,
    manifest: &ReleaseManifest,
    entry: &ReleaseManifestTarget,
    index: usize,
    embedded_app: &Digest,
    native_target: ReleaseTarget,
    native: &Digest,
) -> Result<TargetPayload, ManifestAdmissionError> {
    let bytes = read_regular(
        root,
        &object_path("payload", &entry.payload),
        "target payload object",
    )?;
    let payload =
        admit(&bytes).map_err(|source| ManifestAdmissionError::Payload { index, source })?;
    let value = payload.value();
    require_binding(payload.digest() == &entry.payload, index, "payload digest")?;
    require_binding(value.target == entry.target, index, "payload target")?;
    require_binding(value.source == manifest.source, index, "payload source")?;
    require_binding(
        value.embedded_app == *embedded_app,
        index,
        "embedded app identity",
    )?;
    require_binding(
        value.target != native_target || value.native == *native,
        index,
        "native executable identity",
    )?;
    Ok(value.clone())
}

fn admit_target_receipt(
    root: &Path,
    entry: &ReleaseManifestTarget,
    index: usize,
    payload: &TargetPayload,
    battery: &Digest,
) -> Result<(), ManifestAdmissionError> {
    let bytes = read_regular(
        root,
        &object_path("receipt", &entry.receipt),
        "detached proof receipt object",
    )?;
    let admitted = admit_receipt(&bytes)
        .map_err(|source| ManifestAdmissionError::Receipt { index, source })?;
    require_binding(admitted.digest() == &entry.receipt, index, "receipt digest")?;
    let receipt = admitted.value();
    for (matches, field) in [
        (receipt.target == entry.target, "receipt target"),
        (receipt.payload == entry.payload, "receipt payload"),
        (receipt.battery == entry.battery, "receipt battery"),
        (receipt.replay == entry.replay, "receipt replay"),
        (
            receipt.environment == entry.environment,
            "receipt environment",
        ),
        (receipt.standing == entry.standing, "receipt standing"),
        (entry.battery == *battery, "current proof battery"),
        (
            entry.standing == ProofStanding::Clean,
            "clean proof standing",
        ),
    ] {
        require_binding(matches, index, field)?;
    }
    require_binding(
        target_environment_matches(entry, payload),
        index,
        "target proof environment",
    )
}

fn target_environment_matches(entry: &ReleaseManifestTarget, payload: &TargetPayload) -> bool {
    match (&entry.environment, entry.target) {
        (ProofEnvironment::Native { .. }, ReleaseTarget::Aarch64AppleDarwin) => true,
        (ProofEnvironment::Oci { image }, ReleaseTarget::X86_64UnknownLinuxMusl) => {
            image.as_str() == payload.pins.proof_container
        }
        _ => false,
    }
}

fn admit_shared_payload(
    first: Option<&TargetPayload>,
    observed: &TargetPayload,
) -> Result<(), ManifestAdmissionError> {
    let Some(first) = first else {
        return Ok(());
    };
    for (matches, field) in [
        (observed.app == first.app, "app identity"),
        (
            observed.embedded_app == first.embedded_app,
            "embedded app identity",
        ),
        (observed.pins == first.pins, "toolchain and container pins"),
    ] {
        if !matches {
            return Err(ManifestAdmissionError::Mixed { field });
        }
    }
    Ok(())
}

fn require_binding(
    matches: bool,
    index: usize,
    field: &'static str,
) -> Result<(), ManifestAdmissionError> {
    if matches {
        Ok(())
    } else {
        Err(ManifestAdmissionError::Binding { index, field })
    }
}

fn read_regular(
    root: &Path,
    relative: &Path,
    label: &'static str,
) -> Result<Vec<u8>, ManifestAdmissionError> {
    validate_relative(relative).map_err(|_| ManifestAdmissionError::OutsideRoot {
        label,
        path: relative.to_path_buf(),
    })?;
    let mut component_path = root.to_path_buf();
    for component in relative
        .parent()
        .expect("fixed object paths have parents")
        .components()
    {
        component_path.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&component_path).map_err(|source| {
            ManifestAdmissionError::Input {
                label,
                path: component_path.clone(),
                source,
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Err(ManifestAdmissionError::Symlink {
                label,
                path: component_path,
            });
        }
        if !metadata.is_dir() {
            return Err(ManifestAdmissionError::NotDirectory {
                label,
                path: component_path,
            });
        }
    }
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path).map_err(|source| ManifestAdmissionError::Input {
        label,
        path: path.clone(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ManifestAdmissionError::Symlink { label, path });
    }
    if !metadata.is_file() {
        return Err(ManifestAdmissionError::NotRegular { label, path });
    }
    let parent = fs::canonicalize(path.parent().expect("fixed object paths have parents"))
        .map_err(|source| ManifestAdmissionError::Input {
            label,
            path: path.clone(),
            source,
        })?;
    if !parent.starts_with(root) {
        return Err(ManifestAdmissionError::OutsideRoot { label, path });
    }
    fs::read(&path).map_err(|source| ManifestAdmissionError::Input {
        label,
        path,
        source,
    })
}

/// Verify both proofs, compose one manifest, and publish its pointer last.
///
/// # Errors
///
/// Refuses any shape other than both supported targets in declaration order,
/// any payload or proof defect, or source disagreement before publication.
/// Immutable payload, receipt, and manifest objects are all attempted before
/// the sole mutable pointer.
pub fn publish_manifest<'a, I, P>(
    inputs: I,
    publisher: &mut P,
) -> Result<PublishedManifest, ManifestError>
where
    I: IntoIterator<Item = ManifestInput<'a>>,
    P: ManifestPublisher,
{
    let inputs = inputs.into_iter().collect::<Vec<_>>();
    if inputs.len() != TARGETS.len() {
        return Err(ManifestError::EntryCount(inputs.len()));
    }

    let mut source = None;
    let mut first_payload: Option<TargetPayload> = None;
    let mut seen = BTreeSet::new();
    let mut targets = Vec::with_capacity(TARGETS.len());
    let mut objects = Vec::with_capacity(TARGETS.len() * 2 + 1);
    for (index, (input, expected)) in inputs.into_iter().zip(TARGETS).enumerate() {
        let admitted =
            admit(input.payload).map_err(|source| ManifestError::Payload { index, source })?;
        if !seen.insert(admitted.value().target) {
            return Err(ManifestError::DuplicateTarget(admitted.value().target));
        }
        if admitted.value().target != expected {
            return Err(ManifestError::TargetOrder {
                index,
                expected,
                observed: admitted.value().target,
            });
        }
        match &source {
            None => source = Some(admitted.value().source.clone()),
            Some(first) if first != &admitted.value().source => {
                return Err(ManifestError::SourceMismatch {
                    first: first.clone(),
                    observed: admitted.value().source.clone(),
                });
            }
            Some(_) => {}
        }
        if let Some(first) = &first_payload {
            for (matches, field) in [
                (admitted.value().app == first.app, "app identity"),
                (
                    admitted.value().embedded_app == first.embedded_app,
                    "embedded app identity",
                ),
                (
                    admitted.value().pins == first.pins,
                    "toolchain and container pins",
                ),
            ] {
                if !matches {
                    return Err(ManifestError::Mixed { field });
                }
            }
        } else {
            first_payload = Some(admitted.value().clone());
        }

        let verified = verify(&admitted, input.receipt, input.replay)
            .map_err(|source| ManifestError::Evidence { index, source })?;
        let payload_digest = verified.payload_digest().clone();
        let receipt_digest = verified.receipt_digest().clone();
        targets.push(ReleaseManifestTarget {
            target: verified.target(),
            payload: payload_digest.clone(),
            receipt: receipt_digest.clone(),
            battery: verified.battery_digest().clone(),
            replay: verified.replay_digest().clone(),
            environment: input.replay.environment().clone(),
            standing: ProofStanding::Clean,
        });
        objects.push((object_path("payload", &payload_digest), input.payload));
        objects.push((object_path("receipt", &receipt_digest), input.receipt));
    }

    let manifest = ReleaseManifest {
        version: RELEASE_MANIFEST_VERSION,
        source: source.expect("two admitted entries establish a source"),
        targets,
    };
    let bytes =
        wire::encode(&manifest).map_err(|source| ManifestError::Encode(source.to_string()))?;
    let digest = Digest::compute(Domain::DistReleaseManifest, &bytes);
    objects.push((object_path("manifest", &digest), bytes.as_slice()));
    objects.sort_by(|left, right| left.0.cmp(&right.0));
    for (path, object) in objects {
        publisher
            .publish(&path, DATA_MODE, object)
            .map_err(|source| ManifestError::Publish { path, source })?;
    }

    let pointer = PathBuf::from(MANIFEST_POINTER);
    publisher
        .publish(&pointer, DATA_MODE, &bytes)
        .map_err(|source| ManifestError::Publish {
            path: pointer.clone(),
            source,
        })?;
    Ok(PublishedManifest {
        manifest,
        bytes,
        digest,
        pointer,
    })
}

fn object_path(kind: &str, digest: &Digest) -> PathBuf {
    PathBuf::from(format!("objects/{kind}-{digest}.json"))
}

fn validate_relative(path: &Path) -> Result<(), ManifestPublishError> {
    let valid = !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if valid {
        Ok(())
    } else {
        Err(ManifestPublishError::InvalidPath(path.to_path_buf()))
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), ManifestPublishError> {
    use std::os::unix::fs::PermissionsExt as _;

    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|source| {
        ManifestPublishError::Io {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), ManifestPublishError> {
    Ok(())
}

#[derive(Debug)]
pub enum ManifestError {
    EntryCount(usize),
    Payload {
        index: usize,
        source: PayloadError,
    },
    TargetOrder {
        index: usize,
        expected: ReleaseTarget,
        observed: ReleaseTarget,
    },
    DuplicateTarget(ReleaseTarget),
    SourceMismatch {
        first: Digest,
        observed: Digest,
    },
    Mixed {
        field: &'static str,
    },
    Evidence {
        index: usize,
        source: EvidenceError,
    },
    Encode(String),
    Publish {
        path: PathBuf,
        source: ManifestPublishError,
    },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntryCount(count) => write!(
                formatter,
                "release manifest requires exactly two target proofs, found {count}"
            ),
            Self::Payload { index, source } => {
                write!(
                    formatter,
                    "release manifest payload {index} is refused: {source}"
                )
            }
            Self::TargetOrder {
                index,
                expected,
                observed,
            } => write!(
                formatter,
                "release manifest target {index} is {observed}, expected {expected}"
            ),
            Self::DuplicateTarget(target) => {
                write!(formatter, "release manifest repeats target {target}")
            }
            Self::SourceMismatch { first, observed } => write!(
                formatter,
                "release manifest source {observed} disagrees with first source {first}"
            ),
            Self::Mixed { field } => {
                write!(formatter, "release manifest mixes target {field}")
            }
            Self::Evidence { index, source } => {
                write!(
                    formatter,
                    "release manifest proof {index} is refused: {source}"
                )
            }
            Self::Encode(source) => write!(formatter, "release manifest does not encode: {source}"),
            Self::Publish { path, source } => write!(
                formatter,
                "release manifest publication failed at {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for ManifestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Payload { source, .. } => Some(source),
            Self::Evidence { source, .. } => Some(source),
            Self::Publish { source, .. } => Some(source),
            Self::EntryCount(_)
            | Self::TargetOrder { .. }
            | Self::DuplicateTarget(_)
            | Self::SourceMismatch { .. }
            | Self::Mixed { .. }
            | Self::Encode(_) => None,
        }
    }
}

/// Why a sealed release pointer or one of its immutable objects was refused.
#[derive(Debug)]
pub enum ManifestAdmissionError {
    Input {
        label: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Symlink {
        label: &'static str,
        path: PathBuf,
    },
    NotRegular {
        label: &'static str,
        path: PathBuf,
    },
    NotDirectory {
        label: &'static str,
        path: PathBuf,
    },
    OutsideRoot {
        label: &'static str,
        path: PathBuf,
    },
    StageSymlink(PathBuf),
    StageNotDirectory(PathBuf),
    ManifestDecode(String),
    ManifestEncode(String),
    ManifestNonCanonical,
    ManifestVersion(u16),
    ManifestObject,
    EntryCount(usize),
    TargetOrder {
        index: usize,
        expected: ReleaseTarget,
        observed: ReleaseTarget,
    },
    Payload {
        index: usize,
        source: PayloadError,
    },
    Receipt {
        index: usize,
        source: EvidenceError,
    },
    Binding {
        index: usize,
        field: &'static str,
    },
    Mixed {
        field: &'static str,
    },
}

impl fmt::Display for ManifestAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input {
                label,
                path,
                source,
            } => write!(
                formatter,
                "{label} {} is unreadable: {source}",
                path.display()
            ),
            Self::Symlink { label, path } => {
                write!(formatter, "{label} {} is a symlink", path.display())
            }
            Self::NotRegular { label, path } => {
                write!(
                    formatter,
                    "{label} {} is not a regular file",
                    path.display()
                )
            }
            Self::NotDirectory { label, path } => {
                write!(
                    formatter,
                    "{label} parent {} is not a directory",
                    path.display()
                )
            }
            Self::OutsideRoot { label, path } => {
                write!(
                    formatter,
                    "{label} {} escapes the release stage",
                    path.display()
                )
            }
            Self::StageSymlink(path) => {
                write!(formatter, "release stage {} is a symlink", path.display())
            }
            Self::StageNotDirectory(path) => write!(
                formatter,
                "release stage {} is not a directory",
                path.display()
            ),
            Self::ManifestDecode(source) => {
                write!(formatter, "release manifest does not decode: {source}")
            }
            Self::ManifestEncode(source) => {
                write!(formatter, "release manifest does not encode: {source}")
            }
            Self::ManifestNonCanonical => {
                formatter.write_str("release manifest pointer is not canonical")
            }
            Self::ManifestVersion(version) => {
                write!(
                    formatter,
                    "release manifest version {version} is not current"
                )
            }
            Self::ManifestObject => {
                formatter.write_str("release manifest object disagrees with the current pointer")
            }
            Self::EntryCount(count) => write!(
                formatter,
                "release manifest requires exactly two targets, found {count}"
            ),
            Self::TargetOrder {
                index,
                expected,
                observed,
            } => write!(
                formatter,
                "release manifest target {index} is {observed}, expected {expected}"
            ),
            Self::Payload { index, source } => {
                write!(formatter, "release payload {index} is refused: {source}")
            }
            Self::Receipt { index, source } => {
                write!(formatter, "release receipt {index} is refused: {source}")
            }
            Self::Binding { index, field } => {
                write!(
                    formatter,
                    "release target {index} has a stale or mismatched {field}"
                )
            }
            Self::Mixed { field } => write!(formatter, "release targets mix {field}"),
        }
    }
}

impl Error for ManifestAdmissionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Input { source, .. } => Some(source),
            Self::Payload { source, .. } => Some(source),
            Self::Receipt { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub enum ManifestPublishError {
    InvalidPath(PathBuf),
    Io { path: PathBuf, source: io::Error },
    Atomic(atomic::Error),
}

impl fmt::Display for ManifestPublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(path) => {
                write!(formatter, "invalid manifest path {}", path.display())
            }
            Self::Io { path, source } => {
                write!(
                    formatter,
                    "manifest I/O failed at {}: {source}",
                    path.display()
                )
            }
            Self::Atomic(source) => {
                write!(formatter, "manifest atomic publication failed: {source}")
            }
        }
    }
}

impl Error for ManifestPublishError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Atomic(source) => Some(source),
            Self::InvalidPath(_) => None,
        }
    }
}

impl From<atomic::Error> for ManifestPublishError {
    fn from(source: atomic::Error) -> Self {
        Self::Atomic(source)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use wire::manifest::{
        BuildPins, InventoryEntry, ProofEnvironment, ProofReceipt, ReleaseManifest, ReleaseTarget,
        TargetPayload,
    };
    use wire::{Digest, Domain};

    use super::{
        FileManifestPublisher, MANIFEST_POINTER, ManifestAdmissionError, ManifestError,
        ManifestInput, ManifestPublishError, ManifestPublisher, admit_manifest, object_path,
        publish_manifest,
    };
    use crate::{Battery, CheckKind, CheckStanding, EvidenceError, ReplayVerdict};

    const OLD_POINTER: &[u8] = b"old release manifest";
    static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

    struct Prepared {
        payload: Vec<u8>,
        receipt: Vec<u8>,
        replay: ReplayVerdict,
    }

    #[derive(Default)]
    struct MemoryPublisher {
        files: BTreeMap<PathBuf, (u32, Vec<u8>)>,
        writes: Vec<PathBuf>,
        fail_at: Option<usize>,
    }

    impl ManifestPublisher for MemoryPublisher {
        fn publish(
            &mut self,
            path: &Path,
            mode: u32,
            bytes: &[u8],
        ) -> Result<(), ManifestPublishError> {
            let attempt = self.writes.len();
            self.writes.push(path.to_path_buf());
            if self.fail_at == Some(attempt) {
                return Err(ManifestPublishError::Io {
                    path: path.to_path_buf(),
                    source: io::Error::other("fixture interruption"),
                });
            }
            self.files
                .insert(path.to_path_buf(), (mode, bytes.to_vec()));
            Ok(())
        }
    }

    #[test]
    fn both_verified_targets_form_one_stable_manifest_and_pointer_last() {
        let source = digest(Domain::DistArtifact, b"source");
        let darwin = prepared(ReleaseTarget::Aarch64AppleDarwin, source.clone(), true);
        let linux = prepared(ReleaseTarget::X86_64UnknownLinuxMusl, source.clone(), true);
        let mut first = MemoryPublisher::default();
        let published = publish_manifest(inputs(&darwin, &linux), &mut first)
            .expect("both clean proofs publish");
        let mut second = MemoryPublisher::default();
        let repeated = publish_manifest(inputs(&darwin, &linux), &mut second)
            .expect("the same proofs publish identically");

        assert_eq!(published, repeated);
        assert_eq!(first.files, second.files);
        assert_eq!(published.manifest.source, source);
        assert_eq!(published.manifest.targets.len(), 2);
        assert_eq!(
            published
                .manifest
                .targets
                .iter()
                .map(|entry| entry.target)
                .collect::<Vec<_>>(),
            [
                ReleaseTarget::Aarch64AppleDarwin,
                ReleaseTarget::X86_64UnknownLinuxMusl,
            ]
        );
        assert_eq!(
            published.digest,
            Digest::compute(Domain::DistReleaseManifest, &published.bytes)
        );
        let decoded: ReleaseManifest =
            wire::decode(&published.bytes).expect("manifest bytes are canonical wire");
        assert_eq!(decoded, published.manifest);
        assert_eq!(first.writes.last(), Some(&published.pointer));
        assert_eq!(published.pointer, PathBuf::from(MANIFEST_POINTER));
        assert!(
            first.writes[..first.writes.len() - 1]
                .iter()
                .all(|path| path.starts_with("objects")),
            "every immutable object precedes the pointer"
        );
    }

    #[test]
    fn missing_extra_duplicate_and_reordered_targets_refuse_before_publication() {
        let source = digest(Domain::DistArtifact, b"source");
        let darwin = prepared(ReleaseTarget::Aarch64AppleDarwin, source.clone(), true);
        let linux = prepared(ReleaseTarget::X86_64UnknownLinuxMusl, source, true);

        let cases = [
            (
                vec![input(&darwin)],
                "missing",
                matches_error as fn(&ManifestError) -> bool,
            ),
            (
                vec![input(&darwin), input(&linux), input(&darwin)],
                "extra",
                matches_error,
            ),
            (
                vec![input(&darwin), input(&darwin)],
                "duplicate",
                duplicate_error,
            ),
            (
                vec![input(&linux), input(&darwin)],
                "reordered",
                order_error,
            ),
        ];

        for (inputs, label, expected) in cases {
            let mut publisher = MemoryPublisher::default();
            let error = publish_manifest(inputs, &mut publisher)
                .expect_err("invalid target set must refuse");
            assert!(expected(&error), "{label} produced {error:?}");
            assert!(publisher.writes.is_empty(), "{label} wrote before refusal");
        }
    }

    fn matches_error(error: &ManifestError) -> bool {
        matches!(error, ManifestError::EntryCount(_))
    }

    fn duplicate_error(error: &ManifestError) -> bool {
        matches!(error, ManifestError::DuplicateTarget(_))
    }

    fn order_error(error: &ManifestError) -> bool {
        matches!(error, ManifestError::TargetOrder { .. })
    }

    #[test]
    fn mixed_sources_mismatched_evidence_and_non_clean_proofs_refuse_without_writes() {
        let first_source = digest(Domain::DistArtifact, b"first source");
        let other_source = digest(Domain::DistArtifact, b"other source");
        let darwin = prepared(
            ReleaseTarget::Aarch64AppleDarwin,
            first_source.clone(),
            true,
        );
        let linux = prepared(ReleaseTarget::X86_64UnknownLinuxMusl, first_source, true);
        let other_linux = prepared(ReleaseTarget::X86_64UnknownLinuxMusl, other_source, true);
        let drifted_linux = drift_toolchain(&linux);
        let refused_linux = prepared(
            ReleaseTarget::X86_64UnknownLinuxMusl,
            digest(Domain::DistArtifact, b"first source"),
            false,
        );

        let mut publisher = MemoryPublisher::default();
        let mixed = publish_manifest(inputs(&darwin, &other_linux), &mut publisher)
            .expect_err("mixed sources refuse");
        assert!(matches!(mixed, ManifestError::SourceMismatch { .. }));
        assert!(publisher.writes.is_empty());

        let mut publisher = MemoryPublisher::default();
        let mixed = publish_manifest(inputs(&darwin, &drifted_linux), &mut publisher)
            .expect_err("mixed toolchain pins refuse");
        assert!(matches!(mixed, ManifestError::Mixed { .. }));
        assert!(publisher.writes.is_empty());

        let mismatched = ManifestInput::new(&darwin.payload, &linux.receipt, &linux.replay);
        let mut publisher = MemoryPublisher::default();
        let mismatch = publish_manifest([mismatched, input(&linux)], &mut publisher)
            .expect_err("cross-target evidence refuses");
        assert!(matches!(mismatch, ManifestError::Evidence { .. }));
        assert!(publisher.writes.is_empty());

        let mut publisher = MemoryPublisher::default();
        let refused = publish_manifest(inputs(&darwin, &refused_linux), &mut publisher)
            .expect_err("non-clean proof refuses");
        assert!(matches!(
            refused,
            ManifestError::Evidence {
                source: EvidenceError::Refused,
                ..
            }
        ));
        assert!(publisher.writes.is_empty());
    }

    #[test]
    fn every_receipt_binding_is_rechecked_before_the_first_write() {
        let source = digest(Domain::DistArtifact, b"source");
        let darwin = prepared(ReleaseTarget::Aarch64AppleDarwin, source.clone(), true);
        let linux = prepared(ReleaseTarget::X86_64UnknownLinuxMusl, source, true);

        for field in 0..5 {
            let mut receipt: ProofReceipt =
                wire::decode(&linux.receipt).expect("fixture receipt decodes");
            match field {
                0 => receipt.target = ReleaseTarget::Aarch64AppleDarwin,
                1 => receipt.payload = digest(Domain::DistTargetPayload, b"other payload"),
                2 => receipt.battery = digest(Domain::ProofBattery, b"other battery"),
                3 => receipt.replay = digest(Domain::ProofReplayV2, b"other replay"),
                4 => {
                    receipt.environment = ProofEnvironment::native(digest(
                        Domain::ProofNativeRunner,
                        b"wrong environment",
                    ));
                }
                _ => unreachable!("the field sweep is closed"),
            }
            let receipt = wire::encode(&receipt).expect("tampered receipt remains canonical");
            let tampered = ManifestInput::new(&linux.payload, &receipt, &linux.replay);
            let mut publisher = MemoryPublisher::default();

            let error = publish_manifest([input(&darwin), tampered], &mut publisher)
                .expect_err("every receipt binding is load-bearing");

            assert!(matches!(error, ManifestError::Evidence { .. }));
            assert!(
                publisher.writes.is_empty(),
                "receipt field {field} wrote before refusal"
            );
        }
    }

    #[test]
    fn interrupted_object_publication_preserves_the_previous_pointer() {
        let source = digest(Domain::DistArtifact, b"source");
        let darwin = prepared(ReleaseTarget::Aarch64AppleDarwin, source.clone(), true);
        let linux = prepared(ReleaseTarget::X86_64UnknownLinuxMusl, source, true);
        let pointer = PathBuf::from(MANIFEST_POINTER);
        let mut publisher = MemoryPublisher {
            files: BTreeMap::from([(pointer.clone(), (0o644, OLD_POINTER.to_vec()))]),
            writes: Vec::new(),
            fail_at: Some(2),
        };

        let error = publish_manifest(inputs(&darwin, &linux), &mut publisher)
            .expect_err("interrupted object write fails");

        assert!(matches!(error, ManifestError::Publish { .. }));
        assert_eq!(publisher.files[&pointer].1, OLD_POINTER);
        assert!(!publisher.writes.contains(&pointer));
    }

    #[test]
    fn filesystem_publication_commits_exact_objects_and_pointer() {
        let scratch = Scratch::new();
        let source = digest(Domain::DistArtifact, b"source");
        let darwin = prepared(ReleaseTarget::Aarch64AppleDarwin, source.clone(), true);
        let linux = prepared(ReleaseTarget::X86_64UnknownLinuxMusl, source, true);
        let mut publisher = FileManifestPublisher::new(&scratch.root);

        let published = publish_manifest(inputs(&darwin, &linux), &mut publisher)
            .expect("filesystem publication succeeds");
        let darwin_payload: TargetPayload =
            wire::decode(&darwin.payload).expect("Darwin payload decodes");
        let admitted = admit_manifest(
            &scratch.root,
            &darwin_payload.embedded_app,
            ReleaseTarget::Aarch64AppleDarwin,
            &darwin_payload.native,
        )
        .expect("the complete published object graph admits");

        assert_eq!(admitted, published);
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let linked = scratch.root.join("linked-stage");
            symlink(&scratch.root, &linked).expect("stage symlink fixture");
            assert!(matches!(
                admit_manifest(
                    &linked,
                    &darwin_payload.embedded_app,
                    ReleaseTarget::Aarch64AppleDarwin,
                    &darwin_payload.native,
                ),
                Err(ManifestAdmissionError::StageSymlink(_))
            ));
        }
        assert_eq!(
            fs::read(scratch.root.join(&published.pointer)).expect("pointer is readable"),
            published.bytes
        );
        let objects = fs::read_dir(scratch.root.join("objects"))
            .expect("objects directory is readable")
            .collect::<Result<Vec<_>, _>>()
            .expect("every object entry is readable");
        assert_eq!(objects.len(), 5, "two payloads, two receipts, one manifest");
        assert!(
            objects
                .iter()
                .all(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            assert_eq!(
                fs::metadata(scratch.root.join(&published.pointer))
                    .expect("pointer metadata is readable")
                    .permissions()
                    .mode()
                    & 0o777,
                0o644
            );
        }
    }

    #[test]
    fn manifest_admission_refuses_missing_stale_and_mixed_objects() {
        let source = digest(Domain::DistArtifact, b"source");
        let darwin = prepared(ReleaseTarget::Aarch64AppleDarwin, source.clone(), true);
        let linux = prepared(ReleaseTarget::X86_64UnknownLinuxMusl, source, true);
        let expected: TargetPayload = wire::decode(&darwin.payload).expect("payload decodes");

        let missing = Scratch::new();
        let mut publisher = FileManifestPublisher::new(&missing.root);
        let published =
            publish_manifest(inputs(&darwin, &linux), &mut publisher).expect("fixture publishes");
        fs::remove_file(missing.root.join(object_path(
            "receipt",
            &published.manifest.targets[1].receipt,
        )))
        .expect("remove receipt object");
        assert!(matches!(
            admit_manifest(
                &missing.root,
                &expected.embedded_app,
                expected.target,
                &expected.native,
            ),
            Err(ManifestAdmissionError::Input { .. })
        ));

        let stale = Scratch::new();
        let mut publisher = FileManifestPublisher::new(&stale.root);
        publish_manifest(inputs(&darwin, &linux), &mut publisher).expect("fixture publishes");
        let mut pointer: ReleaseManifest =
            wire::decode(&fs::read(stale.root.join(MANIFEST_POINTER)).expect("read pointer"))
                .expect("pointer decodes");
        pointer.version += 1;
        fs::write(
            stale.root.join(MANIFEST_POINTER),
            wire::encode(&pointer).expect("stale pointer encodes"),
        )
        .expect("write stale pointer");
        assert!(matches!(
            admit_manifest(
                &stale.root,
                &expected.embedded_app,
                expected.target,
                &expected.native,
            ),
            Err(ManifestAdmissionError::ManifestVersion(_))
        ));

        let mixed = Scratch::new();
        let mut publisher = FileManifestPublisher::new(&mixed.root);
        let published =
            publish_manifest(inputs(&darwin, &linux), &mut publisher).expect("fixture publishes");
        fs::write(
            mixed.root.join(object_path(
                "payload",
                &published.manifest.targets[1].payload,
            )),
            &darwin.payload,
        )
        .expect("replace Linux object with Darwin bytes");
        assert!(matches!(
            admit_manifest(
                &mixed.root,
                &expected.embedded_app,
                expected.target,
                &expected.native,
            ),
            Err(ManifestAdmissionError::Binding { .. })
        ));
    }

    #[test]
    fn filesystem_publisher_refuses_every_path_outside_its_root() {
        let scratch = Scratch::new();
        let mut publisher = FileManifestPublisher::new(&scratch.root);

        for path in [Path::new(""), Path::new("../escape"), Path::new("/escape")] {
            assert!(matches!(
                publisher.publish(path, 0o644, b"escape"),
                Err(ManifestPublishError::InvalidPath(observed)) if observed == path
            ));
        }
        assert_eq!(
            fs::read_dir(&scratch.root)
                .expect("scratch remains readable")
                .count(),
            0,
            "a refused path creates nothing"
        );
    }

    struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "spice-proof-manifest-{}-{}",
                std::process::id(),
                NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("unique scratch root is created");
            Self { root }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).expect("scratch root is removed");
        }
    }

    fn inputs<'a>(darwin: &'a Prepared, linux: &'a Prepared) -> [ManifestInput<'a>; 2] {
        [input(darwin), input(linux)]
    }

    fn input(prepared: &Prepared) -> ManifestInput<'_> {
        ManifestInput::new(&prepared.payload, &prepared.receipt, &prepared.replay)
    }

    fn prepared(target: ReleaseTarget, source: Digest, clean: bool) -> Prepared {
        let app = digest(Domain::DistArtifact, b"app");
        let embedded_app = digest(Domain::DistArtifact, b"embedded app");
        let native = digest(Domain::DistArtifact, target.as_str().as_bytes());
        let pins = BuildPins {
            rust: "rustc 1.97.1".to_owned(),
            wasm_target: "wasm32-unknown-unknown".to_owned(),
            trunk: "trunk 0.21.14".to_owned(),
            cargo_mutants: "cargo-mutants 27.1.0".to_owned(),
            proof_container: format!("registry.example/spice-proof@sha256:{}", "0".repeat(64)),
        };
        let inventory = vec![
            InventoryEntry {
                path: "app/app.wasm".to_owned(),
                mode: 0o644,
                bytes: 3,
                digest: app.clone(),
            },
            InventoryEntry {
                path: "bin/spice".to_owned(),
                mode: 0o755,
                bytes: 3,
                digest: native.clone(),
            },
            InventoryEntry {
                path: "embed/app.bin".to_owned(),
                mode: 0o644,
                bytes: 3,
                digest: embedded_app.clone(),
            },
        ];
        let payload = TargetPayload {
            version: 1,
            target,
            source,
            app,
            embedded_app,
            native,
            pins_digest: digest(Domain::DistPins, &wire::encode(&pins).expect("pins encode")),
            inventory_digest: digest(
                Domain::DistInventory,
                &wire::encode(&inventory).expect("inventory encodes"),
            ),
            pins,
            inventory,
        };
        let payload_bytes = wire::encode(&payload).expect("payload encodes");
        let payload_digest = digest(Domain::DistTargetPayload, &payload_bytes);
        let environment = match target {
            ReleaseTarget::Aarch64AppleDarwin => {
                ProofEnvironment::native(digest(Domain::ProofNativeRunner, b"native runner"))
            }
            ReleaseTarget::X86_64UnknownLinuxMusl => {
                ProofEnvironment::oci(&payload.pins.proof_container)
                    .expect("fixture OCI image is pinned")
            }
        };
        let battery = Battery::current().digest().clone();
        let replay = if clean {
            ReplayVerdict::fixture_clean(target, environment, battery)
        } else {
            let mut standings = [CheckStanding::Clean; CheckKind::ALL.len()];
            standings[0] = CheckStanding::Failed { code: Some(1) };
            ReplayVerdict::fixture_with(target, environment, battery, standings)
        };
        let receipt = replay
            .receipt(&payload, payload_digest)
            .expect("matching replay makes a receipt");
        Prepared {
            payload: payload_bytes,
            receipt: wire::encode(&receipt).expect("receipt encodes"),
            replay,
        }
    }

    fn drift_toolchain(prepared: &Prepared) -> Prepared {
        let mut payload: TargetPayload =
            wire::decode(&prepared.payload).expect("fixture payload decodes");
        payload.pins.trunk.push_str("-drifted");
        payload.pins_digest = digest(
            Domain::DistPins,
            &wire::encode(&payload.pins).expect("drifted pins encode"),
        );
        let payload = wire::encode(&payload).expect("drifted payload encodes");
        let receipt = prepared
            .replay
            .receipt(
                &wire::decode(&payload).expect("drifted payload decodes"),
                digest(Domain::DistTargetPayload, &payload),
            )
            .expect("drifted payload remains target matched");
        Prepared {
            payload,
            receipt: wire::encode(&receipt).expect("drifted receipt encodes"),
            replay: prepared.replay.clone(),
        }
    }

    fn digest(domain: Domain, bytes: &[u8]) -> Digest {
        Digest::compute(domain, bytes)
    }
}
