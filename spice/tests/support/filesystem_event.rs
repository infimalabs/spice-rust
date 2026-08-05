//! The one filesystem observer used by asynchronous test waits.
//!
//! A native callback is the sole mechanism that announces an arriving state.
//! Callers arm it before a controlled trigger and read their state once
//! immediately afterwards, closing the only setup race without adding a
//! time-driven second path. After that read, they block until this callback,
//! another explicit completion signal, or their own horizon answers.

use std::path::Path;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// Arm a recursive native observer on `watched` and keep it alive for the caller.
pub(crate) fn arm(
    watched: &Path,
    what: &str,
    callback: impl FnMut(notify::Result<notify::Event>) + Send + 'static,
) -> RecommendedWatcher {
    let mut observer = notify::recommended_watcher(callback)
        .unwrap_or_else(|error| panic!("{what} observer starts: {error}"));
    observer
        .watch(watched, RecursiveMode::Recursive)
        .unwrap_or_else(|error| panic!("{what} observer arms on {}: {error}", watched.display()));
    observer
}
