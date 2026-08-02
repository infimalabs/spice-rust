mod complexity;
mod file;
mod magic;
mod markdown;

use std::error::Error;
use std::fmt;

use super::EvidenceState;

pub use complexity::{ComplexityRecord, parse_lizard_csv};
pub use file::{
    FileShapeMeasurement, RepositoryDocumentMeasurement, measure_file_shape,
    measure_repository_document,
};
pub use magic::{MagicFinding, scan_magic_regressions};
pub use markdown::{MarkdownLinkFinding, scan_markdown_link_case};

/// Exact reason compiled parsing could not produce a measurement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseError {
    /// The selected input is outside the parser's declared file class.
    NotApplicable { reason: String },
    /// Predecessor input has no exact current representation.
    Unsupported {
        source_field: String,
        reason: String,
        recovery: String,
    },
    /// Selected bytes or parser output violate the compiled grammar.
    Malformed { input: String, reason: String },
    /// Two inputs collapse to one current key.
    Collision { key: String, reason: String },
}

impl ParseError {
    /// Convert parser refusal into the closed metric evidence vocabulary.
    #[must_use]
    pub fn into_evidence_state(self) -> EvidenceState {
        match self {
            Self::NotApplicable { reason } => EvidenceState::NotApplicable { reason },
            Self::Unsupported {
                source_field,
                reason,
                recovery,
            } => EvidenceState::predecessor_unsupported(source_field, reason, recovery),
            Self::Malformed { input, reason } => EvidenceState::Failed {
                reason: format!("malformed {input}: {reason}"),
                exit_code: None,
            },
            Self::Collision { key, reason } => EvidenceState::Failed {
                reason: format!("key collision {key:?}: {reason}"),
                exit_code: None,
            },
        }
    }

    pub(super) fn malformed(input: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Malformed {
            input: input.into(),
            reason: reason.into(),
        }
    }

    pub(super) fn unsupported(
        source_field: impl Into<String>,
        reason: impl Into<String>,
        recovery: impl Into<String>,
    ) -> Self {
        Self::Unsupported {
            source_field: source_field.into(),
            reason: reason.into(),
            recovery: recovery.into(),
        }
    }

    pub(super) fn collision(key: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Collision {
            key: key.into(),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotApplicable { reason } => write!(formatter, "not applicable: {reason}"),
            Self::Unsupported {
                source_field,
                reason,
                recovery,
            } => write!(
                formatter,
                "unsupported predecessor field {source_field}: {reason}; recovery: {recovery}"
            ),
            Self::Malformed { input, reason } => {
                write!(formatter, "malformed {input}: {reason}")
            }
            Self::Collision { key, reason } => write!(formatter, "key collision {key:?}: {reason}"),
        }
    }
}

impl Error for ParseError {}

pub(super) fn canonical_path(path: &str) -> Result<String, ParseError> {
    let invalid = path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains('\\')
        || path.contains(['\r', '\n'])
        || path.split('/').any(|part| matches!(part, "" | "." | ".."));
    if invalid {
        Err(ParseError::malformed(
            "repository path",
            format!("{path:?} is not canonical repository-relative text"),
        ))
    } else {
        Ok(path.to_owned())
    }
}
