use super::capture::{CaptureWorker, FailureContext, start_capture};
use super::guardian::{group_probe_state, group_state};
use super::launch::spawn_inner_refusing_descriptor_setup;
use super::{
    Completion, ProcessCompletion, ProcessState, WaitError, WaitStep, platform, resolve_signal,
    resolve_signal_against, spawn, spawn_observed, wait_for_exit_notice,
};
use crate::lock::{Acquisition, LockFile, LockName, tests::unique_lock_path};
use crate::observe::{
    Observation, ObservationFailure, ObserveError, ObservedRecord, OutputStream, RecordBoundary,
    RecordObserver,
};
use crate::spec::{
    Argv, Cancellation, CancellationToken, Deadlines, Environment, Output, ProcessGroup, Redaction,
    RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy,
};
use crate::surface::process_state;
use std::fs;
use std::io::{self, Read};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;

#[derive(Debug)]
struct ForwardObserver {
    records: mpsc::Sender<(Vec<u8>, RecordBoundary)>,
    failures: mpsc::Sender<ObservationFailure>,
}

impl RecordObserver for ForwardObserver {
    fn observe(&self, record: ObservedRecord<'_>) -> Result<(), ObserveError> {
        self.records
            .send((record.bytes().to_vec(), record.boundary()))
            .map_err(|error| ObserveError::new(error.to_string()))
    }

    fn failed(&self, failure: &ObservationFailure) {
        let _result = self.failures.send(failure.clone());
    }
}

#[test]
fn selected_records_and_opaque_diagnostics_drain_before_wait() {
    let (records, received) = mpsc::channel();
    let (failures, failed) = mpsc::channel();
    let observer = Arc::new(ForwardObserver { records, failures });
    let mut spec = test_spec(
        "/bin/sh",
        [
            "-c",
            concat!(
                "i=0; ",
                "while [ $i -lt 12000 ]; do ",
                "printf 'record-%05d-abcdefghijklmnopqrstuvwxyz\\n' \"$i\"; ",
                "printf 'diagnostic-%05d-abcdefghijklmnopqrstuvwxyz\\n' \"$i\" >&2; ",
                "i=$((i + 1)); ",
                "done; ",
                "printf 'terminal-record'",
            ),
        ],
        Environment::empty(),
        Cancellation::Never,
        Duration::from_secs(10),
        StreamPolicy::Capture { limit: 1024 },
    );
    spec.output = Output::new(
        StdinPolicy::Null,
        StreamPolicy::Capture { limit: 1024 },
        StreamPolicy::Capture { limit: 1024 },
    );
    let observation =
        Observation::new(OutputStream::Stdout, 128, 64, observer).expect("bounded observation");
    let running = spawn_observed(spec, observation).expect("observed child launches");

    for index in 0..12000 {
        let (bytes, boundary) = received
            .recv_timeout(Duration::from_secs(10))
            .expect("records drain while Running is only held");
        assert_eq!(boundary, RecordBoundary::Complete);
        assert_eq!(
            bytes,
            format!("record-{index:05}-abcdefghijklmnopqrstuvwxyz\n").as_bytes()
        );
    }
    assert_eq!(
        received
            .recv_timeout(Duration::from_secs(2))
            .expect("terminal record drains"),
        (b"terminal-record".to_vec(), RecordBoundary::Terminal)
    );
    assert!(
        failed.try_recv().is_err(),
        "observation failed unexpectedly"
    );

    let outcome = running
        .wait()
        .expect("child is reaped after all records arrive");
    assert!(matches!(outcome.completion(), Completion::Exited(status) if status.success()));
    let stdout = outcome.stdout().expect("selected output remains bounded");
    let stderr = outcome.stderr().expect("diagnostics remain bounded");
    assert_eq!((stdout.bytes().len(), stderr.bytes().len()), (1024, 1024));
    assert!(stdout.truncated() && stderr.truncated());
    assert!(stdout.complete() && stderr.complete());
}

#[derive(Debug)]
struct BlockingObserver {
    entered: mpsc::Sender<()>,
    release: Arc<(Mutex<bool>, Condvar)>,
    failures: mpsc::Sender<ObservationFailure>,
}

impl RecordObserver for BlockingObserver {
    fn observe(&self, _record: ObservedRecord<'_>) -> Result<(), ObserveError> {
        let _sent = self.entered.send(());
        let (lock, changed) = &*self.release;
        let released = lock.lock().expect("release lock");
        let _released = changed
            .wait_while(released, |released| !*released)
            .expect("release wait");
        Ok(())
    }

