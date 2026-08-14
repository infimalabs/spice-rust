use super::*;

#[test]
fn watch_follows_a_real_filesystem_notification_then_reconciles_tape() {
    let scratch = Scratch::new("watch-notification");
    let (_, tape) = session(scratch.path());
    let root = scratch.path().to_string_lossy().into_owned();
    let mut child = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args(["--json", "watch", &root])
        .env_remove(steer::hook::INBOX)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("watch launches");
    let stdout = child.stdout.take().expect("stdout is piped");
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line.expect("watch speaks UTF-8")).is_err() {
                break;
            }
        }
    });

    let first = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("armed watch emits its initial cursor state");
    let initial: StreamOutcome = wire::decode(first.as_bytes()).expect("initial DTO decodes");
    assert!(matches!(
        initial.observations.as_slice(),
        [Observation::Absent { .. }]
    ));

    let ready = await_watch_readiness(scratch.path(), &receiver);
    assert_eq!(ready.mode, Mode::Watch);

    tape.tee(&record(CODEX_PROSE))
        .expect("a record arrives after the watcher is armed");
    let observed = loop {
        let line = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("filesystem activity wakes the blocking watcher");
        let outcome: StreamOutcome = wire::decode(line.as_bytes()).expect("wake DTO decodes");
        if outcome
            .observations
            .iter()
            .any(|observation| matches!(observation, Observation::Event { .. }))
        {
            break outcome;
        }
    };
    assert_eq!(observed.mode, Mode::Watch);

    child.kill().expect("fixture follower stops");
    let status = child.wait().expect("fixture follower is reaped");
    assert!(
        !status.success(),
        "the endless follower was explicitly stopped"
    );
    drop(receiver);
    reader.join().expect("stdout reader joins");
    scratch.remove();
}

fn await_watch_readiness(root: &Path, receiver: &mpsc::Receiver<String>) -> StreamOutcome {
    let readiness = root.join("watch-readiness.probe");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut attempt = 0_u64;
    let line = loop {
        attempt += 1;
        fs::write(&readiness, attempt.to_string()).expect("mutate watcher readiness probe");
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "watcher never acknowledged readiness");
        match receiver.recv_timeout(remaining.min(Duration::from_secs(1))) {
            Ok(line) => break line,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("watcher exited before acknowledging readiness")
            }
        }
    };
    let outcome: StreamOutcome = wire::decode(line.as_bytes()).expect("readiness DTO decodes");
    assert!(
        outcome
            .observations
            .iter()
            .all(|observation| !matches!(observation, Observation::Event { .. })),
        "readiness probe manufactured a tape event: {outcome:?}"
    );
    outcome
}
