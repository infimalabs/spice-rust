use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

use wire::Digest;

use crate::repository::ResolveError;

/// Planning, admission, publication, or reversal failure.
#[derive(Debug)]
pub enum InstallError {
    Repository(ResolveError),
    Encode(serde_json::Error),
    Decode(serde_json::Error),
    Atomic(atomic::Error),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    PathShape(PathBuf),
    PathOutsideRoot(PathBuf),
    ParentChanged(PathBuf),
    ExclusionsNotUtf8,
    DivergentExclusions,
    BinaryNotRegular(PathBuf),
    BinaryEmpty(PathBuf),
    BinaryContextChanged,
    DigestMismatch {
        expected: Digest,
        observed: Digest,
    },
    InvalidRecord(String),
    LegacyRecord(PathBuf),
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
            Self::Encode(error) => write!(formatter, "install plan did not encode: {error}"),
            Self::Decode(error) => write!(formatter, "install record did not decode: {error}"),
            Self::Atomic(error) => write!(formatter, "install publication failed: {error}"),
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
                "install path resolves outside its permitted root: {}",
                path.display()
            ),
            Self::ParentChanged(path) => write!(
                formatter,
                "install destination parent changed after planning: {}",
                path.display()
            ),
            Self::ExclusionsNotUtf8 => formatter.write_str("Git exclusions are not UTF-8"),
            Self::DivergentExclusions => {
                formatter.write_str("Git exclusions carry a divergent spice-rust managed block")
            }
            Self::BinaryNotRegular(path) => {
                write!(
                    formatter,
                    "Rust binary is not a regular file: {}",
                    path.display()
                )
            }
            Self::BinaryEmpty(path) => {
                write!(formatter, "Rust binary is empty: {}", path.display())
            }
            Self::BinaryContextChanged => {
                formatter.write_str("Rust binary selection differs from the recorded plan")
            }
            Self::DigestMismatch { expected, observed } => write!(
                formatter,
                "install plan digest mismatch: expected {expected}, observed {observed}"
            ),
            Self::InvalidRecord(reason) => write!(formatter, "invalid install record: {reason}"),
            Self::LegacyRecord(path) => write!(
                formatter,
                concat!(
                    "legacy install record at {} belongs to the retired ",
                    "SURFACE-1kJQkpFs installer; unapply it with the release that wrote it ",
                    "before using the coexistence installer"
                ),
                path.display()
            ),
            Self::RecordMissing => formatter.write_str("no Rust install record exists"),
            Self::ApplyInProgress => formatter.write_str("Rust installation is incomplete"),
            Self::UnapplyInProgress => formatter.write_str("Rust unapply is incomplete"),
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
            Self::Encode(error) | Self::Decode(error) => Some(error),
            Self::Atomic(error) => Some(error),
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
