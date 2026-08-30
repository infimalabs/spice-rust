#![cfg(unix)]

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant as MonotonicInstant};

use board::allocate::{Mint, mint};
use bus::{Bus, CommitOutcome};
use journal::{FactCause, JournalWrite};
use notify::Watcher as _;
use wire::body::SourceBoundary;
use wire::intent::{TaskAnswer, TaskDone};
use wire::{
    Actor, CanonicalUuid, ConfigSource, Digest, Domain, Instant, Outcome, Producer, TaskHandle,
    TaskStamp, TrustState, WorkflowHead, body,
};

#[path = "support/fixture_root.rs"]
mod fixture_root;
#[path = "support/readiness.rs"]
mod readiness;
#[path = "support/staged_app.rs"]
mod staged_app;

use readiness::Readiness;

const VALIDATION: &str = "one process-level evidence execution passed";

struct Fixture {
    root: PathBuf,
    cargo_home: PathBuf,
    journal: PathBuf,
    lane: CanonicalUuid,
    task: TaskHandle,
    claim: wire::Seq,
    evidence_release: PathBuf,
    evidence_count: PathBuf,
    remote: PathBuf,
    publication_release: PathBuf,
    publication_marker: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = fixture_root::claim(&std::env::temp_dir(), "task-completion-cli");
        let (cargo_home, tracked_policy, evidence_release, evidence_count) =
            initialize_fixture_repository(&root);
        let remote = prepare_conflicted_publication(&root);
        let journal = root.join(".git/.spice-rs/data/spicejournal.sqlite3");
        let resident = Resident::start_at(&root, &cargo_home);
        resident.stop_at(&root);
        grant_policy(&journal, &tracked_policy);
        let lane = uuid(0x71);
        let occupant = uuid(0x72);
        let (task, claim) = seed_active_task(&root, &journal, lane, occupant);
        fs::write(root.join(".git/MERGE_MSG"), completion_message(&task, lane))
            .expect("fixture allocator merge message");
        let publication_release = root.join(".git/publication-release");
        let publication_marker = root.join(".git/publication-marker");
        fifo(&publication_release);
        Self {
            root,
            cargo_home,
            journal,
            lane,
            task,
            claim,
            evidence_release,
            evidence_count,
            remote,
            publication_release,
            publication_marker,
        }
    }

    fn invoke_lane(&self, arguments: &[&str]) -> Output {
        self.lane_command(arguments)
            .output()
            .expect("lane command launches")
    }

    fn spawn_lane(&self, arguments: &[&str]) -> Child {
        self.lane_command(arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("lane command starts")
    }

    fn lane_command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        command
            .arg("--json")
            .args(arguments)
            .env(spice::agent_launch::SPICE_RS_LANE, self.lane.to_string())
            .current_dir(&self.root);
        command
    }

    fn done(&self) -> (Output, TaskAnswer) {
        let output = self.invoke_lane(&[
            "task",
            "done",
            self.task.as_str(),
            "--validation",
            VALIDATION,
        ]);
        let answer = decode_task_answer(&output, "task completion");
        (output, answer)
    }

    fn terminal_path(&self) -> PathBuf {
        self.root
            .join(".git/.spice-rs/task-completion")
            .join(self.claim.get().to_string())
            .join("terminal.bin")
    }

    fn install_publication_hook(&self) {
        let hook = self.remote.join("hooks/reference-transaction");
        fs::write(
            &hook,
            format!(
                concat!(
                    "#!/bin/sh\n",
                    "if [ \"$1\" = committed ]; then\n",
                    "  : > {:?}\n",
                    "  IFS= read -r _ < {:?}\n",
                    "fi\n",
                ),
                self.publication_marker.display().to_string(),
                self.publication_release.display().to_string(),
            ),
        )
        .expect("publication hook writes");
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o700))
            .expect("publication hook is executable");
    }

    fn retry(&self) -> TaskAnswer {
        self.done().1
    }
}