    fn failed(&self, failure: &ObservationFailure) {
        let _sent = self.failures.send(failure.clone());
    }
}

#[test]
fn one_blocked_sink_call_terminates_and_reaps_without_more_output() {
    let (entered, observed_entered) = mpsc::channel();
    let (failures, observed_failure) = mpsc::channel();
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let observer = Arc::new(BlockingObserver {
        entered,
        release: Arc::clone(&release),
        failures,
    });
    let observation =
        Observation::new(OutputStream::Stdout, 32, 1, observer).expect("bounded observation");
    let running = spawn_observed(
        test_spec(
            "/bin/sh",
            ["-c", "printf 'one\\n'; exec /usr/bin/tail -f /dev/null"],
            Environment::empty(),
            Cancellation::Never,
            Duration::from_secs(10),
            StreamPolicy::Capture { limit: 64 },
        ),
        observation,
    )
    .expect("backpressure child launches");
    observed_entered
        .recv_timeout(Duration::from_secs(1))
        .expect("sink begins its blocked record");
    let failure = observed_failure
        .recv_timeout(Duration::from_secs(2))
        .expect("bounded reader reports backpressure after cleanup");
    assert!(matches!(
        failure,
        ObservationFailure::SinkBackpressure {
            stream: OutputStream::Stdout,
            capacity: 1,
            ..
        }
    ));
    let outcome = running.wait().expect("backpressured child was reaped");
    assert!(matches!(
        outcome.observation_failure(),
        Some(ObservationFailure::SinkBackpressure { .. })
    ));

    let (lock, changed) = &*release;
    *lock.lock().expect("release lock") = true;
    changed.notify_all();
}

/// Far enough past the spec's drain deadline that reaching it is unmistakable.
const OBSERVER_BOUND: Duration = Duration::from_secs(10);

/// A reader that says when it has reached end of output.
///
/// The capture worker records end of output as its own fact one statement
/// after this returns zero, so announcing it here is what lets a test know the
/// reader half of completion is settled without watching a clock for it.
struct AnnouncedEof {
    bytes: io::Cursor<Vec<u8>>,
    ended: mpsc::Sender<()>,
}

impl Read for AnnouncedEof {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.bytes.read(buffer)?;
        if read == 0 {
            let _sent = self.ended.send(());
        }
        Ok(read)
    }
}

#[test]
fn an_observer_still_working_at_the_drain_deadline_is_named_rather_than_read_as_complete() {
    let (ended, reader_ended) = mpsc::channel();
    let (entered, observed_entered) = mpsc::channel();
    let (failures, observed_failure) = mpsc::channel();
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let observer: Arc<dyn RecordObserver> = Arc::new(BlockingObserver {
        entered,
        release: Arc::clone(&release),
        failures,
    });
    let mut running = spawn(test_spec(
        "/usr/bin/true",
        std::iter::empty::<&str>(),
        Environment::empty(),
        Cancellation::Never,
        Duration::from_secs(10),
        StreamPolicy::Null,
    ))
    .expect("abandoned-drain child launches");
    let context = Arc::new(FailureContext {
        pid: running.pid,
        group: running.spec.group,
        owner: running.spec.wait.owner().to_owned(),
        grace: running.spec.grace,
        reap: running.spec.deadlines.reap(),
        process: Arc::clone(&running.process),
        wake: running
            .cancellation_wake
            .try_clone()
            .expect("fixture wake clones"),
        failed: std::sync::atomic::AtomicBool::new(false),
    });
    // The reader reaches end of output while the observer is still held, which
    // forces the two halves of capture completion apart. The observer's own
    // bound is set far past the spec's drain deadline so the record is merely
    // outstanding rather than failed: nothing here is broken, it is unfinished.
    // The record ends in a newline, so the framer has nothing left to flush at
    // end of output and the worker goes straight from the announcement below to
    // recording it.
    running.stderr = Some(start_capture(
        AnnouncedEof {
            bytes: io::Cursor::new(b"record\n".to_vec()),
            ended,
        },
        64,
        CaptureWorker {
            stream: OutputStream::Stderr,
            observation: Some(
                Observation::new(OutputStream::Stderr, 32, 1, Arc::clone(&observer))
                    .expect("abandoned-drain observation"),
            ),
            failure_observer: Some(Arc::clone(&observer)),
            sink_deadline: OBSERVER_BOUND,
            failure: context,
            wake: running
                .cancellation_wake
                .try_clone()
                .expect("capture wake clones"),
            observer_wake: Some(
                running
                    .cancellation_wake
                    .try_clone()
                    .expect("observer wake clones"),
            ),
        },
    ));
    observed_entered
        .recv_timeout(Duration::from_secs(2))
        .expect("observer begins the record it will not finish");
    reader_ended
        .recv_timeout(Duration::from_secs(2))
        .expect("reader reaches end of output");

    let started = std::time::Instant::now();
    let outcome = running.wait().expect("abandoned-drain child reaps");
    let elapsed = started.elapsed();

    assert!(
        !outcome.outputs_drained(),
        "an outstanding record was reported as a completed drain"
    );
    assert!(
        elapsed < OBSERVER_BOUND / 4,
        "the drain was abandoned on the observer's bound, not the spec's: {elapsed:?}"
    );
    // The two answers a caller already had cannot see this: the reader did
    // reach end of output, and an observer that is still going has not failed.
    assert!(
        outcome
            .stderr()
            .expect("diagnostics were captured")
            .complete(),
        "the reader reached end of output"
    );
    assert!(outcome.observation_failure().is_none());
    assert!(observed_failure.try_recv().is_err());

    let (lock, changed) = &*release;
    *lock.lock().expect("release lock") = true;
    changed.notify_all();
}

