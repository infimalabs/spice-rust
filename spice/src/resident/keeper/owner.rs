//! What install, apply, stop, and uninstall ask of the facility, in order.

use std::fs;
use std::io;
use std::path::Path;

use super::scheduler::Scheduler;

/// Register or refresh the task from `definition`, allow it, and start it;
/// an instance already running is left alone.
///
/// # Errors
///
/// Returns the facility's refusal at the first step that fails.
pub(crate) fn apply(scheduler: &mut dyn Scheduler, task: &str, definition: &str) -> io::Result<()> {
    scheduler.register(task, definition)?;
    scheduler.set_enabled(task, true)?;
    scheduler.run(task)
}

/// Forbid the facility to start the task again; an unregistered task is
/// already held.
///
/// This runs before a resident is drained, so the keeper finds the task
/// disabled when that resident exits and stops instead of relaunching.
///
/// # Errors
///
/// Returns the facility's refusal to change the task.
pub(crate) fn hold(scheduler: &mut dyn Scheduler, task: &str) -> io::Result<()> {
    if scheduler.status(task)?.is_none() {
        return Ok(());
    }
    scheduler.set_enabled(task, false)
}

/// End any keeper still running and remove the registration.
///
/// # Errors
///
/// Returns the facility's refusal at the first step that fails.
pub(crate) fn release(scheduler: &mut dyn Scheduler, task: &str) -> io::Result<()> {
    scheduler.stop(task)?;
    scheduler.delete(task)
}

/// Remove the owner's files; absent entries are fine.
///
/// # Errors
///
/// Returns the first removal failure other than a missing entry.
pub(crate) fn remove_artifacts(paths: &[&Path]) -> io::Result<()> {
    for path in paths {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