fn initialize_fixture_repository(root: &std::path::Path) -> (PathBuf, String, PathBuf, PathBuf) {
    git(root, &["init", "--quiet", "--initial-branch", "main"]);
    git(root, &["config", "user.name", "Spice Test"]);
    git(root, &["config", "user.email", "spice@example.test"]);
    let cargo_home = root.join(".git/fixture-cargo-home");
    fs::create_dir(&cargo_home).expect("fixture Cargo home");
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"completion-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
    )
    .expect("fixture Cargo manifest");
    fs::write(
        root.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"completion-fixture\"\nversion = \"0.0.0\"\n",
    )
    .expect("fixture Cargo lock");
    fs::create_dir(root.join("src")).expect("fixture source directory");
    fs::write(root.join("src/lib.rs"), "pub fn fixture() {}\n").expect("fixture base source");
    staged_app::seed(root);
    let skill = root.join(".git/.spice-install/agents/skills/spice/SKILL.md");
    fs::create_dir_all(skill.parent().expect("fixture skill parent"))
        .expect("fixture skill directory");
    fs::write(&skill, include_bytes!("../agent/SKILL.md")).expect("fixture skill");
    let vendor = root.join("fake-claude");
    fs::write(
        &vendor,
        concat!(
            "#!/bin/sh\n",
            "trap 'exit 0' TERM INT\n",
            "printf '%s\\n' '",
            "{\"type\":\"system\",\"subtype\":\"init\",",
            "\"session_id\":\"018f82d4-6f59-7d06-92e3-bce913116374\"}'\n",
            "while [ -d .git ]; do sleep 1; done\n",
        ),
    )
    .expect("fixture vendor");
    fs::set_permissions(&vendor, fs::Permissions::from_mode(0o700))
        .expect("fixture vendor is executable");
    let evidence_release = root.join(".git/evidence-release");
    let evidence_count = root.join(".git/evidence-count");
    let evidence_command = root.join(".git/evidence-command");
    fifo(&evidence_release);
    fs::write(
        &evidence_command,
        format!(
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' evidence >> {:?}\n",
                "IFS= read -r _ < {:?}\n",
            ),
            evidence_count.display().to_string(),
            evidence_release.display().to_string(),
        ),
    )
    .expect("fixture evidence command");
    fs::set_permissions(&evidence_command, fs::Permissions::from_mode(0o700))
        .expect("fixture evidence command is executable");
    let tracked_policy = format!(
        concat!(
            "[tasks]\nstems = [\"canvas\"]\n\n",
            "[vendor.claude]\nbinary = {:?}\n\n",
            "[policy]\n",
            "pre_commit_builtins = false\n",
            "pre_commit = [{{ label = \"delayed evidence\", run = [{:?}] }}]\n\n",
            "[policy.file_shape]\nsource_suffixes = [\".rs\", \".toml\"]\n\n",
            "[policy.languages]\ncomplexity = [\".rs\"]\nmagic = [\".rs\"]\n",
        ),
        vendor.display().to_string(),
        evidence_command.display().to_string(),
    );
    fs::write(root.join("spice-rust.toml"), &tracked_policy).expect("fixture task policy");
    git(root, &["add", "."]);
    git(root, &["commit", "--quiet", "-m", "fixture base"]);
    (cargo_home, tracked_policy, evidence_release, evidence_count)
}

fn prepare_conflicted_publication(root: &std::path::Path) -> PathBuf {
    let remote = root.join(".git/upstream.git");
    let remote_text = remote.display().to_string();
    git(root, &["init", "--quiet", "--bare", &remote_text]);
    git(root, &["remote", "add", "origin", &remote_text]);
    git(
        root,
        &["push", "--quiet", "--set-upstream", "origin", "main"],
    );

    fs::write(
        root.join("src/lib.rs"),
        "pub fn fixture() { println!(\"lane\"); }\n",
    )
    .expect("fixture lane source");
    git(root, &["add", "src/lib.rs"]);
    git(root, &["commit", "--quiet", "-m", "lane work"]);

    let upstream_work = root.join(".git/upstream-work");
    let upstream_work_text = upstream_work.display().to_string();
    git(
        root,
        &["clone", "--quiet", &remote_text, &upstream_work_text],
    );
    git(&upstream_work, &["config", "user.name", "Spice Test"]);
    git(
        &upstream_work,
        &["config", "user.email", "spice@example.test"],
    );
    fs::write(
        upstream_work.join("src/lib.rs"),
        "pub fn fixture() { println!(\"upstream\"); }\n",
    )
    .expect("fixture upstream source");
    git(&upstream_work, &["add", "src/lib.rs"]);
    git(
        &upstream_work,
        &["commit", "--quiet", "-m", "upstream work"],
    );
    git(&upstream_work, &["push", "--quiet", "origin", "main"]);
    fs::remove_dir_all(&upstream_work).expect("remove upstream work clone");

    git(root, &["fetch", "--quiet", "origin"]);
    git_failure(root, &["merge", "--no-ff", "origin/main"]);
    fs::write(root.join(".git/MERGE_MODE"), "no-ff\n").expect("fixture allocator merge mode");
    fs::write(
        root.join("src/lib.rs"),
        "pub fn fixture() { println!(\"resolved\"); }\n",
    )
    .expect("resolve fixture source");
    git(root, &["add", "src/lib.rs"]);
    remote
}

