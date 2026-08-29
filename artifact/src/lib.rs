//! Task-addressed text and binary evidence outside the authoritative journal.

#![forbid(unsafe_code)]

mod model;
mod store;

pub use model::{
    Artifact, ArtifactAnswer, ArtifactContent, BackupFile, BackupImage, MediaType, PruneOperation,
    PrunePlan, PruneSkip, Retention, VocabularyError,
};
pub use store::{AddBytesRequest, AddRequest, Error, ErrorClass, Replacement, Store};
