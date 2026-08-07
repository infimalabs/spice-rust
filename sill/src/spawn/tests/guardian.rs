struct ProtectedFixture {
    root: PathBuf,
    topology: LockTopology,
}

impl ProtectedFixture {
    fn new(label: &str) -> Self {
        let root = unique_lock_path(label);
        fs::create_dir(&root).expect("protected fixture root creates");
        let root = fs::canonicalize(root).expect("protected fixture root canonicalizes");
        let config: LockConfig = toml::from_str(concat!(
            "busy = 71\n",
            "taken = 72\n",
            "full = 73\n",
            "[protected]\n",
            "path = \"protected.lock\"\n",
        ))
        .expect("protected fixture configuration is admitted");
        let topology = LockTopology::new(&root, &config);
        Self { root, topology }
    }

    fn acquire(&self) -> HeldShard {
        match self
            .topology
            .try_acquire("protected", None)
            .expect("protected fixture lock is observable")
        {
            LockAttempt::Acquired(held) => held,
            LockAttempt::Contended { .. } => panic!("fresh protected fixture lock contended"),
        }
    }

    fn held_member(&self) -> (PathBuf, fs::File) {
        let path = self.root.join("member.fifo");
        let encoded = CString::new(path.as_os_str().as_bytes()).expect("fixture path has no nul");
        // SAFETY: the path names a new entry inside this fixture's private root.
        assert_eq!(unsafe { libc::mkfifo(encoded.as_ptr(), 0o600) }, 0);
        let hold = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("fixture holds both ends of the member fifo");
        (path, hold)
    }

