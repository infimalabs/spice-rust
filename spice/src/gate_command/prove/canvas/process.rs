//! Bounded execution and complete descendant cleanup for one Canvas shard.

use std::ffi::OsString;
use std::path::Path;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::super::ProveFailure;

const OUTPUT_LIMIT: usize = 1024 * 1024;
const OUTPUT_DRAIN: Duration = Duration::from_secs(5);
const REAP_DEADLINE: Duration = Duration::from_secs(5);
const TERMINATION_GRACE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
struct Bounds {
    runtime: Duration,
    output_drain: Duration,
    reap: Duration,
    grace: Duration,
    capture: usize,
}

impl Bounds {
    const fn production(timeout_millis: u64) -> Self {
        Self {
            runtime: Duration::from_millis(timeout_millis),
            output_drain: OUTPUT_DRAIN,
            reap: REAP_DEADLINE,
            grace: TERMINATION_GRACE,
            capture: OUTPUT_LIMIT,
        }
    }
}

pub(super) fn run(
    shard: &str,
    program: OsString,
    arguments: impl IntoIterator<Item = OsString>,
    cwd: &Path,
    environment: sill::Environment,
    timeout_millis: u64,
) -> Result<(), ProveFailure> {
    run_with_bounds(
        shard,
        program,
        arguments,
        cwd,
        environment,
        Bounds::production(timeout_millis),
    )
}

fn run_with_bounds(
    shard: &str,
    program: OsString,
    arguments: impl IntoIterator<Item = OsString>,
    cwd: &Path,
    environment: sill::Environment,
    bounds: Bounds,
) -> Result<(), ProveFailure> {
    let capture = sill::StreamPolicy::Capture {
        limit: bounds.capture,
    };
    let spec = sill::Spec {
        argv: sill::Argv::new(program, arguments).map_err(|error| {
            ProveFailure::fault("gate::prove-canvas-shard-process-invalid", error)
        })?,
        cwd: cwd.to_path_buf(),
        environment,
        group: sill::ProcessGroup::New,
        deadlines: sill::Deadlines::new(bounds.runtime, bounds.output_drain, bounds.reap).map_err(
            |error| ProveFailure::fault("gate::prove-canvas-shard-process-invalid", error),
        )?,
        wait: sill::WaitPolicy::new(
            format!("Canvas proof shard {shard}"),
            sill::Cancellation::Never,
            sill::RestartDisposition::Never,
        )
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-shard-process-invalid", error))?,
        output: sill::Output::new(sill::StdinPolicy::Null, capture, capture),
        redaction: sill::Redaction::All,
        grace: bounds.grace,
    };
    let running = sill::spawn(spec)
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-shard-unavailable", error))?;
    #[cfg(unix)]
    let descendants = match sill::DescendantGroups::discover(running.id()) {
        Ok(descendants) => descendants,
        Err(error) => {
            let signal = running.signal_kill();
            let wait = running.wait();
            return Err(ProveFailure::fault(
                "gate::prove-canvas-shard-teardown-failed",
                format!(
                    "shard={shard} descendant discovery failed: {error}; root kill={signal:?}; root wait={wait:?}"
                ),
            ));
        }
    };
    let remaining = running.runtime_remaining();
    let step = running
        .wait_for(remaining)
        .map_err(|error| teardown_failure(shard, error))?;
    let (outcome, expired) = match step {
        sill::WaitStep::Exited(outcome) => (outcome, false),
        sill::WaitStep::Running(running) => {
            #[cfg(unix)]
            {
                (reap_timed_out(shard, *running, descendants, bounds)?, true)
            }
            #[cfg(not(unix))]
            {
                (
                    running
                        .wait()
                        .map_err(|error| teardown_failure(shard, error))?,
                    true,
                )
            }
        }
    };
    classify(shard, bounds.runtime, expired, &outcome)
}

