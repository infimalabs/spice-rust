//! Real Git-parent interruption regression for pre-commit supervision.

use super::*;

use std::ffi::CString;
use std::fs;
use std::io::Read as _;
use std::net::{TcpListener, TcpStream};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use parent::ProcessExitObserver;

const ROLE: &str = "SPICE_PRE_COMMIT_PARENT_TEST_ROLE"; // env-policy: allow
const ROOT: &str = "SPICE_PRE_COMMIT_PARENT_TEST_ROOT"; // env-policy: allow
const SOCKET: &str = "SPICE_PRE_COMMIT_PARENT_TEST_SOCKET"; // env-policy: allow
const EXECUTABLE: &str = "SPICE_PRE_COMMIT_PARENT_TEST_EXE"; // env-policy: allow
const FILTER: &str = "interrupted_git_parent_cancels_gate_tree_and_releases_lock";

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let root = env::temp_dir().join(format!(
            "spice-pre-commit-parent-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        fs::create_dir(&root).expect("create parent-loss fixture");
        Self { root }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn interrupted_git_parent_cancels_gate_tree_and_releases_lock() {
    if env::var_os(ROLE).is_some() {
        run_dispatcher_fixture();
        return;
    }

    let scratch = Scratch::new();
    initialize_repository(&scratch.root);
    let hook = scratch.root.join(".git/hooks/pre-commit");
    fs::write(
        &hook,
        format!("#!/bin/sh\nexec \"$SPICE_PRE_COMMIT_PARENT_TEST_EXE\" {FILTER} --nocapture\n"),
    )
    .expect("write parent-loss hook");
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700))
        .expect("make parent-loss hook executable");

    let blocked = scratch.root.join("block-gate");
    fs::write(&blocked, b"block\n").expect("arm blocking gate fixture");
    fs::write(scratch.root.join("change"), b"interrupted\n").expect("write staged change");
    require_git(&scratch.root, &["add", "change"]);

    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .expect("bind dispatcher readiness listener");
    let socket = listener
        .local_addr()
        .expect("read dispatcher readiness address")
        .to_string();
    let executable = env::current_exe().expect("locate pre-commit test executable");
    let mut commit = Command::new("git")
        .args(["commit", "-m", "interrupted gate"])
        .current_dir(&scratch.root)
        .env(ROLE, "dispatcher")
        .env(ROOT, &scratch.root)
        .env(SOCKET, &socket)
        .env(EXECUTABLE, &executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("launch plain commit with blocking gate");

    let (mut readiness, _) = listener
        .accept()
        .expect("dispatcher announces complete tree");
    let mut message = String::new();
    readiness
        .read_to_string(&mut message)
        .expect("read dispatcher tree announcement");
    let pids = message
        .split_whitespace()
        .map(|pid| pid.parse::<u32>().expect("announced pid is numeric"))
        .collect::<Vec<_>>();
    let [dispatcher, worker, descendant] = pids.as_slice() else {
        panic!("dispatcher announces its complete process tree: {message:?}");
    };
    let dispatcher_exit = live_observer(*dispatcher);
    let worker_exit = live_observer(*worker);
    let descendant_exit = live_observer(*descendant);

    commit.kill().expect("interrupt only the git commit parent");
    let interrupted = commit.wait().expect("reap interrupted git parent");
    assert!(!interrupted.success(), "the interrupted commit cannot land");
    descendant_exit.wait().expect("worker descendant exits");
    worker_exit.wait().expect("worker is terminated and reaped");
    dispatcher_exit.wait().expect("orphaned dispatcher exits");
    assert_eq!(
        fs::read_to_string(scratch.root.join("cancelled"))
            .expect("dispatcher records cancellation completion"),
        "cancelled\n",
    );

    fs::remove_file(blocked).expect("disarm blocking gate fixture");
    let retry = Command::new("git")
        .args(["commit", "-m", "after interrupted gate"])
        .current_dir(&scratch.root)
        .env(ROLE, "dispatcher")
        .env(ROOT, &scratch.root)
        .env(SOCKET, &socket)
        .env(EXECUTABLE, &executable)
        .output()
        .expect("retry plain commit");
    assert!(
        retry.status.success(),
        "released pre-commit lock admits retry: stdout={} stderr={}",
        String::from_utf8_lossy(&retry.stdout),
        String::from_utf8_lossy(&retry.stderr),
    );
    assert_eq!(
        git_stdout(&scratch.root, &["log", "-1", "--pretty=%s"]),
        "after interrupted gate\n",
    );
}

fn run_dispatcher_fixture() {
    let root = PathBuf::from(env::var_os(ROOT).expect("dispatcher fixture root"));
    let _dispatch = DispatchGuard::acquire::<Report>()
        .unwrap_or_else(|outcome| panic!("dispatcher acquires pre-commit lock: {outcome:?}"));
    let parent = ParentCancellation::capture().expect("observe git commit parent");
    if !root.join("block-gate").exists() {
        return;
    }

    let fifo = root.join("worker-ready.fifo");
    create_fifo(&fifo);
    let script = concat!(
        "sleep 30 & child=$!; ",
        "printf '%s %s\\n' \"$$\" \"$child\" > \"$1\"; ",
        "wait",
    );
    let spec = worker_spec_at_with_cancellation(
        "/bin/sh",
        [
            OsString::from("-c"),
            OsString::from(script),
            OsString::from("pre-commit-parent-worker"),
            fifo.as_os_str().to_owned(),
        ],
        &root,
        ProcessBounds {
            runtime: Duration::from_secs(2),
            output_drain: Duration::from_millis(100),
            reap: Duration::from_millis(200),
            grace: Duration::from_millis(100),
        },
        parent.cancellation(),
    )
    .expect("declare parent-bound worker");
    let running = sill::spawn(spec).expect("launch parent-bound worker");
    let worker = running.id();
    let mut announced = String::new();
    File::open(&fifo)
        .expect("open worker readiness fifo")
        .read_to_string(&mut announced)
        .expect("read worker readiness fifo");
    let worker_tree = announced
        .split_whitespace()
        .map(|pid| pid.parse::<u32>().expect("worker pid is numeric"))
        .collect::<Vec<_>>();
    let [shell, descendant] = worker_tree.as_slice() else {
        panic!("worker announces shell and descendant: {announced:?}");
    };
    assert_eq!(*shell, worker, "Sill worker is the announced group leader");

    let socket = env::var(SOCKET).expect("dispatcher readiness address");
    let mut readiness = TcpStream::connect(socket).expect("connect dispatcher readiness");
    writeln!(readiness, "{} {worker} {descendant}", std::process::id())
        .expect("announce dispatcher tree");
    drop(readiness);

    let waited = running.wait().expect("cancel and reap parent-bound worker");
    assert!(
        matches!(waited.completion(), Completion::Cancelled(_)),
        "parent exit cancels worker: {:?}",
        waited.completion(),
    );
    fs::write(root.join("cancelled"), b"cancelled\n")
        .expect("record completed parent cancellation");
    fs::remove_file(fifo).expect("remove readiness fifo");
}

fn live_observer(pid: u32) -> ProcessExitObserver {
    match ProcessExitObserver::or_ended(pid).expect("arm process-exit observer") {
        observer @ ProcessExitObserver::Watching(_) => observer,
        ProcessExitObserver::Ended => panic!("process {pid} ended before interruption"),
    }
}

fn create_fifo(path: &Path) {
    let path = CString::new(path.as_os_str().as_bytes()).expect("fifo path has no NUL");
    // SAFETY: the path is valid for this call and mode grants only fixture-owner access.
    let result = unsafe { libc::mkfifo(path.as_ptr(), 0o600) };
    assert_eq!(
        result,
        0,
        "create worker readiness fifo: {}",
        io::Error::last_os_error(),
    );
}

fn initialize_repository(root: &Path) {
    require_git(root, &["init", "--quiet"]);
    require_git(root, &["config", "user.name", "Pre-commit Parent Fixture"]);
    require_git(
        root,
        &["config", "user.email", "pre-commit@example.invalid"],
    );
    fs::write(root.join("initial"), b"initial\n").expect("write initial fixture source");
    require_git(root, &["add", "initial"]);
    require_git(root, &["commit", "--quiet", "-m", "initial"]);
}

fn require_git(root: &Path, arguments: &[&str]) {
    let output = git(root, arguments);
    assert!(
        output.status.success(),
        "git {arguments:?}: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn git_stdout(root: &Path, arguments: &[&str]) -> String {
    let output = git(root, arguments);
    assert!(output.status.success(), "git {arguments:?} succeeds");
    String::from_utf8(output.stdout).expect("git output is UTF-8")
}

fn git(root: &Path, arguments: &[&str]) -> Output {
    Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("launch git fixture command")
}
