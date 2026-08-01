use std::fmt;

use dialect::{Classification, Document, Refusal, Warning};
use wire::Digest;

/// Digest-bound, fully classified document ready for application.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Preview {
    digest: Digest,
    document: Document,
    warnings: Vec<Warning>,
}

impl Preview {
    /// Classify bytes and bind the caller-computed digest of those exact bytes.
    pub fn classify(input: &str, digest: Digest) -> Result<Self, PreviewRefusal> {
        match dialect::classify(input) {
            Classification::Current(document) => Ok(Self {
                digest,
                document,
                warnings: Vec::new(),
            }),
            Classification::Convertible { document, warnings } => Ok(Self {
                digest,
                document,
                warnings,
            }),
            Classification::Refused { refusals, warnings } => {
                Err(PreviewRefusal { refusals, warnings })
            }
        }
    }

    /// Digest the apply command must re-present.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }

    /// Complete classified graph applied as one unit.
    #[must_use]
    pub const fn document(&self) -> &Document {
        &self.document
    }

    /// Non-fatal normalization warnings carried by the preview.
    #[must_use]
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }
}

/// The authority seam that owns one all-or-nothing journal transaction.
///
/// Implementations must either commit the complete document graph and all of
/// its heads or leave authority unchanged. Board invokes this method exactly
/// once, never once per node.
pub trait AuthorityTransaction {
    type Error;

    fn commit_document(&mut self, document: &Document) -> Result<(), Self::Error>;
}

/// Apply a preview only when the observed input digest is still exact.
pub fn apply<A: AuthorityTransaction>(
    authority: &mut A,
    preview: &Preview,
    observed: &Digest,
) -> Result<ApplyReceipt, ApplyError<A::Error>> {
    if preview.digest != *observed {
        return Err(ApplyError::StaleDigest {
            previewed: preview.digest.clone(),
            observed: observed.clone(),
        });
    }

    authority
        .commit_document(&preview.document)
        .map_err(ApplyError::Transaction)?;
    Ok(ApplyReceipt {
        digest: preview.digest.clone(),
        nodes: preview.document.nodes.len(),
    })
}

/// Successful all-or-nothing document application.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplyReceipt {
    pub digest: Digest,
    pub nodes: usize,
}

/// A document that cannot reach preview.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewRefusal {
    pub refusals: Vec<Refusal>,
    pub warnings: Vec<Warning>,
}

impl fmt::Display for PreviewRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "document refused with {} refusal(s) and {} warning(s)",
            self.refusals.len(),
            self.warnings.len()
        )
    }
}

impl std::error::Error for PreviewRefusal {}

/// Why a preview was not committed.
#[derive(Debug)]
pub enum ApplyError<E> {
    StaleDigest { previewed: Digest, observed: Digest },
    Transaction(E),
}

impl<E: fmt::Display> fmt::Display for ApplyError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleDigest {
                previewed,
                observed,
            } => write!(
                formatter,
                "document digest changed after preview: expected {previewed}, observed {observed}"
            ),
            Self::Transaction(error) => write!(formatter, "document transaction failed: {error}"),
        }
    }
}

impl<E> std::error::Error for ApplyError<E>
where
    E: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::StaleDigest { .. } => None,
            Self::Transaction(error) => Some(error),
        }
    }
}
