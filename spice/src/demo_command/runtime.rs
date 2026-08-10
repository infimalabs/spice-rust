fn seed_journal(state: &spice::repository::StatePaths, root: &Path) -> DemoResult<()> {
    let mut journal = journal::Journal::open(state.journal())
        .map_err(|error| environment_fault("demo::journal-unavailable", error.to_string()))?;
    crate::resident::admit_steering_index(&mut journal)
        .map_err(|outcome| environment_fault("demo::journal-invalid", format!("{outcome:?}")))?;
    let task: wire::TaskHandle = DEMO_TASK.parse().expect("demo task handle is valid");
    let previous = seed_task(&mut journal, &task)?;
    seed_turns(&mut journal, &task, previous)?;
    seed_lanes(&mut journal, root)?;
    journal
        .verify()
        .map_err(|error| environment_fault("demo::journal-invalid", error.to_string()))?;
    Ok(())
}

fn seed_task(journal: &mut journal::Journal, task: &wire::TaskHandle) -> DemoResult<wire::Seq> {
    let born = task.born().expect("demo task carries a canonical stamp");
    let phase: wire::Phase = "todo".parse().expect("demo phase is valid");
    let origin: wire::Origin = format!("task:{DEMO_STAMP}")
        .parse()
        .expect("demo task origin is valid");
    let mut write = JournalWrite::new(
        Actor::Spice,
        origin.clone(),
        instant(CANNED_TRANSCRIPT[0].0),
        Producer::CommandDoor,
    );
    let added = write
        .fact(
            body::TaskAdded {
                task: born.clone(),
                title: DEMO_TITLE.to_owned(),
                description: None,
                acceptance: Vec::new(),
                project: DEMO_PROJECT.to_owned(),
                tags: vec!["demo".to_owned()],
                depends: Vec::new(),
                flow: vec![phase.clone()],
                phase: phase.clone(),
                revision: 1,
            },
            FactCause::Root,
        )
        .map_err(build_failure)?;
    let statement_origin = origin;
    let statement_task = born.clone();
    let workflow_task = born.clone();
    let workflow_phase = phase.clone();
    write.task_head(added, move |revision| TaskHead {
        born: statement_task,
        queue_origin: revision,
        title: DEMO_TITLE.to_owned(),
        description: None,
        acceptance: Vec::new(),
        project: DEMO_PROJECT.to_owned(),
        tags: vec!["demo".to_owned()],
        dependencies: Vec::new(),
        provenance: statement_origin,
        revision,
    });
    write.workflow_head(workflow_task, added, move |_| WorkflowHead {
        flow: vec![workflow_phase.clone()],
        phase: workflow_phase,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    });
    Ok(journal.apply(write).map_err(commit_failure)?.last())
}

fn seed_turns(
    journal: &mut journal::Journal,
    task: &wire::TaskHandle,
    mut previous: wire::Seq,
) -> DemoResult<()> {
    for (timestamp, text) in CANNED_TRANSCRIPT {
        let mut write = JournalWrite::new(
            Actor::Spice,
            format!("task:{DEMO_STAMP}")
                .parse()
                .expect("demo task origin is valid"),
            instant(timestamp),
            Producer::CommandDoor,
        );
        write
            .fact(
                body::TaskNoted {
                    task: task.born().expect("demo task carries a canonical stamp"),
                    note: text.to_owned(),
                },
                FactCause::Existing(previous),
            )
            .map_err(build_failure)?;
        previous = journal.apply(write).map_err(commit_failure)?.last();
    }
    Ok(())
}

