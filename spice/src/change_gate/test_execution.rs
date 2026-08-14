//! Policy-selected test-engine dispatch over one immutable selected source.

use std::io::Write;
use std::path::Path;

use gate::{
    ChangeSelectionMode, TestSelectionReport, WORKSPACE_TEST_CONCURRENCY_CAP, WorkspaceTestRoots,
};
use sill::Environment;

use super::ChangeGateError;

pub(super) enum TestExecution {
    Packaged(Vec<crate::config::TestEngine>),
    CurrentSourceCargo,
}

#[expect(
    clippy::too_many_arguments,
    reason = "test mode, exact source, roots, environment, and progress are independent authorities"
)]
pub(super) fn run_selected_tests(
    mode: ChangeSelectionMode,
    selected_root: &Path,
    writable_root: &Path,
    source: wire::Digest,
    changes: &[String],
    execution: TestExecution,
    environment: Environment,
    progress: &mut impl Write,
) -> Result<TestSelectionReport, ChangeGateError> {
    let cargo_selected = match execution {
        TestExecution::CurrentSourceCargo => true,
        TestExecution::Packaged(engines) => {
            engines.contains(&crate::config::TestEngine::CargoWorkspace)
                && gate::cargo_workspace_tests_applicable(
                    selected_root,
                    changes,
                    mode == ChangeSelectionMode::Exhaustive,
                )
        }
    };
    if !cargo_selected {
        return TestSelectionReport::complete_without_engines(source, changes.to_vec())
            .map_err(|error| ChangeGateError::protocol("test-selection-invalid", error));
    }

    let roots = WorkspaceTestRoots::new(selected_root, writable_root);
    match mode {
        ChangeSelectionMode::Local => gate::run_selected_workspace_tests_at(
            roots,
            source,
            changes,
            environment,
            WORKSPACE_TEST_CONCURRENCY_CAP,
            progress,
        ),
        ChangeSelectionMode::Exhaustive => gate::run_exhaustive_selected_workspace_tests_at(
            roots,
            source,
            changes,
            environment,
            WORKSPACE_TEST_CONCURRENCY_CAP,
            progress,
        ),
    }
    .map_err(|error| ChangeGateError::refusal("change-gate-tests-refused", error))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[test]
    fn generic_repository_completes_without_resolving_cargo() {
        let root = fixture_root("generic-zero-engine");
        fs::create_dir_all(&root).expect("create generic repository fixture");
        let source = wire::Digest::compute(wire::Domain::GateTree, b"generic source");
        let changes = vec!["README.md".to_owned(), "assets/icon.svg".to_owned()];
        let environment =
            Environment::new([(OsString::from("PATH"), root.clone().into_os_string())])
                .expect("minimal environment");
        let mut progress = Vec::new();

        let report = run_selected_tests(
            ChangeSelectionMode::Local,
            &root,
            &root,
            source.clone(),
            &changes,
            TestExecution::Packaged(Vec::new()),
            environment,
            &mut progress,
        )
        .expect("zero-engine repository needs no Cargo executable");

        assert_eq!(report.source(), &source);
        assert_eq!(report.changed_sources(), changes);
        assert!(report.engines().is_empty());
        assert!(report.tests().is_empty());
        assert!(report.edges().is_empty());
        assert!(report.admits());
        assert!(progress.is_empty());
        fs::remove_dir_all(root).expect("remove generic repository fixture");
    }

    #[test]
    fn declared_cargo_engine_is_skipped_for_an_irrelevant_scope() {
        let root = fixture_root("cargo-irrelevant-scope");
        fs::create_dir_all(&root).expect("create Cargo repository fixture");
        fs::write(root.join("Cargo.toml"), "[workspace]\nresolver = \"2\"\n")
            .expect("write Cargo workspace authority");
        let environment =
            Environment::new([(OsString::from("PATH"), root.clone().into_os_string())])
                .expect("minimal environment");
        let mut progress = Vec::new();

        let report = run_selected_tests(
            ChangeSelectionMode::Local,
            &root,
            &root,
            wire::Digest::compute(wire::Domain::GateTree, b"documentation-only source"),
            &["README.md".to_owned()],
            TestExecution::Packaged(vec![crate::config::TestEngine::CargoWorkspace]),
            environment,
            &mut progress,
        )
        .expect("irrelevant Cargo scope needs no Cargo executable");

        assert!(report.engines().is_empty());
        assert!(report.admits());
        assert!(progress.is_empty());
        fs::remove_dir_all(root).expect("remove Cargo repository fixture");
    }

    #[test]
    fn applicable_declared_cargo_engine_refuses_when_cargo_is_absent() {
        let root = fixture_root("cargo-tool-absent");
        fs::create_dir_all(root.join("src")).expect("create Cargo source fixture");
        fs::write(root.join("Cargo.toml"), "[workspace]\nresolver = \"2\"\n")
            .expect("write Cargo workspace authority");
        let environment =
            Environment::new([(OsString::from("PATH"), root.clone().into_os_string())])
                .expect("minimal environment");

        let error = run_selected_tests(
            ChangeSelectionMode::Local,
            &root,
            &root,
            wire::Digest::compute(wire::Domain::GateTree, b"Cargo source"),
            &["src/lib.rs".to_owned()],
            TestExecution::Packaged(vec![crate::config::TestEngine::CargoWorkspace]),
            environment,
            &mut Vec::new(),
        )
        .expect_err("applicable declared Cargo engine needs its compiled executable");

        assert_eq!(error.reason(), "gate::change-gate-tests-refused");
        assert!(error.detail().contains("metadata launch failed"), "{error}");
        fs::remove_dir_all(root).expect("remove absent-Cargo fixture");
    }

    #[test]
    fn current_source_surface_retains_cargo_without_packaged_applicability() {
        let root = fixture_root("current-source-cargo");
        fs::create_dir_all(&root).expect("create current-source fixture");
        let environment =
            Environment::new([(OsString::from("PATH"), root.clone().into_os_string())])
                .expect("minimal environment");

        let error = run_selected_tests(
            ChangeSelectionMode::Local,
            &root,
            &root,
            wire::Digest::compute(wire::Domain::GateTree, b"current-source documentation"),
            &["README.md".to_owned()],
            TestExecution::CurrentSourceCargo,
            environment,
            &mut Vec::new(),
        )
        .expect_err("the explicit current-source surface retains Cargo execution");

        assert_eq!(error.reason(), "gate::change-gate-tests-refused");
        assert!(error.detail().contains("metadata launch failed"), "{error}");
        fs::remove_dir_all(root).expect("remove current-source fixture");
    }

    fn fixture_root(name: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "spice-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ))
    }
}
