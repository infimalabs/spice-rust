//! Supervisor records a test leaves behind without running a supervisor.

use std::io;

use wire::CanonicalUuid;

use super::{
    CompletionField, PersistedCompletion, Phase, RuntimePaths, STATE_VERSION, State, now_millis,
    publish_json, supervisor_directory,
};

/// Leave the record a supervisor writes once its agent has exited, standing
/// in for a reap the resident never got to account.
pub(in crate::resident) fn record_exited_for_tests(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    session: wire::LaneSession,
) -> io::Result<()> {
    let directory = supervisor_directory(paths, lane, occupant);
    std::fs::create_dir_all(&directory)?;
    let state = State {
        version: STATE_VERSION,
        supervisor: std::process::id(),
        worktree: session.worktree.clone(),
        vendor: session.vendor.as_str().to_owned(),
        lane,
        occupant,
        phase: Phase::Exited,
        // No live process group can carry this id, so nothing a test reaps
        // through this record could ever be signalled.
        group: Some(i32::MAX.unsigned_abs()),
        session: Some(session),
        git: Some("fixture-head".to_owned()),
        started_millis: Some(now_millis()?),
        runtime_millis: Some(4 * 60 * 60 * 1_000),
        grace_millis: Some(20_000),
        reap_millis: Some(20_000),
        completion: CompletionField::some(PersistedCompletion::Exited { status_raw: 0 }),
        failure: None,
    };
    publish_json(&directory.join("state.json"), &state)
}