fn seed_lanes(journal: &mut journal::Journal, root: &Path) -> DemoResult<()> {
    let epoch = uuid(DEMO_EPOCH);
    let lineage_id = uuid(DEMO_LANE);
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("resident:{epoch}")
            .parse()
            .expect("demo resident origin is valid"),
        instant(CANNED_TRANSCRIPT[3].0),
        Producer::ResidentRecognition,
    );
    let mut cause = FactCause::Root;
    let mut seated = None;
    for seed in DEMO_LANES {
        let fact = write
            .fact(
                body::LaneSeated {
                    lane: uuid(seed.lane),
                    lineage_id,
                    capability: uuid(seed.capability),
                    name: Some(seed.name.to_owned()),
                    worktree: root.display().to_string(),
                    life: "drive".to_owned(),
                    vendor: crate::resident::SIMULATED_VENDOR
                        .parse()
                        .expect("simulated demo vendor is valid"),
                },
                cause,
            )
            .map_err(build_failure)?;
        cause = FactCause::Pending(fact);
        seated = Some(fact);
    }
    let worktree = root.display().to_string();
    write.residency_head(seated.expect("the demo always seeds lanes"), move |_| {
        ResidencyHead {
            epoch,
            state: ResidentState::Dark,
            topology_revision: 0,
            lanes: DEMO_LANES
                .into_iter()
                .map(|seed| wire::LaneResidency {
                    lane: uuid(seed.lane),
                    lineage_id,
                    name: Some(seed.name.to_owned()),
                    worktree: worktree.clone(),
                    life: "drive".to_owned(),
                    vendor: crate::resident::SIMULATED_VENDOR
                        .parse()
                        .expect("simulated demo vendor is valid"),
                    occupant: Some(uuid(seed.occupant)),
                    lineage: vec![uuid(seed.occupant)],
                    capabilities: vec![uuid(seed.capability)],
                    session: None,
                    disposition: crate::resident::SIMULATED_DISPOSITION.to_owned(),
                })
                .collect(),
            subscriptions: vec![wire::LineageSubscription {
                lineage_id,
                policy: wire::intent::LineageLife::Drive,
                manual_projects: Vec::new(),
                auto_projects: Vec::new(),
            }],
        }
    });
    journal.apply(write).map_err(commit_failure)?;
    Ok(())
}

fn validate_seeded_journal(state: &spice::repository::StatePaths) -> DemoResult<()> {
    let reader = journal::JournalReader::open(state.journal())
        .map_err(|error| environment_fault("demo::journal-unavailable", error.to_string()))?;
    reader
        .verify()
        .map_err(|error| environment_fault("demo::journal-invalid", error.to_string()))?;
    let snapshot = reader
        .fact_snapshot()
        .map_err(|error| environment_fault("demo::journal-invalid", error.to_string()))?;
    let task = snapshot.facts().iter().any(|envelope| {
        envelope.fact.kind() == wire::FactKind::TaskAdded
            && envelope
                .fact
                .body()
                .get("task")
                .and_then(serde_json::Value::as_str)
                == Some(DEMO_STAMP)
    });
    let lanes = DEMO_LANES.iter().all(|seed| {
        snapshot.facts().iter().any(|envelope| {
            envelope.fact.kind() == wire::FactKind::LaneSeated
                && envelope
                    .fact
                    .body()
                    .get("lane")
                    .and_then(serde_json::Value::as_str)
                    == Some(seed.lane)
        })
    });
    let residency = reader
        .read(|read| read.residency_head())
        .map_err(|error| environment_fault("demo::journal-invalid", error.to_string()))?;
    let simulated = residency.is_some_and(|head| {
        head.body().lanes.len() == DEMO_LANES.len()
            && DEMO_LANES.iter().all(|seed| {
                head.body().lanes.iter().any(|lane| {
                    lane.lane == uuid(seed.lane)
                        && lane.lineage_id == uuid(DEMO_LANE)
                        && lane.capabilities == [uuid(seed.capability)]
                        && lane.occupant == Some(uuid(seed.occupant))
                        && lane.lineage == [uuid(seed.occupant)]
                        && lane.vendor.as_str() == crate::resident::SIMULATED_VENDOR
                        && lane.disposition == crate::resident::SIMULATED_DISPOSITION
                        && lane.session.is_none()
                })
            })
    });
    let notes = snapshot
        .facts()
        .iter()
        .filter(|envelope| envelope.fact.kind() == wire::FactKind::TaskNoted)
        .count();
    if !task || !lanes || !simulated || notes < CANNED_TRANSCRIPT.len() {
        return Err(refusal(
            "demo::seed-moved",
            Some("the seeded task, two simulated lanes, and four canned turns"),
            Some(&format!(
                "task={task}, lanes={lanes}, simulated={simulated}, turns={notes}"
            )),
            "restore the seeded journal or remove this disposable root",
        )
        .into());
    }
    Ok(())
}

fn transcript_bytes() -> DemoResult<Vec<u8>> {
    let mut bytes = Vec::new();
    for (timestamp, text) in CANNED_TRANSCRIPT {
        bytes.extend(
            wire::encode(&DemoTurn {
                timestamp,
                role: "assistant",
                text,
            })
            .map_err(|error| protocol_fault("demo::transcript-invalid", error.to_string()))?,
        );
        bytes.push(b'\n');
    }
    Ok(bytes)
}

fn expected_head() -> String {
    format!("{SEED_BRANCH} at {SEED_COMMIT}")
}

