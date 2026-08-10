use super::*;

use sill::{
    Argv, Cancellation, Deadlines, Environment, ObservationFailure, Output, OutputStream,
    ProcessGroup, Redaction, RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy,
};
use std::os::unix::fs::PermissionsExt as _;
use std::process::Command as ProcessCommand;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
const PROOF_BOUND: Duration = Duration::from_secs(5);

#[test]
fn real_judge_does_not_block_the_current_thread_resident_surface() {
    let root = fixture("real-judge-liveness");
    let paths = RuntimePaths::discover(&root).expect("runtime paths resolve");
    fs::create_dir_all(&paths.data).expect("resident data directory");
    stage_canvas_app(&root);
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let StartupSurface { mut config, trust } =
        load_startup_surface(&repository).expect("startup surface loads");
    config.taste = taste::Bag::new(vec![
        taste::Taste::new(
            "polling".parse().expect("taste name"),
            ["poll*"],
            "Wait for an event.",
        )
        .expect("taste row"),
    ])
    .expect("taste bag");
    config.judge_enabled = true;
    config.lane = toml::from_str(
        r#"
deaths = 1
hold = "0s"
"#,
    )
    .expect("immediate fixture hold policy");

    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let epoch = uuid(1);
    commit_startup(&bus, &repository, &trust, epoch, None).expect("resident seats");

    let taste_lane = uuid(10);
    let taste_occupant = uuid(11);
    let failure_lane = uuid(20);
    let failure_occupant = uuid(21);
    let exit_lane = uuid(30);
    let exit_occupant = uuid(31);
    let mut seats = Seats::new();
    seats
        .seat(
            taste_lane,
            taste_occupant,
            process_spec("exec /usr/bin/tail -f /dev/null"),
        )
        .expect("taste occupant starts");
    seats
        .seat(
            failure_lane,
            failure_occupant,
            process_spec("exec /usr/bin/tail -f /dev/null"),
        )
        .expect("failure occupant starts");
    seats
        .seat(exit_lane, exit_occupant, process_spec("exit 17"))
        .expect("exit occupant starts");
    seed_lane(&bus, epoch, taste_lane, taste_occupant, uuid(12), &root);
    seed_lane(&bus, epoch, failure_lane, failure_occupant, uuid(22), &root);
    seed_lane(&bus, epoch, exit_lane, exit_occupant, uuid(32), &root);

    let resident_lock = ResidentLock::acquire(&paths).expect("resident lock acquired");
    resident_lock
        .set_epoch(epoch)
        .expect("resident epoch publishes");
    let judge = JudgeFixture::new(&root);
    let mut adjudication = taste_surface::start_process_adjudication(
        judge.script.to_string_lossy().into_owned(),
        Duration::from_secs(30),
    )
    .expect("process adjudication starts");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread resident runtime");

    let judge_pid = runtime.block_on(async {
        let timer_commits = bus.reader().subscribe();
        let proof_commits = bus.reader().subscribe();
        let timers = timers::Runtime::start(config.projects.clone(), config.steering.clone())
            .expect("timer authority starts");
        let lifecycle = lifecycle::Runtime::new(&config.lane);
        let mut progress = Vec::new();
        let mut endpoint = Vec::new();
        let started = start_surface(
            &bus,
            &paths,
            epoch,
            current_residency(&bus, epoch)
                .expect("current residency")
                .body(),
            &config,
            SurfaceLaunch {
                binding: CanvasBinding::local(EPHEMERAL_LOOPBACK),
                open: false,
                progress: &mut progress,
                endpoint: &mut endpoint,
            },
        )
        .await
        .expect("Canvas surface starts");
        let StartedSurface {
            capabilities,
            discovery,
            control,
            published,
            shutdown,
            server_task,
            message_publisher,
            progress,
        } = started;
        let capture::Channels {
            reporter,
            records,
            failures,
            exits,
        } = capture::channels();
        let command_surface = command_surface::Surface::new(
            &bus,
            &repository,
            &paths,
            epoch,
            lane_surface::Runtime::new(&capabilities, &discovery, reporter.clone()),
            lane_recovery::Runtime::new(reporter.clone()),
            &config,
        );
        let (submitter, completions) = adjudication.parts();
        let entered_path = judge.entered.clone();
        let entered = tokio::task::spawn_blocking(move || {
            fs::read_to_string(entered_path).expect("judge entry FIFO reports its process")
        });
        let exercise = exercise_live_surface(
            &bus,
            &paths,
            reporter,
            proof_commits,
            entered,
            taste_lane,
            taste_occupant,
            failure_lane,
            failure_occupant,
            exit_lane,
            exit_occupant,
        );
        let drive = drive_surface(
            &bus,
            epoch,
            &mut seats,
            &command_surface,
            control,
            published,
            shutdown,
            server_task,
            message_publisher,
            Some(submitter),
            Some(completions),
            records,
            failures,
            exits,
            progress,
            timers,
            timer_commits,
            lifecycle,
        );
        let (driven, judge_pid) = tokio::join!(drive, exercise);
        driven.expect("Down stops the live surface");
        judge_pid
    });

    assert!(seats.is_empty(), "Down drains every supervised occupant");
    assert_eq!(
        adjudication.pending_len(),
        2,
        "the later capture reached the submitter while the first judge stayed open"
    );
    let cancellation_started = std::time::Instant::now();
    drop(adjudication);
    assert!(
        cancellation_started.elapsed() < Duration::from_secs(2),
        "resident shutdown must cancel the judge instead of waiting for its runtime deadline"
    );
    assert_process_gone(judge_pid.trim());

    drop(judge);
    drop(resident_lock);
    bus.verify().expect("resident authority verifies");
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[allow(
    clippy::too_many_arguments,
    reason = "the liveness exercise names each independent resident event source"
)]
async fn exercise_live_surface(
    bus: &Bus,
    paths: &RuntimePaths,
    reporter: std::sync::Arc<dyn lane::capture::CaptureReporter>,
    mut commits: bus::BroadcastSubscriber,
    entered: tokio::task::JoinHandle<String>,
    taste_lane: CanonicalUuid,
    taste_occupant: CanonicalUuid,
    failure_lane: CanonicalUuid,
    failure_occupant: CanonicalUuid,
    exit_lane: CanonicalUuid,
    exit_occupant: CanonicalUuid,
) -> String {
    reporter
        .record(taste_record(
            taste_lane,
            taste_occupant,
            "polling now",
            false,
        ))
        .expect("first taste record reports");
    let judge_pid = tokio::time::timeout(PROOF_BOUND, entered)
        .await
        .expect("real judge starts within the fixture bound")
        .expect("judge entry reader joins");

    expect_reply(
        "first lane read",
        execute_command(
            paths.canvas.clone(),
            &["lane"],
            crate::command_client::SelectedCommand::LaneRead,
        )
        .await,
    );
    reporter
        .record(taste_record(
            taste_lane,
            taste_occupant,
            "polling later",
            true,
        ))
        .expect("later taste record reports");
    reporter.failed(lane::capture::CaptureFailure {
        lane: failure_lane,
        occupant: failure_occupant,
        failure: ObservationFailure::SinkRefused {
            stream: OutputStream::Stdout,
            detail: "fixture tape refusal".to_owned(),
        },
    });
    reporter.exited(lane::capture::CaptureExit {
        lane: exit_lane,
        occupant: exit_occupant,
    });
    wait_for_runtime_evidence(bus, &mut commits, taste_lane).await;

    expect_reply(
        "later lane read",
        execute_command(
            paths.canvas.clone(),
            &["lane"],
            crate::command_client::SelectedCommand::LaneRead,
        )
        .await,
    );
    expect_reply(
        "resident Down",
        execute_command(
            paths.canvas.clone(),
            &["down"],
            crate::command_client::SelectedCommand::Down,
        )
        .await,
    );
    judge_pid
}

async fn execute_command(
    discovery: PathBuf,
    contract: &'static [&'static str],
    command: crate::command_client::SelectedCommand<'static>,
) -> Outcome<wire::intent::CommandAnswer> {
    tokio::task::spawn_blocking(move || {
        crate::command_client::execute(
            &discovery,
            crate::cli::contract(contract).expect("fixture command contract"),
            command,
        )
    })
    .await
    .expect("resident command client joins")
}

