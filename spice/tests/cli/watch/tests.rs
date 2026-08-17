use super::*;

fn outcome(observations: Vec<Observation>) -> StreamOutcome {
    StreamOutcome {
        mode: Mode::Watch,
        root: PathBuf::from("/fixture-root"),
        observations,
    }
}

fn prose(tape: &Path, span: Span, text: &str) -> Observation {
    Observation::Event {
        lane: "lane-a".to_owned(),
        dialect: Dialect::Codex,
        tape: tape.to_path_buf(),
        spans: vec![span],
        event: SurfaceEvent::Prose {
            text: text.to_owned(),
            phase: None,
            media: Vec::new(),
        },
    }
}

#[test]
fn reconciliation_requires_the_exact_post_tee_identity_and_payload() {
    let expected_tape = PathBuf::from("/fixture-root/lane-a/codex");
    let expected_span = Span {
        member: 4,
        offset: 91,
        length: 17,
    };
    let expected = ExpectedTapeEvent {
        tape: expected_tape.clone(),
        span: expected_span,
    };
    let unrelated = prose(
        Path::new("/fixture-root/lane-b/codex"),
        expected_span,
        CODEX_PROSE_TEXT,
    );
    let wrong_span = prose(
        &expected_tape,
        Span {
            member: 4,
            offset: 90,
            length: 17,
        },
        CODEX_PROSE_TEXT,
    );
    let wrong_payload = prose(&expected_tape, expected_span, "not the durable target");
    let exact = prose(&expected_tape, expected_span, CODEX_PROSE_TEXT);

    let canary = Observation::Canary {
        lane: "lane-a".to_owned(),
        dialect: Dialect::Codex,
        tape: expected_tape,
        expected: EventKind::Session,
    };

    assert!(!expected.appears_in(&outcome(vec![unrelated])));
    assert!(!expected.appears_in(&outcome(vec![wrong_span])));
    assert!(!expected.appears_in(&outcome(vec![wrong_payload])));
    assert!(expected.appears_in(&outcome(vec![canary, exact])));
}

#[test]
fn cleanup_timeout_evidence_names_the_original_single_bound_and_pending_work() {
    let detail = cleanup::cleanup_deadline_detail();

    assert!(detail.contains("original 30s test-wide deadline"));
    assert!(detail.contains("kill/reap"));
    assert!(detail.contains("stdout/stderr drain completion"));
}

#[cfg(unix)]
mod unix {
    use super::*;

    use std::io;
    use std::os::unix::process::ExitStatusExt as _;
    use std::panic::AssertUnwindSafe;
    use std::process::ExitStatus;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::{Receiver, Sender};
    use std::thread::JoinHandle;

    use cleanup::{CleanupJob, CleanupWorker};

    #[derive(Default)]
    struct Calls {
        probes: AtomicUsize,
        kills: AtomicUsize,
        reaps: AtomicUsize,
    }

    enum Probe {
        Natural(Option<ExitStatus>),
        Running,
    }

    struct FakeChild {
        calls: Arc<Calls>,
        live: Arc<AtomicBool>,
        probe: Probe,
        status: ExitStatus,
        reaped: Option<Sender<()>>,
    }

    impl FakeChild {
        fn natural(status: ExitStatus, calls: Arc<Calls>, live: Arc<AtomicBool>) -> Self {
            Self {
                calls,
                live,
                probe: Probe::Natural(Some(status)),
                status,
                reaped: None,
            }
        }

        fn running(
            status: ExitStatus,
            calls: Arc<Calls>,
            live: Arc<AtomicBool>,
            reaped: Sender<()>,
        ) -> Self {
            Self {
                calls,
                live,
                probe: Probe::Running,
                status,
                reaped: Some(reaped),
            }
        }
    }

    impl ChildControl for FakeChild {
        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            self.calls.probes.fetch_add(1, Ordering::SeqCst);
            match &mut self.probe {
                Probe::Natural(status) => Ok(status.take()),
                Probe::Running => Ok(None),
            }
        }

        fn kill(&mut self) -> io::Result<()> {
            self.calls.kills.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn wait(&mut self) -> io::Result<ExitStatus> {
            self.calls.reaps.fetch_add(1, Ordering::SeqCst);
            if let Some(reaped) = self.reaped.take() {
                let _ = reaped.send(());
            }
            Ok(self.status)
        }
    }

    impl Drop for FakeChild {
        fn drop(&mut self) {
            self.live.store(false, Ordering::SeqCst);
        }
    }

    struct ReaderLife(Arc<AtomicBool>);

    impl Drop for ReaderLife {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    fn stdout_reader(live: Arc<AtomicBool>) -> JoinHandle<Result<(), String>> {
        std::thread::spawn(move || {
            let _life = ReaderLife(live);
            Ok(())
        })
    }