#[cfg(unix)]
fn reap_timed_out(
    shard: &str,
    running: sill::Running,
    mut descendants: sill::DescendantGroups,
    bounds: Bounds,
) -> Result<sill::WaitOutcome, ProveFailure> {
    // Capture and signal every currently visible detached group before the
    // root is allowed to exit and reparent those descendants.
    let initial_descendant_termination = descendants.terminate();
    let root_terminal = Arc::new(AtomicBool::new(false));
    let cleanup_terminal = Arc::clone(&root_terminal);
    let cleanup = std::thread::Builder::new()
        .name(format!("canvas-shard-cleanup-{shard}"))
        .spawn(move || {
            descendants.reap_while_root_terminates(
                bounds.grace,
                bounds.reap,
                bounds.output_drain,
                &cleanup_terminal,
            )
        })
        .map_err(|error| {
            ProveFailure::fault(
                "gate::prove-canvas-shard-teardown-failed",
                format!("shard={shard} descendant cleanup could not start: {error}"),
            )
        })?;
    let root = running.wait();
    if root.is_ok() {
        root_terminal.store(true, Ordering::Release);
    }
    let descendants = cleanup.join().map_err(|_| {
        ProveFailure::fault(
            "gate::prove-canvas-shard-teardown-failed",
            format!("shard={shard} descendant cleanup panicked"),
        )
    })?;
    let outcome = root.map_err(|error| teardown_failure(shard, error))?;
    initial_descendant_termination.map_err(|error| {
        ProveFailure::fault(
            "gate::prove-canvas-shard-teardown-failed",
            format!(
                "shard={shard} initial descendant termination failed: {error}; {}",
                process_detail(shard, &outcome),
            ),
        )
    })?;
    descendants.map_err(|error| {
        ProveFailure::fault(
            "gate::prove-canvas-shard-teardown-failed",
            format!(
                "shard={shard} descendant cleanup failed: {error}; {}",
                process_detail(shard, &outcome),
            ),
        )
    })?;
    Ok(outcome)
}

fn classify(
    shard: &str,
    timeout: Duration,
    expired: bool,
    outcome: &sill::WaitOutcome,
) -> Result<(), ProveFailure> {
    let detail = process_detail(shard, outcome);
    if !outcome.outputs_drained()
        || outcome
            .stdout()
            .is_some_and(|captured| !captured.complete())
        || outcome
            .stderr()
            .is_some_and(|captured| !captured.complete())
        || outcome.observation_failure().is_some()
    {
        return Err(ProveFailure::fault(
            "gate::prove-canvas-shard-teardown-failed",
            detail,
        ));
    }
    if expired {
        return Err(ProveFailure::refusal(
            "gate::prove-canvas-shard-timed-out",
            format!("timeout_ms={} {detail}", timeout.as_millis()),
        ));
    }
    match outcome.completion() {
        sill::Completion::Exited(status) if status.success() => Ok(()),
        sill::Completion::Exited(status) if process_signal(status).is_some() => Err(
            ProveFailure::refusal("gate::prove-canvas-shard-signaled", detail),
        ),
        sill::Completion::Exited(_) => Err(ProveFailure::refusal(
            "gate::prove-canvas-shard-refused",
            detail,
        )),
        sill::Completion::TimedOut(_) => Err(ProveFailure::refusal(
            "gate::prove-canvas-shard-timed-out",
            format!("timeout_ms={} {detail}", timeout.as_millis()),
        )),
        sill::Completion::Cancelled(_) => Err(ProveFailure::fault(
            "gate::prove-canvas-shard-cancelled",
            detail,
        )),
    }
}

#[cfg(unix)]
fn process_signal(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt as _;

    status.signal()
}

#[cfg(not(unix))]
fn process_signal(_status: &std::process::ExitStatus) -> Option<i32> {
    None
}

fn teardown_failure(shard: &str, error: sill::WaitError) -> ProveFailure {
    ProveFailure::fault(
        "gate::prove-canvas-shard-teardown-failed",
        format!("shard={shard} {error}; stdout=<unavailable> stderr=<unavailable>"),
    )
}

fn process_detail(shard: &str, outcome: &sill::WaitOutcome) -> String {
    format!(
        "shard={shard} status={} stdout={} stderr={} outputs_drained={} observation_failure={}",
        outcome.completion().status(),
        captured_diagnostic(outcome.stdout()),
        captured_diagnostic(outcome.stderr()),
        outcome.outputs_drained(),
        outcome
            .observation_failure()
            .map_or_else(|| "none".to_owned(), std::string::ToString::to_string,),
    )
}