fn expect_reply(label: &str, outcome: Outcome<wire::intent::CommandAnswer>) {
    assert!(matches!(outcome, Outcome::Reply(_)), "{label}: {outcome:?}");
}

async fn wait_for_runtime_evidence(
    bus: &Bus,
    commits: &mut bus::BroadcastSubscriber,
    taste_lane: CanonicalUuid,
) {
    let taste_lane = taste_lane.to_string();
    tokio::time::timeout(PROOF_BOUND, async {
        loop {
            match commits.recv().await {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("proof commit stream closed while resident was live")
                }
            }
            let snapshot = bus.fact_snapshot().expect("runtime facts read");
            let count = |kind| {
                snapshot
                    .facts()
                    .iter()
                    .filter(|fact| fact.fact.kind() == kind)
                    .count()
            };
            let later_capture = snapshot.facts().iter().any(|fact| {
                fact.fact.kind() == wire::FactKind::LaneCompacted
                    && fact.fact.body()["lane"].as_str() == Some(taste_lane.as_str())
                    && fact.fact.body()["offset"] == 200
            });
            if later_capture
                && count(wire::FactKind::LaneDegraded) == 1
                && count(wire::FactKind::LaneExited) == 1
                && count(wire::FactKind::LaneResumed) == 2
            {
                return;
            }
        }
    })
    .await
    .expect("captures, failures, exits, commit wakes, and lifecycle timers stay serviceable");
}