fn expected_manifest() -> DemoManifest {
    DemoManifest {
        schema: 1,
        task: DEMO_TASK.to_owned(),
        lane: uuid(DEMO_LANE),
        transcript: TRANSCRIPT_NAME.to_owned(),
    }
}

fn environment(
    root: PathBuf,
    transcript: PathBuf,
    state: spice::repository::StatePaths,
    executable: &Path,
    automatic: bool,
) -> DemoResult<Environment> {
    let root_text = require_utf8(&root, "demo root")?;
    let executable = require_utf8(executable, "current spice executable")?;
    let quoted_root = shell_quote(root_text);
    let quoted_executable = shell_quote(executable);
    Ok(Environment {
        launch_command: format!(
            "cd {quoted_root} && {quoted_executable} demo --reuse {quoted_root}"
        ),
        remove_command: format!("rm -rf -- {quoted_root}"),
        root,
        transcript,
        state,
        automatic,
    })
}

fn canvas_environment_with<F>(
    environment: Environment,
    cleanup: bool,
    signals: bool,
    open: bool,
    interrupt_after_launch: bool,
    browser: F,
) -> Outcome<DemoAnswer>
where
    F: FnOnce(&str) -> Result<(), String>,
{
    let (events, receiver) = mpsc::channel();
    let _watcher = match watch_environment(&environment, events.clone()) {
        Ok(watcher) => watcher,
        Err(failure) => return preserve_environment_failure(&environment, *failure),
    };
    let relay = match signal_relay(signals, events.clone()) {
        Ok(relay) => relay,
        Err(failure) => return preserve_environment_failure(&environment, *failure),
    };
    let resident = match start_resident(&environment, events.clone()) {
        Ok(resident) => resident,
        Err(failure) => return preserve_environment_failure(&environment, *failure),
    };

    let ready = wait_for_discovery(&environment, &receiver);
    let interrupted_before_launch = match ready {
        Ok(false) => false,
        Ok(true) => true,
        Err(failure) => {
            drop(relay);
            drop(resident);
            return preserve_environment_failure(&environment, *failure);
        }
    };
    if interrupted_before_launch {
        let outcome = stop_resident(&environment, &receiver);
        drop(relay);
        return match outcome {
            Ok(()) => {
                let _ = resident.join();
                reply(&environment, "interrupted", "preserved", None)
            }
            Err(failure) => preserve_environment_failure(&environment, *failure),
        };
    }

    // The endpoint is admitted on every path; only the browser is optional.
    // Resolving it here rather than inside the launcher is what lets a plain
    // run still print an endpoint an operator or agent can open deliberately,
    // and it keeps demo on the one admission path `up` already uses.
    let opened = crate::command_client::open_canvas_with(
        environment.state.canvas(),
        || {},
        |endpoint| if open { browser(endpoint) } else { Ok(()) },
    );
    let endpoint = match opened {
        Outcome::Reply(answer) => answer.body.endpoint,
        Outcome::Refusal(refusal) => {
            let _ = stop_resident(&environment, &receiver);
            drop(relay);
            let _ = resident.join();
            return preserve_environment_failure(&environment, Outcome::Refusal(refusal));
        }
        Outcome::Fault(fault) => {
            let _ = stop_resident(&environment, &receiver);
            drop(relay);
            let _ = resident.join();
            return preserve_environment_failure(&environment, Outcome::Fault(fault));
        }
    };
    if interrupt_after_launch {
        let _ = events.send(Event::Signal(signal_hook::consts::signal::SIGINT));
    }
    let completion = wait_for_resident(&environment, &receiver);
    drop(relay);
    let _ = resident.join();
    finish_canvas(&environment, cleanup, endpoint, completion)
}

fn watch_environment(
    environment: &Environment,
    events: Sender<Event>,
) -> DemoResult<notify::PollWatcher> {
    let journal = environment.state.journal().to_path_buf();
    let mut watcher = notify::PollWatcher::new(
        move |event: notify::Result<notify::Event>| {
            let observed = readiness_change(event, &journal);
            let _ = events.send(Event::Changed(observed));
        },
        notify::Config::default().with_poll_interval(WATCH_SCAN_INTERVAL),
    )
    .map_err(|error| {
        environment_fault(
            "demo::readiness-watcher-unavailable",
            format!("the canvas readiness watcher could not be created: {error}"),
        )
    })?;
    watcher
        .watch(&environment.root, RecursiveMode::Recursive)
        .map_err(|error| {
            environment_fault(
                "demo::readiness-watch-refused",
                format!(
                    "{} could not be watched for canvas discovery: {error}",
                    environment.root.display()
                ),
            )
        })?;
    Ok(watcher)
}

