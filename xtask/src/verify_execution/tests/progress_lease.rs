use std::ffi::OsString;

use super::*;
use crate::verify_execution::{
    FULL_VERIFIER_ABSOLUTE_RUNTIME, FULL_VERIFIER_PROCESS_BOUNDS, FULL_VERIFIER_PROGRESS_BOUNDS,
    FULL_VERIFIER_PROGRESS_INACTIVITY, ProcessBounds, ProgressLeaseBounds, ProgressSource,
    SignalGuard, StreamingRunError, is_lease_renewal_record, is_live_progress_record,
    run_bounded_process_streaming, run_process_with_progress, write_deferred_supervisor_output,
};

const NON_COOPERATIVE_CHILD_TEST: &str =
    "verify_execution::tests::progress_lease::non_cooperative_in_process_verifier_fixture";
const PROGRESS_LEASE_CHILD_TEST: &str =
    "verify_execution::tests::progress_lease::progress_lease_child_fixture";
const SUPERVISOR_PID_MARKER: &str = "verifier-supervisor-fixture pid=";

struct RefusingOutput;

impl std::io::Write for RefusingOutput {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "fixture output sink closed",
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn nested_workspace_and_browser_elapsed_heartbeats_do_not_advance_the_action_cursor() {
    assert!(
        !ProgressSource::WorkspaceStdout
            .advances(b"[rust-test] stage=progress elapsed_ms=30000 running=6\n")
    );
    assert!(
        ProgressSource::WorkspaceStdout
            .advances(b"[rust-test] stage=terminal shard=41/157 disposition=success\n")
    );
    assert!(
        !ProgressSource::BrowserStdout
            .advances(b"canvas shard progress elapsed_ms=30002 running=ownership remaining=2\n")
    );
    assert!(ProgressSource::BrowserStdout.advances(b"CANVAS-SHARD {\"kind\":\"scenario-end\"}\n"));
    assert!(ProgressSource::CargoStderr.advances(b"    Checking gate v0.0.0\n"));
}

#[cfg(unix)]
#[test]
fn observed_child_output_advances_a_cursor_without_changing_complete_capture() {
    let cursor = Arc::new(AtomicU64::new(0));
    let observed = Arc::clone(&cursor);
    let run = run_process_with_progress(
        "selected child-output progress fixture",
        "/bin/sh",
        ["-c", "printf 'stdout-record\\n'; printf 'one\\ntwo\\n' >&2"],
        &CancellationToken::new(),
        1,
        &[],
        ProgressSource::CargoStderr,
        Arc::new(move || {
            observed.fetch_add(1, Ordering::Relaxed);
        }),
    )
    .expect("the selected child stream is observed and completely captured");
    let CommandRun::Exited(output) = run else {
        panic!("the output-progress fixture did not exit: {run:?}");
    };
    assert_eq!(cursor.load(Ordering::Relaxed), 2);
    assert_eq!(output.stdout, b"stdout-record\n");
    assert_eq!(output.stderr, b"one\ntwo\n");
}

fn fixture_progress_bounds(
    inactivity: Duration,
    absolute_runtime: Duration,
) -> ProgressLeaseBounds {
    ProgressLeaseBounds {
        inactivity,
        process: ProcessBounds {
            runtime: absolute_runtime,
            grace: Duration::from_millis(100),
            output_drain: Duration::from_millis(100),
            reap: Duration::from_millis(100),
            output_limit: crate::verify_execution::OUTPUT_LIMIT,
        },
    }
}

#[test]
fn outer_supervisor_has_a_tight_progress_lease_inside_one_absolute_limit() {
    let bounds = FULL_VERIFIER_PROCESS_BOUNDS;
    let absolute = bounds.runtime + bounds.grace + bounds.reap + bounds.output_drain;
    assert_eq!(absolute, FULL_VERIFIER_ABSOLUTE_RUNTIME);
    assert_eq!(bounds.runtime, Duration::from_secs(15 * 60 - 3));
    assert_eq!(
        FULL_VERIFIER_PROGRESS_BOUNDS.inactivity,
        FULL_VERIFIER_PROGRESS_INACTIVITY
    );
    assert_eq!(FULL_VERIFIER_PROGRESS_INACTIVITY, Duration::from_secs(90));
    assert!(FULL_VERIFIER_PROGRESS_INACTIVITY < FULL_VERIFIER_TEST_RUNTIME);
    assert_eq!(
        bounds.output_limit,
        crate::verify_execution::OUTPUT_LIMIT * VERIFY_ACTIONS.len() * 2,
        "the outer capture retains both per-action stream bounds"
    );
    assert!(
        bounds.runtime > FULL_VERIFIER_TEST_RUNTIME,
        "the hard supervisor must outlive the child-owned deadline"
    );
}

#[test]
fn verifier_diagnostics_are_live_but_only_genuine_transitions_and_cursors_renew() {
    let scheduler = b"[verify] scheduler workers=3\n";
    let compiler = b"[verify] stage=progress ordinal=3/13 identity=compiler\n";
    let shard = b"[verify] stage=progress ordinal=9/13 identity=workspace-tests\n";
    let browser = b"[verify] stage=progress ordinal=8/13 identity=browser\n";
    let setup = b"[verify] stage=setup-progress identity=workspace-prebuild\n";
    let advancement = b"[verify] stage=advance ordinal=9/13 cursor=41 identity=workspace-tests\n";
    let setup_advancement = b"[verify] stage=setup-advance cursor=7 identity=workspace-prebuild\n";
    let terminal = b"[verify] stage=terminal ordinal=13/13\n";
    let action = b"compiler output\n";
    let final_record = b"final action bytes";
    assert!(is_live_progress_record(scheduler));
    assert!(is_live_progress_record(compiler));
    assert!(is_live_progress_record(shard));
    assert!(is_live_progress_record(browser));
    assert!(is_live_progress_record(setup));
    assert!(is_live_progress_record(advancement));
    assert!(is_live_progress_record(setup_advancement));
    assert!(is_live_progress_record(terminal));
    for diagnostic in [
        scheduler.as_slice(),
        compiler.as_slice(),
        shard.as_slice(),
        browser.as_slice(),
        setup.as_slice(),
    ] {
        assert!(
            !is_lease_renewal_record(diagnostic),
            "elapsed or scheduler state alone must not renew: {}",
            String::from_utf8_lossy(diagnostic)
        );
    }
    assert!(is_lease_renewal_record(advancement));
    assert!(is_lease_renewal_record(setup_advancement));
    assert!(is_lease_renewal_record(terminal));
    assert!(!is_live_progress_record(action));
    assert!(!is_live_progress_record(
        &[b"[verify] ".as_slice(), &vec![b'x'; 8 * 1024]].concat()
    ));

    let captured = [
        scheduler.as_slice(),
        action.as_slice(),
        terminal.as_slice(),
        final_record.as_slice(),
    ]
    .concat();
    let mut deferred = Vec::new();
    write_deferred_supervisor_output(&mut deferred, &captured)
        .expect("deferred supervisor output writes");
    assert_eq!(
        deferred,
        [action.as_slice(), final_record.as_slice()].concat()
    );
}

#[cfg(unix)]
#[test]
fn cpu_saturated_progress_renews_the_inactivity_lease_past_its_original_deadline() {
    let bounds = fixture_progress_bounds(Duration::from_millis(80), Duration::from_secs(1));
    let executable = std::env::current_exe().expect("resolve the xtask unit-test process");
    let count = OsString::from("6");
    let interval = OsString::from("30");
    let started = Instant::now();
    let mut live = Vec::new();
    let run = run_bounded_process_streaming(
        "active progress lease fixture",
        executable,
        benchmark_arguments(PROGRESS_LEASE_CHILD_TEST),
        &std::env::current_dir().expect("resolve verifier fixture cwd"),
        &CancellationToken::new(),
        None,
        &[
            ("PROGRESS_FIXTURE_COUNT", count.as_os_str()),
            ("PROGRESS_FIXTURE_INTERVAL_MS", interval.as_os_str()),
        ],
        bounds,
        &mut live,
    )
    .expect("supervise active progress fixture");
    let elapsed = started.elapsed();
    let CommandRun::Exited(output) = run else {
        panic!("active progress fixture did not exit: {run:?}");
    };
    assert!(output.status.success, "active progress fixture succeeds");
    assert!(
        elapsed > bounds.inactivity,
        "the fixture must outlive the original inactivity lease: {elapsed:?}"
    );
    let live = String::from_utf8(live).expect("active progress is UTF-8");
    let captured = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        live.matches("fixture=lease-renewal").count(),
        6,
        "every captured progress record must be live: live={live:?} captured={captured:?}"
    );
}

