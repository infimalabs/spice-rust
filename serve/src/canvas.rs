//! The browser app canvas: the `app` namespace of the spice rack and the
//! boundary a resident serves app routes through.

mod assets;

/// Extension of the app rack staged beside each generation directory:
/// `generations/G.zip` holds generation G's `app` namespace.
pub const ARCHIVE_EXTENSION: &str = "zip";

pub(crate) use assets::CanvasAnswer;
pub use assets::{CanvasAssets, EmbeddedArchive};