fn readiness_change(event: notify::Result<notify::Event>, journal: &Path) -> Result<(), String> {
    match event {
        Ok(_) => Ok(()),
        Err(error) if is_missing_readiness_transient(&error, journal) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn is_missing_readiness_transient(error: &notify::Error, journal: &Path) -> bool {
    let notify::ErrorKind::Io(source) = &error.kind else {
        return false;
    };
    // PollWatcher enumerates each path before reading its metadata. SQLite may
    // remove a transient sidecar and atomic publication may rename its staging
    // file between those steps. No other scan error should disappear into a
    // generic change notification.
    source.kind() == io::ErrorKind::NotFound
        && !error.paths.is_empty()
        && error.paths.iter().all(|path| {
            (is_known_journal_sidecar(path, journal) || is_atomic_temporary(path))
                && matches!(
                    fs::symlink_metadata(path),
                    Err(source) if source.kind() == io::ErrorKind::NotFound
                )
        })
}

fn is_known_journal_sidecar(path: &Path, journal: &Path) -> bool {
    ["-journal", "-wal", "-shm"].into_iter().any(|suffix| {
        let mut sidecar = journal.as_os_str().to_owned();
        sidecar.push(suffix);
        path.as_os_str() == sidecar
    })
}

fn is_atomic_temporary(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(body) = name
        .strip_prefix(".spice-atomic-")
        .and_then(|name| name.strip_suffix(".tmp"))
    else {
        return false;
    };
    let mut fields = body.split('-');
    matches!(
        (fields.next(), fields.next(), fields.next()),
        (Some(process), Some(sequence), None)
            if [process, sequence].into_iter().all(|field| {
                !field.is_empty() && field.bytes().all(|byte| byte.is_ascii_digit())
            })
    )
}

fn signal_relay(signals: bool, events: Sender<Event>) -> DemoResult<Option<SignalRelay>> {
    if signals {
        SignalRelay::install(events).map(Some).map_err(|error| {
            environment_fault("demo::signals-unavailable", error.to_string()).into()
        })
    } else {
        Ok(None)
    }
}

fn start_resident(environment: &Environment, events: Sender<Event>) -> DemoResult<JoinHandle<()>> {
    let root = environment.root.clone();
    std::thread::Builder::new()
        .name("spice-demo-resident".to_owned())
        .spawn(move || {
            let outcome = crate::resident::up_from(
                &root,
                crate::resident::CanvasBinding::local(crate::resident::EPHEMERAL_LOOPBACK),
            );
            let _ = events.send(Event::Resident(outcome));
        })
        .map_err(|error| {
            environment_fault("demo::resident-launch-failed", error.to_string()).into()
        })
}

fn finish_canvas(
    environment: &Environment,
    cleanup: bool,
    endpoint: String,
    completion: DemoResult<bool>,
) -> Outcome<DemoAnswer> {
    match completion {
        Ok(true) => reply(
            environment,
            "canvas",
            "interrupted-preserved",
            Some(endpoint),
        ),
        Ok(false) if cleanup => match fs::remove_dir_all(&environment.root) {
            Ok(()) => reply(environment, "canvas", "removed", Some(endpoint)),
            Err(error) => preserve_environment_failure(
                environment,
                environment_fault(
                    "demo::cleanup-failed",
                    format!("{}: {error}", environment.root.display()),
                ),
            ),
        },
        Ok(false) => reply(environment, "canvas", "preserved", Some(endpoint)),
        Err(failure) => preserve_environment_failure(environment, *failure),
    }
}

fn wait_for_discovery(environment: &Environment, receiver: &Receiver<Event>) -> DemoResult<bool> {
    let deadline = Monotonic::now() + READY_DEADLINE;
    loop {
        if environment.state.canvas().is_file() {
            return Ok(false);
        }
        let remaining = deadline.saturating_duration_since(Monotonic::now());
        match receiver.recv_timeout(remaining) {
            Ok(Event::Changed(Ok(()))) => {}
            Ok(Event::Changed(Err(error))) => {
                return Err(environment_fault(
                    "demo::readiness-scan-failed",
                    format!(
                        "{} could not be scanned for canvas discovery: {error}",
                        environment.root.display()
                    ),
                )
                .into());
            }
            Ok(Event::Resident(outcome)) => return Err(rebody(outcome).into()),
            Ok(Event::Signal(_)) => return Ok(true),
            Err(RecvTimeoutError::Timeout) => {
                return Err(environment_fault(
                    "demo::resident-readiness-timeout",
                    format!("canvas discovery was not published within {READY_DEADLINE:?}"),
                )
                .into());
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(environment_fault(
                    "demo::resident-events-closed",
                    "resident readiness channel disconnected",
                )
                .into());
            }
        }
    }
}

fn wait_for_resident(environment: &Environment, receiver: &Receiver<Event>) -> DemoResult<bool> {
    loop {
        match receiver.recv() {
            Ok(Event::Changed(_)) => {}
            Ok(Event::Resident(Outcome::Reply(_))) => return Ok(false),
            Ok(Event::Resident(outcome)) => return Err(rebody(outcome).into()),
            Ok(Event::Signal(_signal)) => {
                stop_resident(environment, receiver)?;
                return Ok(true);
            }
            Err(_) => {
                return Err(environment_fault(
                    "demo::resident-events-closed",
                    "resident lifecycle channel disconnected",
                )
                .into());
            }
        }
    }
}

fn stop_resident(environment: &Environment, receiver: &Receiver<Event>) -> DemoResult<()> {
    let contract = crate::cli::contract(&["down"]).expect("down contract is registered");
    match crate::resident::down_from(&environment.root, contract) {
        Outcome::Reply(_) => {}
        outcome => return Err(rebody(outcome).into()),
    }
    let deadline = Monotonic::now() + STOP_DEADLINE;
    loop {
        let remaining = deadline.saturating_duration_since(Monotonic::now());
        match receiver.recv_timeout(remaining) {
            Ok(Event::Changed(_)) | Ok(Event::Signal(_)) => {}
            Ok(Event::Resident(Outcome::Reply(_))) => return Ok(()),
            Ok(Event::Resident(outcome)) => return Err(rebody(outcome).into()),
            Err(RecvTimeoutError::Timeout) => {
                return Err(environment_fault(
                    "demo::resident-stop-timeout",
                    format!("resident did not stop within {STOP_DEADLINE:?}"),
                )
                .into());
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(environment_fault(
                    "demo::resident-events-closed",
                    "resident lifecycle channel disconnected",
                )
                .into());
            }
        }
    }
}

fn reply(
    environment: &Environment,
    mode: &str,
    lifecycle: &str,
    endpoint: Option<String>,
) -> Outcome<DemoAnswer> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body: DemoAnswer {
            root: environment.root.display().to_string(),
            transcript: environment.transcript.display().to_string(),
            mode: mode.to_owned(),
            lifecycle: lifecycle.to_owned(),
            endpoint,
            launch_command: environment.launch_command.clone(),
            remove_command: environment.remove_command.clone(),
        },
    })
}