#[derive(Debug)]
struct RefusingObserver {
    failures: mpsc::Sender<ObservationFailure>,
}

impl RecordObserver for RefusingObserver {
    fn observe(&self, _record: ObservedRecord<'_>) -> Result<(), ObserveError> {
        Err(ObserveError::new("fixture tape refusal"))
    }

    fn failed(&self, failure: &ObservationFailure) {
        let _sent = self.failures.send(failure.clone());
    }
}

#[test]
fn sink_refusal_and_framing_overflow_are_named_and_reaped() {
    let (failures, refused) = mpsc::channel();
    let refusing = Observation::new(
        OutputStream::Stdout,
        32,
        1,
        Arc::new(RefusingObserver { failures }),
    )
    .expect("refusing observation");
    let refusal = spawn_observed(
        test_spec(
            "/bin/sh",
            ["-c", "printf 'record\\n'; exec /usr/bin/tail -f /dev/null"],
            Environment::empty(),
            Cancellation::Never,
            Duration::from_secs(10),
            StreamPolicy::Capture { limit: 64 },
        ),
        refusing,
    )
    .expect("refusal child launches");
    assert!(matches!(
        refused.recv_timeout(Duration::from_secs(2)).expect("refusal reports"),
        ObservationFailure::SinkRefused { detail, .. } if detail == "fixture tape refusal"
    ));
    let refusal = refusal.wait().expect("refusal child reaps");
    assert!(matches!(
        refusal.observation_failure(),
        Some(ObservationFailure::SinkRefused { .. })
    ));

    let (records, received) = mpsc::channel();
    let (failures, overflowed) = mpsc::channel();
    let overflow = Observation::new(
        OutputStream::Stdout,
        4,
        1,
        Arc::new(ForwardObserver { records, failures }),
    )
    .expect("overflow observation");
    let overflow = spawn_observed(
        test_spec(
            "/bin/sh",
            ["-c", "printf '12345'; exec /usr/bin/tail -f /dev/null"],
            Environment::empty(),
            Cancellation::Never,
            Duration::from_secs(10),
            StreamPolicy::Capture { limit: 64 },
        ),
        overflow,
    )
    .expect("overflow child launches");
    assert!(matches!(
        overflowed
            .recv_timeout(Duration::from_secs(2))
            .expect("overflow reports"),
        ObservationFailure::FramingOverflow {
            limit: 4,
            observed: 5,
            ..
        }
    ));
    assert_eq!(
        received
            .recv_timeout(Duration::from_secs(2))
            .expect("overflow bytes remain observable"),
        (b"12345".to_vec(), RecordBoundary::Terminal)
    );
    let overflow = overflow.wait().expect("overflow child reaps");
    assert!(matches!(
        overflow.observation_failure(),
        Some(ObservationFailure::FramingOverflow { .. })
    ));
}

struct FailingReader;

impl Read for FailingReader {
    fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("fixture reader failure"))
    }
}

