//! Hermetic repository enforcement.

pub mod message;
pub mod names;

pub use message::{CommitMessageError, OriginTrailer, OriginTrailerError, validate_commit_message};
pub use names::{RETIRED_NAMES, RetiredMatch, RetiredName, retired_matches};