    fn acquire_before(&self, duration: Duration) -> HeldShard {
        let started = Instant::now();
        loop {
            match self
                .topology
                .try_acquire("protected", None)
                .expect("protected fixture lock remains observable")
            {
                LockAttempt::Acquired(held) => return held,
                LockAttempt::Contended { .. } => {}
            }
            assert!(
                started.elapsed() < duration,
                "lock guardian did not release after its protected group ended"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ProtectedFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn blocked_group_spec(fifo: &Path) -> Spec {
    test_spec(
        "/bin/sh",
        [
            OsString::from("-c"),
            OsString::from("/bin/cat < \"$1\" >/dev/null 2>&1 & child=$!; kill -0 \"$child\""),
            OsString::from("sill-guardian-member"),
            fifo.as_os_str().to_owned(),
        ],
        Environment::empty(),
        Cancellation::Never,
        Duration::from_secs(5),
        StreamPolicy::Null,
    )
}

fn reap_protected_leader(running: &mut super::Running) -> std::process::ExitStatus {
    let started = Instant::now();
    loop {
        if let Some(status) = running
            .take_child_exit()
            .expect("protected fixture leader remains reapable")
        {
            return status;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "protected fixture leader did not exit"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The guardian, rather than this wrapper, holds the lock while a protected
/// group member is still running.
#[test]
fn a_real_guardian_outlives_the_wrapper_while_a_group_member_is_live() {
    let fixture = ProtectedFixture::new("guardian-group-lifetime");
    let mut held = fixture.acquire();
    let (fifo, member_lifetime) = fixture.held_member();
    let running = held
        .spawn_protected(blocked_group_spec(&fifo))
        .expect("protected fixture launches");
    let group = running.id();
    let guardian = running
        .guardian_id()
        .expect("protected fixture retains its guardian");

    let outcome = running.wait().expect("protected fixture leader reaps");
    assert!(matches!(outcome.completion(), Completion::Exited(status) if status.success()));
    assert_eq!(
        group_state(group),
        process_state::ProcessState::Live,
        "the protected member did not outlive its group leader"
    );

    drop(held);
    let observed = Instant::now();
    while observed.elapsed() < Duration::from_millis(500) {
        assert!(
            !process_state::process_terminal(guardian),
            "the lock guardian exited while a protected group member was live"
        );
        assert_eq!(group_state(group), process_state::ProcessState::Live);
        match fixture
            .topology
            .try_acquire("protected", None)
            .expect("guardian-held fixture lock is observable")
        {
            LockAttempt::Contended { .. } => {}
            LockAttempt::Acquired(replacement) => {
                drop(replacement);
                panic!("the guardian released possession while a group member was live");
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    drop(member_lifetime);
    drop(fixture.acquire_before(Duration::from_secs(2)));
}

/// `Running::wait` must not join a guardian on an unreadable group answer.
///
/// The real guardian still watches the live group under its original identity;
/// only `Running` is given an identity too wide for the join probe after its
/// leader has been reaped. That stages `Indeterminate` at the otherwise
/// unreachable join gate without replacing the guardian or the group it holds.
#[test]
fn running_wait_does_not_join_a_guardian_for_an_unreadable_live_group() {
    let fixture = ProtectedFixture::new("guardian-join-gate");
    let mut held = fixture.acquire();
    let (fifo, member_lifetime) = fixture.held_member();
    let mut running = held
        .spawn_protected(blocked_group_spec(&fifo))
        .expect("protected fixture launches");
    let group = running.id();
    assert!(reap_protected_leader(&mut running).success());
    assert_eq!(
        group_state(group),
        process_state::ProcessState::Live,
        "the protected member did not outlive its group leader"
    );
    running.pid = u32::try_from(libc::pid_t::MAX).expect("pid_t fits u32") + 1;

    let (finished, returned) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let _sent = finished.send(running.wait());
    });
    let prompt = returned.recv_timeout(Duration::from_millis(500));
    drop(member_lifetime);
    let (waited, joined_live_guardian) = match prompt {
        Ok(waited) => (waited, false),
        Err(mpsc::RecvTimeoutError::Timeout) => (
            returned
                .recv_timeout(Duration::from_secs(2))
                .expect("guardian join returns after the protected member exits"),
            true,
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("protected wait thread disconnected without an answer")
        }
    };
    waiter.join().expect("protected wait thread does not panic");
    let outcome = waited.expect("protected fixture wait succeeds");
    assert!(matches!(outcome.completion(), Completion::Exited(status) if status.success()));

    drop(held);
    drop(fixture.acquire_before(Duration::from_secs(2)));
    assert!(
        !joined_live_guardian,
        "Running::wait joined a guardian on an unreadable pre-absence answer"
    );
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

/// A transiently unreadable pre-kill probe gets one chance to observe the end.
///
/// The initial unreadable answer is the old boolean collapse point. The second
/// answer makes an already-ended group observable, so returning false on the
/// first answer would unnecessarily enter both termination waits.
#[test]
fn an_unreadable_pre_kill_probe_rechecks_an_already_ended_group() {
    let mut answers = [
        process_state::ProcessState::Indeterminate,
        process_state::ProcessState::Terminal,
    ]
    .into_iter();

    assert!(
        execution_group_terminal_against(true, || {
            answers.next().expect("the bounded probe has an answer")
        }),
        "the first unreadable answer skipped the re-probe and entered both termination waits"
    );
    assert_eq!(
        answers.next(),
        None,
        "the pre-kill decision exceeded its one-re-probe bound"
    );
}

/// Re-probing unreadability never turns silence or an observed survivor into an
/// end that would skip termination.
#[test]
fn an_unreadable_pre_kill_probe_never_settles_a_group_not_observed_terminal() {
    for second in [
        process_state::ProcessState::Live,
        process_state::ProcessState::Indeterminate,
    ] {
        let mut answers = [process_state::ProcessState::Indeterminate, second].into_iter();
        assert!(
            !execution_group_terminal_against(true, || {
                answers.next().expect("the bounded probe has an answer")
            }),
            "an unreadable group skipped termination without an observed end"
        );
        assert_eq!(
            answers.next(),
            None,
            "the pre-kill decision exceeded its one-re-probe bound"
        );
    }
}

/// A child that never led a group of its own settles without asking the kernel.
///
/// `requires_group_terminal` is false for an inherited group and for a new group
/// whose guardian still holds it, and the decision must answer true there
/// without observing anything. The observer panics rather than returning an
/// answer because both halves of that sentence need pinning at once: a decision
/// that returned the wrong value fails on the assertion, and one that reached
/// the probe at all fails on the panic, which no returned answer could
/// distinguish. The kernel is worth not asking here because there is no group of
/// the child's own to name -- probing an inherited child's negated identity asks
/// about a group it never led, and the answer settles nothing the caller did not
/// already know.
#[test]
fn a_child_that_never_led_a_group_settles_without_probing() {
    assert!(
        execution_group_terminal_against(false, || panic!(
            "the pre-kill decision probed a group the child never led"
        )),
        "a child that never led a group of its own was not settled before any probe"
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