#[test]
fn selected_reader_failure_uses_the_same_bounded_cleanup_path() {
    let running = spawn(test_spec(
        "/usr/bin/tail",
        ["-f", "/dev/null"],
        Environment::empty(),
        Cancellation::Never,
        Duration::from_secs(10),
        StreamPolicy::Null,
    ))
    .expect("reader-failure child launches");
    let (records, _received) = mpsc::channel();
    let (failures, failed) = mpsc::channel();
    let observer: Arc<dyn RecordObserver> = Arc::new(ForwardObserver { records, failures });
    let observation = Observation::new(OutputStream::Stdout, 32, 1, Arc::clone(&observer))
        .expect("reader-failure observation");
    let context = Arc::new(FailureContext {
        pid: running.pid,
        group: running.spec.group,
        owner: running.spec.wait.owner().to_owned(),
        grace: running.spec.grace,
        reap: running.spec.deadlines.reap(),
        process: Arc::clone(&running.process),
        wake: running
            .cancellation_wake
            .try_clone()
            .expect("fixture wake clones"),
        failed: std::sync::atomic::AtomicBool::new(false),
    });
    let _capture = start_capture(
        FailingReader,
        32,
        CaptureWorker {
            stream: OutputStream::Stdout,
            observation: Some(observation),
            failure_observer: Some(observer),
            sink_deadline: Duration::from_millis(50),
            failure: context,
            wake: running
                .cancellation_wake
                .try_clone()
                .expect("capture wake clones"),
            observer_wake: Some(
                running
                    .cancellation_wake
                    .try_clone()
                    .expect("observer wake clones"),
            ),
        },
    );
    assert!(matches!(
        failed.recv_timeout(Duration::from_secs(2)).expect("reader failure reports"),
        ObservationFailure::Reader { detail, .. }
            if detail.contains("fixture reader failure")
    ));
    let outcome = running.wait().expect("reader-failure child reaps");
    assert!(matches!(
        outcome.observation_failure(),
        Some(ObservationFailure::Reader { .. })
    ));
}

#[test]
fn opaque_reader_failure_uses_the_selected_observer_cleanup_path() {
    let mut running = spawn(test_spec(
        "/usr/bin/tail",
        ["-f", "/dev/null"],
        Environment::empty(),
        Cancellation::Never,
        Duration::from_secs(10),
        StreamPolicy::Null,
    ))
    .expect("opaque-reader-failure child launches");
    let (records, _received) = mpsc::channel();
    let (failures, failed) = mpsc::channel();
    let observer: Arc<dyn RecordObserver> = Arc::new(ForwardObserver { records, failures });
    let context = Arc::new(FailureContext {
        pid: running.pid,
        group: running.spec.group,
        owner: running.spec.wait.owner().to_owned(),
        grace: running.spec.grace,
        reap: running.spec.deadlines.reap(),
        process: Arc::clone(&running.process),
        wake: running
            .cancellation_wake
            .try_clone()
            .expect("fixture wake clones"),
        failed: std::sync::atomic::AtomicBool::new(false),
    });
    running.stderr = Some(start_capture(
        FailingReader,
        32,
        CaptureWorker {
            stream: OutputStream::Stderr,
            observation: None,
            failure_observer: Some(observer),
            sink_deadline: Duration::from_millis(50),
            failure: context,
            wake: running
                .cancellation_wake
                .try_clone()
                .expect("capture wake clones"),
            observer_wake: None,
        },
    ));
    assert!(matches!(
        failed
            .recv_timeout(Duration::from_secs(2))
            .expect("opaque reader failure reports"),
        ObservationFailure::Reader {
            stream: OutputStream::Stderr,
            detail,
        } if detail.contains("fixture reader failure")
    ));
    let outcome = running.wait().expect("opaque-reader-failure child reaps");
    assert!(matches!(
        outcome.observation_failure(),
        Some(ObservationFailure::Reader {
            stream: OutputStream::Stderr,
            ..
        })
    ));
}

#[derive(Debug)]
struct PanickingObserver {
    failures: mpsc::Sender<ObservationFailure>,
}

impl RecordObserver for PanickingObserver {
    fn observe(&self, _record: ObservedRecord<'_>) -> Result<(), ObserveError> {
        panic!("fixture observer loss")
    }

    fn failed(&self, failure: &ObservationFailure) {
        let _sent = self.failures.send(failure.clone());
    }
}

