//! The native window `spice app` opens on the console.
//!
//! The shell is a window over the resident's served app and owns nothing
//! else: the URL it loads has already passed the discovery admission a
//! browser launch applies, and the resident, journal, and lanes never learn
//! which surface is looking at them. It runs on the calling thread, which
//! must be the process main thread, and returns once the operator closes the
//! window.

use std::fmt;

/// Why no window could be shown.
#[derive(Debug)]
pub enum WindowError {
    /// This build has no native webview; the system browser remains the way in.
    Unsupported,
    /// The window system or webview refused to start.
    Platform(String),
}

impl fmt::Display for WindowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str(
                "this build opens no native window; use `spice up --open` for the system browser",
            ),
            Self::Platform(detail) => {
                write!(formatter, "the native window could not open: {detail}")
            }
        }
    }
}

impl std::error::Error for WindowError {}

#[cfg(target_os = "macos")]
mod window;

/// Show `url` in one native window titled `title` and block until it closes.
///
/// Links the page asks to open in a new window leave through `external`, so
/// the console's outbound links reach the system browser rather than a second
/// webview. `external` receives the exact URL the page named.
#[cfg(target_os = "macos")]
pub fn show(
    title: &str,
    url: &str,
    external: impl Fn(&str) + Send + Sync + 'static,
) -> Result<(), WindowError> {
    window::show(title, url, external)
}

/// Show `url` in one native window titled `title` and block until it closes.
#[cfg(not(target_os = "macos"))]
pub fn show(
    _title: &str,
    _url: &str,
    _external: impl Fn(&str) + Send + Sync + 'static,
) -> Result<(), WindowError> {
    Err(WindowError::Unsupported)
}
