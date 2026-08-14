//! Installed-boundary proof for repositories with no packaged test engine.

use super::*;

#[test]
fn generic_repository_publishes_zero_engine_evidence_without_invoking_cargo() {
    let repository = Scratch::new("generic-repository");
    git(repository.path(), &["init", "-q"]);
    git(
        repository.path(),
        &["config", "user.email", "generic@example.invalid"],
    );
    git(
        repository.path(),
        &["config", "user.name", "Generic Fixture"],
    );
    fs::write(
        repository.path().join("README.md"),
        "# Generic repository\n",
    )
    .expect("write generic repository source");
    git(repository.path(), &["add", "README.md"]);
    git(repository.path(), &["commit", "-qm", "fixture"]);
    fs::write(
        repository.path().join("README.md"),
        "# Generic repository\n\nChanged documentation.\n",
    )
    .expect("modify generic repository source");
    git(repository.path(), &["add", "README.md"]);

    let programs = Scratch::new("generic-programs");
    let cargo_marker = programs.path().join("cargo-invoked");
    executable(
        &programs.path().join("cargo"),
        &format!(
            "#!/bin/sh\nprintf invoked > '{}'\nexit 91\n",
            cargo_marker.display(),
        ),
    );

    let boundary = run_commit_worker_with_path(repository.path(), Some(programs.path()));
    let Some((kind, document)) = boundary.terminal.split_first() else {
        panic!("generic staged source produced no terminal record");
    };
    assert_eq!(
        *kind, b'R',
        "generic repository did not admit: {boundary:?}"
    );
    let change_report = packaged_change_report(document);
    let tests = selection_report(&boundary.progress);

    assert_eq!(change_report.selection().source(), tests.source());
    assert_eq!(change_report.tests(), &tests);
    assert_eq!(tests.changed_sources(), &["README.md"]);
    assert!(tests.engines().is_empty());
    assert!(tests.tests().is_empty());
    assert!(tests.edges().is_empty());
    assert!(tests.admits());
    assert!(
        !boundary.progress.contains("[rust-test] ")
            && !boundary.progress.contains("[rust-test-selection] "),
        "zero-engine generic policy emitted Cargo execution progress: {}",
        boundary.progress,
    );
    assert!(
        !cargo_marker.exists(),
        "zero-engine generic policy invoked Cargo"
    );
}
