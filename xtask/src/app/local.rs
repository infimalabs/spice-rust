//! Lane-local browser app compilation.

use std::fs;
use std::io::Write;

use super::{
    AppBuildError, AppBuildRole, ReleaseEnvironment, capture_stage_sources, run_trunk,
    workspace_root,
};

/// Compile the browser app into the worktree's development stage.
///
/// Unlike the reproducible committed stage, this output may contain
/// uncommitted lane bytes. The resident prefers it to the embedded app while
/// it exists, and staging and checking use source- and role-distinct targets.
/// The old directory is removed because finalization refuses stale artifacts.
///
/// # Errors
///
/// Returns an unreadable workspace root, a development stage that cannot be
/// removed, a missing Cargo home, Trunk failure, or a refused generated stage.
pub fn local_app(
    environment: &ReleaseEnvironment,
    output: &mut impl Write,
) -> Result<(), AppBuildError> {
    let root = workspace_root()?;
    let stage = root.join(spice::canvas::LOCAL_STAGE);
    if stage.exists() {
        fs::remove_dir_all(&stage).map_err(|source| AppBuildError::Io {
            path: stage.clone(),
            source,
        })?;
    }
    let sources = capture_stage_sources(&root)?;
    run_trunk(
        &root,
        environment,
        &stage,
        AppBuildRole::Local,
        sources.identity(),
        output,
    )?;
    writeln!(
        output,
        "[app] {} is now used by the next `spice up` in this worktree; \
         delete it to go back to the embedded app",
        stage.display()
    )
    .map_err(AppBuildError::Output)
}