fn taste_record(
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    text: &str,
    compacted: bool,
) -> lane::capture::CaptureRecord {
    let offset = if compacted { 200 } else { 100 };
    let locus = listen::Locus::new(7, offset, text.len() as u64);
    let mut events = vec![listen::Event::Prose(listen::Prose {
        role: listen::ProseRole::Assistant,
        text: text.to_owned(),
        phase: Some("final".to_owned()),
        media: Vec::new(),
        locus,
    })];
    if compacted {
        events.push(listen::Event::Compaction(listen::Compaction {
            summary: None,
            locus,
        }));
    }
    lane::capture::CaptureRecord {
        lane,
        occupant,
        stream: vendor::RecordStream::Stdout,
        boundary: sill::RecordBoundary::Complete,
        locus,
        declaration: None,
        decoded: listen::Decoded::Events {
            context: Default::default(),
            events,
        },
    }
}

struct JudgeFixture {
    script: PathBuf,
    entered: PathBuf,
    release: PathBuf,
}

impl JudgeFixture {
    fn new(root: &Path) -> Self {
        let script = root.join("judge");
        let entered = root.join("judge-entered");
        let release = root.join("judge-release");
        make_fifo(&entered);
        make_fifo(&release);
        fs::write(
            &script,
            format!(
                "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' \"$$\" > {}\ncat {} >/dev/null\nprintf 'YES\\n'\n",
                shell_quote(&entered),
                shell_quote(&release),
            ),
        )
        .expect("judge script writes");
        let mut permissions = fs::metadata(&script)
            .expect("judge script metadata")
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&script, permissions).expect("judge script is executable");
        Self {
            script,
            entered,
            release,
        }
    }
}

impl Drop for JudgeFixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.script);
        let _ = fs::remove_file(&self.entered);
        let _ = fs::remove_file(&self.release);
    }
}