#[cfg(unix)]
#[test]
fn trickle_progress_cannot_renew_past_the_absolute_process_tree_limit() {
    let bounds = fixture_progress_bounds(Duration::from_millis(80), Duration::from_millis(260));
    let executable = std::env::current_exe().expect("resolve the xtask unit-test process");
    let count = OsString::from("100");
    let interval = OsString::from("30");
    let started = Instant::now();
    let mut live = Vec::new();
    let run = run_bounded_process_streaming(
        "trickle progress lease fixture",
        executable,
        benchmark_arguments(PROGRESS_LEASE_CHILD_TEST),
        &std::env::current_dir().expect("resolve verifier fixture cwd"),
        &CancellationToken::new(),
        None,
        &[
            ("PROGRESS_FIXTURE_COUNT", count.as_os_str()),
            ("PROGRESS_FIXTURE_INTERVAL_MS", interval.as_os_str()),
        ],
        bounds,
        &mut live,
    )
    .expect("supervise trickle progress fixture");
    let elapsed = started.elapsed();
    assert!(
        matches!(run, CommandRun::TimedOut(_)),
        "trickle progress crossed the absolute limit: {run:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(200) && elapsed < Duration::from_secs(1),
        "absolute limit was not the bounded terminal: {elapsed:?}"
    );
    assert!(
        String::from_utf8(live)
            .expect("trickle progress is UTF-8")
            .matches("fixture=lease-renewal")
            .count()
            >= 4,
        "the fixture must make enough progress to renew its inactivity lease"
    );
}

