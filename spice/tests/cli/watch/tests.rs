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

#[cfg(unix)]
mod unix {
    use super::*;

    use std::panic::AssertUnwindSafe;

    #[test]
    fn natural_exit_after_the_expected_dto_is_refused_with_drained_evidence() {
        let started = Instant::now();
        let deadline = started + Duration::from_secs(2);
        let (process, events) = cleanup::spawn_fixture(
            "printf 'fatal-after-event' >&2; exit 17",
            Duration::from_secs(1),
        )
        .expect("natural-exit fixture launches");
        let pid = process.id();
        await_exit(&events);
        let target = outcome(vec![prose(
            Path::new("/fixture-root/lane-a/codex"),
            Span {
                member: 1,
                offset: 0,
                length: 99,
            },
            CODEX_PROSE_TEXT,
        )]);
        let watch = WatchProcess {
            process: Some(process),
            events,
            status: None,
            exit_origin: Some(ExitOrigin::Natural),
            stderr: Vec::new(),
            observations: vec![target],
            cleanup_errors: Vec::new(),
            phase: "tape event reconciliation",
            started,
            deadline,
        };

        let panic = std::panic::catch_unwind(AssertUnwindSafe(|| watch.finish()))
            .expect_err("a natural post-DTO exit must fail the fixture");
        let diagnostic = panic_text(panic.as_ref());

        assert!(diagnostic.contains("exited naturally before explicit fixture termination"));
        assert!(diagnostic.contains("exit status: 17"));
        assert!(diagnostic.contains("fatal-after-event"));
        assert!(diagnostic.contains(CODEX_PROSE_TEXT));
        assert!(diagnostic.contains("cleanup_errors=[]"));
        assert!(!process_exists(pid), "natural child {pid} was not reaped");
    }

    #[test]
    fn stuck_pipe_cleanup_times_out_reaps_once_and_leaves_no_live_process_or_reader() {
        let script = concat!(
            "trap '' TERM; ",
            "(trap '' TERM; exec /bin/sleep 60) & ",
            "printf '%s\\n' \"$!\"; wait"
        );
        let (process, events) = cleanup::spawn_fixture(script, Duration::from_millis(20))
            .expect("stuck-pipe fixture launches");
        let parent = process.id();
        let descendant = await_pid(&events);
        let started = Instant::now();
        let waited = process
            .wait()
            .expect("deadline kills the group, reaps once, and joins captures");

        assert!(matches!(waited.completion(), sill::Completion::TimedOut(_)));
        assert!(waited.outputs_drained());
        assert!(waited.stdout().is_some_and(sill::CapturedOutput::complete));
        assert!(waited.stderr().is_some_and(sill::CapturedOutput::complete));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!process_exists(parent), "parent {parent} survived cleanup");
        assert!(
            !process_live(descendant),
            "pipe-holding descendant {descendant} remained live after cleanup"
        );
    }

    fn await_exit(events: &mpsc::Receiver<WatchEvent>) {
        loop {
            match events
                .recv_timeout(Duration::from_secs(1))
                .expect("natural child exit is observed")
            {
                WatchEvent::Exited => return,
                WatchEvent::Record(_) => {}
                WatchEvent::Failed(detail) => panic!("observation failed: {detail}"),
            }
        }
    }

    fn await_pid(events: &mpsc::Receiver<WatchEvent>) -> u32 {
        match events
            .recv_timeout(Duration::from_secs(1))
            .expect("descendant pid record is observed")
        {
            WatchEvent::Record(record) => std::str::from_utf8(&record)
                .expect("pid record is UTF-8")
                .trim()
                .parse()
                .expect("pid record is numeric"),
            WatchEvent::Exited => panic!("stuck fixture exited before publishing its pid"),
            WatchEvent::Failed(detail) => panic!("observation failed: {detail}"),
        }
    }

    fn process_exists(pid: u32) -> bool {
        let pid = rustix::process::Pid::from_raw(
            pid.try_into()
                .expect("fixture pid fits the native pid type"),
        )
        .expect("fixture pid is positive");
        match rustix::process::test_kill_process(pid) {
            Ok(()) | Err(rustix::io::Errno::PERM) => true,
            Err(rustix::io::Errno::SRCH) => false,
            Err(error) => panic!("process liveness probe failed: {error}"),
        }
    }

    fn process_live(pid: u32) -> bool {
        #[cfg(target_os = "linux")]
        {
            let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
                Ok(stat) => stat,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
                Err(error) => panic!("process status probe failed: {error}"),
            };
            let (_, fields) = stat.rsplit_once(") ").expect("process stat fields split");
            return !matches!(fields.as_bytes().first(), Some(b'Z' | b'X' | b'x'));
        }
        #[cfg(target_os = "macos")]
        {
            let mut information = std::mem::MaybeUninit::<libc::proc_bsdshortinfo>::uninit();
            let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdshortinfo>())
                .expect("process information size fits c_int");
            // SAFETY: the process information destination is writable for its complete size.
            let bytes = unsafe {
                libc::proc_pidinfo(
                    pid as libc::c_int,
                    libc::PROC_PIDT_SHORTBSDINFO,
                    0,
                    information.as_mut_ptr().cast(),
                    size,
                )
            };
            if bytes == size {
                // SAFETY: an exact-size result initialized the process information.
                return unsafe { information.assume_init() }.pbsi_status != libc::SZOMB;
            }
            let error = std::io::Error::last_os_error();
            if bytes == 0 && error.raw_os_error() == Some(libc::ESRCH) {
                return false;
            }
            panic!("process status probe returned {bytes} bytes: {error}");
        }
    }

    fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
        panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
            .unwrap_or_else(|| "non-text panic".to_owned())
    }
}
