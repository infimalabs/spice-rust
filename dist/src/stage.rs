//! Ordered builds and manifest-last atomic staging.

use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use atomic::Destination;
use wire::manifest::{
    BuildPins, InventoryEntry, MAC_SYMBOL_ARTIFACT_VERSION, MacSymbolArtifact, MacSymbolMetadata,
    ProductVersion, ReleaseTarget, TARGET_PAYLOAD_VERSION, TargetPayload,
};
use wire::{Digest, Domain};

use crate::canvas::Canvas;
use crate::pins::{PinError, RequiredPins};

const DATA_MODE: u32 = 0o644;
const EXECUTABLE_MODE: u32 = 0o755;
const MAC_SYMBOL_PATH: &str = "symbols/spice.dSYM.json";
const WINDOWS_DEBUG_PATH: &str = "symbols/spice.pdb";

/// Build work occurs in this fixed order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuildStep {
    App,
    Native,
}

/// A bounded failure from a concrete build adapter.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct BuildError {
    message: String,
}

impl BuildError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Heavyweight build implementation supplied by the executable rim.
pub trait Builder {
    /// Build the app and admit its staged canvas archive as generation G.
    fn build_app(&mut self) -> Result<Canvas, BuildError>;
    /// Link `canvas` into the executable, then derive symbols, strip, and sign
    /// it, and answer the final bytes.
    fn build_native(
        &mut self,
        target: ReleaseTarget,
        canvas: &Canvas,
    ) -> Result<Vec<u8>, BuildError>;
    /// Run the final executable once and answer its version report.
    fn report_version(&mut self) -> Result<String, BuildError>;

    fn take_symbol_artifact(&mut self) -> Option<Vec<u8>> {
        None
    }
    /// Take the raw PDB produced beside a Windows release executable.
    fn take_windows_debug_artifact(&mut self) -> Option<Vec<u8>> {
        None
    }
}

/// Publication boundary used by the real stage and interruption fixtures.
pub trait Publisher {
    fn publish(&mut self, path: &Path, mode: u32, bytes: &[u8]) -> Result<(), PublishError>;
}

/// Filesystem publication rooted at one canonical stage directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilePublisher {
    root: PathBuf,
}

impl FilePublisher {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Publisher for FilePublisher {
    fn publish(&mut self, path: &Path, mode: u32, bytes: &[u8]) -> Result<(), PublishError> {
        validate_relative(path)?;
        let destination = self.root.join(path);
        let parent = destination
            .parent()
            .ok_or_else(|| PublishError::InvalidPath(path.to_path_buf()))?;
        fs::create_dir_all(parent).map_err(|source| PublishError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let mut staged = Destination::new(&destination)?.stage()?;
        staged.write_all(bytes).map_err(|source| PublishError::Io {
            path: staged.temporary_path().to_path_buf(),
            source,
        })?;
        set_mode(staged.temporary_path(), mode)?;
        staged.commit()?;
        Ok(())
    }
}

/// One completed, receipt-free target stage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedTarget {
    pub payload: TargetPayload,
    pub payload_bytes: Vec<u8>,
    pub payload_digest: Digest,
    pub payload_path: PathBuf,
    /// The canvas archive linked into the staged executable.
    pub canvas: Canvas,
}

/// Pin- and version-guarded deterministic stage assembly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pipeline {
    pins: RequiredPins,
    version: ProductVersion,
}

impl Pipeline {
    #[must_use]
    pub const fn new(pins: RequiredPins, version: ProductVersion) -> Self {
        Self { pins, version }
    }

