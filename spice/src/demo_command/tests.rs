use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tungstenite::client::IntoClientRequest as _;
use tungstenite::http::header::ORIGIN;
use tungstenite::{Message, WebSocket};
use wire::intent::{BoardAnswer, BoardRead, CommandAnswer, LaneAnswer, LaneRead};
use wire::{
    Command, CommandKind, ResidentState, Response, StreamFrame, StreamRequest, ViewSubject,
    WireVersion,
};

use super::*;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

/// A staged lane app the seed copies into the demo root, so the stage
/// itself lives only as long as the seed that reads it.
fn fixture_app() -> (Scratch, canvas::AppSource) {
    let root = Scratch::new("app");
    let stage = root.0.join(canvas::APP_STAGE);
    let identity = "5555555555555555555555555555555555555555555555555555555555555555";
    let generation = stage.join(canvas::APP_STAGE_GENERATIONS).join(identity);
    fs::create_dir_all(&generation).expect("demo fixture app generation");
    for asset in canvas::APP.assets() {
        fs::write(
            generation.join(asset.file()),
            format!("demo fixture app {}\n", asset.file()),
        )
        .expect("demo fixture app asset");
    }
    fs::write(
        stage.join(canvas::APP_STAGE_CURRENT),
        format!("{identity}\n"),
    )
    .expect("demo fixture app pointer");
    let source = canvas::AppSource::resolve(&root.0).expect("demo fixture app resolves");
    (root, source)
}

fn seed_test_environment(root: Option<&Path>, executable: &Path) -> DemoResult<Environment> {
    require_utf8(executable, "current spice executable")?;
    let (root, automatic) = prepare_root(root)?;
    let (_stage, app) = fixture_app();
    let result = born_repository(&root)
        .and_then(|()| finish_seed_prepared_root(&root, executable, automatic, &app));
    result.map_err(|failure| preserve_failure(&root, executable, *failure).into())
}

fn seed_test_prepared_root(
    root: &Path,
    executable: &Path,
    automatic: bool,
) -> DemoResult<Environment> {
    born_repository(root)?;
    let (_stage, app) = fixture_app();
    finish_seed_prepared_root(root, executable, automatic, &app)
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-demo-unit-{label}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("demo fixture root is exclusively created");
        Self(root)
    }

    fn child(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn seed_is_deterministic_quoted_and_reopenable_without_touching_parent_state() {
    let scratch = Scratch::new("seed");
    let root = scratch.child("demo root's");
    let executable = Path::new("/opt/spice bin/spice's");
    let environment = seed_test_environment(Some(&root), executable).expect("demo seed succeeds");

    assert_eq!(environment.root, fs::canonicalize(&root).unwrap());
    assert_eq!(
        environment.transcript,
        environment.root.join(TRANSCRIPT_NAME)
    );
    assert!(environment.state.journal().is_file());
    assert_eq!(seeded_head(&environment.root).unwrap(), expected_head());
    assert_eq!(
        fs::read(&environment.transcript).unwrap(),
        transcript_bytes().unwrap()
    );
    assert!(environment.launch_command.starts_with("cd '"));
    assert!(environment.launch_command.contains("'\"'\"'"));
    assert!(environment.launch_command.contains(" demo --reuse "));
    assert_eq!(environment.remove_command.matches("rm -rf --").count(), 1);

    let reader = journal::JournalReader::open(environment.state.journal()).unwrap();
    let facts = reader.fact_snapshot().unwrap();
    let kinds = facts
        .facts()
        .iter()
        .map(|envelope| envelope.fact.kind())
        .collect::<Vec<_>>();
    assert_eq!(kinds[0], wire::FactKind::TaskAdded);
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == wire::FactKind::TaskNoted)
            .count(),
        CANNED_TRANSCRIPT.len()
    );
    assert_eq!(kinds.last(), Some(&wire::FactKind::LaneStarted));
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == wire::FactKind::LaneSeated)
            .count(),
        DEMO_LANES.len()
    );
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == wire::FactKind::LaneStarted)
            .count(),
        DEMO_LANES.len()
    );
    for seed in DEMO_LANES {
        assert!(facts.facts().iter().any(|envelope| {
            envelope
                .fact
                .read::<body::LaneStarted>()
                .is_some_and(|started| {
                    started.lane == uuid(seed.lane)
                        && started.occupant == uuid(seed.occupant)
                        && started.epoch == uuid(DEMO_EPOCH)
                })
        }));
    }
    for pair in facts.facts()[..=CANNED_TRANSCRIPT.len()].windows(2) {
        assert_eq!(pair[1].cause, Some(pair[0].seq));
    }

    let reopened = reopen_environment(&root, executable).expect("complete seed reopens");
    assert_eq!(reopened.root, environment.root);
    assert_eq!(reopened.launch_command, environment.launch_command);
}

