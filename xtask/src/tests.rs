use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use super::*;
use crate::verify_execution::{verify_package_with, verify_with};

enum Outcome {
    Success,
    Exit(i32),
    Launch,
}
/// The report a source-development entry point produces, so the order test
/// can pin the policy action without reaching a real repository.
const ENTRY_POINT_FIXTURE: &str = "fixture automatic publisher owns installation\n";

#[derive(Default)]
struct RefuseDiagnosticWrite {
    accepted: Vec<u8>,
}

impl Write for RefuseDiagnosticWrite {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.starts_with(b"bounded battery refusal") {
            Err(io::Error::other("fixture diagnostic output refusal"))
        } else {
            self.accepted.extend_from_slice(bytes);
            Ok(bytes.len())
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct FakeState {
    outcomes: VecDeque<Outcome>,
    scoped_outcomes: VecDeque<Outcome>,
    entry_point: Option<Result<String, String>>,
    browser: Option<io::Result<BrowserRun>>,
    structural: Option<Result<BatteryRun, String>>,
    battery: Option<Result<BatteryRun, String>>,
    landing: Option<Result<landing::Report, landing::Error>>,
    calls: Vec<&'static str>,
    scoped_calls: Vec<(ScopedCommand, String)>,
    state_accepts: usize,
    evidence_publications: usize,
}

struct FakeRunner {
    state: Mutex<FakeState>,
}

impl FakeRunner {
    fn state(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().expect("fixture runner lock")
    }

    fn calls(&self) -> Vec<&'static str> {
        self.state().calls.clone()
    }

    fn scoped_calls(&self) -> Vec<(ScopedCommand, String)> {
        self.state().scoped_calls.clone()
    }

    fn state_accepts(&self) -> usize {
        self.state().state_accepts
    }

    fn evidence_publications(&self) -> usize {
        self.state().evidence_publications
    }

    fn set_entry_point(&self, outcome: Result<String, String>) {
        self.state().entry_point = Some(outcome);
    }

    fn set_browser(&self, outcome: io::Result<BrowserRun>) {
        self.state().browser = Some(outcome);
    }

    fn set_battery(&self, outcome: BatteryRun) {
        self.state().battery = Some(Ok(outcome));
    }

    fn set_structural(&self, outcome: BatteryRun) {
        self.state().structural = Some(Ok(outcome));
    }

    fn set_landing(&self, outcome: Result<landing::Report, landing::Error>) {
        self.state().landing = Some(outcome);
    }

    fn set_scoped_outcomes(&self, outcomes: impl IntoIterator<Item = Outcome>) {
        self.state().scoped_outcomes = outcomes.into_iter().collect();
    }
}

impl Runner for FakeRunner {
    fn entry_point(&self) -> Result<String, String> {
        let mut state = self.state();
        state.calls.push(ENTRY_POINT_LABEL);
        state
            .entry_point
            .take()
            .expect("fixture has one entry-point outcome")
    }

    fn format(
        &self,
        cancellation: &CancellationToken,
        _cargo_jobs: usize,
    ) -> Result<CommandRun, RunError> {
        let mut state = self.state();
        state.calls.push(FORMAT_LABEL);
        fake_command(
            state.outcomes.pop_front().expect("fixture has an outcome"),
            cancellation,
            "fixture formatter launch refusal",
        )
    }

    fn run(
        &self,
        step: Step,
        cancellation: &CancellationToken,
        _cargo_jobs: usize,
    ) -> Result<CommandRun, RunError> {
        let mut state = self.state();
        state.calls.push(step.label);
        fake_command(
            state.outcomes.pop_front().expect("fixture has an outcome"),
            cancellation,
            "fixture launch refusal",
        )
    }

    fn run_with_readiness(
        &self,
        step: Step,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        readiness: ActionReadiness,
    ) -> Result<CommandRun, RunError> {
        readiness();
        self.run(step, cancellation, cargo_jobs)
    }

    fn run_scoped(
        &self,
        step: ScopedCommand,
        package: &str,
        cancellation: &CancellationToken,
        _cargo_jobs: usize,
    ) -> Result<CommandRun, RunError> {
        let mut state = self.state();
        state.scoped_calls.push((step, package.to_owned()));
        let outcome = state
            .scoped_outcomes
            .pop_front()
            .expect("fixture has a scoped outcome");
        fake_command(outcome, cancellation, "fixture scoped launch refusal")
    }

