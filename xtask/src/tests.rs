use std::sync::Mutex;

use super::*;
use crate::verify_execution::verify_package_with;

#[test]
fn repository_wide_verify_is_disabled_before_any_work_starts() {
    let mut output = Vec::new();
    let error = verify(&mut output).expect_err("bare verify is retired");

    assert!(matches!(error, VerifyError::RepositoryWideDisabled));
    assert!(output.is_empty());

    let error = verify_child(&mut output).expect_err("private full verifier child is retired");
    assert!(matches!(error, VerifyError::RepositoryWideDisabled));
    assert!(output.is_empty());
}

struct FakeRunner {
    calls: Mutex<Vec<ScopedCommand>>,
    fail: Option<ScopedCommand>,
}

impl FakeRunner {
    fn admitting() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail: None,
        }
    }

    fn refusing(command: ScopedCommand) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail: Some(command),
        }
    }

    fn calls(&self) -> Vec<ScopedCommand> {
        self.calls.lock().expect("fixture calls lock").clone()
    }
}

impl Runner for FakeRunner {
    fn run_scoped(
        &self,
        command: ScopedCommand,
        _package: &str,
        _cancellation: &CancellationToken,
        cargo_jobs: usize,
    ) -> Result<CommandRun, RunError> {
        assert_eq!(cargo_jobs, 1, "scoped Cargo work must remain single-job");
        self.calls.lock().expect("fixture calls lock").push(command);
        let admitted = self.fail != Some(command);
        Ok(CommandRun::Exited(ProcessOutput::fixture(
            admitted,
            Some(if admitted { 0 } else { 17 }),
        )))
    }
}

#[test]
fn scoped_verify_runs_only_the_three_package_commands_and_names_every_omission() {
    let runner = FakeRunner::admitting();
    let mut output = Vec::new();

    verify_package_with("gate", &runner, &mut output).expect("package scope admits");

    assert_eq!(runner.calls(), SCOPED_COMMANDS);
    let output = String::from_utf8(output).expect("scope output is UTF-8");
    assert!(output.contains(concat!(
        "[verify] scoped package gate; slice-local: ",
        "add named downstream packages or run staged selection before completion\n"
    )));
    assert!(output.contains(&format!(
        "[verify] repository-wide actions skipped: {}\n",
        RETIRED_REPOSITORY_ACTIONS.join(", ")
    )));
    assert!(output.contains("[verify] scoped gate arms run: none\n"));
    for command in SCOPED_COMMANDS {
        assert!(output.contains(&command.label("gate")));
    }
}

#[test]
fn scoped_verify_refuses_invalid_package_names_before_output() {
    for package in ["", "gate other", "../gate", "gate/package"] {
        let runner = FakeRunner::admitting();
        let mut output = Vec::new();

        let error = verify_package_with(package, &runner, &mut output)
            .expect_err("invalid package scope refuses");

        assert!(matches!(error, VerifyError::Package(name) if name == package));
        assert!(runner.calls().is_empty());
        assert!(output.is_empty());
    }
}

#[test]
fn scoped_command_failure_stops_before_later_package_commands() {
    let runner = FakeRunner::refusing(ScopedCommand::Test);
    let mut output = Vec::new();

    let error = verify_package_with("wire", &runner, &mut output)
        .expect_err("refusing package test stops the slice");

    assert!(matches!(
        error,
        VerifyError::ScopedExit { code: Some(17), .. }
    ));
    assert_eq!(runner.calls(), [ScopedCommand::Clippy, ScopedCommand::Test]);
    assert!(
        !String::from_utf8(output)
            .expect("scope output is UTF-8")
            .contains(&ScopedCommand::DocTest.label("wire"))
    );
}