fn uuid(value: &str) -> CanonicalUuid {
    value.parse().expect("demo UUID constants are canonical")
}

fn instant(value: &str) -> wire::Instant {
    wire::Instant::new(value.parse().expect("demo timestamp constants parse"))
        .expect("demo timestamps fit the native instant range")
}

fn build_failure(error: journal::BuildError) -> Outcome<DemoAnswer> {
    protocol_fault("demo::journal-proposal-invalid", error.to_string())
}

fn commit_failure(error: journal::CommitError) -> Outcome<DemoAnswer> {
    environment_fault("demo::journal-write-failed", error.to_string())
}

fn rebody(outcome: Outcome<()>) -> Outcome<DemoAnswer> {
    match outcome {
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
        Outcome::Reply(_) => Outcome::Fault(Fault {
            class: FaultClass::Protocol,
            reason: "demo::resident-ended-before-ready".to_owned(),
            detail: "the demo resident ended before its lifecycle wait completed".to_owned(),
        }),
    }
}

fn refusal(
    reason: &str,
    expected: Option<&str>,
    observed: Option<&str>,
    detail: &str,
) -> Outcome<DemoAnswer> {
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            reason.to_owned(),
            Some("demo-root".to_owned()),
            expected.map(str::to_owned),
            observed.map(str::to_owned),
        )
        .with_detail(detail),
    ))
}

fn protocol_fault(reason: &str, detail: impl Into<String>) -> Outcome<DemoAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Protocol,
        reason: reason.to_owned(),
        detail: detail.into(),
    })
}

fn environment_fault(reason: &str, detail: impl Into<String>) -> Outcome<DemoAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.into(),
    })
}