    fn stderr_reader(
        live: Arc<AtomicBool>,
        release: Option<Receiver<()>>,
        bytes: &'static [u8],
    ) -> JoinHandle<Result<Vec<u8>, String>> {
        std::thread::spawn(move || {
            let _life = ReaderLife(live);
            if let Some(release) = release {
                release
                    .recv()
                    .map_err(|error| format!("release disconnected: {error}"))?;
            }
            Ok(bytes.to_vec())
        })
    }

    #[test]
    fn natural_exit_after_the_expected_dto_is_refused_with_drained_evidence() {
        let status = ExitStatus::from_raw(17 << 8);
        let calls = Arc::new(Calls::default());
        let child_live = Arc::new(AtomicBool::new(true));
        let stdout_live = Arc::new(AtomicBool::new(true));
        let stderr_live = Arc::new(AtomicBool::new(true));
        let (sender, receiver) = mpsc::channel();
        drop(sender);
        let target = outcome(vec![prose(
            Path::new("/fixture-root/lane-a/codex"),
            Span {
                member: 1,
                offset: 0,
                length: 99,
            },
            CODEX_PROSE_TEXT,
        )]);
        let started = Instant::now();
        let watch = WatchProcess {
            child: Some(Box::new(FakeChild::natural(
                status,
                Arc::clone(&calls),
                Arc::clone(&child_live),
            ))),
            stdout: Some(receiver),
            stdout_reader: Some(stdout_reader(Arc::clone(&stdout_live))),
            stderr_reader: Some(stderr_reader(
                Arc::clone(&stderr_live),
                None,
                b"fatal-after-event",
            )),
            status: None,
            exit_origin: None,
            stderr: Vec::new(),
            observations: vec![target],
            cleanup_errors: Vec::new(),
            phase: "tape event reconciliation",
            started,
            deadline: started + Duration::from_secs(2),
        };

        let panic = std::panic::catch_unwind(AssertUnwindSafe(|| watch.finish()))
            .expect_err("a natural post-DTO exit must fail the fixture");
        let diagnostic = panic_text(panic.as_ref());

        assert!(diagnostic.contains("exited naturally before explicit fixture termination"));
        assert!(diagnostic.contains("exit status: 17"));
        assert!(diagnostic.contains("fatal-after-event"));
        assert!(diagnostic.contains(CODEX_PROSE_TEXT));
        assert_eq!(calls.probes.load(Ordering::SeqCst), 1);
        assert_eq!(calls.kills.load(Ordering::SeqCst), 0);
        assert_eq!(calls.reaps.load(Ordering::SeqCst), 0);
        assert!(!child_live.load(Ordering::SeqCst));
        assert!(!stdout_live.load(Ordering::SeqCst));
        assert!(!stderr_live.load(Ordering::SeqCst));
    }

    #[test]
    fn stuck_pipe_cleanup_times_out_then_finishes_without_a_second_reap_or_leak() {
        let status = ExitStatus::from_raw(9);
        let calls = Arc::new(Calls::default());
        let child_live = Arc::new(AtomicBool::new(true));
        let stdout_live = Arc::new(AtomicBool::new(true));
        let stderr_live = Arc::new(AtomicBool::new(true));
        let (reaped_sender, reaped) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let (stdout_sender, stdout) = mpsc::channel();
        drop(stdout_sender);
        let job = CleanupJob::new(
            Some(Box::new(FakeChild::running(
                status,
                Arc::clone(&calls),
                Arc::clone(&child_live),
                reaped_sender,
            ))),
            Some(stdout),
            Some(stdout_reader(Arc::clone(&stdout_live))),
            Some(stderr_reader(
                Arc::clone(&stderr_live),
                Some(blocked),
                b"drained-after-release",
            )),
            None,
            None,
        );
        let worker = CleanupWorker::spawn(job);
        reaped
            .recv_timeout(Duration::from_secs(1))
            .expect("the worker reaps before the pipe-drain stall");
        let observed = Instant::now();
        let Err(worker) = worker.wait_until(observed + Duration::from_millis(20)) else {
            panic!("the blocked stderr drain must consume the cleanup deadline");
        };

        assert!(observed.elapsed() < Duration::from_secs(1));
        assert_eq!(calls.probes.load(Ordering::SeqCst), 1);
        assert_eq!(calls.kills.load(Ordering::SeqCst), 1);
        assert_eq!(calls.reaps.load(Ordering::SeqCst), 1);
        release.send(()).expect("release the blocked stderr drain");
        let Ok(report) = worker.wait_until(Instant::now() + Duration::from_secs(1)) else {
            panic!("released cleanup must publish completion");
        };
        assert!(report.is_ok());
        assert_eq!(calls.reaps.load(Ordering::SeqCst), 1);
        assert!(!child_live.load(Ordering::SeqCst));
        assert!(!stdout_live.load(Ordering::SeqCst));
        assert!(!stderr_live.load(Ordering::SeqCst));
    }

    fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
        panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
            .unwrap_or_else(|| "non-text panic".to_owned())
    }
}
