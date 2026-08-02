use std::fmt;
use std::str::FromStr;

use serde::de::{Deserializer, Error as _};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use wire::{Digest, Instant, TaskHandle};

pub(crate) const STORE_VERSION: u16 = 1;

/// Retention policy attached to one artifact citation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Retention {
    Permanent,
    Prunable,
}

impl fmt::Display for Retention {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Permanent => "permanent",
            Self::Prunable => "prunable",
        })
    }
}

impl FromStr for Retention {
    type Err = VocabularyError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "permanent" => Ok(Self::Permanent),
            "prunable" => Ok(Self::Prunable),
            _ => Err(VocabularyError::Retention(text.to_owned())),
        }
    }
}

/// One admitted task artifact media type.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MediaType(String);

impl MediaType {
    pub const TEXT_PLAIN: &'static str = "text/plain";
    pub const TEXT_MARKDOWN: &'static str = "text/markdown";
    pub const APPLICATION_JSON: &'static str = "application/json";
    pub const TEXT_CSV: &'static str = "text/csv";
    pub const TEXT_TSV: &'static str = "text/tab-separated-values";
    pub const IMAGE_PNG: &'static str = "image/png";
    pub const IMAGE_JPEG: &'static str = "image/jpeg";
    pub const IMAGE_WEBP: &'static str = "image/webp";
    pub const APPLICATION_PDF: &'static str = "application/pdf";

    pub const ALL: &'static [&'static str] = &[
        Self::APPLICATION_JSON,
        Self::APPLICATION_PDF,
        Self::IMAGE_JPEG,
        Self::IMAGE_PNG,
        Self::IMAGE_WEBP,
        Self::TEXT_CSV,
        Self::TEXT_MARKDOWN,
        Self::TEXT_PLAIN,
        Self::TEXT_TSV,
    ];

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_text(&self) -> bool {
        matches!(
            self.0.as_str(),
            Self::TEXT_PLAIN
                | Self::TEXT_MARKDOWN
                | Self::APPLICATION_JSON
                | Self::TEXT_CSV
                | Self::TEXT_TSV
        )
    }
}

impl fmt::Display for MediaType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for MediaType {
    type Err = VocabularyError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let value = text.to_ascii_lowercase();
        if Self::ALL.contains(&value.as_str()) {
            Ok(Self(value))
        } else {
            Err(VocabularyError::MediaType(text.to_owned()))
        }
    }
}

impl Serialize for MediaType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MediaType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(D::Error::custom)
    }
}

/// Stable metadata and provenance for one task artifact citation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub id: String,
    pub name: String,
    pub digest: Digest,
    pub media_type: MediaType,
    pub bytes: u64,
    pub created_at: Instant,
    pub source: String,
    pub retention: Retention,
}

/// Content returned after a show operation proves payload integrity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "representation")]
pub enum ArtifactContent {
    Text { text: String },
    Binary { path: String },
}

/// One result shape for all task artifact command leaves.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "answer")]
pub enum ArtifactAnswer {
    Added {
        task: TaskHandle,
        artifact: Artifact,
    },
    Listed {
        task: TaskHandle,
        artifacts: Vec<Artifact>,
    },
    Shown {
        task: TaskHandle,
        artifact: Artifact,
        content: ArtifactContent,
    },
    Pruned {
        plan: PrunePlan,
        applied: bool,
    },
}

/// One deterministic removal in a retention preview.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PruneOperation {
    pub order: u64,
    pub task: TaskHandle,
    pub artifact_id: String,
    pub name: String,
}

/// One artifact-bearing task excluded by current workflow state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PruneSkip {
    pub order: u64,
    pub task: TaskHandle,
    pub status: String,
}

/// Ordered, side-effect-free retention decision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrunePlan {
    pub schema_version: u16,
    pub artifact_root_exists: bool,
    pub older_than_seconds: Option<u64>,
    pub operations: Vec<PruneOperation>,
    pub skipped: Vec<PruneSkip>,
    #[serde(skip)]
    pub(crate) changes: Vec<PruneChange>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PruneChange {
    pub task: TaskHandle,
    pub original: Vec<Artifact>,
    pub remaining: Vec<Artifact>,
    pub remove: Vec<Artifact>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub version: u16,
    pub task: TaskHandle,
    pub artifacts: Vec<Artifact>,
}

impl Manifest {
    pub(crate) fn empty(task: TaskHandle) -> Self {
        Self {
            version: STORE_VERSION,
            task,
            artifacts: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObjectMetadata {
    pub version: u16,
    pub digest: Digest,
    pub bytes: u64,
}

/// One canonical artifact-store member carried by backup and restore.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupFile {
    pub path: String,
    pub mode: u32,
    pub bytes: Vec<u8>,
}

/// A complete, provenance-validated artifact-store generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupImage {
    pub(crate) files: Vec<BackupFile>,
}

impl BackupImage {
    #[must_use]
    pub fn files(&self) -> &[BackupFile] {
        &self.files
    }
}

/// A caller-supplied artifact vocabulary value was not admitted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VocabularyError {
    Retention(String),
    MediaType(String),
}

impl fmt::Display for VocabularyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Retention(value) => write!(
                formatter,
                "artifact retention must be permanent or prunable: {value:?}"
            ),
            Self::MediaType(value) => write!(
                formatter,
                "unsupported artifact media type {value:?}; admitted values are {}",
                MediaType::ALL.join(", ")
            ),
        }
    }
}

impl std::error::Error for VocabularyError {}