#[test]
fn seeded_roots_are_born_on_one_commit_wherever_they_are_created() {
    let scratch = Scratch::new("born");
    let first = scratch.child("first root");
    let second = scratch.child("a second root under a much longer name");
    for root in [&first, &second] {
        seed_test_environment(Some(root), Path::new("/opt/spice"))
            .expect("born demo seed succeeds");
    }

    let born = "refs/heads/main at a53ad58b1356a5f7abea7437975321850e04b217";
    assert_eq!(
        seeded_head(&first).unwrap(),
        born,
        "the first seeded root is not born on the deterministic commit"
    );
    assert_eq!(
        seeded_head(&second).unwrap(),
        born,
        "a longer root path moved the seeded commit"
    );
    assert_eq!(
        expected_head(),
        born,
        "reopening expects a commit the seed never writes"
    );
}

#[test]
fn reuse_refuses_a_seeded_root_whose_branch_or_commit_moved() {
    let scratch = Scratch::new("moved");
    let root = scratch.child("moved-root");
    let executable = Path::new("/opt/spice");
    let environment =
        seed_test_environment(Some(&root), executable).expect("moved-root seed succeeds");
    let head = environment.root.join(".git/HEAD");
    let branch = environment.root.join(".git/refs/heads/main");
    let seeded = fs::read(&branch).unwrap();
    let refuses = |observed: &str| {
        let outcome = *reopen_environment(&root, executable).expect_err("a moved seed refuses");
        let Outcome::Refusal(refusal) = outcome else {
            panic!("a moved seed must refuse: {outcome:?}");
        };
        assert_eq!(refusal.reason(), "demo::commit-moved");
        let diagnostic = refusal.diagnostic().expect("demo refusal is diagnostic");
        assert_eq!(diagnostic.expected(), Some(expected_head().as_str()));
        assert_eq!(diagnostic.observed(), Some(observed));
        assert_eq!(
            diagnostic.detail(),
            Some("reset this demo root to its seeded commit or seed a new one")
        );
    };

    fs::write(&branch, "9c3f4bd0bd0b1e2ee59a1e0e9d2b47a3f7c4a5d6\n").unwrap();
    refuses("refs/heads/main at 9c3f4bd0bd0b1e2ee59a1e0e9d2b47a3f7c4a5d6");

    fs::write(&branch, &seeded).unwrap();
    fs::write(&head, "ref: refs/heads/operator\n").unwrap();
    refuses("refs/heads/operator at no commit");

    fs::write(&head, format!("ref: {SEED_BRANCH}\n")).unwrap();
    assert!(
        reopen_environment(&root, executable).is_ok(),
        "the named recovery restores a reusable demo root"
    );
}

