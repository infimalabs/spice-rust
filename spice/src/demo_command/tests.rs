use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tungstenite::client::IntoClientRequest as _;
use tungstenite::http::header::ORIGIN;
use tungstenite::{Message, WebSocket};
use wire::intent::{BoardAnswer, BoardRead, CommandAnswer, LaneAnswer, LaneRead};
use wire::{Command, CommandKind, Response, StreamFrame, StreamRequest, ViewSubject, WireVersion};

use super::*;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

fn app() -> canvas::AppSource {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let root = ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("spice-demo-unit-app-{}", std::process::id()));
        let stage = root.join(canvas::APP_STAGE);
        fs::create_dir_all(&stage).expect("demo fixture app stage");
        for asset in canvas::APP.assets() {
            fs::write(
                stage.join(asset.file()),
                format!("demo fixture app {}\n", asset.file()),
            )
            .expect("demo fixture app asset");
        }
        root
    });
    canvas::AppSource::resolve(root).expect("demo fixture app resolves")
}

fn seed_test_environment(root: Option<&Path>, executable: &Path) -> DemoResult<Environment> {
    require_utf8(executable, "current spice executable")?;
    let (root, automatic) = prepare_root(root)?;
    let result = born_repository(&root)
        .and_then(|()| finish_seed_prepared_root(&root, executable, automatic, &app()));
    result.map_err(|failure| preserve_failure(&root, executable, *failure).into())
}

fn seed_test_prepared_root(
    root: &Path,
    executable: &Path,
    automatic: bool,
) -> DemoResult<Environment> {
    born_repository(root)?;
    finish_seed_prepared_root(root, executable, automatic, &app())
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
    assert_eq!(kinds.last(), Some(&wire::FactKind::LaneSeated));
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
        assert_eq!(refusal.reason, "demo::commit-moved");
        assert_eq!(refusal.expected.as_deref(), Some(expected_head().as_str()));
        assert_eq!(refusal.observed.as_deref(), Some(observed));
        assert_eq!(
            refusal.remedy,
            "reset this demo root to its seeded commit or seed a new one"
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
            refusal.reason.as_str(),
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
    assert_eq!(refusal.reason, "demo::path-refused");
    assert!(!root.exists(), "path refusal must precede seed mutation");
}

#[test]
fn native_canvas_stream_renders_seeded_lane_task_and_canned_turns() {
    let scratch = Scratch::new("browser");
    let root = scratch.child("browser-root");
    let environment = seed_test_environment(Some(&root), Path::new("/opt/spice"))
        .expect("browser demo seed succeeds");
    let root_for_down = environment.root.clone();
    let outcome = canvas_environment(environment, false, false, |discovery_path| {
        crate::command_client::open_canvas_with(
            discovery_path,
            || {},
            |endpoint| {
                let discovery = read_discovery(discovery_path);
                assert_eq!(endpoint, discovery.endpoint);
                assert_browser_seed(&discovery);
                assert!(matches!(
                    crate::resident::down_from(
                        &root_for_down,
                        crate::cli::contract(&["down"]).expect("down contract"),
                    ),
                    Outcome::Reply(_)
                ));
                Ok(())
            },
        )
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

#[test]
fn launch_failure_preserves_one_reusable_or_removable_recovery_root() {
    let scratch = Scratch::new("launch-failure");
    let root = scratch.child("failed-root");
    let environment = seed_test_environment(Some(&root), Path::new("/opt/spice"))
        .expect("failure demo seed succeeds");
    let outcome = canvas_environment(environment, true, false, |discovery_path| {
        crate::command_client::open_canvas_with(
            discovery_path,
            || {},
            |_endpoint| Err("fixture browser launcher failed".to_owned()),
        )
    });
    let Outcome::Fault(fault) = outcome else {
        panic!("launcher failure is typed: {outcome:?}");
    };
    assert_eq!(fault.reason, "canvas::launch-failed");
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
    let outcome = canvas_environment_with(environment, true, false, true, |discovery_path| {
        crate::command_client::open_canvas_with(discovery_path, || {}, |_endpoint| Ok(()))
    });
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
    let outcome = canvas_environment(environment, true, false, |discovery_path| {
        crate::command_client::open_canvas_with(
            discovery_path,
            || {},
            |_endpoint| {
                assert!(matches!(
                    crate::resident::down_from(
                        &root_for_down,
                        crate::cli::contract(&["down"]).expect("down contract"),
                    ),
                    Outcome::Reply(_)
                ));
                Ok(())
            },
        )
    });
    let Outcome::Reply(answer) = outcome else {
        panic!("clean demo service replies: {outcome:?}");
    };
    assert_eq!(answer.body.lifecycle, "removed");
    assert!(!root.exists());
    assert!(scratch.0.exists(), "cleanup stays scoped to the demo root");
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
    let notes = packed
        .rows
        .iter()
        .filter(|fact| fact.kind() == wire::FactKind::TaskNoted)
        .map(|fact| {
            fact.body()["note"]
                .as_str()
                .expect("demo note is text")
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        notes,
        CANNED_TRANSCRIPT
            .iter()
            .map(|(_, text)| (*text).to_owned())
            .collect::<Vec<_>>()
    );
    stream.close(None).unwrap();

    let board = issue(
        discovery,
        accepted.epoch,
        CommandKind::BoardRead,
        &BoardRead { family: None },
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
    let lane = lanes
        .lanes
        .iter()
        .find(|lane| lane.lane == uuid(DEMO_LANE))
        .expect("demo lane is visible");
    assert_eq!(lane.disposition, "vacant");
    assert_eq!(lane.worktree, lanes.lanes[0].worktree);
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
