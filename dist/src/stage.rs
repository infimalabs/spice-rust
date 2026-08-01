//! Ordered builds and manifest-last atomic staging.

use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use atomic::Destination;
use wire::manifest::{BuildPins, InventoryEntry, ReleaseTarget, TargetPayload};
use wire::{Digest, Domain};

use crate::pins::{PinError, RequiredPins};

const PAYLOAD_VERSION: u16 = 1;
const DATA_MODE: u32 = 0o644;
const EXECUTABLE_MODE: u32 = 0o755;

/// Build work occurs in this fixed order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuildStep {
    App,
    Embed,
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
    fn build_app(&mut self) -> Result<Vec<u8>, BuildError>;
    fn embed_app(&mut self, app: &[u8]) -> Result<Vec<u8>, BuildError>;
    fn build_native(
        &mut self,
        target: ReleaseTarget,
        embedded_app: &[u8],
    ) -> Result<Vec<u8>, BuildError>;
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
}

/// Pin-guarded deterministic stage assembly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pipeline {
    pins: RequiredPins,
}

impl Pipeline {
    #[must_use]
    pub const fn new(pins: RequiredPins) -> Self {
        Self { pins }
    }

    /// Build and publish one target. The payload pointer is always last.
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
        let artifacts = build_artifacts(target, builder)?;
        let payload = make_payload(target, source, self.pins.get(), &artifacts)?;
        let payload_bytes = wire::encode(&payload)?;
        let payload_digest = Digest::compute(Domain::TargetPayload, &payload_bytes);
        publish_artifacts(publisher, &artifacts)?;
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
    #[error("stage I/O failed at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error(transparent)]
    Atomic(#[from] atomic::Error),
}

#[derive(Clone, Debug)]
struct Artifact {
    path: &'static str,
    mode: u32,
    bytes: Vec<u8>,
    digest: Digest,
}

fn build_artifacts<B: Builder>(
    target: ReleaseTarget,
    builder: &mut B,
) -> Result<Vec<Artifact>, StageError> {
    let app = builder.build_app().map_err(|source| StageError::Build {
        step: BuildStep::App,
        source,
    })?;
    let embedded = builder
        .embed_app(&app)
        .map_err(|source| StageError::Build {
            step: BuildStep::Embed,
            source,
        })?;
    let native = builder
        .build_native(target, &embedded)
        .map_err(|source| StageError::Build {
            step: BuildStep::Native,
            source,
        })?;
    Ok(vec![
        artifact("app/app.wasm", DATA_MODE, app),
        artifact("embed/app.bin", DATA_MODE, embedded),
        artifact("bin/spice", EXECUTABLE_MODE, native),
    ])
}

fn artifact(path: &'static str, mode: u32, bytes: Vec<u8>) -> Artifact {
    let digest = Digest::compute(Domain::Artifact, &bytes);
    Artifact {
        path,
        mode,
        bytes,
        digest,
    }
}

fn make_payload(
    target: ReleaseTarget,
    source: Digest,
    pins: &BuildPins,
    artifacts: &[Artifact],
) -> Result<TargetPayload, serde_json::Error> {
    let mut inventory = artifacts.iter().map(inventory_entry).collect::<Vec<_>>();
    inventory.sort_by(|left, right| left.path.cmp(&right.path));
    let pins_digest = Digest::compute(Domain::Pins, &wire::encode(pins)?);
    let inventory_digest = Digest::compute(Domain::Inventory, &wire::encode(&inventory)?);
    Ok(TargetPayload {
        version: PAYLOAD_VERSION,
        target,
        source,
        app: artifacts[0].digest.clone(),
        embedded_app: artifacts[1].digest.clone(),
        native: artifacts[2].digest.clone(),
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
    ordered.sort_by_key(|artifact| artifact.path);
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

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), PublishError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|source| PublishError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), PublishError> {
    Ok(())
}
