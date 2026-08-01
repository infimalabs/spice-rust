#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use sill::LockReport;
use wire::{Outcome, ReplyPosition};

const BUSY_EXIT: i32 = 71;
const TAKEN_EXIT: i32 = 72;
const FULL_EXIT: i32 = 73;
const EVENT_HORIZON: Duration = Duration::from_secs(10);

struct Scratch(PathBuf);

impl Scratch {
    fn repository(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-lock-cli-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("exclusive scratch repository is created");
        fs::create_dir(root.join(".git")).expect("fixture Git directory is created");
        fs::write(
            root.join("spice.toml"),
            format!(
                concat!(
                    "[lock]\n",
                    "busy = {}\n",
                    "taken = {}\n",
                    "full = {}\n",
                    "[lock.single]\n",
                    "shards = 1\n",
                    "[lock.pool]\n",
                    "shards = 2\n",
                ),
                BUSY_EXIT, TAKEN_EXIT, FULL_EXIT,
            ),
        )
        .expect("fixture lock configuration is written");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn lock_root(&self) -> PathBuf {
        fs::canonicalize(self.0.join(".git"))
            .expect("fixture Git directory canonicalizes")
            .join(".spice/locks")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Spoken {
    status: ExitStatus,
    out: Vec<u8>,
    err: Vec<u8>,
}

fn invoke(repository: &Path, arguments: &[&str]) -> Spoken {
    let output = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args(arguments)
        .current_dir(repository)
        .env_remove(steer::hook::INBOX)
        .output()
        .expect("spice lock launches");
    Spoken {
        status: output.status,
        out: output.stdout,
        err: output.stderr,
    }
}

#[test]
fn report_has_text_json_parity_and_never_materializes_lock_state() {
    let repository = Scratch::repository("report");
    let expected_root = repository.lock_root();

    let text = invoke(repository.path(), &["lock"]);
    assert_eq!(text.status.code(), Some(0));
    assert!(text.err.is_empty());
    let text_report: LockReport = wire::decode(&text.out).expect("text body is the lock report");
    assert!(!expected_root.exists(), "text reporting created lock state");

    let json = invoke(repository.path(), &["--json", "lock"]);
    assert_eq!(json.status.code(), Some(0));
    assert!(json.err.is_empty());
    let json_outcome: Outcome<LockReport> =
        wire::decode(&json.out).expect("JSON is the typed lock outcome");
    let Outcome::Reply(reply) = json_outcome else {
        panic!("lock report did not reply");
    };
    assert_eq!(reply.position, ReplyPosition::Direct {});
    assert_eq!(reply.body, text_report);
    assert_eq!(reply.body.root, expected_root.display().to_string());
    assert_eq!(reply.body.exits.busy, BUSY_EXIT);
    assert_eq!(reply.body.exits.taken, TAKEN_EXIT);
    assert_eq!(reply.body.exits.full, FULL_EXIT);
    assert_eq!(
        reply
            .body
            .pools
            .iter()
            .map(|pool| (pool.name.as_str(), pool.shards))
            .collect::<Vec<_>>(),
        [("pool", 2), ("single", 1)]
    );
    assert!(!expected_root.exists(), "JSON reporting created lock state");
}

#[test]
fn run_preserves_exact_stream_environment_working_directory_and_child_exit() {
    let repository = Scratch::repository("exact-child");
    let nested = repository.path().join("nested");
    fs::create_dir(&nested).expect("nested child working directory is created");
    let output = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args([
            "--json",
            "lock",
            "run",
            "pool",
            "--shard",
            "1",
            "--",
            "/bin/sh",
            "-c",
            "printf '%s|%s|%s' \"$LOCK_TEST_MARKER\" \"$1\" \"$PWD\"; printf 'err' >&2; exit 41",
            "lock-child",
            "argument with spaces",
        ])
        .current_dir(&nested)
        .env_remove(steer::hook::INBOX)
        .env("LOCK_TEST_MARKER", "exact")
        .output()
        .expect("lock-held child launches");

    assert_eq!(output.status.code(), Some(41));
    assert_eq!(
        output.stdout,
        format!(
            "exact|argument with spaces|{}",
            fs::canonicalize(&nested)
                .expect("fixture child directory canonicalizes")
                .display()
        )
        .as_bytes()
    );
    assert_eq!(output.stderr, b"err");
    assert!(repository.lock_root().join("pool/1").is_file());

    let retry = invoke(
        repository.path(),
        &["lock", "run", "pool", "--shard", "1", "--", "/usr/bin/true"],
    );
    assert_eq!(retry.status.code(), Some(0), "released shard reacquires");
    assert!(retry.out.is_empty());
    assert!(retry.err.is_empty());
}

#[test]
fn child_signal_and_launch_failures_map_to_shell_statuses() {
    let repository = Scratch::repository("child-status");

    let signalled = invoke(
        repository.path(),
        &[
            "lock",
            "run",
            "single",
            "--",
            "/bin/sh",
            "-c",
            "kill -TERM $$",
        ],
    );
    assert_eq!(signalled.status.code(), Some(143));
    assert!(signalled.out.is_empty());
    assert!(signalled.err.is_empty());

    let missing = invoke(
        repository.path(),
        &[
            "--json",
            "lock",
            "run",
            "single",
            "--",
            "/definitely/missing/spice-lock-child",
        ],
    );
    assert_eq!(missing.status.code(), Some(127));
    assert!(missing.out.is_empty());
    let outcome: Outcome<()> = wire::decode(&missing.err).expect("launch fault is typed JSON");
    assert!(matches!(
        outcome,
        Outcome::Fault(fault) if fault.reason == "lock::child-launch-failed"
    ));
}

#[test]
fn separate_processes_prove_busy_taken_full_and_release() {
    let repository = Scratch::repository("contention");
    let single = Holder::spawn(repository.path(), "single", 0, false);

    let taken = invoke(
        repository.path(),
        &[
            "--json",
            "lock",
            "run",
            "single",
            "--shard",
            "0",
            "--",
            "/usr/bin/true",
        ],
    );
    assert_refusal(&taken, TAKEN_EXIT, "lock::taken");

    let busy = invoke(
        repository.path(),
        &["lock", "run", "single", "--", "/usr/bin/true"],
    );
    assert_eq!(busy.status.code(), Some(BUSY_EXIT));
    assert!(busy.out.is_empty());
    assert_eq!(
        String::from_utf8(busy.err).expect("text refusal is UTF-8"),
        "spice: lock::busy; wait for the current holder to finish or select another configured shard\n"
    );

    let pool_zero = Holder::spawn(repository.path(), "pool", 0, false);
    let pool_one = Holder::spawn(repository.path(), "pool", 1, false);
    let full = invoke(
        repository.path(),
        &["--json", "lock", "run", "pool", "--", "/usr/bin/true"],
    );
    assert_refusal(&full, FULL_EXIT, "lock::full");

    single.release();
    pool_zero.release();
    pool_one.release();
    let retry = invoke(
        repository.path(),
        &["lock", "run", "single", "--", "/usr/bin/true"],
    );
    assert_eq!(retry.status.code(), Some(0));
}

#[test]
fn abandoned_parent_releases_possession_while_its_child_is_still_alive() {
    let repository = Scratch::repository("abandoned");
    let holder = Holder::spawn(repository.path(), "single", 0, false);
    let (mut child_stdin, output) = holder.kill_parent();

    let retry = invoke(
        repository.path(),
        &["lock", "run", "single", "--", "/usr/bin/true"],
    );
    assert_eq!(
        retry.status.code(),
        Some(0),
        "an exec child retained the abandoned parent's lock descriptor"
    );

    child_stdin
        .write_all(b"release\n")
        .expect("orphaned fixture child is released");
    drop(child_stdin);
    output.join().expect("orphan output reader joins");
}

#[test]
fn foreground_process_group_signal_reaches_child_and_releases_lock() {
    use std::os::unix::process::ExitStatusExt;

    let repository = Scratch::repository("group-signal");
    let holder = Holder::spawn(repository.path(), "single", 0, true);
    let status = holder.signal_group(15);
    assert_eq!(status.signal(), Some(15));

    let retry = invoke(
        repository.path(),
        &["lock", "run", "single", "--", "/usr/bin/true"],
    );
    assert_eq!(retry.status.code(), Some(0));
}

#[test]
fn refused_selection_and_configuration_never_materialize_state() {
    let repository = Scratch::repository("refused");
    for arguments in [
        vec!["lock", "run", "missing", "--", "/usr/bin/true"],
        vec!["lock", "run", "pool", "--shard", "2", "--", "/usr/bin/true"],
        vec!["lock", "run", "../outside", "--", "/usr/bin/true"],
    ] {
        let refused = invoke(repository.path(), &arguments);
        assert_eq!(refused.status.code(), Some(2));
        assert!(refused.out.is_empty());
        assert!(!String::from_utf8_lossy(&refused.err).contains("release::not-landed"));
    }
    assert!(!repository.lock_root().exists());

    fs::write(
        repository.path().join("spice.toml"),
        "[lock.pool]\nshards = 0\n",
    )
    .expect("malformed configuration is installed");
    let malformed = invoke(repository.path(), &["--json", "lock"]);
    assert_eq!(malformed.status.code(), Some(2));
    let outcome: Outcome<()> =
        wire::decode(&malformed.err).expect("configuration refusal is typed JSON");
    assert!(matches!(
        outcome,
        Outcome::Refusal(refusal) if refusal.reason == "lock::configuration-refused"
    ));
    assert!(!repository.lock_root().exists());
}

fn assert_refusal(spoken: &Spoken, code: i32, reason: &str) {
    assert_eq!(spoken.status.code(), Some(code));
    assert!(spoken.out.is_empty());
    assert!(!String::from_utf8_lossy(&spoken.err).contains("release::not-landed"));
    let outcome: Outcome<()> = wire::decode(&spoken.err).expect("contention is typed JSON");
    assert!(matches!(
        outcome,
        Outcome::Refusal(refusal) if refusal.reason == reason
    ));
}

struct Holder {
    child: Child,
    stdin: Option<ChildStdin>,
    output: thread::JoinHandle<()>,
}

impl Holder {
    fn spawn(repository: &Path, name: &str, shard: u32, own_group: bool) -> Self {
        use std::os::unix::process::CommandExt;

        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        command
            .args([
                "lock",
                "run",
                name,
                "--shard",
                &shard.to_string(),
                "--",
                "/bin/sh",
                "-c",
                "printf 'ready\\n'; IFS= read -r release",
            ])
            .current_dir(repository)
            .env_remove(steer::hook::INBOX)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if own_group {
            command.process_group(0);
        }
        let mut child = command.spawn().expect("lock holder launches");
        let stdin = child.stdin.take().expect("holder stdin is piped");
        let stdout = child.stdout.take().expect("holder stdout is piped");
        let (sender, receiver) = mpsc::channel();
        let output = thread::spawn(move || {
            let mut lines = BufReader::new(stdout).lines();
            let first = lines.next().transpose();
            let _ = sender.send(first);
            for line in lines {
                line.expect("holder stdout remains UTF-8");
            }
        });
        let ready = receiver
            .recv_timeout(EVENT_HORIZON)
            .expect("holder child reports readiness")
            .expect("holder readiness is readable");
        assert_eq!(ready.as_deref(), Some("ready"));
        Self {
            child,
            stdin: Some(stdin),
            output,
        }
    }

    fn release(mut self) {
        self.stdin
            .take()
            .expect("live holder owns stdin")
            .write_all(b"release\n")
            .expect("holder child is released");
        let status = self.child.wait().expect("holder wrapper is reaped");
        assert_eq!(status.code(), Some(0));
        self.output.join().expect("holder output reader joins");
    }

    fn kill_parent(mut self) -> (ChildStdin, thread::JoinHandle<()>) {
        self.child.kill().expect("holder wrapper is killed");
        let status = self.child.wait().expect("killed holder is reaped");
        assert!(!status.success());
        (
            self.stdin.take().expect("orphan child retains piped stdin"),
            self.output,
        )
    }

    fn signal_group(mut self, signal: i32) -> ExitStatus {
        let process_group = -i32::try_from(self.child.id()).expect("child PID fits i32");
        let result = unsafe { kill(process_group, signal) };
        assert_eq!(result, 0, "fixture process group accepts signal");
        self.stdin.take();
        let status = self.child.wait().expect("signalled wrapper is reaped");
        self.output.join().expect("holder output reader joins");
        status
    }
}

unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}
