use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

use wire::Digest;

use crate::repository::{ResolveError, StateError};

/// Planning, admission, publication, or reversal failure.
#[derive(Debug)]
pub enum InstallError {
    Repository(ResolveError),
    State(StateError),
    Encode(serde_json::Error),
    Decode(serde_json::Error),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    PathShape(PathBuf),
    PathOutsideRoot(PathBuf),
    ParentChanged(PathBuf),
    BinaryNotRegular(PathBuf),
    BinaryEmpty(PathBuf),
    BinaryContextChanged,
    DigestMismatch {
        expected: Digest,
        observed: Digest,
    },
    InvalidRecord(String),
    LegacyRecord {
        path: PathBuf,
        owner: &'static str,
    },
    RecordMissing,
    ApplyInProgress,
    UnapplyInProgress,
    Drift {
        path: String,
        observed: Option<Digest>,
    },
    Interrupted {
        operation: usize,
    },
}

impl InstallError {
    pub(crate) fn io(operation: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(error) => error.fmt(formatter),
            Self::State(error) => error.fmt(formatter),
            Self::Encode(error) => write!(formatter, "install plan did not encode: {error}"),
            Self::Decode(error) => write!(formatter, "install record did not decode: {error}"),
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "failed to {operation} at {}: {source}",
                path.display()
            ),
            Self::PathShape(path) => write!(
                formatter,
                "install path is not a real regular file or directory: {}",
                path.display()
            ),
            Self::PathOutsideRoot(path) => write!(
                formatter,
                "install path resolves outside its canonical permitted root: {}",
                path.display()
            ),
            Self::ParentChanged(path) => write!(
                formatter,
                "install destination parent generation changed after planning: {}",
                path.display()
            ),
            Self::BinaryNotRegular(path) => {
                write!(
                    formatter,
                    "Spice executable is not a regular file: {}",
                    path.display()
                )
            }
            Self::BinaryEmpty(path) => {
                write!(formatter, "Spice executable is empty: {}", path.display())
            }
            Self::BinaryContextChanged => {
                formatter.write_str("Spice executable selection differs from the recorded plan")
            }
            Self::DigestMismatch { expected, observed } => write!(
                formatter,
                "install plan digest mismatch: expected {expected}, observed {observed}"
            ),
            Self::InvalidRecord(reason) => write!(formatter, "invalid install record: {reason}"),
            Self::LegacyRecord { path, owner } => write!(
                formatter,
                "install record at {} belongs to the retired release {owner}; \
                 unapply it with that release before installing Spice",
                path.display(),
            ),
            Self::RecordMissing => formatter.write_str("no Spice install record exists"),
            Self::ApplyInProgress => formatter.write_str("Spice installation is incomplete"),
            Self::UnapplyInProgress => formatter.write_str("Spice unapply is incomplete"),
            Self::Drift { path, observed } => write!(
                formatter,
                "installed path drifted at {path}; observed {}",
                observed
                    .as_ref()
                    .map_or_else(|| "absence".to_owned(), ToString::to_string)
            ),
            Self::Interrupted { operation } => {
                write!(formatter, "fixture interrupted after operation {operation}")
            }
        }
    }
}

impl Error for InstallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Repository(error) => Some(error),
            Self::State(error) => Some(error),
            Self::Encode(error) | Self::Decode(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<ResolveError> for InstallError {
    fn from(error: ResolveError) -> Self {
        Self::Repository(error)
    }
}

impl From<StateError> for InstallError {
    fn from(error: StateError) -> Self {
        Self::State(error)
    }
}