#[test]
fn observer_panic_is_named_as_loss_and_reaped() {
    let (failures, failed) = mpsc::channel();
    let observation = Observation::new(
        OutputStream::Stdout,
        32,
        1,
        Arc::new(PanickingObserver { failures }),
    )
    .expect("panic observation");
    let running = spawn_observed(
        test_spec(
            "/bin/sh",
            ["-c", "printf 'record\\n'; exec /usr/bin/tail -f /dev/null"],
            Environment::empty(),
            Cancellation::Never,
            Duration::from_secs(10),
            StreamPolicy::Capture { limit: 64 },
        ),
        observation,
    )
    .expect("observer-loss child launches");
    assert!(matches!(
        failed
            .recv_timeout(Duration::from_secs(2))
            .expect("observer loss reports"),
        ObservationFailure::ObserverLost {
            stream: OutputStream::Stdout
        }
    ));
    let outcome = running.wait().expect("observer-loss child reaps");
    assert!(matches!(
        outcome.observation_failure(),
        Some(ObservationFailure::ObserverLost { .. })
    ));
}

#[test]
fn output_capture_is_bounded_and_environment_is_explicit() {
    let spec = test_spec(
        "/bin/sh",
        ["-c", "printf '%s' \"$ONLY\"; printf '56789'"],
        Environment::new([("ONLY", "1234")]).expect("valid environment"),
        Cancellation::Never,
        Duration::from_secs(1),
        StreamPolicy::Capture { limit: 4 },
    );
    let outcome = spawn(spec)
        .expect("spawn bounded output child")
        .wait()
        .expect("wait for bounded output child");

    assert!(matches!(outcome.completion(), Completion::Exited(status) if status.success()));
    let stdout = outcome.stdout().expect("stdout was captured");
    assert_eq!(stdout.bytes(), b"1234");
    assert!(stdout.truncated());
    assert!(stdout.complete());
}

#[test]
fn ambient_environment_is_cleared_before_explicit_entries_are_set() {
    let spec = test_spec(
        "/usr/bin/env",
        std::iter::empty::<&str>(),
        Environment::new([("SILL_ONLY", "exact")]).expect("valid environment"),
        Cancellation::Never,
        Duration::from_secs(1),
        StreamPolicy::Capture { limit: 1024 },
    );
    let outcome = spawn(spec)
        .expect("spawn environment probe")
        .wait()
        .expect("wait for environment probe");

    assert!(matches!(outcome.completion(), Completion::Exited(status) if status.success()));
    assert_eq!(
        outcome.stdout().expect("stdout was captured").bytes(),
        b"SILL_ONLY=exact\n"
    );
}

#[test]
fn runtime_deadline_terminates_the_whole_process_group() {
    let spec = test_spec(
        "/bin/sh",
        ["-c", "/usr/bin/tail -f /dev/null & wait"],
        Environment::empty(),
        Cancellation::Never,
        Duration::from_millis(20),
        StreamPolicy::Null,
    );
    let outcome = spawn(spec)
        .expect("spawn process group")
        .wait()
        .expect("reap timed-out process group");

    assert!(matches!(outcome.completion(), Completion::TimedOut(_)));
}

#[test]
fn leader_exit_does_not_complete_while_a_group_member_remains_live() {
    let pid_file = unique_lock_path("residual-process-group");
    let spec = test_spec(
        "/bin/sh",
        [
            "-c",
            concat!(
                "trap '' TERM; ",
                "/usr/bin/tail -f /dev/null >/dev/null 2>&1 & ",
                "printf '%s\\n' \"$!\" > \"$PID_FILE\"; ",
                "exit 0",
            ),
        ],
        Environment::new([("PID_FILE", pid_file.as_os_str())]).expect("valid environment"),
        Cancellation::Never,
        Duration::from_secs(1),
        StreamPolicy::Null,
    );
    let running = spawn(spec).expect("spawn residual process group");

    let running = match running
        .wait_for(Duration::from_millis(100))
        .expect("observe leader exit")
    {
        WaitStep::Running(running) => *running,
        WaitStep::Exited(_) => panic!("leader exit was mistaken for process-group completion"),
    };
    let child = fs::read_to_string(&pid_file)
        .expect("residual child pid reads")
        .trim()
        .parse::<u32>()
        .expect("residual child pid parses");

    running
        .signal_terminate()
        .expect("terminate residual process group");
    let running = match running
        .wait_for(Duration::from_millis(30))
        .expect("observe ignored termination")
    {
        WaitStep::Running(running) => *running,
        WaitStep::Exited(_) => panic!("TERM-ignoring group was reported complete"),
    };
    running.signal_kill().expect("kill residual process group");
    let outcome = match running
        .wait_for(Duration::from_secs(1))
        .expect("reap residual process group")
    {
        WaitStep::Exited(outcome) => outcome,
        WaitStep::Running(_) => panic!("killed process group survived its reap bound"),
    };

    assert!(matches!(outcome.completion(), Completion::Exited(status) if status.success()));
    assert!(
        crate::surface::process_state::process_terminal(child),
        "residual group member remained live"
    );
    fs::remove_file(pid_file).expect("remove residual pid fixture");
}