    /// Build and publish one target. The payload pointer is always last.
    ///
    /// Pin verification is a release-path check, not a hot-path one, and it is
    /// ordered first deliberately: it compares the observed pins against the
    /// required ones in memory, with no I/O, so the cost of catching drift is
    /// nothing next to the build it refuses to start.
    ///
    /// # Errors
    ///
    /// Refuses pin drift before calling the builder, names the failed build
    /// step, and preserves a previously published payload when object
    /// publication is interrupted.
    pub fn stage<B: Builder, P: Publisher>(
        &self,
        target: ReleaseTarget,
        source: Digest,
        observed: &BuildPins,
        builder: &mut B,
        publisher: &mut P,
    ) -> Result<StagedTarget, StageError> {
        self.pins.verify(observed)?;
        let built = build_artifacts(target, &self.version, builder)?;
        let payload = make_payload(
            target,
            source,
            &built.canvas,
            built.symbols,
            self.pins.get(),
            &built.artifacts,
        )?;
        let payload_bytes = wire::encode(&payload)?;
        let payload_digest = Digest::compute(Domain::DistTargetPayload, &payload_bytes);
        publish_artifacts(publisher, &built.artifacts)?;
        let payload_path = PathBuf::from(format!("targets/{target}/payload.json"));
        publisher
            .publish(&payload_path, DATA_MODE, &payload_bytes)
            .map_err(|source| StageError::Publish {
                path: payload_path.clone(),
                source,
            })?;
        Ok(StagedTarget {
            payload,
            payload_bytes,
            payload_digest,
            payload_path,
            canvas: built.canvas,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StageError {
    #[error(transparent)]
    Pin(#[from] PinError),
    #[error("{step:?} build failed: {source}")]
    Build { step: BuildStep, source: BuildError },
    #[error("failed to publish {path}: {source}")]
    Publish { path: PathBuf, source: PublishError },
    #[error("target payload encoding failed: {0}")]
    Encode(#[from] serde_json::Error),
}

/// A typed publication failure.
#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error("stage path must be relative and contain no parent traversal: {0}")]
    InvalidPath(PathBuf),
    #[error("stage publication refused at {path}: {reason}")]
    Guard { path: PathBuf, reason: String },
    #[error("stage I/O failed at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error(transparent)]
    Atomic(#[from] atomic::Error),
}

#[derive(Clone, Debug)]
struct Artifact {
    path: String,
    mode: u32,
    bytes: Vec<u8>,
    digest: Digest,
}

struct BuiltArtifacts {
    canvas: Canvas,
    symbols: Option<MacSymbolMetadata>,
    artifacts: Vec<Artifact>,
}

fn build_artifacts<B: Builder>(
    target: ReleaseTarget,
    version: &ProductVersion,
    builder: &mut B,
) -> Result<BuiltArtifacts, StageError> {
    let canvas = builder.build_app().map_err(|source| StageError::Build {
        step: BuildStep::App,
        source,
    })?;
    let native = builder
        .build_native(target, &canvas)
        .map_err(|source| StageError::Build {
            step: BuildStep::Native,
            source,
        })?;
    canvas.verify_linked(&native).map_err(native_build_error)?;
    if target == ReleaseTarget::X86_64PcWindowsMsvc {
        canvas
            .verify_windows_authenticode(&native)
            .map_err(native_build_error)?;
    }
    let report = builder
        .report_version()
        .map_err(|source| StageError::Build {
            step: BuildStep::Native,
            source,
        })?;
    canvas
        .verify_release_report(version, &report)
        .map_err(native_build_error)?;
    let symbols = builder.take_symbol_artifact();
    let windows_debug = builder.take_windows_debug_artifact();
    let mut artifacts = Vec::new();
    let native = artifact(target.executable_path(), EXECUTABLE_MODE, native);
    let symbols = match (target, symbols) {
        (ReleaseTarget::Aarch64AppleDarwin, Some(bytes)) => {
            let (artifact, metadata) = symbol_artifact(bytes, &native.digest)?;
            artifacts.push(artifact);
            Some(metadata)
        }
        (ReleaseTarget::Aarch64AppleDarwin, None) => {
            return Err(native_build_error(
                "macOS build produced no symbol artifact",
            ));
        }
        (ReleaseTarget::X86_64UnknownLinuxMusl | ReleaseTarget::X86_64PcWindowsMsvc, None) => None,
        (ReleaseTarget::X86_64UnknownLinuxMusl, Some(_)) => {
            return Err(native_build_error("Linux build produced a symbol artifact"));
        }
        (ReleaseTarget::X86_64PcWindowsMsvc, Some(_)) => {
            return Err(native_build_error(
                "Windows build produced a macOS symbol artifact",
            ));
        }
    };
    match (target, windows_debug) {
        (ReleaseTarget::X86_64PcWindowsMsvc, Some(bytes)) if !bytes.is_empty() => {
            artifacts.push(artifact(WINDOWS_DEBUG_PATH, DATA_MODE, bytes));
        }
        (ReleaseTarget::X86_64PcWindowsMsvc, Some(_)) => {
            return Err(native_build_error("Windows build produced an empty PDB"));
        }
        (ReleaseTarget::X86_64PcWindowsMsvc, None) => {
            return Err(native_build_error("Windows build produced no PDB"));
        }
        (ReleaseTarget::Aarch64AppleDarwin | ReleaseTarget::X86_64UnknownLinuxMusl, None) => {}
        (_, Some(_)) => {
            return Err(native_build_error(
                "non-Windows build produced a Windows debug artifact",
            ));
        }
    }
    artifacts.push(native);
    Ok(BuiltArtifacts {
        canvas,
        symbols,
        artifacts,
    })
}

fn symbol_artifact(
    bytes: Vec<u8>,
    native: &Digest,
) -> Result<(Artifact, MacSymbolMetadata), StageError> {
    let document: MacSymbolArtifact = wire::decode(&bytes)
        .map_err(|error| native_build_error(format!("symbol artifact does not decode: {error}")))?;
    if document.version != MAC_SYMBOL_ARTIFACT_VERSION
        || document.executable != *native
        || wire::encode(&document).ok().as_deref() != Some(bytes.as_slice())
    {
        return Err(native_build_error(
            "symbol artifact is noncanonical or has stale executable binding",
        ));
    }
    let artifact = artifact(MAC_SYMBOL_PATH, DATA_MODE, bytes);
    let metadata = MacSymbolMetadata {
        path: artifact.path.clone(),
        mode: artifact.mode,
        bytes: artifact.bytes.len() as u64,
        digest: artifact.digest.clone(),
        uuid: document.uuid,
        executable: document.executable,
    };
    if !metadata.uuid_is_canonical() {
        return Err(native_build_error("symbol artifact UUID is not canonical"));
    }
    Ok((artifact, metadata))
}

fn native_build_error(message: impl ToString) -> StageError {
    StageError::Build {
        step: BuildStep::Native,
        source: BuildError::new(message.to_string()),
    }
}

fn artifact(path: impl Into<String>, mode: u32, bytes: Vec<u8>) -> Artifact {
    let digest = Digest::compute(Domain::DistArtifact, &bytes);
    Artifact {
        path: path.into(),
        mode,
        bytes,
        digest,
    }
}

fn make_payload(
    target: ReleaseTarget,
    source: Digest,
    canvas: &Canvas,
    symbols: Option<MacSymbolMetadata>,
    pins: &BuildPins,
    artifacts: &[Artifact],
) -> Result<TargetPayload, serde_json::Error> {
    let mut inventory = artifacts.iter().map(inventory_entry).collect::<Vec<_>>();
    inventory.sort_by(|left, right| left.path.cmp(&right.path));
    let pins_digest = Digest::compute(Domain::DistPins, &wire::encode(pins)?);
    let inventory_digest = Digest::compute(Domain::DistInventory, &wire::encode(&inventory)?);
    Ok(TargetPayload {
        version: TARGET_PAYLOAD_VERSION,
        target,
        source,
        app: canvas.generation().clone(),
        canvas: canvas.archive().clone(),
        native: artifacts
            .iter()
            .find(|artifact| artifact.path == target.executable_path())
            .expect("the built inventory carries the native executable")
            .digest
            .clone(),
        symbols,
        pins: pins.clone(),
        pins_digest,
        inventory,
        inventory_digest,
    })
}

fn inventory_entry(artifact: &Artifact) -> InventoryEntry {
    InventoryEntry {
        path: artifact.path.to_owned(),
        mode: artifact.mode,
        bytes: artifact.bytes.len() as u64,
        digest: artifact.digest.clone(),
    }
}

fn publish_artifacts<P: Publisher>(
    publisher: &mut P,
    artifacts: &[Artifact],
) -> Result<(), StageError> {
    let mut ordered = artifacts.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.path.cmp(&right.path));
    for artifact in ordered {
        let path = object_path(artifact);
        publisher
            .publish(&path, artifact.mode, &artifact.bytes)
            .map_err(|source| StageError::Publish { path, source })?;
    }
    Ok(())
}

fn object_path(artifact: &Artifact) -> PathBuf {
    PathBuf::from(format!("objects/{}-{:04o}", artifact.digest, artifact.mode))
}

fn validate_relative(path: &Path) -> Result<(), PublishError> {
    let valid = !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if valid {
        Ok(())
    } else {
        Err(PublishError::InvalidPath(path.to_path_buf()))
    }
}

fn set_mode(path: &Path, mode: u32) -> Result<(), PublishError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|source| {
            PublishError::Io {
                path: path.to_path_buf(),
                source,
            }
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}
