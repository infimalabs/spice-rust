//! The browser app canvas: its archive format and the boundary a resident
//! serves app routes through.

mod archive;
mod assets;

/// Extension of the archive staged beside each generation directory:
/// `generations/G.zip` holds generation G.
pub const ARCHIVE_EXTENSION: &str = "zip";

pub use archive::{
    CanvasArchive, CanvasArchiveError, CanvasMember, MAX_MEMBER_BYTES, MAX_MEMBERS, MAX_TOTAL_BYTES,
};
pub(crate) use assets::CanvasAnswer;
pub use assets::{CanvasAssets, EmbeddedArchive};