#[test]
fn new_seed_refuses_nonempty_and_non_directory_roots_without_mutation() {
    let scratch = Scratch::new("root-refusal");
    let nonempty = scratch.child("operator repository");
    fs::create_dir(&nonempty).unwrap();
    let sentinel = nonempty.join("sentinel.txt");
    fs::write(&sentinel, b"operator bytes\n").unwrap();
    let file = scratch.child("operator-file");
    fs::write(&file, b"also operator bytes\n").unwrap();

    for root in [&nonempty, &file] {
        let outcome = *seed_test_environment(Some(root), Path::new("/opt/spice"))
            .expect_err("protected root refuses");
        let Outcome::Refusal(refusal) = outcome else {
            panic!("protected root must refuse: {outcome:?}");
        };
        assert!(matches!(
            refusal.reason(),
            "demo::root-not-empty" | "demo::root-refused"
        ));
    }
    assert_eq!(fs::read(&sentinel).unwrap(), b"operator bytes\n");
    assert_eq!(fs::read(&file).unwrap(), b"also operator bytes\n");
}

#[cfg(unix)]
#[test]
fn non_utf8_executable_refuses_before_the_demo_root_exists() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;

    let scratch = Scratch::new("executable-refusal");
    let root = scratch.child("uncreated-root");
    let executable = PathBuf::from(OsString::from_vec(b"/opt/spice-\xff".to_vec()));
    let outcome = *seed_test_environment(Some(&root), &executable)
        .expect_err("non-reproducible executable path refuses");

    let Outcome::Refusal(refusal) = outcome else {
        panic!("non-UTF-8 executable path must refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason(), "demo::path-refused");
    assert!(!root.exists(), "path refusal must precede seed mutation");
}

#[test]
fn native_canvas_stream_begins_live_and_reads_seeded_lane_task() {
    let scratch = Scratch::new("browser");
    let root = scratch.child("browser-root");
    let environment = seed_test_environment(Some(&root), Path::new("/opt/spice"))
        .expect("browser demo seed succeeds");
    let root_for_down = environment.root.clone();
    let discovery_path = environment.state.canvas().to_path_buf();
    let outcome = canvas_environment_with(environment, false, false, true, false, |endpoint| {
        let discovery = read_discovery(&discovery_path);
        assert_eq!(endpoint, discovery.endpoint);
        assert_browser_seed(&discovery);
        let Outcome::Reply(stopped) = crate::resident::down_from_operator(
            &root_for_down,
            crate::cli::contract(&["down"]).expect("down contract"),
        ) else {
            panic!("the demo resident stops on request");
        };
        assert_eq!(
            (stopped.body.from, stopped.body.to),
            (ResidentState::Seated, ResidentState::Dark),
            "down answers with the transition it made"
        );
        Ok(())
    });
    let Outcome::Reply(answer) = outcome else {
        panic!("served browser demo replies: {outcome:?}");
    };
    assert_eq!(answer.body.mode, "canvas");
    assert_eq!(answer.body.lifecycle, "preserved");
    assert!(
        answer
            .body
            .endpoint
            .as_deref()
            .is_some_and(|value| value.starts_with("http://127.0.0.1:"))
    );
    assert!(root.exists());
}

/// Seed one demo root these gate cases can run to completion.
///
/// Both interrupt after launch rather than stopping the resident from inside
/// the launcher, which is how every other case here ends its run. That pattern
/// is unavailable to the case that matters: a plain run never calls the
/// launcher at all, so a launcher that shuts the resident down would hang
/// exactly when it passes. The injected signal is sent on both paths, so the
/// two cases below differ only in the flag under test.
fn gate_environment(name: &str) -> (Scratch, Environment) {
    let scratch = Scratch::new(name);
    let root = scratch.child("demo-root");
    fs::create_dir(&root).unwrap();
    let environment = seed_test_prepared_root(&root, Path::new("/opt/spice"), true)
        .expect("gate demo seed succeeds");
    (scratch, environment)
}

/// Drive the generated parser all the way through `run_with`, pinning the
/// adjacent `signals` and `open` arguments to their intended meanings. The
/// test-only interruption ends each resident after the launch gate, independent
/// of whether the launcher runs; swapping those arguments therefore makes the
/// `--open` assertion fail promptly instead of leaving the plain case resident.
#[test]
fn parsed_demo_open_flag_reaches_the_canvas_gate() {
    let control = RunControl {
        signals: false,
        interrupt_after_launch: true,
    };

    let (_plain_scratch, plain_environment) = gate_environment("plain-no-open");
    let plain_root = plain_environment.root.to_str().unwrap();
    let plain = crate::cli::command()
        .try_get_matches_from(["spice", "demo", "--reuse", plain_root])
        .expect("the plain demo invocation parses");
    let outcome = run_with(crate::cli::selected_matches(&plain), control, |endpoint| {
        panic!("a plain demo run must not open a browser: {endpoint}")
    });
    let Outcome::Reply(answer) = outcome else {
        panic!("a plain demo run replies: {outcome:?}");
    };
    assert_eq!(answer.body.mode, "canvas");
    assert!(
        answer
            .body
            .endpoint
            .as_deref()
            .is_some_and(|value| value.starts_with("http://127.0.0.1:")),
        "the endpoint is reported even though nothing was opened: {:?}",
        answer.body.endpoint
    );

    let (_open_scratch, open_environment) = gate_environment("explicit-open");
    let open_root = open_environment.root.to_str().unwrap();
    let open = crate::cli::command()
        .try_get_matches_from(["spice", "demo", "--reuse", open_root, "--open"])
        .expect("the opening demo invocation parses");
    let mut dialled = None;
    let outcome = run_with(crate::cli::selected_matches(&open), control, |endpoint| {
        dialled = Some(endpoint.to_owned());
        Ok(())
    });
    let Outcome::Reply(answer) = outcome else {
        panic!("an opening demo run replies: {outcome:?}");
    };
    assert!(dialled.is_some(), "--open reaches the launcher");
    assert_eq!(dialled.as_deref(), answer.body.endpoint.as_deref());
}

/// The gate above is only reachable if the flag exists and defaults off. This
/// asks the generated parser rather than the contract string, because the
/// string is what registers the flag and a test that re-read it would pass on
/// a `demo` that never grew one.
#[test]
fn demo_registers_open_and_defaults_it_off() {
    let parse = |arguments: &[&str]| {
        let parsed = crate::cli::command()
            .try_get_matches_from(arguments)
            .expect("the demo invocation parses");
        crate::cli::selected_matches(&parsed).get_flag("open")
    };
    assert!(
        !parse(&["spice", "demo"]),
        "a plain demo run is the one an agent makes without thinking about it"
    );
    assert!(parse(&["spice", "demo", "--open"]));
    assert!(
        crate::cli::command()
            .try_get_matches_from(["spice", "demo", "--no-open"])
            .is_err(),
        "opting out of a launching default is the shape this task refused"
    );
}

#[test]
fn launch_failure_preserves_one_reusable_or_removable_recovery_root() {
    let scratch = Scratch::new("launch-failure");
    let root = scratch.child("failed-root");
    let environment = seed_test_environment(Some(&root), Path::new("/opt/spice"))
        .expect("failure demo seed succeeds");
    let outcome = canvas_environment_with(environment, true, false, true, false, |_endpoint| {
        Err("fixture browser launcher failed".to_owned())
    });
    let Outcome::Fault(fault) = outcome else {
        panic!("launcher failure is typed: {outcome:?}");
    };
    // Printing the whole fault, not just the reason, is what makes a failure
    // here diagnosable. Every step before the launcher can refuse too, and
    // under load one of them has; a bare left/right on the reason threw away
    // the detail naming which one.
    assert_eq!(
        fault.reason, "canvas::launch-failed",
        "a step before the launcher refused instead, and this is the whole fault: {fault:?}",
    );
    assert!(fault.detail.contains("fixture browser launcher failed"));
    assert!(fault.detail.contains("retry with"));
    assert!(fault.detail.contains("remove with"));
    assert!(root.exists(), "failure never removes recovery state");
    assert!(reopen_environment(&root, Path::new("/opt/spice")).is_ok());
}

#[test]
fn interruption_preserves_even_an_automatic_cleanup_root() {
    let scratch = Scratch::new("interruption");
    let root = scratch.child("automatic-root");
    fs::create_dir(&root).unwrap();
    let environment = seed_test_prepared_root(&root, Path::new("/opt/spice"), true)
        .expect("automatic demo seed succeeds");
    let outcome =
        canvas_environment_with(environment, true, false, false, true, |_endpoint| Ok(()));
    let Outcome::Reply(answer) = outcome else {
        panic!("interrupted demo replies with recovery: {outcome:?}");
    };
    assert_eq!(answer.body.lifecycle, "interrupted-preserved");
    assert!(root.exists());
    assert!(answer.body.launch_command.contains("--reuse"));
    assert!(answer.body.remove_command.contains("rm -rf --"));
}

#[test]
fn clean_automatic_service_removes_only_its_demo_root() {
    let scratch = Scratch::new("cleanup");
    let root = scratch.child("automatic-root");
    fs::create_dir(&root).unwrap();
    let environment = seed_test_prepared_root(&root, Path::new("/opt/spice"), true)
        .expect("automatic cleanup seed succeeds");
    let root_for_down = fs::canonicalize(&root).unwrap();
    let outcome = canvas_environment_with(environment, true, false, true, false, |_endpoint| {
        let Outcome::Reply(stopped) = crate::resident::down_from_operator(
            &root_for_down,
            crate::cli::contract(&["down"]).expect("down contract"),
        ) else {
            panic!("the demo resident stops on request");
        };
        assert_eq!(
            (stopped.body.from, stopped.body.to),
            (ResidentState::Seated, ResidentState::Dark),
            "down answers with the transition it made"
        );
        Ok(())
    });
    let Outcome::Reply(answer) = outcome else {
        panic!("clean demo service replies: {outcome:?}");
    };
    assert_eq!(answer.body.lifecycle, "removed");
    assert!(!root.exists());
    assert!(scratch.0.exists(), "cleanup stays scoped to the demo root");
}

#[test]
fn disappearing_known_publication_files_are_a_benign_readiness_change() {
    let scratch = Scratch::new("transient-journal");
    let root = scratch.child("automatic-root");
    fs::create_dir(&root).expect("the automatic demo root is created");
    let environment = seed_test_prepared_root(&root, Path::new("/opt/spice"), true)
        .expect("the automatic demo root is seeded");
    assert!(environment.automatic);
    let journal = environment.state.journal();
    let sidecar = journal.with_extension("sqlite3-journal");
    fs::write(&sidecar, []).expect("the journal sidecar is discovered");
    let error =
        notify::Error::io(io::Error::from(io::ErrorKind::NotFound)).add_path(sidecar.clone());

    let destination = atomic::Destination::new(root.join("published")).expect("atomic destination");
    let staged = destination.stage().expect("atomic publication is staged");
    let temporary = staged.temporary_path().to_path_buf();

    fs::remove_file(&sidecar).expect("the journal sidecar disappears before inspection");
    drop(staged);

    assert_eq!(
        readiness_change(Err(error.add_path(temporary)), journal),
        Ok(())
    );
}

#[test]
fn persistent_and_unrelated_readiness_scan_failures_are_preserved() {
    let scratch = Scratch::new("readiness-errors");
    let journal = scratch.child("spicejournal.sqlite3");
    let sidecar = journal.with_extension("sqlite3-journal");
    fs::write(&sidecar, []).expect("the persistent journal sidecar exists");

    let persistent = readiness_change(
        Err(notify::Error::io(io::Error::from(io::ErrorKind::NotFound)).add_path(sidecar.clone())),
        &journal,
    )
    .expect_err("a persistent path is not dismissed as a disappearance race");
    assert!(persistent.contains(&sidecar.display().to_string()));

    let destination = atomic::Destination::new(scratch.child("published"))
        .expect("persistent atomic destination");
    let staged = destination.stage().expect("persistent atomic stage");
    let temporary = staged.temporary_path().to_path_buf();
    let persistent_atomic =
        readiness_change(
            Err(notify::Error::io(io::Error::from(io::ErrorKind::NotFound))
                .add_path(temporary.clone())),
            &journal,
        )
        .expect_err("a persistent atomic path is not dismissed as a disappearance race");
    assert!(persistent_atomic.contains(&temporary.display().to_string()));
    drop(staged);

    fs::remove_file(&sidecar).expect("the journal sidecar is absent for the I/O fixture");
    let unrelated_io = readiness_change(
        Err(
            notify::Error::io(io::Error::from(io::ErrorKind::PermissionDenied))
                .add_path(sidecar.clone()),
        ),
        &journal,
    )
    .expect_err("an unrelated I/O error remains a readiness failure");
    assert!(unrelated_io.contains(&sidecar.display().to_string()));

    let unrelated_path = scratch.child("unrelated.tmp");
    let unrelated_missing = readiness_change(
        Err(notify::Error::io(io::Error::from(io::ErrorKind::NotFound))
            .add_path(unrelated_path.clone())),
        &journal,
    )
    .expect_err("an unrelated missing path remains a readiness failure");
    assert!(unrelated_missing.contains(&unrelated_path.display().to_string()));

    let malformed_atomic = scratch.child(".spice-atomic-01-2.tmp");
    let malformed_missing = readiness_change(
        Err(notify::Error::io(io::Error::from(io::ErrorKind::NotFound))
            .add_path(malformed_atomic.clone())),
        &journal,
    )
    .expect_err("a malformed atomic lookalike remains a readiness failure");
    assert!(malformed_missing.contains(&malformed_atomic.display().to_string()));
}

/// A readiness scan that fails refuses under its own name.
///
/// Three unrelated failures used to share `demo::readiness-unavailable`:
/// creating the watcher, registering the watch, and a scan error raised while
/// waiting. One string for three causes is why a real failure under load could
/// only be reported as "not the reason I wanted" -- the panic could not say
/// which of the three had happened.
///
/// This forces the third, which is also the one that actually fires: a
/// `PollWatcher` does *not* reject a missing directory at registration, it
/// accepts the watch and then reports the failure from inside the scan. So an
/// unwatchable root arrives as `demo::readiness-scan-failed`, and the two
/// pre-resident reasons stay reserved for genuine watcher-construction and
/// registration faults rather than absorbing this one.
///
/// The scan is raised while waiting for discovery, which is before the browser
/// is reached, so this also pins that a readiness refusal never launches.
#[test]
fn an_unwatchable_root_refuses_by_its_own_reason_before_the_browser_is_reached() {
    let scratch = Scratch::new("unwatchable");
    let root = scratch.child("missing-root");
    let environment = seed_test_environment(Some(&root), Path::new("/opt/spice"))
        .expect("unwatchable demo seed succeeds");
    // Removed only after seeding, so the environment is well formed and the
    // single thing wrong is that the directory to watch is gone.
    fs::remove_dir_all(&root).expect("the watched root is removed before the watch");

    let mut launched = false;
    let outcome = canvas_environment_with(environment, false, false, true, false, |_endpoint| {
        launched = true;
        Ok(())
    });
    let Outcome::Fault(fault) = outcome else {
        panic!("an unwatchable root is typed: {outcome:?}");
    };
    assert_eq!(
        fault.reason, "demo::readiness-scan-failed",
        "a failed scan is named apart from watcher creation and watch registration: {fault:?}",
    );
    assert!(
        fault.detail.contains(&root.display().to_string()),
        "the detail names the path that could not be watched: {}",
        fault.detail,
    );
    assert!(
        !launched,
        "a watch refused before the resident starts never reaches the browser",
    );
}

fn read_discovery(path: &Path) -> canvas::Discovery {
    wire::decode(&fs::read(path).expect("demo discovery reads")).expect("demo discovery decodes")
}

fn assert_browser_seed(discovery: &canvas::Discovery) {
    let (mut stream, accepted) = authenticate(discovery);
    let request = StreamRequest::Subscribe {
        v: WireVersion::CURRENT,
        id: uuid("018f82d4-6f59-7d06-92e3-bce913116420"),
        epoch: discovery.epoch,
        subjects: vec![ViewSubject::Facts],
        after: None,
    };
    send(&mut stream, &request);
    let StreamFrame::Facts { packed, .. } = receive(&mut stream) else {
        panic!("browser stream begins with seeded facts");
    };
    assert!(
        packed.rows.is_empty(),
        "a fresh operator starts at the current live barrier without replaying demo history"
    );
    assert!(packed.overflow.is_none());
    stream.close(None).unwrap();

    let board = issue(
        discovery,
        accepted.epoch,
        CommandKind::BoardRead,
        &BoardRead::All { family: None },
    );
    let CommandAnswer::Board(board) = board else {
        panic!("demo board command returns a board");
    };
    assert_board(&board);
    let lanes = issue(
        discovery,
        accepted.epoch,
        CommandKind::LaneRead,
        &LaneRead {},
    );
    let CommandAnswer::Lane(lanes) = lanes else {
        panic!("demo lane command returns lanes");
    };
    assert_lanes(&lanes);
}

fn assert_board(board: &BoardAnswer) {
    let task = board
        .tasks
        .iter()
        .find(|task| task.statement.body.handle.as_str() == DEMO_TASK)
        .expect("demo task is visible");
    assert_eq!(task.presentation_title(), DEMO_TITLE);
    assert_eq!(task.statement.body.project, DEMO_PROJECT);
}

fn assert_lanes(lanes: &LaneAnswer) {
    assert_eq!(lanes.lanes.len(), DEMO_LANES.len());
    for seed in DEMO_LANES {
        let lane = lanes
            .lanes
            .iter()
            .find(|lane| lane.lane == uuid(seed.lane))
            .expect("each simulated demo lane is visible");
        assert_eq!(lane.team, uuid(DEMO_LANE));
        assert_eq!(lane.vendor.as_str(), crate::resident::SIMULATED_VENDOR);
        assert_eq!(lane.disposition, crate::resident::SIMULATED_DISPOSITION);
        assert_eq!(lane.occupant, Some(uuid(seed.occupant)));
        assert_eq!(lane.liveness, wire::intent::LaneLiveness::Running);
        assert_eq!(lane.worktree, lanes.lanes[0].worktree);
    }
}

fn issue<B: Serialize>(
    discovery: &canvas::Discovery,
    epoch: CanonicalUuid,
    kind: CommandKind,
    body: &B,
) -> CommandAnswer {
    let (mut socket, _) = authenticate(discovery);
    let command = Command {
        v: WireVersion::CURRENT,
        id: uuid("018f82d4-6f59-7d06-92e3-bce913116421"),
        epoch,
        kind,
        expected: Vec::new(),
        body,
    };
    send(&mut socket, &command);
    let response: Response<serde_json::Value> = receive(&mut socket);
    socket.close(None).unwrap();
    let Outcome::Reply(reply) = response.outcome else {
        panic!("demo browser command replies: {:?}", response.outcome);
    };
    kind.binding()
        .answer
        .decode(&wire::encode(&reply.body).expect("reply value encodes"))
        .expect("reply body matches its command binding")
}

fn authenticate(discovery: &canvas::Discovery) -> (WebSocket<TcpStream>, wire::Accepted) {
    let authority = discovery.endpoint.strip_prefix("http://").unwrap();
    let address: SocketAddr = authority.parse().unwrap();
    let stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut request = format!("ws://{authority}/socket")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(ORIGIN, discovery.endpoint.parse().unwrap());
    let (mut socket, _) = tungstenite::client(request, stream).unwrap();
    send(
        &mut socket,
        &wire::Hello {
            wire: WireVersion::CURRENT,
            capability: discovery.capability,
            epoch: discovery.epoch,
        },
    );
    let accepted = receive(&mut socket);
    (socket, accepted)
}

fn send<T: Serialize>(socket: &mut WebSocket<TcpStream>, value: &T) {
    socket
        .send(Message::Binary(wire::encode(value).unwrap().into()))
        .unwrap();
}

fn receive<T: serde::de::DeserializeOwned>(socket: &mut WebSocket<TcpStream>) -> T {
    wire::decode(&socket.read().unwrap().into_data()).unwrap()
}
