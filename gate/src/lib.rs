//! Hermetic repository enforcement.

pub mod message;

pub use message::{CommitMessageError, OriginTrailer, OriginTrailerError, validate_commit_message};