#[cfg(unix)]
#[test]
fn silent_progress_lease_reaps_a_non_cooperative_in_process_verifier() {
    let progress_interval = Duration::from_millis(60);
    let bounds = fixture_progress_bounds(progress_interval * 3, Duration::from_secs(2));
    let executable = std::env::current_exe().expect("resolve the xtask unit-test process");
    let started = Instant::now();
    let mut live = Vec::new();
    let run = run_bounded_process_streaming(
        "non-cooperative verifier fixture",
        executable,
        benchmark_arguments(NON_COOPERATIVE_CHILD_TEST),
        &std::env::current_dir().expect("resolve verifier fixture cwd"),
        &CancellationToken::new(),
        None,
        &[],
        bounds,
        &mut live,
    )
    .expect("supervise the non-cooperative verifier fixture");
    let elapsed = started.elapsed();
    let CommandRun::TimedOut(output) = run else {
        panic!("non-cooperative verifier did not reach the hard timeout: {run:?}");
    };
    let stdout = String::from_utf8(output.stdout).expect("fixture stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("fixture stderr is UTF-8");
    let transcript = format!("{stdout}{stderr}");
    let live = String::from_utf8(live).expect("live fixture output is UTF-8");
    assert!(
        live.matches("stage=progress ordinal=1/13").count() >= 2,
        "the compressed scheduler stayed responsive without renewing the lease: {live:?}"
    );
    assert!(!live.contains("stage=advance"));
    let pid = transcript
        .lines()
        .find_map(|line| {
            line.find(SUPERVISOR_PID_MARKER)
                .map(|offset| &line[offset + SUPERVISOR_PID_MARKER.len()..])
        })
        .and_then(|pid| pid.parse::<u32>().ok())
        .unwrap_or_else(|| panic!("fixture emitted no pid marker: {transcript}"));
    let scaled_envelope = bounds.inactivity
        + bounds.process.grace
        + bounds.process.reap
        + bounds.process.output_drain;
    assert!(
        elapsed <= scaled_envelope + Duration::from_secs(1),
        "supervisor exceeded the scaled hard envelope: elapsed={elapsed:?} envelope={scaled_envelope:?}"
    );
    assert!(
        !process_exists(pid),
        "supervisor returned while owned verifier process {pid} was still alive"
    );
}

#[cfg(unix)]
#[test]
fn streaming_outer_supervisor_preserves_interruption_and_group_cleanup() {
    let bounds = fixture_progress_bounds(Duration::from_secs(1), Duration::from_secs(10));
    let pid_file = fixture_pid_file("streaming-supervisor-cancel");
    let cancellation = CancellationToken::new();
    let controller_token = cancellation.clone();
    let controller_pid_file = pid_file.clone();
    let controller = thread::spawn(move || {
        wait_for_path(&controller_pid_file);
        controller_token.cancel();
    });
    let mut live = Vec::new();
    let started = Instant::now();
    let run = run_bounded_process_streaming(
        "streaming cancellation fixture",
        "/bin/sh",
        [
            "-c",
            concat!(
                "printf '[verify] stage=advance ordinal=1/13 cursor=1 fixture=cancelled\\n'; ",
                "sleep 30 & printf '%s %s\\n' \"$$\" \"$!\" > \"$PID_FILE\"; wait"
            ),
        ],
        &std::env::current_dir().expect("resolve streaming fixture cwd"),
        &cancellation,
        None,
        &[("PID_FILE", pid_file.as_os_str())],
        bounds,
        &mut live,
    )
    .expect("stream and cancel the verifier fixture");
    controller
        .join()
        .expect("stream cancellation controller joins");
    let CommandRun::Cancelled(output) = run else {
        panic!("streaming fixture did not report cancellation: {run:?}");
    };
    let progress = "[verify] stage=advance ordinal=1/13 cursor=1 fixture=cancelled\n";
    assert_eq!(
        String::from_utf8(live).expect("live output is UTF-8"),
        progress
    );
    assert_eq!(
        String::from_utf8(output.stdout).expect("captured output is UTF-8"),
        progress
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "streaming cancellation ignored its token"
    );
    assert_processes_gone(&pid_file, "streaming supervisor cancellation");
    fs::remove_file(pid_file).expect("remove streaming cancellation pid file");
}

#[cfg(unix)]
#[test]
fn live_output_failure_cancels_and_reaps_the_supervised_group() {
    let bounds = fixture_progress_bounds(Duration::from_secs(1), Duration::from_secs(10));
    let pid_file = fixture_pid_file("streaming-output-failure");
    let error = run_bounded_process_streaming(
        "streaming output failure fixture",
        "/bin/sh",
        [
            "-c",
            concat!(
                "sleep 30 & printf '%s %s\\n' \"$$\" \"$!\" > \"$PID_FILE\"; ",
                "printf '[verify] stage=advance ordinal=1/13 cursor=1 fixture=closed-output\\n'; wait"
            ),
        ],
        &std::env::current_dir().expect("resolve output failure fixture cwd"),
        &CancellationToken::new(),
        None,
        &[("PID_FILE", pid_file.as_os_str())],
        bounds,
        &mut RefusingOutput,
    )
    .expect_err("a closed live output sink refuses the supervisor");
    assert!(matches!(
        error,
        StreamingRunError::Output(error)
            if error.kind() == std::io::ErrorKind::BrokenPipe
    ));
    assert_processes_gone(&pid_file, "streaming output failure");
    fs::remove_file(pid_file).expect("remove streaming output failure pid file");
}

#[cfg(unix)]
#[test]
#[ignore = "launched exactly by the progress-lease parent test"]
fn non_cooperative_in_process_verifier_fixture() {
    let cancellation = CancellationToken::new();
    let interrupted = Arc::new(AtomicBool::new(false));
    let signal_guard = SignalGuard::install(cancellation.clone(), Arc::clone(&interrupted))
        .expect("install fixture signal bridge");
    println!("{SUPERVISOR_PID_MARKER}{}", std::process::id());
    std::io::stdout().flush().expect("flush fixture pid marker");

    let runner = TimedRunner::uniform(Duration::from_secs(60));
    let mut output = std::io::stdout();
    let _ = verify_with_deadline_and_rebalance(
        &runner,
        &mut output,
        Budget::fixture(1, 1, 0),
        cancellation,
        interrupted,
        Duration::from_secs(10),
        Duration::from_millis(60),
        |budget| budget,
    );
    drop(signal_guard);
}

#[cfg(unix)]
#[test]
#[ignore = "launched exactly by the progress-lease parent tests"]
#[expect(
    clippy::disallowed_methods,
    reason = "the isolated fixture reads only its two parent-injected timing values"
)]
fn progress_lease_child_fixture() {
    let count = std::env::var("PROGRESS_FIXTURE_COUNT")
        .expect("fixture count is present")
        .parse::<usize>()
        .expect("fixture count is an integer");
    let interval = std::env::var("PROGRESS_FIXTURE_INTERVAL_MS")
        .expect("fixture interval is present")
        .parse::<u64>()
        .expect("fixture interval is an integer");
    // Libtest writes the test name without a trailing newline. Production
    // launches the inner verifier directly, so align this fixture's first frame.
    println!();
    for index in 0..count {
        println!(
            "[verify] stage=advance ordinal=1/13 cursor={} fixture=lease-renewal index={index}",
            index + 1
        );
        std::io::stdout().flush().expect("flush lease progress");
        let until = Instant::now() + Duration::from_millis(interval);
        let mut value = u64::try_from(index).expect("small progress fixture index");
        while Instant::now() < until {
            value = std::hint::black_box(value.rotate_left(7).wrapping_add(0x9e37_79b9));
        }
        std::hint::black_box(value);
    }
}
