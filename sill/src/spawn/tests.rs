use super::capture::{CaptureWorker, FailureContext, start_capture};
use super::{Completion, spawn, spawn_observed};
use crate::lock::{Acquisition, LockFile, LockName, tests::unique_lock_path};
use crate::observe::{
    Observation, ObservationFailure, ObserveError, ObservedRecord, OutputStream, RecordBoundary,
    RecordObserver,
};
use crate::spec::{
    Argv, Cancellation, CancellationToken, Deadlines, Environment, Output, ProcessGroup, Redaction,
    RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy,
};
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
        Err(io::Error::other("fixture selected reader failure"))
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
    let observer = Arc::new(ForwardObserver { records, failures });
    let observation = Observation::new(OutputStream::Stdout, 32, 1, observer)
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
            if detail.contains("fixture selected reader failure")
    ));
    let outcome = running.wait().expect("reader-failure child reaps");
    assert!(matches!(
        outcome.observation_failure(),
        Some(ObservationFailure::Reader { .. })
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