/// A group that refuses a signal because it is tearing itself down is
/// answering for its own end, and a group that refuses one while a member is
/// still live is refusing on permission. The two are one errno apart and the
/// difference lives only in the moment between the refusal and the reading, so
/// the refusal is handed over already collected and each case is asserted here
/// against real groups rather than raced for.
#[test]
fn a_group_refusing_its_own_teardown_is_forgiven_and_a_live_one_is_not() {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    fn refused(code: i32) -> Option<io::Error> {
        Some(io::Error::from_raw_os_error(code))
    }

    let mut reaped = Command::new("/usr/bin/true");
    reaped.process_group(0);
    let mut reaped = reaped.spawn().expect("reaped group fixture launches");
    let gone = reaped.id();
    assert!(reaped.wait().expect("reaped group fixture reaps").success());

    let mut running = Command::new("/usr/bin/tail");
    running
        .args(["-f", "/dev/null"])
        .process_group(0)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut running = running.spawn().expect("live group fixture launches");
    let held = running.id();

    // Nothing refused is nothing to answer for, and no group is enumerated.
    assert!(resolve_signal(None, held, ProcessGroup::New, "owner").is_ok());

    // ESRCH is the child that finished first, whatever it was aimed at. The
    // live group proves the answer is the errno's and not the group's.
    for group in [ProcessGroup::New, ProcessGroup::Inherit] {
        assert!(resolve_signal(refused(platform::ESRCH), held, group, "owner").is_ok());
    }

    // A group with nothing left to signal is the teardown this forgives, and
    // is the answer that used to fail the caller that asked for it.
    assert!(resolve_signal(refused(platform::EPERM), gone, ProcessGroup::New, "owner").is_ok());

    // A group still holding a live member refused on permission, which stays a
    // fault, named as one, carrying the errno the kernel gave.
    let fault = resolve_signal(refused(platform::EPERM), held, ProcessGroup::New, "owner")
        .expect_err("a live group refuses on permission");
    match fault {
        WaitError::Io {
            owner,
            operation,
            source,
        } => {
            assert_eq!(owner, "owner");
            assert_eq!(operation, "signalling child process group");
            assert_eq!(source.raw_os_error(), Some(platform::EPERM));
        }
        WaitError::ReapDeadlineExceeded { .. } => panic!("a refused signal is not a reap deadline"),
    }

    // One process cannot be partway through becoming absent, so a refusal
    // about one is never softened by what the group of that pid looks like.
    assert!(
        resolve_signal(
            refused(platform::EPERM),
            gone,
            ProcessGroup::Inherit,
            "owner"
        )
        .is_err()
    );

    // Nothing else is forgiven, however finished the group is.
    for code in [libc::EACCES, libc::EINVAL, libc::EIO] {
        assert!(
            resolve_signal(refused(code), gone, ProcessGroup::New, "owner").is_err(),
            "errno {code} was forgiven"
        );
    }

    // SAFETY: this kills the group the fixture above created and nothing else.
    assert_eq!(
        unsafe { libc::kill(-(held as libc::pid_t), libc::SIGKILL) },
        0
    );
    assert!(!running.wait().expect("live group fixture reaps").success());
}

#[test]
fn cancellation_terminates_and_reaps_the_child() {
    let token = CancellationToken::new();
    let spec = test_spec(
        "/usr/bin/tail",
        ["-f", "/dev/null"],
        Environment::empty(),
        Cancellation::Token(token.clone()),
        Duration::from_secs(1),
        StreamPolicy::Null,
    );
    let running = spawn(spec).expect("spawn cancellable child");
    let waiter = std::thread::spawn(move || running.wait());
    assert!(
        token.wait_for_registration(Duration::from_secs(1)),
        "wait did not register its cancellation callback"
    );
    token.cancel();
    let outcome = waiter
        .join()
        .expect("cancellation waiter thread must not panic")
        .expect("reap cancelled child");

    assert!(matches!(outcome.completion(), Completion::Cancelled(_)));
}