    fn browser(
        &self,
        _cancellation: &CancellationToken,
        _cargo_jobs: usize,
        _canvas_workers: usize,
    ) -> Result<BrowserRun, RunError> {
        let mut state = self.state();
        state.calls.push(BROWSER_LABEL);
        state
            .browser
            .take()
            .expect("fixture has one browser outcome")
            .map_err(RunError::Launch)
    }

    fn workspace_test_prebuild(
        &self,
        _cancellation: &CancellationToken,
        _cargo_jobs: usize,
    ) -> Result<CommandRun, RunError> {
        Ok(CommandRun::Exited(ProcessOutput::status_only(Status {
            success: true,
            code: Some(0),
        })))
    }

    fn workspace_tests(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        workspace_workers: usize,
    ) -> Result<CommandRun, RunError> {
        self.run(
            Step::cargo(WORKSPACE_TEST_LABEL, &[]),
            cancellation,
            cargo_jobs.min(workspace_workers),
        )
    }

    fn structural_preflight(
        &self,
        _cancellation: &CancellationToken,
    ) -> Result<BatteryRun, RunError> {
        let mut state = self.state();
        state.calls.push(STRUCTURAL_PREFLIGHT_LABEL);
        state
            .structural
            .take()
            .expect("fixture has one structural outcome")
            .map_err(RunError::Cleanup)
    }

    fn battery(&self, _cancellation: &CancellationToken) -> Result<BatteryRun, RunError> {
        let mut state = self.state();
        state.calls.push(BATTERY_LABEL);
        state
            .battery
            .take()
            .expect("fixture has one battery outcome")
            .map_err(RunError::Cleanup)
    }

    fn accept_battery_state(&self) -> Result<(), String> {
        self.state().state_accepts += 1;
        Ok(())
    }

    fn publish_full_verifier_evidence(&self) -> Result<(), String> {
        self.state().evidence_publications += 1;
        Ok(())
    }