fn captured_diagnostic(output: Option<&sill::CapturedOutput>) -> String {
    output.map_or_else(
        || "<none>".to_owned(),
        |output| {
            let marker = if output.truncated() {
                "<truncated>"
            } else {
                ""
            };
            format!("{marker}{}", String::from_utf8_lossy(output.bytes()))
        },
    )
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt as _;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::super::CanvasWorkspace;
    use super::*;

    const DETACHED_FIXTURE_MODE: &str = "SPICE_CANVAS_DETACHED_FIXTURE"; // env-policy: not-an-env SPICE_CANVAS_DETACHED_FIXTURE
    const DETACHED_FIXTURE_PID: &str = "SPICE_CANVAS_DETACHED_PID"; // env-policy: not-an-env SPICE_CANVAS_DETACHED_PID
    const TEST_OUTPUT_DRAIN: Duration = Duration::from_secs(2);
    const TEST_REAP_DEADLINE: Duration = Duration::from_millis(500);
    const TEST_TERMINATION_GRACE: Duration = Duration::from_millis(20);

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "spice-canvas-shard-test-{label}-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
            ));
            std::fs::create_dir(&root).expect("create shard fixture root");
            Self { root }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.root.join(name)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn test_bounds(runtime: Duration, capture: usize) -> Bounds {
        Bounds {
            runtime,
            output_drain: TEST_OUTPUT_DRAIN,
            reap: TEST_REAP_DEADLINE,
            grace: TEST_TERMINATION_GRACE,
            capture,
        }
    }

    fn run_shell(
        shard: &str,
        root: &Path,
        script: &str,
        environment: sill::Environment,
        bounds: Bounds,
    ) -> Result<(), ProveFailure> {
        run_with_bounds(
            shard,
            OsString::from("/bin/sh"),
            [OsString::from("-c"), OsString::from(script)],
            root,
            environment,
            bounds,
        )
    }

    fn wait_for_file(path: &Path) {
        let started = std::time::Instant::now();
        while !path.is_file() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "timed out waiting for synchronized fixture {}",
                path.display(),
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn failure_parts(failure: ProveFailure) -> (&'static str, String, String) {
        match failure {
            ProveFailure::Refusal { reason, detail } => ("refusal", reason, detail),
            ProveFailure::Fault { reason, detail } => ("fault", reason, detail),
        }
    }

    fn success<T>(result: Result<T, ProveFailure>) -> T {
        match result {
            Ok(value) => value,
            Err(ProveFailure::Refusal { reason, detail })
            | Err(ProveFailure::Fault { reason, detail }) => {
                panic!("unexpected failure {reason}: {detail}")
            }
        }
    }

    #[cfg(unix)]
    fn process_exists(pid: u32) -> bool {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: signal zero changes no process state and probes this exact pid.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    #[test]
    fn completing_shard_passes_through_the_supervised_boundary() {
        let fixture = Fixture::new("complete");
        success(run_shell(
            "complete",
            &fixture.root,
            "printf 'complete'; printf 'diagnostic' >&2",
            sill::Environment::empty(),
            test_bounds(Duration::from_secs(1), 1024),
        ));
    }

    #[cfg(unix)]
    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the subprocess fixture reads its explicitly declared control environment"
    )]
    fn detached_process_fixture() {
        if std::env::var_os(DETACHED_FIXTURE_MODE).is_none() {
            return;
        }
        // SAFETY: this changes only the fixture subprocess's process group and
        // gives it a new group containing no other process.
        assert_eq!(unsafe { libc::setpgid(0, 0) }, 0);
        let pid_path =
            std::env::var_os(DETACHED_FIXTURE_PID).expect("detached fixture pid path is declared");
        std::fs::write(pid_path, std::process::id().to_string())
            .expect("publish detached fixture pid");
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    #[cfg(unix)]
    #[test]
    fn timeout_reaps_descendants_and_removes_the_owned_workspace_before_refusal() {
        let fixture = Fixture::new("timeout-cleanup");
        let workspace = fixture.path("proof-workspace");
        let ready = fixture.path("ready");
        let root_pid = fixture.path("root-pid");
        let child_pid = fixture.path("child-pid");
        let helper = std::env::current_exe().expect("resolve shard fixture executable");
        let environment = sill::Environment::new([
            ("READY", ready.as_os_str()),
            ("ROOT_PID", root_pid.as_os_str()),
            ("HELPER", helper.as_os_str()),
            (DETACHED_FIXTURE_MODE, OsStr::new("1")),
            (DETACHED_FIXTURE_PID, child_pid.as_os_str()),
        ])
        .expect("valid fixture environment");
        let thread_workspace = workspace.clone();
        let thread = std::thread::spawn(move || {
            std::fs::create_dir(&thread_workspace).expect("create owned proof workspace");
            let owned = CanvasWorkspace(thread_workspace.clone());
            let result = run_shell(
                "timeout-cleanup",
                &thread_workspace,
                concat!(
                    "trap '' TERM; ",
                    "printf '%s' \"$$\" > \"$ROOT_PID\"; ",
                    "\"$HELPER\" --exact gate_command::prove::canvas::process::tests::detached_process_fixture --nocapture & ",
                    "while [ ! -s \"$SPICE_CANVAS_DETACHED_PID\" ]; do /bin/sleep 0.01; done; ",
                    "printf ready > \"$READY\"; wait",
                ),
                environment,
                test_bounds(Duration::from_secs(1), 1024),
            );
            drop(owned);
            result
        });

        wait_for_file(&ready);
        let root_pid = std::fs::read_to_string(root_pid)
            .expect("read root pid")
            .parse::<u32>()
            .expect("parse root pid");
        let child_pid = std::fs::read_to_string(child_pid)
            .expect("read child pid")
            .parse::<u32>()
            .expect("parse child pid");
        let failure = thread
            .join()
            .expect("join timed-out shard")
            .expect_err("uncompleted shard must time out");
        let (kind, reason, detail) = failure_parts(failure);

        assert_eq!(kind, "refusal", "{reason}: {detail}");
        assert_eq!(reason, "gate::prove-canvas-shard-timed-out");
        assert!(detail.contains("timeout_ms=1000"));
        assert!(!process_exists(root_pid), "root process survived refusal");
        assert!(!process_exists(child_pid), "descendant survived refusal");
        assert!(!workspace.exists(), "proof workspace survived refusal");
    }

    #[test]
    fn concurrent_shards_keep_independent_registry_bounds() {
        let completing = Fixture::new("concurrent-complete");
        let expiring = Fixture::new("concurrent-expire");
        let completing_ready = completing.path("ready");
        let expiring_ready = expiring.path("ready");
        let release = completing.path("release");
        let completing_environment = sill::Environment::new([
            ("READY", completing_ready.as_os_str()),
            ("RELEASE", release.as_os_str()),
        ])
        .expect("valid completing environment");
        let expiring_environment = sill::Environment::new([("READY", expiring_ready.as_os_str())])
            .expect("valid expiring environment");
        let completing_root = completing.root.clone();
        let expiring_root = expiring.root.clone();
        let completing_thread = std::thread::spawn(move || {
            run_shell(
                "concurrent-complete",
                &completing_root,
                concat!(
                    "printf ready > \"$READY\"; ",
                    "while [ ! -f \"$RELEASE\" ]; do /bin/sleep 0.01; done",
                ),
                completing_environment,
                test_bounds(Duration::from_secs(5), 1024),
            )
        });
        let expiring_thread = std::thread::spawn(move || {
            run_shell(
                "concurrent-expire",
                &expiring_root,
                "printf ready > \"$READY\"; while :; do /bin/sleep 1; done",
                expiring_environment,
                test_bounds(Duration::from_millis(500), 1024),
            )
        });

        wait_for_file(&completing_ready);
        wait_for_file(&expiring_ready);
        std::fs::write(&release, b"release").expect("release completing shard");

        success(completing_thread.join().expect("join completing shard"));
        let failure = expiring_thread
            .join()
            .expect("join expiring shard")
            .expect_err("unreleased shard must honor its own bound");
        assert_eq!(
            failure_parts(failure).1,
            "gate::prove-canvas-shard-timed-out"
        );
    }

    #[cfg(unix)]
    #[test]
    fn signal_nonzero_and_teardown_failures_have_distinct_typed_diagnostics() {
        let fixture = Fixture::new("typed-failures");
        let nonzero = run_shell(
            "nonzero",
            &fixture.root,
            "printf '123456789'; printf 'abcdefghi' >&2; exit 7",
            sill::Environment::empty(),
            test_bounds(Duration::from_secs(1), 4),
        )
        .expect_err("nonzero shard must refuse");
        let (_, nonzero_reason, nonzero_detail) = failure_parts(nonzero);
        assert_eq!(nonzero_reason, "gate::prove-canvas-shard-refused");
        assert!(nonzero_detail.contains("stdout=<truncated>1234"));
        assert!(nonzero_detail.contains("stderr=<truncated>abcd"));

        let signaled = run_shell(
            "signaled",
            &fixture.root,
            "kill -TERM $$",
            sill::Environment::empty(),
            test_bounds(Duration::from_secs(1), 1024),
        )
        .expect_err("signaled shard must refuse");
        assert_eq!(
            failure_parts(signaled).1,
            "gate::prove-canvas-shard-signaled"
        );

        let late_success =
            sill::WaitOutcome::externally_reaped(std::process::ExitStatus::from_raw(0));
        let late_success = classify(
            "late-success",
            Duration::from_millis(1),
            true,
            &late_success,
        )
        .expect_err("a process observed live at its bound cannot publish a pass");
        assert_eq!(
            failure_parts(late_success).1,
            "gate::prove-canvas-shard-timed-out"
        );

        let teardown = teardown_failure(
            "teardown",
            sill::WaitError::ReapDeadlineExceeded {
                owner: "fixture".to_owned(),
                deadline: Duration::from_millis(1),
            },
        );
        let (kind, teardown_reason, _) = failure_parts(teardown);
        assert_eq!(kind, "fault");
        assert_eq!(teardown_reason, "gate::prove-canvas-shard-teardown-failed");
    }
}
