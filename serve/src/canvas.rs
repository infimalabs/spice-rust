//! The browser app canvas an installed Spice executable carries.

mod archive;

/// Extension of the archive staged beside each generation directory:
/// `generations/G.zip` holds generation G.
pub const ARCHIVE_EXTENSION: &str = "zip";

pub use archive::{
    CanvasArchive, CanvasArchiveError, CanvasMember, MAX_MEMBER_BYTES, MAX_MEMBERS, MAX_TOTAL_BYTES,
};
