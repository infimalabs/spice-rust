use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;

use sill::{SpecError, WaitError};

/// Why a sealed environment was not established or executed.
#[derive(Debug)]
pub enum ContainerError {
    RuntimeNotAbsolute(PathBuf),
    InspectRuntime {
        path: PathBuf,
        source: io::Error,
    },
    RuntimeNotRegular(PathBuf),
    RuntimeNotExecutable(PathBuf),
    RuntimeProbeSpec(SpecError),
    SpawnRuntimeProbe(io::Error),
    WaitRuntimeProbe(WaitError),
    RuntimeDialectNotAttested {
        observed: String,
    },
    ImageNotDigestPinned(String),
    ImageNotResolved {
        image: String,
        observed: String,
    },
    ImageIdentityDrift {
        expected: String,
        observed: String,
    },
    InspectWorkspace {
        path: PathBuf,
        source: io::Error,
    },
    WorkspaceNotDirectory(PathBuf),
    UnrepresentableWorkspace(PathBuf),
    ZeroOutputLimit,
    EmptyProgram,
    NulInCommand,
    CreateAttestationChallenge(io::Error),
    Spec(SpecError),
    SpawnRuntime(io::Error),
    Wait(WaitError),
    MissingCapture(&'static str),
    RuntimeControlRefused {
        operation: &'static str,
        observed: String,
    },
    RuntimeNotAttested {
        expected: String,
        observed: String,
    },
}

impl fmt::Display for ContainerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeNotAbsolute(path) => {
                write!(
                    formatter,
                    "proof runtime path must be absolute: {}",
                    path.display()
                )
            }
            Self::InspectRuntime { path, source } => {
                write!(
                    formatter,
                    "cannot inspect proof runtime {}: {source}",
                    path.display()
                )
            }
            Self::RuntimeNotRegular(path) => {
                write!(
                    formatter,
                    "proof runtime is not a regular file: {}",
                    path.display()
                )
            }
            Self::RuntimeNotExecutable(path) => {
                write!(
                    formatter,
                    "proof runtime is not executable: {}",
                    path.display()
                )
            }
            Self::RuntimeProbeSpec(source) => {
                write!(
                    formatter,
                    "invalid sealed OCI runtime identity probe: {source}"
                )
            }
            Self::SpawnRuntimeProbe(source) => {
                write!(
                    formatter,
                    "cannot start sealed OCI runtime identity probe: {source}"
                )
            }
            Self::WaitRuntimeProbe(source) => {
                write!(
                    formatter,
                    "sealed OCI runtime identity probe failed: {source}"
                )
            }
            Self::RuntimeDialectNotAttested { observed } => write!(
                formatter,
                "proof runtime did not attest a Docker or Podman interface: {observed}"
            ),
            Self::ImageNotDigestPinned(image) => write!(
                formatter,
                "proof image must be an exact sha256 digest reference, not a tag: {image:?}"
            ),
            Self::ImageNotResolved { image, observed } => write!(
                formatter,
                "proof runtime does not hold proof image {image}, so load it with \
                 dist/proof-image/load.sh before sealing: {observed}"
            ),
            Self::ImageIdentityDrift { expected, observed } => write!(
                formatter,
                "proof runtime resolved a different appliance: expected {expected:?}, \
                 observed {observed:?}"
            ),
            Self::InspectWorkspace { path, source } => write!(
                formatter,
                "cannot inspect proof workspace {}: {source}",
                path.display()
            ),
            Self::WorkspaceNotDirectory(path) => {
                write!(
                    formatter,
                    "proof workspace is not a directory: {}",
                    path.display()
                )
            }
            Self::UnrepresentableWorkspace(path) => write!(
                formatter,
                "proof workspace cannot be represented in an OCI mount: {}",
                path.display()
            ),
            Self::ZeroOutputLimit => {
                formatter.write_str("proof output capture limit must be nonzero")
            }
            Self::EmptyProgram => formatter.write_str("proof battery program must not be empty"),
            Self::NulInCommand => formatter.write_str("proof battery command must not contain NUL"),
            Self::CreateAttestationChallenge(source) => {
                write!(
                    formatter,
                    "cannot create sealed runtime challenge: {source}"
                )
            }
            Self::Spec(source) => write!(formatter, "invalid sealed process contract: {source}"),
            Self::SpawnRuntime(source) => {
                write!(formatter, "cannot start sealed runtime: {source}")
            }
            Self::Wait(source) => write!(formatter, "sealed runtime wait failed: {source}"),
            Self::MissingCapture(stream) => {
                write!(formatter, "sealed runtime returned no {stream} capture")
            }
            Self::RuntimeControlRefused {
                operation,
                observed,
            } => write!(
                formatter,
                "sealed runtime could not {operation}: {observed}"
            ),
            Self::RuntimeNotAttested { expected, observed } => write!(
                formatter,
                "sealed runtime did not prove its OCI isolation contract: expected {expected:?}, \
                 observed {observed:?}"
            ),
        }
    }
}

impl Error for ContainerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InspectRuntime { source, .. }
            | Self::InspectWorkspace { source, .. }
            | Self::CreateAttestationChallenge(source)
            | Self::SpawnRuntimeProbe(source)
            | Self::SpawnRuntime(source) => Some(source),
            Self::RuntimeProbeSpec(source) | Self::Spec(source) => Some(source),
            Self::WaitRuntimeProbe(source) | Self::Wait(source) => Some(source),
            Self::RuntimeNotAbsolute(_)
            | Self::RuntimeNotRegular(_)
            | Self::RuntimeNotExecutable(_)
            | Self::RuntimeDialectNotAttested { .. }
            | Self::ImageNotDigestPinned(_)
            | Self::ImageNotResolved { .. }
            | Self::ImageIdentityDrift { .. }
            | Self::WorkspaceNotDirectory(_)
            | Self::UnrepresentableWorkspace(_)
            | Self::ZeroOutputLimit
            | Self::EmptyProgram
            | Self::NulInCommand
            | Self::MissingCapture(_)
            | Self::RuntimeControlRefused { .. }
            | Self::RuntimeNotAttested { .. } => None,
        }
    }
}

pub(crate) fn validate_command(program: &str, arguments: &[String]) -> Result<(), ContainerError> {
    if program.is_empty() {
        return Err(ContainerError::EmptyProgram);
    }
    if program.contains('\0') || arguments.iter().any(|argument| argument.contains('\0')) {
        return Err(ContainerError::NulInCommand);
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
pub(crate) fn is_executable(_metadata: &fs::Metadata) -> bool {
    false
}