impl Drop for Fixture {
    fn drop(&mut self) {
        make_fixture_writable(&self.root);
        fs::remove_dir_all(&self.root).expect("fixture cleanup");
    }
}

fn make_fixture_writable(path: &std::path::Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    if metadata.is_dir() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .expect("fixture directory becomes writable");
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                make_fixture_writable(&entry.path());
            }
        }
    } else {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .expect("fixture file becomes writable");
    }
}

struct Resident(Option<Child>);

impl Resident {
    fn start(fixture: &Fixture) -> Self {
        Self::start_at(&fixture.root, &fixture.cargo_home)
    }

    fn start_at(root: &std::path::Path, cargo_home: &std::path::Path) -> Self {
        let canvas = root.join(".git/.spice-rs/canvas");
        let prior_discovery = fs::read(&canvas).ok();
        let child = Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(["--json", "up", "--foreground", "--port", "0"])
            .env_remove(spice::agent_launch::SPICE_RS_LANE)
            .env(spice::cargo_isolation::BOOTSTRAP_HOME_VARIABLE, cargo_home)
            .env(
                spice::cargo_isolation::NATIVE_CARGO_VARIABLE,
                fixture_native_tool("cargo"),
            )
            .env(
                spice::cargo_isolation::NATIVE_RUSTC_VARIABLE,
                fixture_native_tool("rustc"),
            )
            .env(
                spice::cargo_isolation::NATIVE_RUSTDOC_VARIABLE,
                fixture_native_tool("rustdoc"),
            )
            .current_dir(root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("resident starts");
        let mut resident = Self(Some(child));
        let runtime = canvas.parent().expect("canvas runtime");
        fs::create_dir_all(runtime).expect("runtime directory");
        match readiness::wait_until_ready(
            resident.0.as_mut().expect("resident child"),
            runtime,
            readiness::ARRIVAL_HORIZON,
            "resident discovery",
            || {
                fs::read(&canvas)
                    .ok()
                    .is_some_and(|current| Some(&current) != prior_discovery.as_ref())
            },
        ) {
            Readiness::Ready => resident,
            Readiness::Exited => {
                let output = resident
                    .0
                    .take()
                    .expect("exited resident child")
                    .wait_with_output()
                    .expect("exited resident output");
                panic!(
                    "resident exited before readiness: stdout={} stderr={}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }

    fn stop(self, fixture: &Fixture) {
        self.stop_at(&fixture.root);
    }

    fn kill(mut self) {
        let mut child = self.0.take().expect("resident child");
        child.kill().expect("resident is killed");
        child.wait().expect("killed resident is reaped");
    }

    fn stop_at(mut self, root: &std::path::Path) {
        let down = Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(["--json", "down"])
            .env_remove(spice::agent_launch::SPICE_RS_LANE)
            .current_dir(root)
            .output()
            .expect("resident down launches");
        assert!(
            down.status.success(),
            "resident down failed: {}",
            String::from_utf8_lossy(&down.stderr)
        );
        let output = self
            .0
            .take()
            .expect("resident child")
            .wait_with_output()
            .expect("resident exits");
        assert!(
            output.status.success(),
            "resident failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

impl Drop for Resident {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
#[ignore = "exhaustive resident and Git interruption fixture; run explicitly"]
fn completion_handoff_recovers_across_disconnect_publication_failure_and_restart() {
    let fixture = Fixture::new();
    let resident = assert_running_handoff(&fixture);
    interrupt_after_git_publication(&fixture, resident);
    assert_terminal_replay(&fixture);
}

fn assert_running_handoff(fixture: &Fixture) -> Resident {
    let resident = Resident::start(fixture);
    let started = MonotonicInstant::now();
    let (initial_output, running) = fixture.done();
    assert!(
        initial_output.status.success(),
        "initial completion succeeds"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "initial completion hid its durable handoff for {:?}",
        started.elapsed()
    );
    let completion = running
        .completion
        .as_ref()
        .expect("initial completion returns Running");
    assert_eq!(completion.state, wire::intent::TaskCompletionState::Running);
    assert_eq!(
        completion.operation,
        TaskDone::operation_for_claim(fixture.claim)
    );
    assert_eq!(fixture.retry(), running, "running retry is byte-equivalent");

    let status = fixture.invoke_lane(&["task", "status"]);
    assert!(
        status.status.success(),
        "resident stopped answering while evidence ran: {}",
        String::from_utf8_lossy(&status.stderr)
    );

    let evidence_started = FileArrival::arm(&fixture.evidence_count);
    if !evidence_started.wait_nonempty_for(Duration::from_secs(30)) {
        let terminal = fixture.terminal_path();
        let progress = terminal.with_file_name("progress.jsonl");
        panic!(
            "evidence did not start: progress={} terminal={:?}",
            fs::read_to_string(progress).unwrap_or_else(|error| error.to_string()),
            fs::read(terminal).unwrap_or_else(|error| error.to_string().into_bytes()),
        );
    }
    resident
}

fn interrupt_after_git_publication(fixture: &Fixture, resident: Resident) {
    fixture.install_publication_hook();
    let published = FileArrival::arm(&fixture.publication_marker);
    let terminal = FileArrival::arm(&fixture.terminal_path());
    release_fifo(&fixture.evidence_release);
    assert!(
        terminal.wait_nonempty_for(Duration::from_secs(10)),
        "completion worker publishes its terminal"
    );
    assert_eq!(
        fs::read_to_string(&fixture.evidence_count).unwrap(),
        "evidence\n"
    );

    let mut interrupted = fixture.spawn_lane(&[
        "task",
        "done",
        fixture.task.as_str(),
        "--validation",
        VALIDATION,
    ]);
    if !published.wait_present_for(Duration::from_secs(10)) {
        interrupted.kill().expect("stalled retry is killed");
        let output = interrupted
            .wait_with_output()
            .expect("stalled retry output reads");
        panic!(
            "retry completed before publication: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    let published_head = git_output(
        &fixture.root,
        &[
            "--git-dir",
            &fixture.remote.display().to_string(),
            "rev-parse",
            "refs/heads/main",
        ],
    );
    resident.kill();
    release_fifo(&fixture.publication_release);
    let interrupted = interrupted
        .wait_with_output()
        .expect("disconnected completion exits");
    assert!(
        !interrupted.status.success(),
        "completion unexpectedly replied after its resident was killed"
    );
    assert_eq!(task_done_count(&fixture.journal, &fixture.task), 0);
    assert_eq!(
        git_output(&fixture.root, &["rev-parse", "HEAD"]),
        published_head,
        "Git publication completed before journal publication was interrupted"
    );
}

fn assert_terminal_replay(fixture: &Fixture) {
    let recovered = Resident::start(fixture);
    let (terminal_output, terminal_answer) = fixture.done();
    assert!(terminal_answer.completion.is_none());
    assert!(terminal_answer.learning.is_none());
    assert_eq!(terminal_answer.status, "completed");

    std::thread::scope(|scope| {
        let retries = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    let (output, answer) = fixture.done();
                    assert_eq!(answer, terminal_answer);
                    assert_same_output(&output, &terminal_output);
                })
            })
            .collect::<Vec<_>>();
        for retry in retries {
            retry.join().expect("retry process joins");
        }
    });
    let status = fixture.invoke_lane(&["task", "status"]);
    assert!(
        status.status.success(),
        "resident stopped answering after retries: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    recovered.stop(fixture);

    let restarted = Resident::start(fixture);
    let (restarted_output, restarted_answer) = fixture.done();
    assert_eq!(restarted_answer, terminal_answer);
    assert_same_output(&restarted_output, &terminal_output);
    assert_eq!(task_done_count(&fixture.journal, &fixture.task), 1);
    assert_eq!(
        fs::read_to_string(&fixture.evidence_count).unwrap(),
        "evidence\n"
    );
    restarted.stop(fixture);
}

struct FileArrival {
    path: PathBuf,
    receiver: std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    _watcher: notify::RecommendedWatcher,
}

impl FileArrival {
    fn arm(path: &std::path::Path) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })
        .expect("file arrival watcher starts");
        watcher
            .watch(
                path.parent().expect("observed file has a parent"),
                notify::RecursiveMode::NonRecursive,
            )
            .expect("file arrival parent is watched");
        Self {
            path: path.to_path_buf(),
            receiver,
            _watcher: watcher,
        }
    }

    fn wait_present_for(&self, timeout: Duration) -> bool {
        self.wait_for(timeout, |metadata| metadata.is_file())
    }

    fn wait_nonempty_for(&self, timeout: Duration) -> bool {
        self.wait_for(timeout, |metadata| {
            metadata.is_file() && metadata.len() != 0
        })
    }

    fn wait_for(&self, timeout: Duration, ready: impl Fn(&fs::Metadata) -> bool) -> bool {
        if fs::metadata(&self.path).is_ok_and(|metadata| ready(&metadata)) {
            return true;
        }
        let started = MonotonicInstant::now();
        loop {
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return false;
            }
            match self.receiver.recv_timeout(remaining) {
                Ok(event) => {
                    event.expect("file arrival watcher remains healthy");
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return false,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("file arrival watcher closed");
                }
            }
            if fs::metadata(&self.path).is_ok_and(|metadata| ready(&metadata)) {
                return true;
            }
        }
    }
}

fn decode_task_answer(output: &Output, context: &str) -> TaskAnswer {
    assert!(
        output.status.success(),
        "{context} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let Outcome::Reply(reply) =
        wire::decode::<Outcome<TaskAnswer>>(&output.stdout).expect("typed task outcome")
    else {
        panic!("{context} did not reply");
    };
    reply.body
}

fn assert_same_output(observed: &Output, expected: &Output) {
    assert_eq!(observed.status.code(), expected.status.code());
    assert_eq!(observed.stdout, expected.stdout);
    assert_eq!(observed.stderr, expected.stderr);
}

fn release_fifo(path: &std::path::Path) {
    let mut release = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("release FIFO has a waiting reader");
    release
        .write_all(b"release\n")
        .expect("release event writes");
}

fn fifo(path: &std::path::Path) {
    let status = Command::new("/usr/bin/mkfifo")
        .arg(path)
        .status()
        .expect("mkfifo launches");
    assert!(status.success(), "mkfifo creates {path:?}");
}

fn git(root: &std::path::Path, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("fixture Git launches");
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_failure(root: &std::path::Path, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("fixture Git launches");
    assert!(
        !output.status.success(),
        "git {arguments:?} unexpectedly succeeded"
    );
}

fn git_output(root: &std::path::Path, arguments: &[&str]) -> String {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("fixture Git launches");
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("Git output is UTF-8")
        .trim()
        .to_owned()
}

fn seed_active_task(
    root: &std::path::Path,
    path: &std::path::Path,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> (TaskHandle, wire::Seq) {
    let journal = journal::Journal::open(path).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let (epoch, now, origin) = seed_lane_authority(root, &bus, lane, occupant);
    let claimed = seed_claimed_task(&bus, epoch, now, origin, lane, occupant);
    bus.verify().expect("fixture authority verifies");
    bus.shutdown().expect("fixture bus stops");
    claimed
}

fn seed_lane_authority(
    root: &std::path::Path,
    bus: &Bus,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> (CanonicalUuid, Instant, wire::Origin) {
    let residency = bus
        .residency_head()
        .expect("residency reads")
        .expect("resident established residency");
    let epoch = residency.body().epoch;
    let capability = uuid(0x73);
    let worktree = fs::canonicalize(root)
        .expect("fixture root canonicalizes")
        .display()
        .to_string();
    let identity = uuid(0x74).to_string();
    let record =
        format!("{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"{identity}\"}}\n");
    let locus = tape::Tape::open(
        root.join(".git/.spice-rs/tape")
            .join(lane.to_string())
            .join("claude"),
    )
    .expect("fixture tape opens")
    .tee(record.as_bytes())
    .expect("session record reaches tape");
    let session = wire::LaneSession {
        lane,
        occupant,
        worktree: worktree.clone(),
        vendor: "claude".parse().expect("fixture vendor"),
        launch: Some(body::LaneLaunchIdentity::new("sonnet", "high")),
        epoch,
        stream: "stdout".to_owned(),
        identity,
        locus: wire::TapeLocus {
            member: locus.member,
            offset: locus.offset,
            length: locus.length,
        },
    };
    let mut next = residency.body().clone();
    next.lanes.push(wire::LaneResidency {
        lane,
        lineage_id: lane,
        name: None,
        worktree: worktree.clone(),
        life: "drive".to_owned(),
        vendor: "claude".parse().expect("fixture vendor"),
        occupant: Some(occupant),
        lineage: vec![occupant],
        capabilities: vec![capability],
        session: Some(session.clone()),
        disposition: "occupied".to_owned(),
    });
    next.subscriptions.push(wire::LineageSubscription {
        lineage_id: lane,
        policy: wire::intent::LineageLife::Drain,
        manual_projects: Vec::new(),
        auto_projects: Vec::new(),
    });
    let origin: wire::Origin = "task:completion-process".parse().expect("fixture origin");
    let resident_origin: wire::Origin = format!("resident:{epoch}")
        .parse()
        .expect("resident origin");
    let now = Instant::new(jiff::Timestamp::now()).expect("fixture clock");
    let mut write = JournalWrite::new(
        Actor::Spice,
        resident_origin,
        now,
        Producer::ResidentRecognition,
    );
    let seated = write
        .fact(
            body::LaneSeated {
                lane,
                lineage_id: lane,
                capability,
                name: None,
                worktree,
                life: "drive".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
            },
            FactCause::Existing(residency.position()),
        )
        .expect("lane seating fact");
    let declared = write
        .fact(
            body::LaneSessionDeclared {
                lane: session.lane,
                occupant: session.occupant,
                worktree: session.worktree,
                vendor: session.vendor,
                launch: session.launch,
                epoch: session.epoch,
                stream: session.stream,
                identity: session.identity,
                member: session.locus.member,
                offset: session.locus.offset,
                length: session.locus.length,
            },
            FactCause::Pending(seated),
        )
        .expect("lane session declaration");
    let started = write
        .fact(
            body::LaneStarted {
                lane,
                occupant,
                epoch,
                group: 1,
            },
            FactCause::Pending(declared),
        )
        .expect("lane start fact");
    write.residency_head(started, move |_| next);
    commit(bus, write, "lane seating");
    (epoch, now, origin)
}

fn seed_claimed_task(
    bus: &Bus,
    epoch: CanonicalUuid,
    now: Instant,
    origin: wire::Origin,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> (TaskHandle, wire::Seq) {
    let stamp = TaskStamp::for_millis(1_901_001).expect("fixture task stamp");
    let task = TaskHandle::for_project("canvas.resident", &stamp);
    let workflow = pending_workflow();
    let mut write = JournalWrite::new(Actor::Operator, origin.clone(), now, Producer::CommandDoor);
    let added = write
        .fact(
            body::TaskAdded {
                task: stamp.clone(),
                title: "Replay a completed terminal".to_owned(),
                description: None,
                acceptance: Vec::new(),
                project: "canvas.resident".to_owned(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: workflow.flow.clone(),
                phase: workflow.phase.clone(),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("task added fact");
    let head_stamp = stamp.clone();
    let head_origin = origin.clone();
    write.task_head(added, move |position| wire::TaskHead {
        born: head_stamp,
        queue_origin: position,
        title: "Replay a completed terminal".to_owned(),
        description: None,
        acceptance: Vec::new(),
        project: "canvas.resident".to_owned(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: head_origin,
        revision: position,
    });
    write.workflow_head(stamp.clone(), added, move |_| workflow);
    commit(bus, write, "task creation");

    let expiry = Instant::new(
        now.get()
            .checked_add(jiff::SignedDuration::from_hours(1))
            .expect("fixture lease expiry"),
    )
    .expect("fixture expiry is canonical");
    let maximum = Instant::new(
        now.get()
            .checked_add(jiff::SignedDuration::from_hours(2))
            .expect("fixture lease maximum"),
    )
    .expect("fixture maximum is canonical");
    let actor = Actor::Lane(lane);
    let mut write = JournalWrite::new(actor.clone(), origin.clone(), now, Producer::CommandDoor);
    mint(
        &mut write,
        Mint {
            task: &stamp,
            actor: &actor,
            workflow: &pending_workflow(),
            project: "canvas.resident",
            lane,
            occupant,
            epoch,
            at: now,
            expiry,
            maximum,
            git: None,
            recovery: None,
            source: SourceBoundary::Proven { heads: Vec::new() },
        },
    )
    .expect("claim mints");
    commit(bus, write, "claim mint");

    let claim = bus
        .read({
            let stamp = stamp.clone();
            move |read| Ok(read.claim_head(&stamp)?.expect("claim head exists"))
        })
        .expect("claim head reads");
    (task, claim.body().claimed)
}

fn pending_workflow() -> WorkflowHead {
    WorkflowHead {
        flow: vec!["todo".parse().expect("todo phase")],
        phase: "todo".parse().expect("todo phase"),
        status: "pending".to_owned(),
        review: "none".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    }
}

fn task_done_count(path: &std::path::Path, task: &TaskHandle) -> usize {
    let stamp = task.born().expect("fixture task has a stamp");
    journal::JournalReader::open(path)
        .expect("journal reader opens")
        .fact_snapshot()
        .expect("completion facts read")
        .facts()
        .iter()
        .filter_map(|fact| fact.fact.read::<body::TaskDone>())
        .filter(|done| done.task == stamp)
        .count()
}

fn completion_message(task: &TaskHandle, lane: CanonicalUuid) -> String {
    let key = task
        .as_str()
        .rsplit_once('-')
        .map_or(task.as_str(), |(_, key)| key);
    format!(
        concat!(
            "canvas: Replay a completed terminal {task}\n\n",
            "Task-Key: {key}\n",
            "Task-Phase: todo\n",
            "Task-Project: canvas.resident\n",
            "Task-Session: {lane}\n",
        ),
        task = task,
        key = key,
        lane = lane,
    )
}

fn grant_policy(path: &std::path::Path, tracked: &str) {
    let digest = policy_digest(tracked);
    let mut authority = journal::Journal::open(path).expect("fixture journal opens for trust");
    let mut write = JournalWrite::new(
        Actor::Spice,
        "recovery:task-completion-configured-command"
            .parse()
            .expect("fixture trust origin"),
        "2030-04-12T18:44:01.271Z"
            .parse()
            .expect("fixture trust instant"),
        Producer::CommandDoor,
    );
    let pended = write
        .fact(
            body::TrustPended {
                surface: "configuration".to_owned(),
                digest: digest.clone(),
                previous: None,
            },
            FactCause::Root,
        )
        .expect("build fixture trust fact");
    write.trust_head(pended, move |_| wire::TrustHead {
        digest,
        state: TrustState::Granted,
    });
    authority.apply(write).expect("commit fixture trust grant");
}

fn policy_digest(tracked: &str) -> Digest {
    let mut framed = Vec::new();
    frame_policy(&mut framed, ConfigSource::Tracked, Some(tracked));
    frame_policy(&mut framed, ConfigSource::Local, None);
    Digest::compute(Domain::ConfigExecutableSurface, &framed)
}

fn frame_policy(framed: &mut Vec<u8>, source: ConfigSource, text: Option<&str>) {
    framed.extend_from_slice(source.as_str().as_bytes());
    framed.push(0);
    match text {
        None => framed.push(0),
        Some(text) => {
            framed.push(1);
            framed.extend_from_slice(
                &u64::try_from(text.len())
                    .expect("fixture policy length fits u64")
                    .to_be_bytes(),
            );
            framed.extend_from_slice(text.as_bytes());
        }
    }
}

fn commit(bus: &Bus, write: JournalWrite, context: &str) {
    let outcome = bus
        .commit(write)
        .unwrap_or_else(|error| panic!("{context}: {error}"));
    assert!(
        matches!(outcome, CommitOutcome::Committed(_)),
        "{context} did not commit: {outcome:?}"
    );
}

fn uuid(tail: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{tail:02x}")
        .parse()
        .expect("fixture UUID")
}

fn fixture_native_tool(name: &str) -> PathBuf {
    let tool = PathBuf::from(env!("CARGO"))
        .parent()
        .expect("Cargo executable has a parent")
        .join(name);
    assert!(tool.is_file(), "fixture tool exists: {tool:?}");
    tool
}