fn make_fifo(path: &Path) {
    let status = ProcessCommand::new("mkfifo")
        .arg(path)
        .status()
        .expect("mkfifo launches");
    assert!(status.success(), "mkfifo creates {}", path.display());
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

fn assert_process_gone(pid: &str) {
    let probe = ProcessCommand::new("kill")
        .args(["-0", pid])
        .output()
        .expect("process probe launches");
    assert!(
        !probe.status.success(),
        "cancelled judge process {pid} survived bounded worker join"
    );
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "spice-resident-{name}-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(root.join(".git")).expect("git directory");
    root
}

fn stage_canvas_app(root: &Path) {
    let identity = wire::Digest::compute(wire::Domain::DistArtifact, b"judge-liveness-app");
    let stage = root.join(canvas::APP_STAGE);
    let generation = stage
        .join(canvas::APP_STAGE_GENERATIONS)
        .join(identity.as_str());
    fs::create_dir_all(&generation).expect("Canvas generation directory");
    for asset in canvas::APP.assets() {
        fs::write(
            generation.join(asset.file()),
            format!("fixture {}", asset.file()),
        )
        .expect("Canvas fixture asset writes");
    }
    fs::write(
        stage.join(canvas::APP_STAGE_CURRENT),
        format!("{identity}\n"),
    )
    .expect("Canvas generation publishes");
}

fn process_spec(command: &str) -> Spec {
    Spec {
        argv: Argv::new("/bin/sh", ["-c", command]).expect("fixture argv"),
        cwd: PathBuf::from("/"),
        environment: Environment::new([("PATH", "/usr/bin:/bin")]).expect("fixture environment"),
        group: ProcessGroup::New,
        deadlines: Deadlines::new(
            Duration::from_millis(100),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect("fixture deadlines"),
        wait: WaitPolicy::new(
            "resident liveness fixture",
            Cancellation::Never,
            RestartDisposition::Never,
        )
        .expect("fixture wait policy"),
        output: Output::new(StdinPolicy::Null, StreamPolicy::Null, StreamPolicy::Null),
        redaction: Redaction::None,
        grace: Duration::from_millis(100),
    }
}

fn seed_lane(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    capability: CanonicalUuid,
    worktree: &Path,
) {
    let current = current_residency(bus, epoch).expect("current residency");
    let mut head = current.body().clone();
    head.topology_revision += 1;
    let worktree = worktree
        .canonicalize()
        .expect("fixture worktree canonicalizes")
        .to_string_lossy()
        .into_owned();
    head.lanes.push(wire::LaneResidency {
        lane,
        lineage_id: lane,
        name: None,
        worktree: worktree.clone(),
        life: "drive".to_owned(),
        vendor: "claude".parse().expect("fixture vendor"),
        occupant: Some(occupant),
        lineage: vec![occupant],
        capabilities: vec![capability],
        session: None,
        disposition: "occupied".to_owned(),
    });
    head.subscriptions.push(wire::LineageSubscription {
        lineage_id: lane,
        policy: wire::intent::LineageLife::Drive,
        manual_projects: Vec::new(),
        auto_projects: Vec::new(),
    });
    head.lanes.sort_by_key(|row| row.lane);
    head.subscriptions.sort_by_key(|row| row.lineage_id);
    let mut write =
        write(Actor::Spice, epoch, Producer::ResidentRecognition).expect("fixture write starts");
    let seated = write
        .fact(
            wire::body::LaneSeated {
                lane,
                lineage_id: lane,
                capability,
                name: None,
                worktree,
                life: "drive".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
            },
            FactCause::Existing(current.position()),
        )
        .expect("lane seats");
    let started = write
        .fact(
            wire::body::LaneStarted {
                lane,
                occupant,
                epoch,
                group: 123,
            },
            FactCause::Pending(seated),
        )
        .expect("occupant starts");
    write.residency_head(started, move |_| head);
    commit(bus, write).expect("fixture lane commits");
}

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
        .parse()
        .expect("fixture UUID")
}
