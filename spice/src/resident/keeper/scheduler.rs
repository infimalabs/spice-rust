//! The registration facility the lifecycle owner lives in.
//!
//! One trait names exactly what the owner needs from it, so the relaunch loop
//! and the install, stop, and uninstall decisions can be exercised in process
//! against a recording stand-in while Windows binds the trait to Task
//! Scheduler over COM.

use std::io;

#[cfg(windows)]
pub(super) mod windows;

/// What the facility currently holds for one task path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Status {
    /// Whether the facility will start the task on its trigger or on `run`.
    pub(crate) enabled: bool,
    /// Whether an instance of the task is executing now.
    pub(crate) running: bool,
}

/// One registration facility for per-user tasks.
pub(crate) trait Scheduler {
    /// Create or replace the task at `path` from its definition; the result
    /// is enabled.
    fn register(&mut self, path: &str, definition: &str) -> io::Result<()>;
    /// The task's status, or `None` when no task is registered at `path`.
    fn status(&mut self, path: &str) -> io::Result<Option<Status>>;
    /// Allow or forbid the facility to start the task.
    fn set_enabled(&mut self, path: &str, enabled: bool) -> io::Result<()>;
    /// Start an instance now; an instance already running is left alone.
    fn run(&mut self, path: &str) -> io::Result<()>;
    /// End every running instance.
    fn stop(&mut self, path: &str) -> io::Result<()>;
    /// Remove the registration; a missing task is already removed.
    fn delete(&mut self, path: &str) -> io::Result<()>;
}