    fn landing(&self) -> Result<landing::Report, landing::Error> {
        let mut state = self.state();
        state.calls.push(landing::LABEL);
        state
            .landing
            .take()
            .expect("fixture has one landing outcome")
    }
}

#[test]
fn production_readiness_refuses_an_ordinary_command_without_forging_the_event() {
    let runner = SystemRunner::standalone();
    let observed = Arc::new(AtomicBool::new(false));
    let callback_observed = Arc::clone(&observed);
    let error = runner
        .run_with_readiness(
            Step::cargo("fixture ordinary command", &[]),
            &CancellationToken::new(),
            1,
            Arc::new(move || callback_observed.store(true, Ordering::Release)),
        )
        .expect_err("ordinary commands have no compiler-readiness authority");

    match error {
        RunError::Launch(error) => assert_eq!(
            error.to_string(),
            "compiler readiness is unavailable for fixture ordinary command"
        ),
        RunError::Cleanup(detail) => panic!("ordinary readiness returned cleanup: {detail}"),
    }
    assert!(!observed.load(Ordering::Acquire));
}

#[test]
fn staged_worktree_inventory_allows_only_index_owned_changes() {
    admit_staged_worktree_status(b"M  src/lib.rs\0A  staged.rs\0R  renamed.rs\0original.rs\0")
        .expect("staged changes and complete rename pairs match the announced index");
    assert!(
        admit_staged_worktree_status(b" M src/lib.rs\0")
            .expect_err("an unstaged edit diverges from the announced index")
            .contains("differs from the announced staged index"),
    );
    assert!(
        admit_staged_worktree_status(b"?? untracked.rs\0")
            .expect_err("an untracked path is absent from the announced source")
            .contains("absent from the announced staged source"),
    );
    assert!(
        admit_staged_worktree_status(b"R  renamed.rs\0")
            .expect_err("a rename without its original path is malformed")
            .contains("omitted the original path"),
    );
}

fn fake_command(
    outcome: Outcome,
    cancellation: &CancellationToken,
    launch_detail: &str,
) -> Result<CommandRun, RunError> {
    if cancellation.is_cancelled() {
        return Ok(CommandRun::Cancelled(ProcessOutput::status_only(Status {
            success: false,
            code: None,
        })));
    }
    match outcome {
        Outcome::Success => Ok(CommandRun::Exited(ProcessOutput::status_only(Status {
            success: true,
            code: Some(0),
        }))),
        Outcome::Exit(code) => Ok(CommandRun::Exited(ProcessOutput::status_only(Status {
            success: false,
            code: Some(code),
        }))),
        Outcome::Launch => Err(RunError::Launch(io::Error::other(launch_detail.to_owned()))),
    }
}

#[test]
fn success_runs_every_step_once_and_prints_ordered_lifecycle_records() {
    let runner = runner(successful_commands());
    let mut output = Vec::new();

    verify_with(&runner, &mut output).expect("verification succeeds");

    assert_exact_calls(&runner, &labels());
    assert_eq!(runner.state_accepts(), 1);
    assert_eq!(runner.evidence_publications(), 1);
    let output = String::from_utf8(output).expect("labels are UTF-8");
    let report =
        serde_json::to_string(&landing::fixture_report(true)).expect("fixture report encodes");
    assert_eq!(output.matches("stage=start").count(), VERIFY_ACTIONS.len());
    assert_eq!(
        output.matches("stage=terminal").count(),
        VERIFY_ACTIONS.len()
    );
    assert!(!output.contains("stage=progress"));

    let mut prior_terminal = 0;
    for (index, action) in VERIFY_ACTIONS.into_iter().enumerate() {
        let ordinal = format!("ordinal={}/{}", index + 1, VERIFY_ACTIONS.len());
        let identity = format!("identity={:?}", action.label());
        let start = output
            .lines()
            .position(|line| {
                line.contains("stage=start")
                    && line.contains(&ordinal)
                    && line.contains(&identity)
                    && line.contains(&format!("family={}", action.family()))
            })
            .expect("action has one exact start record");
        let terminal = output
            .lines()
            .position(|line| {
                line.contains("stage=terminal")
                    && line.contains(&ordinal)
                    && line.contains(&identity)
            })
            .expect("action has one exact terminal record");
        assert!(terminal > start, "action terminates after it starts");
        assert!(terminal >= prior_terminal, "terminal records stay ordered");
        prior_terminal = terminal;
    }
    assert!(output.contains(ENTRY_POINT_FIXTURE));
    assert!(output.contains("{\"admitted\":true}\n"));
    assert!(output.contains(&format!("{report}\n")));
}

#[test]
fn structural_refusal_stops_before_any_expensive_or_browser_action() {
    let runner = runner(successful_commands());
    runner.set_structural(BatteryRun::Refused(
        b"fixture structural refusal\n".to_vec(),
    ));
    let mut output = Vec::new();

    let error =
        verify_with(&runner, &mut output).expect_err("structural preflight refuses the verifier");

    assert!(matches!(error, VerifyError::BatteryRefused));
    assert_eq!(
        runner.calls(),
        [
            RESIDENT_BUILD_LABEL,
            ENTRY_POINT_LABEL,
            FORMAT_LABEL,
            STRUCTURAL_PREFLIGHT_LABEL,
        ]
    );
    assert_eq!(runner.state_accepts(), 0);
    assert_eq!(runner.evidence_publications(), 0);
    let output = String::from_utf8(output).expect("fixture output is UTF-8");
    assert!(output.contains("fixture structural refusal\n"));
    assert!(output.contains(&format!(
        "stage=setup identity={STRUCTURAL_PREFLIGHT_LABEL:?}"
    )));
    assert!(!output.contains(&format!("identity={BROWSER_LABEL:?}")));
    assert!(!output.contains(&format!("identity={BATTERY_LABEL:?}")));
}

#[test]
fn scoped_verify_runs_only_package_commands_and_names_every_omission() {
    let runner = scoped_runner();
    let mut output = Vec::new();

    verify_package_with("gate", &runner, &mut output)
        .expect("scoped package verification succeeds");

    assert!(runner.calls().is_empty());
    let expected_calls = SCOPED_COMMANDS
        .into_iter()
        .map(|command| (command, "gate".to_owned()))
        .collect::<Vec<_>>();
    assert_eq!(runner.scoped_calls(), expected_calls);

    let output = String::from_utf8(output).expect("scoped verifier output is UTF-8");
    assert!(output.starts_with(concat!(
        "[verify] scoped package gate; feedback only: ",
        "run `cargo xtask verify` before task completion\n"
    )));
    let skipped_actions = VERIFY_ACTIONS
        .iter()
        .map(|action| action.label())
        .collect::<Vec<_>>()
        .join(", ");
    assert!(output.contains(&format!(
        "[verify] scoped full actions skipped: {skipped_actions}\n"
    )));
    assert!(output.contains("[verify] scoped gate arms run: none\n"));
    let skipped_arms = gate::GateId::BATTERY
        .into_iter()
        .map(|gate_id| gate_id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    assert!(output.contains(&format!(
        "[verify] scoped gate arms skipped: {skipped_arms}\n"
    )));
    assert_eq!(output.matches("stage=start").count(), 3);
    assert_eq!(output.matches("stage=terminal").count(), 3);
    assert!(!runner.calls().contains(&BROWSER_LABEL));
    assert!(!runner.calls().contains(&BATTERY_LABEL));
    assert!(!runner.calls().contains(&landing::LABEL));
}

#[test]
fn scoped_verify_refuses_an_injected_or_empty_package_name_before_output() {
    for package in ["", "gate\n[verify] forged", "gate/path", "gate package"] {
        let runner = scoped_runner();
        let mut output = Vec::new();

        let error = verify_package_with(package, &runner, &mut output)
            .expect_err("invalid package scope refuses");

        assert!(matches!(error, VerifyError::Package(_)));
        assert!(runner.calls().is_empty());
        assert!(runner.scoped_calls().is_empty());
        assert!(output.is_empty());
    }
}

#[test]
fn scoped_command_failure_stops_before_later_package_commands() {
    let runner = scoped_runner();
    runner.set_scoped_outcomes([Outcome::Exit(17)]);
    let mut output = Vec::new();

    let error = verify_package_with("gate", &runner, &mut output)
        .expect_err("scoped Clippy refusal surfaces");

    assert!(matches!(
        error,
        VerifyError::ScopedExit { code: Some(17), .. }
    ));
    assert!(runner.calls().is_empty());
    assert_eq!(
        runner.scoped_calls(),
        [(ScopedCommand::Clippy, "gate".to_owned())]
    );
}

#[test]
fn verifier_steps_match_the_complete_command_guard() {
    const GUARDED_ACTIONS: [Action; 13] = [
        Action::Command(Step::cargo(
            "cargo build --locked -p spice --bin spice (freshness reference and canvas resident)",
            &["build", "--locked", "-p", "spice", "--bin", "spice"],
        )),
        Action::EntryPoint,
        Action::Format,
        Action::Command(Step::cargo(
            "cargo clippy --locked --workspace --all-targets -- -D warnings",
            &[
                "clippy",
                "--locked",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        )),
        Action::Command(Step::cargo(
            "cargo clippy --locked -p app --target wasm32-unknown-unknown -- -D warnings",
            &[
                "clippy",
                "--locked",
                "-p",
                "app",
                "--target",
                "wasm32-unknown-unknown",
                "--",
                "-D",
                "warnings",
            ],
        )),
        Action::Command(Step::cargo(
            "cargo check --locked --workspace --target wasm32-unknown-unknown",
            &[
                "check",
                "--locked",
                "--workspace",
                "--target",
                "wasm32-unknown-unknown",
            ],
        )),
        Action::Command(Step::cargo(
            "cargo xtask app (canvas compiler evidence)",
            &["xtask", "app"],
        )),
        Action::Browser,
        Action::WorkspaceTests,
        Action::Command(Step::cargo(
            "cargo doc --locked --workspace --lib --bins --no-deps",
            &[
                "doc",
                "--locked",
                "--workspace",
                "--lib",
                "--bins",
                "--no-deps",
            ],
        )),
        Action::Command(Step::cargo(
            "cargo test --locked --workspace --doc",
            &["test", "--locked", "--workspace", "--doc"],
        )),
        Action::Battery,
        Action::Landing,
    ];

    assert_eq!(VERIFY_ACTIONS.as_slice(), GUARDED_ACTIONS.as_slice());
}

/// Cargo's default target set documents `spice` and `xtask` as libraries
/// only: each has a binary whose name collides with its library
/// (`rust-lang/cargo#6313`) and cargo drops it without a word. Both
/// `main.rs` files carry doc comments, so the sweep has to name its targets
/// or those two files are never rustdoc-linted at all.
#[test]
fn documentation_names_its_targets_so_the_colliding_binaries_are_linted() {
    let sweep = VERIFY_ACTIONS
        .iter()
        .find_map(|action| match action {
            Action::Command(step) if step.label == DOCUMENTATION_LABEL => Some(step.arguments),
            _ => None,
        })
        .expect("the verifier has a documentation step");

    assert!(sweep.contains(&"--bins"), "{sweep:?} selects no binaries");
    assert!(sweep.contains(&"--lib"), "{sweep:?} selects no libraries");
    assert!(
        sweep.contains(&"--workspace"),
        "{sweep:?} leaves packages out of the sweep"
    );
}

#[test]
fn verifier_explicitly_runs_workspace_doctests() {
    let (index, step) = VERIFY_ACTIONS
        .iter()
        .enumerate()
        .find_map(|(index, action)| match action {
            Action::Command(step) if step.label == "cargo test --locked --workspace --doc" => {
                Some((index, step))
            }
            _ => None,
        })
        .expect("the verifier has a doctest step");

    assert_eq!(step.arguments, ["test", "--locked", "--workspace", "--doc"]);
    assert_eq!(VERIFY_ACTIONS[index + 1], Action::Battery);
    assert_eq!(VERIFY_ACTIONS[index + 2], Action::Landing);
}

#[test]
fn verifier_owns_exactly_one_battery_action() {
    assert_eq!(
        VERIFY_ACTIONS
            .iter()
            .filter(|action| matches!(action, Action::Battery))
            .count(),
        1
    );
}

/// BROWSER-1kKSWhZk removed one former surface, so real Chromium guards
/// the final production edge after the Canvas app is built. The one native
/// resident build is earlier because it also supplies freshness evidence.
#[test]
fn verifier_builds_one_freshness_reference_and_app_before_browser() {
    let browser = VERIFY_ACTIONS
        .iter()
        .position(|action| *action == Action::Browser)
        .expect("the verifier has a browser action");

    assert_eq!(browser, 7);
    let predecessor = VERIFY_ACTIONS[browser - 1];
    assert!(matches!(predecessor, Action::Command(step) if step.label == CANVAS_APP_LABEL));
    let wasm_predecessor = VERIFY_ACTIONS[browser - 2];
    let is_wasm = match wasm_predecessor {
        Action::Command(step) => step.arguments.contains(&WASM_TARGET),
        _ => false,
    };
    assert!(is_wasm);
    assert!(
        matches!(VERIFY_ACTIONS[0], Action::Command(step) if step.label == RESIDENT_BUILD_LABEL)
    );
    assert_eq!(VERIFY_ACTIONS[1], Action::EntryPoint);
    assert_eq!(
        BROWSER_RECEIPT,
        b"BROWSER-1kKSWhZk browser coverage complete\n"
    );
    assert!(BROWSER_LABEL.contains("BROWSER-1kKSWhZk"));
}

#[test]
fn zero_exit_without_the_browser_receipt_is_not_a_pass() {
    let runner = runner(successful_commands());
    runner.set_browser(Ok(BrowserRun {
        process: CommandRun::Exited(ProcessOutput::status_only(Status {
            success: true,
            code: Some(0),
        })),
        completed: false,
    }));
    let mut output = Vec::new();

    let error = verify_with(&runner, &mut output)
        .expect_err("a skipped browser body emits no completion receipt");

    assert!(matches!(error, VerifyError::BrowserIncomplete));
    let calls = runner.calls();
    assert!(calls.contains(&BROWSER_LABEL));
    assert!(!calls.contains(&landing::LABEL));
    assert_calls_are_unique(&calls);
}

#[test]
fn browser_dependency_and_launch_failures_are_not_incomplete_coverage() {
    let missing_npm = runner(successful_commands());
    missing_npm.set_browser(Err(io::Error::new(
        io::ErrorKind::NotFound,
        "npm unavailable",
    )));
    let launch =
        verify_with(&missing_npm, &mut Vec::new()).expect_err("a missing browser launcher fails");
    assert!(matches!(launch, VerifyError::BrowserLaunch { .. }));

    let missing_browser = runner(successful_commands());
    missing_browser.set_browser(Ok(BrowserRun {
        process: CommandRun::Exited(ProcessOutput::status_only(Status {
            success: false,
            code: Some(127),
        })),
        completed: false,
    }));
    let dependency = verify_with(&missing_browser, &mut Vec::new())
        .expect_err("a missing Node or browser dependency fails its process");
    assert!(matches!(
        dependency,
        VerifyError::BrowserExit { code: Some(127) }
    ));
}

#[test]
fn verifier_lints_the_browser_crate_for_the_wasm_target() {
    let step = VERIFY_ACTIONS
        .iter()
        .find_map(|action| match action {
            Action::Command(step)
                if step.arguments.first() == Some(&"clippy")
                    && step.arguments.contains(&WASM_TARGET) =>
            {
                Some(step)
            }
            _ => None,
        })
        .expect("the verifier has a wasm Clippy step");

    assert_eq!(
        step.arguments,
        [
            "clippy",
            "--locked",
            "-p",
            "app",
            "--target",
            WASM_TARGET,
            "--",
            "-D",
            "warnings",
        ],
    );
    assert!(step.label.contains(WASM_TARGET));
}

/// The battery builds the whole workspace for a second target and says
/// which one.
///
/// The app-only Clippy step reaches browser code; this distinct check also
/// reaches every other workspace crate's target-specific surface. The
/// label carries the triple because the same Cargo command can fail for
/// host reasons too, and a reader seeing only "check failed" would look
/// for a defect that is not there.
#[test]
fn verifier_builds_the_workspace_for_the_browser_target() {
    let step = VERIFY_ACTIONS
        .iter()
        .find_map(|action| match action {
            Action::Command(step) if step.arguments.first() == Some(&"check") => Some(step),
            _ => None,
        })
        .expect("the verifier has a cross-target step");

    assert_eq!(
        step.arguments,
        ["check", "--locked", "--workspace", "--target", WASM_TARGET],
    );
    assert!(
        step.label.contains(WASM_TARGET),
        "the label must name the target it builds for: {}",
        step.label,
    );
}

#[test]
fn source_policy_suppresses_digest_churn_without_weakening_the_default_audit() {
    let suppressed = entry_point_with_policy(true, || {
        panic!("automatic publication must not inspect a concurrently moving receipt")
    })
    .expect("the source repository delegates publication");
    assert!(suppressed.is_empty());

    let stale = entry_point_with_policy(false, || {
        Err("another build; repair=target/debug/spice install".to_owned())
    })
    .expect_err("the default installed-client audit remains strict");
    assert_eq!(stale, "another build; repair=target/debug/spice install");
}

/// With suppression disabled, the audit sits immediately after the one
/// build that supplies its current bytes, so a refusal must stop every
/// later verification action.
///
/// Ordering alone would not settle this: an action that reports and admits
/// occupies the same slot and passes every order assertion here. What is
/// pinned is that the labels stop at the first one and that the refusal
/// reaches the operator verbatim, because the state it names is one no
/// amount of re-running the battery clears.
#[test]
fn default_policy_still_stops_after_a_stale_appliance_audit() {
    let runner = runner(successful_commands());
    runner.set_entry_point(Err("published guard is not this build's".to_owned()));
    let mut output = Vec::new();

    let error = verify_with(&runner, &mut output).expect_err("a stale guard surfaces");

    assert!(matches!(error, VerifyError::EntryPoint(_)));
    assert_eq!(runner.state_accepts(), 0);
    assert_eq!(
        error.to_string(),
        "[verify] published guard is not this build's"
    );
    assert_eq!(runner.calls(), labels()[..2]);
    let output = String::from_utf8(output).expect("labels are UTF-8");
    assert_eq!(output.matches("stage=start").count(), 2);
    assert_eq!(output.matches("stage=terminal").count(), 2);
    assert!(output.contains("disposition=error"));
    assert!(output.contains(&format!("identity={ENTRY_POINT_LABEL:?}")));
    assert!(!output.contains("ordinal=3/13"));
}

#[test]
fn command_failure_stops_before_its_dependent_successor() {
    let runner = runner([Outcome::Success, Outcome::Exit(9), Outcome::Success]);
    let mut output = Vec::new();

    let error = verify_with(&runner, &mut output).expect_err("clippy failure surfaces");

    assert!(matches!(error, VerifyError::Exit { code: Some(9), .. }));
    assert_eq!(runner.state_accepts(), 0);
    let calls = runner.calls();
    assert!(calls.contains(&VERIFY_ACTIONS[2].label()));
    assert!(!calls.contains(&VERIFY_ACTIONS[3].label()));
    assert!(!calls.contains(&landing::LABEL));
    assert_calls_are_unique(&calls);
}

#[test]
fn launch_failure_stops_before_its_dependent_successor() {
    let runner = runner([Outcome::Launch, Outcome::Success, Outcome::Success]);
    let mut output = Vec::new();

    let error = verify_with(&runner, &mut output).expect_err("launch refusal surfaces");

    assert!(matches!(error, VerifyError::Launch { .. }));
    assert_eq!(runner.state_accepts(), 0);
    let calls = runner.calls();
    assert!(calls.contains(&RESIDENT_BUILD_LABEL));
    assert!(!calls.contains(&ENTRY_POINT_LABEL));
    assert!(!calls.contains(&FORMAT_LABEL));
    assert!(!calls.contains(&landing::LABEL));
    assert_calls_are_unique(&calls);
}

#[test]
fn landing_refusal_is_terminal() {
    let runner = runner(successful_commands());
    let report = landing::fixture_report(false);
    let expected = serde_json::to_string(&report).expect("landing fixture report encodes");
    runner.set_landing(Ok(report));
    let mut output = Vec::new();

    let error = verify_with(&runner, &mut output).expect_err("landing refuses");

    assert!(matches!(error, VerifyError::LandingRefused));
    assert_exact_calls(&runner, &labels());
    assert_eq!(runner.state_accepts(), 0);
    assert!(
        String::from_utf8(output)
            .expect("landing output is UTF-8")
            .contains(&format!("{expected}\n"))
    );
}

#[test]
fn battery_refusal_is_terminal_before_landing() {
    let runner = runner(successful_commands());
    runner.set_battery(BatteryRun::Refused(b"bounded battery refusal\n".to_vec()));
    let mut output = Vec::new();

    let error = verify_with(&runner, &mut output).expect_err("battery refusal surfaces");

    assert!(matches!(error, VerifyError::BatteryRefused));
    assert_eq!(runner.state_accepts(), 0);
    let calls = runner.calls();
    assert!(calls.contains(&BATTERY_LABEL));
    assert!(!calls.contains(&landing::LABEL));
    assert_calls_are_unique(&calls);
    let output = String::from_utf8(output).expect("fixture diagnostic is UTF-8");
    assert!(output.contains("bounded battery refusal\n"));
    assert!(output.contains(concat!("stage=terminal ordinal=12/13", " elapsed_ms=")));
    assert!(output.contains("disposition=refused family=in-process:gate-battery"));
    assert!(!output.contains("ordinal=13/13"));
}

#[test]
fn battery_action_failure_is_primary_action_error_not_a_cleanup_fault() {
    let runner = runner(successful_commands());
    runner.set_battery(BatteryRun::Error(
        "fixture battery execution failure".to_owned(),
    ));
    let mut output = Vec::new();

    let error = verify_with(&runner, &mut output).expect_err("battery action failure surfaces");

    assert!(matches!(error, VerifyError::Battery(_)));
    assert_eq!(runner.state_accepts(), 0);
    let output = String::from_utf8(output).expect("fixture output is UTF-8");
    assert!(output.contains("disposition=error family=in-process:gate-battery"));
    assert!(output.contains("primary_ordinal=12 interrupted=false cleanup_faults=0"));
    assert!(!output.contains("scheduler=cleanup-fault"));
}

#[test]
fn battery_render_failure_emits_a_terminal_without_hiding_the_original_error() {
    let runner = runner(successful_commands());
    runner.set_battery(BatteryRun::Refused(b"bounded battery refusal\n".to_vec()));
    let mut output = RefuseDiagnosticWrite::default();

    let error = verify_with(&runner, &mut output).expect_err("diagnostic write refusal surfaces");

    assert!(matches!(error, VerifyError::Output(_)));
    assert_eq!(runner.state_accepts(), 0);
    assert_eq!(
        error.to_string(),
        "[verify] output failed: fixture diagnostic output refusal"
    );
    let calls = runner.calls();
    assert!(calls.contains(&BATTERY_LABEL));
    assert!(!calls.contains(&landing::LABEL));
    assert_calls_are_unique(&calls);
    let output = String::from_utf8(output.accepted).expect("lifecycle output is UTF-8");
    let terminal = output
        .lines()
        .filter(|line| line.contains("stage=terminal ordinal=12/13"))
        .collect::<Vec<_>>();
    assert_eq!(
        terminal.len(),
        1,
        "failed action has one terminal: {output}"
    );
    assert!(terminal[0].contains("disposition=render-error"));
    assert!(!output.contains("ordinal=13/13"));
}

#[test]
fn landing_acquisition_failure_is_terminal() {
    let runner = runner(successful_commands());
    runner.set_landing(Err(landing::fixture_error()));
    let mut output = Vec::new();

    let error = verify_with(&runner, &mut output).expect_err("landing fails");

    assert!(matches!(error, VerifyError::Landing(_)));
    assert_exact_calls(&runner, &labels());
}

#[test]
fn compiled_contract_landing_is_the_verifier_terminus() {
    assert_eq!(VERIFY_ACTIONS.last(), Some(&Action::Landing));
    assert_eq!(VERIFY_ACTIONS[VERIFY_ACTIONS.len() - 2], Action::Battery);
    assert_eq!(
        VERIFY_ACTIONS
            .iter()
            .filter(|action| matches!(action, Action::Command(_)))
            .count(),
        7,
        "exactly seven declared Cargo commands precede compiled landing"
    );
    assert_eq!(
        VERIFY_ACTIONS
            .iter()
            .filter(|action| matches!(action, Action::Format))
            .count(),
        1,
        "one gate-selected formatter action precedes compiled landing"
    );
    assert_eq!(
        VERIFY_ACTIONS
            .iter()
            .filter(|action| matches!(action, Action::Browser))
            .count(),
        1,
        "one mandatory browser action precedes compiled landing"
    );
}

fn runner(outcomes: impl IntoIterator<Item = Outcome>) -> FakeRunner {
    FakeRunner {
        state: Mutex::new(FakeState {
            outcomes: outcomes.into_iter().collect(),
            scoped_outcomes: VecDeque::new(),
            entry_point: Some(Ok(ENTRY_POINT_FIXTURE.to_owned())),
            browser: Some(Ok(BrowserRun {
                process: CommandRun::Exited(ProcessOutput::status_only(Status {
                    success: true,
                    code: Some(0),
                })),
                completed: true,
            })),
            structural: Some(Ok(BatteryRun::Admitted(Vec::new()))),
            battery: Some(Ok(BatteryRun::Admitted(br#"{"admitted":true}"#.to_vec()))),
            landing: Some(Ok(landing::fixture_report(true))),
            calls: Vec::new(),
            scoped_calls: Vec::new(),
            state_accepts: 0,
            evidence_publications: 0,
        }),
    }
}

fn scoped_runner() -> FakeRunner {
    let runner = runner([]);
    runner.set_scoped_outcomes(SCOPED_COMMANDS.iter().map(|_| Outcome::Success));
    runner
}

fn successful_commands() -> Vec<Outcome> {
    VERIFY_ACTIONS
        .iter()
        .filter(|action| {
            matches!(
                action,
                Action::Format | Action::Command(_) | Action::WorkspaceTests
            )
        })
        .map(|_| Outcome::Success)
        .collect()
}

fn labels() -> Vec<&'static str> {
    VERIFY_ACTIONS
        .iter()
        .map(|action| action.label())
        .chain([STRUCTURAL_PREFLIGHT_LABEL])
        .collect()
}

fn assert_exact_calls(runner: &FakeRunner, expected: &[&'static str]) {
    let mut actual = runner.calls();
    let mut expected = expected.to_vec();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected);
}

fn assert_calls_are_unique(calls: &[&'static str]) {
    let mut unique = calls.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        calls.len(),
        "an action executed twice: {calls:?}"
    );
}