#[test]
fn exec_child_does_not_inherit_lock_possession() {
    let path = unique_lock_path("cloexec");
    let lock = LockFile::new(LockName::new("cloexec").expect("valid name"), &path);
    let possession = match lock.try_acquire().expect("acquire parent lock") {
        Acquisition::Acquired(possession) => possession,
        Acquisition::Contended => panic!("fresh parent lock must be available"),
    };
    let token = CancellationToken::new();
    let running = spawn(test_spec(
        "/usr/bin/tail",
        ["-f", "/dev/null"],
        Environment::empty(),
        Cancellation::Token(token.clone()),
        Duration::from_secs(1),
        StreamPolicy::Null,
    ))
    .expect("spawn child while lock is held");

    drop(possession);
    let reacquired = match lock.try_acquire().expect("reacquire after parent release") {
        Acquisition::Acquired(possession) => possession,
        Acquisition::Contended => panic!("exec child inherited the lock descriptor"),
    };
    token.cancel();
    assert!(matches!(
        running
            .wait()
            .expect("reap lock-inheritance probe")
            .completion(),
        Completion::Cancelled(_)
    ));
    drop(reacquired);
    fs::remove_file(path).expect("remove lock-inheritance test file");
}

#[test]
fn descriptor_setup_failure_refuses_protected_child_admission() {
    let sentinel = unique_lock_path("guardian-admission");
    let lock_path = unique_lock_path("guardian-admission-lock");
    let lock = fs::File::create(&lock_path).expect("guardian lock file creates");
    let spec = test_spec(
        "/usr/bin/touch",
        [&sentinel],
        Environment::empty(),
        Cancellation::Never,
        Duration::from_secs(1),
        StreamPolicy::Null,
    );

    let error = spawn_inner_refusing_descriptor_setup(spec, &lock)
        .expect_err("descriptor setup failure must refuse launch");

    assert!(
        error
            .to_string()
            .contains("lock guardian did not complete descriptor setup"),
        "unexpected guardian refusal: {error}"
    );
    assert!(!sentinel.exists(), "protected child was admitted");
    fs::remove_file(lock_path).expect("guardian lock fixture removes");
}

fn test_spec<I, S>(
    program: &str,
    arguments: I,
    environment: Environment,
    cancellation: Cancellation,
    runtime: Duration,
    stdout: StreamPolicy,
) -> Spec
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString>,
{
    Spec {
        argv: Argv::new(program, arguments).expect("valid argv"),
        cwd: std::env::temp_dir(),
        environment,
        group: ProcessGroup::New,
        deadlines: Deadlines::new(
            runtime,
            Duration::from_millis(50),
            Duration::from_millis(100),
        )
        .expect("valid deadlines"),
        wait: WaitPolicy::new("sill spawn test", cancellation, RestartDisposition::Never)
            .expect("valid wait policy"),
        output: Output::new(
            StdinPolicy::Null,
            stdout,
            StreamPolicy::Capture { limit: 1024 },
        ),
        redaction: Redaction::All,
        grace: Duration::from_millis(10),
    }
}

/// A group answer the kernel refuses to give is not a live member.
///
/// All three answers are injected because none can be staged on demand: an
/// EPERM refusal arriving while the very same group is unreadable is precisely
/// the load-dependent coincidence that made this fault undiagnosable in the
/// field. The live answer is asserted beside the other two so the forgiveness
/// cannot be reached by a rule that simply forgives everything.
#[test]
fn an_unreadable_group_is_forgiven_while_a_live_member_still_faults() {
    let refused = || Some(io::Error::from_raw_os_error(platform::EPERM));

    assert!(
        resolve_signal_against(refused(), ProcessGroup::New, "owner", || {
            process_state::ProcessState::Indeterminate
        })
        .is_ok(),
        "an unreadable probe was read as a live member and failed an ordinary teardown"
    );
    assert!(
        resolve_signal_against(refused(), ProcessGroup::New, "owner", || {
            process_state::ProcessState::Terminal
        })
        .is_ok(),
        "a group observed to have ended stopped being forgiven"
    );

    let fault = resolve_signal_against(refused(), ProcessGroup::New, "owner", || {
        process_state::ProcessState::Live
    })
    .expect_err("a positively live member keeps EPERM a permission fault");
    match fault {
        WaitError::Io {
            owner,
            operation,
            source,
        } => {
            assert_eq!(owner, "owner");
            assert_eq!(operation, "signalling child process group");
            assert_eq!(source.raw_os_error(), Some(platform::EPERM));
        }
        WaitError::ReapDeadlineExceeded { .. } => panic!("a refused signal is not a reap deadline"),
    }
}

/// The reap path holds its wait open for an observed survivor and nothing else.
///
/// The unreadable case is given a deadline far longer than the group poll, so a
/// probe that went back to waiting would have to spend all of it before
/// answering; returning settled is only reachable by deciding on the first
/// answer. The live case keeps the deadline short because it is expected to run
/// the wait out, and it is what proves the settled assertion is not vacuous.
#[test]
fn an_unreadable_group_never_reports_a_descendant_that_survived() {
    let exited = || ProcessState {
        completion: Mutex::new(ProcessCompletion {
            notice: Some(Ok(())),
            ..ProcessCompletion::default()
        }),
        exited: Condvar::new(),
    };
    let probed = Mutex::new(Vec::new());

    let process = exited();
    let settled = wait_for_exit_notice(
        4242,
        ProcessGroup::New,
        &process,
        Duration::from_secs(5),
        |pid| {
            probed.lock().expect("probe log is not poisoned").push(pid);
            process_state::ProcessState::Indeterminate
        },
    )
    .expect("an unreadable group is not an observation failure");
    assert!(
        settled,
        "an unreadable group held the reap open until its deadline expired"
    );
    assert_eq!(
        &*probed.lock().expect("probe log is not poisoned"),
        &[4242],
        "the group probe was aimed at a process other than the child"
    );

    let process = exited();
    let survived = wait_for_exit_notice(
        4242,
        ProcessGroup::New,
        &process,
        Duration::from_millis(30),
        |_| process_state::ProcessState::Live,
    )
    .expect("a live group is not an observation failure");
    assert!(
        !survived,
        "a member the kernel described as running stopped counting as a survivor"
    );
}

/// A number that was never a process identity did not observe a live group.
///
/// The conversion is the whole subject: `group_state` takes a `u32` because
/// that is what the standard library reports a child's identity as, and the
/// kernel call underneath takes a signed `pid_t` half that size. An identity
/// above that half used to leave the probe answering "still present" without
/// having asked the kernel anything, which is a claim its one consumer acts on.
#[test]
fn a_group_identity_that_cannot_be_a_pid_is_unreadable_rather_than_present() {
    let unfit = u32::try_from(libc::pid_t::MAX).expect("pid_t fits a u32") + 1;

    assert_eq!(
        group_state(unfit),
        process_state::ProcessState::Indeterminate,
        "an identity that never reached the kernel was reported as an observation"
    );
}

/// A group the kernel does resolve is still reported present.
///
/// This test's own process group is used because its presence needs no
/// arranging and cannot race: the test is running in it. It is what keeps the
/// unreadable answer above from being reachable by a probe that simply never
/// says `Live`, and it drives the real `kill` rather than the classifier, so
/// the wiring from `group_state` to the kernel is covered too.
#[test]
fn a_group_the_kernel_resolves_is_reported_live() {
    // SAFETY: `getpgrp` has no preconditions and returns this process's group.
    let group = unsafe { libc::getpgrp() };
    let group = u32::try_from(group).expect("this process's group is a positive identity");

    assert_eq!(
        group_state(group),
        process_state::ProcessState::Live,
        "the group this test is running in was not observed to be present"
    );
}

/// Every answer signal zero can give is classified, and only one is absence.
///
/// The errno arms are unreachable through the kernel on demand -- EPERM needs a
/// group this process may not signal, and the residual arm needs `kill` to fail
/// for a reason signal zero cannot produce -- so the classifier takes the errno
/// as a parameter and each arm is driven here. EPERM is asserted `Live` rather
/// than unreadable on purpose: the kernel resolved the group and refused only
/// the delivery, so it observed a member.
#[test]
fn each_group_probe_answer_is_classified_and_only_esrch_is_absence() {
    assert_eq!(
        group_probe_state(0, libc::ESRCH),
        process_state::ProcessState::Live,
        "a delivered probe read a stale errno instead of its own result"
    );
    assert_eq!(
        group_probe_state(-1, libc::ESRCH),
        process_state::ProcessState::Terminal,
        "the kernel saying it knows no such group is an observed absence"
    );
    assert_eq!(
        group_probe_state(-1, libc::EPERM),
        process_state::ProcessState::Live,
        "a group resolved and refused was not read as an observed member"
    );
    assert_eq!(
        group_probe_state(-1, libc::EINVAL),
        process_state::ProcessState::Indeterminate,
        "a refusal that established nothing was folded into presence"
    );
}
