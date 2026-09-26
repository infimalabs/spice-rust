#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use sill::{
    Acquisition, Contention, LockAttempt, LockAvailability, LockConfig, LockFile, LockName,
    LockReport, LockTopology, Possession,
};
use wire::Outcome;

#[path = "support/file_change.rs"]
mod file_change;
#[path = "support/git_repository.rs"]
mod git_repository;
#[path = "support/process_exit.rs"]
mod process_exit;
use file_change::FileChangeObserver;
use process_exit::ProcessExitObserver;

const BUSY_EXIT: i32 = 70;
const TAKEN_EXIT: i32 = 71;
const FULL_EXIT: i32 = 72;
const CLOSE_FDS_READY: &str = "LOCK-CLOSE-FDS-READY";
const CLOSE_FDS_PID_FILE: &str = "abrupt-child-pid";
const ACQUIRE_FIXTURE: &str = "unreaped_wrapper_synchronizes_first_topology_acquisition_fixture";
const REPORT_FIXTURE: &str = "terminal_zombie_owner_never_exposes_held_availability_fixture";

#[test]
#[ignore = "subprocess fixture for the abrupt wrapper death test"]
fn close_all_non_stdio_descriptors_and_wait() {
    let pid_path = std::env::current_dir()
        .expect("close-fds helper reads its working directory")
        .join(CLOSE_FDS_PID_FILE);
    fs::write(pid_path, std::process::id().to_string()).expect("close-fds helper records its PID");
    close_non_stdio_descriptors();
    println!("{CLOSE_FDS_READY}");
    std::io::stdout()
        .flush()
        .expect("close-fds helper flushes readiness");
    std::io::copy(&mut std::io::stdin(), &mut std::io::sink())
        .expect("close-fds helper waits for stdin closure");
}

#[cfg(target_os = "linux")]
fn close_non_stdio_descriptors() {
    // SAFETY: the helper deliberately closes its complete non-stdio descriptor range.
    if unsafe { libc::syscall(libc::SYS_close_range, 3, libc::c_uint::MAX, 0) } == 0 {
        return;
    }
    close_descriptor_snapshot();
}

#[cfg(target_os = "macos")]
fn close_non_stdio_descriptors() {
    close_descriptor_snapshot();
}

fn close_descriptor_snapshot() {
    let descriptors = fs::read_dir("/dev/fd")
        .expect("close-fds helper reads its descriptor directory")
        .map(|entry| {
            entry
                .expect("close-fds helper reads a descriptor entry")
                .file_name()
                .to_string_lossy()
                .parse::<libc::c_int>()
                .expect("descriptor directory entries are numeric")
        })
        .collect::<Vec<_>>();
    for descriptor in descriptors.into_iter().filter(|descriptor| *descriptor > 2) {
        // SAFETY: the helper deliberately closes every inherited non-stdio descriptor.
        unsafe {
            libc::close(descriptor);
        }
    }
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-lock-cli-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("lock fixture root is exclusively created");
        git_repository::initialize(&root);
        let root = fs::canonicalize(root).expect("canonicalize lock fixture root");
        fs::write(
            root.join("spice-rust.toml"),
            concat!(
                "[lock]\n",
                "busy = 70\n",
                "taken = 71\n",
                "full = 72\n",
                "\n[lock.solo]\n",
                "path = \"locks/solo.lock\"\n",
                "\n[lock.pool]\n",
                "directory = \"locks/pool\"\n",
                "shards = 2\n",
            ),
        )
        .expect("write lock fixture configuration");
        Self { root }
    }

    fn invoke(&self, argv: &[&str]) -> Spoken {
        let output = Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(argv)
            .current_dir(&self.root)
            .output()
            .expect("spice lock process launches");
        Spoken {
            code: output.status.code(),
            out: output.stdout,
            err: output.stderr,
        }
    }

    fn hold(&self, name: &str, shard: Option<u32>) -> HeldProcess {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        command
            .arg("lock")
            .arg("run")
            .arg(name)
            .current_dir(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(shard) = shard {
            command.arg("--shard").arg(shard.to_string());
        }
        let mut child = command
            .args([
                "--",
                "/bin/sh",
                "-c",
                "printf 'READY\\n'; /bin/cat >/dev/null",
            ])
            .spawn()
            .expect("spawn held lock process");
        let stdout = child.stdout.take().expect("held process stdout is piped");
        let mut reader = BufReader::new(stdout);
        let mut ready = String::new();
        reader
            .read_line(&mut ready)
            .expect("held child writes readiness line");
        assert_eq!(ready, "READY\n");
        HeldProcess {
            child,
            stdout: reader,
        }
    }

    fn lock_root(&self) -> PathBuf {
        self.root.join("locks")
    }

    fn topology(&self) -> LockTopology {
        let config: LockConfig = toml::from_str(concat!(
            "busy = 70\n",
            "taken = 71\n",
            "full = 72\n",
            "[solo]\n",
            "path = 'locks/solo.lock'\n",
        ))
        .expect("fixture lock topology is admitted");
        LockTopology::new(&self.root, &config)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct HeldProcess {
    child: Child,
    stdout: BufReader<std::process::ChildStdout>,
}

impl HeldProcess {
    fn release(mut self) {
        self.child
            .stdin
            .take()
            .expect("held process stdin is piped")
            .write_all(b"release\n")
            .expect("release held child");
        let mut trailing = String::new();
        self.stdout
            .read_to_string(&mut trailing)
            .expect("drain held child stdout");
        let output = self
            .child
            .wait_with_output()
            .expect("held lock process exits");
        assert!(output.status.success(), "held lock process: {output:?}");
        assert!(output.stderr.is_empty(), "held lock stderr: {output:?}");
    }
}

struct AbruptProtected {
    wrapper: Child,
    wrapper_pid: u32,
    release: Option<std::process::ChildStdin>,
    output: BufReader<std::process::ChildStdout>,
    child_pid: libc::pid_t,
    reaped: bool,
}

impl AbruptProtected {
    fn start(fixture: &Fixture) -> Self {
        let helper = std::env::current_exe().expect("locate lock CLI test executable");
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        command
            .args(["lock", "run", "solo", "--"])
            .arg(helper)
            .args([
                "--exact",
                "close_all_non_stdio_descriptors_and_wait",
                "--ignored",
                "--nocapture",
            ])
            .current_dir(&fixture.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut wrapper = command.spawn().expect("abrupt wrapper launches");
        let wrapper_pid = wrapper.id();
        let release = wrapper.stdin.take().expect("wrapper stdin is piped");
        let mut output = BufReader::new(wrapper.stdout.take().expect("wrapper stdout is piped"));
        wait_for_close_fds_ready(&mut output);
        let child_pid = fs::read_to_string(fixture.root.join(CLOSE_FDS_PID_FILE))
            .expect("child PID reads")
            .parse()
            .expect("child PID parses");

        // The child's announcement is enough to kill on. It arrives before the
        // wrapper could have finished any record -- `spawn` returns only once
        // that `exec` has happened -- and that is precisely what this fixture
        // wants to hold the wrapper to: the guardian, not the wrapper, is what
        // names the group, so a wrapper killed here has already handed over
        // everything a later reader needs. Waiting for the record instead would
        // be waiting on the wrapper, and would hide a regression that put the
        // publication back on the side that can die.
        let exited = ProcessExitObserver::new(wrapper_pid);
        // SAFETY: SIGKILL terminates only the wrapper retained by this fixture.
        assert_eq!(
            unsafe { libc::kill(wrapper_pid as libc::pid_t, libc::SIGKILL) },
            0
        );
        exited.wait();
        assert_process_zombie(wrapper_pid);
        await_topology_published_without_its_wrapper(fixture);

        Self {
            wrapper,
            wrapper_pid,
            release: Some(release),
            output,
            child_pid,
            reaped: false,
        }
    }

    fn terminate_group(&mut self) {
        drop(self.release.take());
        let mut trailing = String::new();
        self.output
            .read_to_string(&mut trailing)
            .expect("protected process group closes stdout on exit");
        assert!(
            trailing.contains("test result: ok"),
            "abrupt child output: {trailing:?}"
        );
        assert_process_terminal(self.child_pid);
        assert_process_zombie(self.wrapper_pid);
    }

    fn reap(mut self) {
        assert!(
            !self
                .wrapper
                .wait()
                .expect("wrapper reaps after assertions")
                .success()
        );
        self.reaped = true;
    }

    /// Every input the acquisition decision reads, spelled for a failure message.
    ///
    /// A refusal reported only as contention names none of the evidence it was
    /// decided from, so a rare failure would arrive with nothing to read.
    fn synchronization_evidence(&self, fixture: &Fixture) -> String {
        let path = fixture.lock_root().join("solo.lock");
        let record = fs::read(&path).unwrap_or_else(|error| error.to_string().into_bytes());
        format!(
            "owner record {:?}; protected group {}; protected child {}; wrapper {}",
            String::from_utf8_lossy(&record),
            signal_zero(-self.child_pid),
            signal_zero(self.child_pid),
            signal_zero(self.wrapper_pid as libc::pid_t),
        )
    }
}

/// Block until the record names the group, with nothing alive that could name it.
///
/// This runs after the wrapper is a confirmed zombie, so it is the one witness
/// that publication does not belong to the wrapper: the group appearing here is
/// the guardian's writing, because there is no longer anybody else. A slot the
/// wrapper reserved and nobody filled would hang here rather than pass, which
/// is what makes the wait an assertion rather than a courtesy.
///
/// The watch is registered before the first read, so a record written between
/// the two is delivered to the wait rather than lost by it, and the loop
/// re-reads on every write until the group is there to be read.
fn await_topology_published_without_its_wrapper(fixture: &Fixture) {
    let path = fixture.lock_root().join("solo.lock");
    let written = FileChangeObserver::new(&path);
    loop {
        let record = fs::read(&path).expect("owner record reads");
        let record = String::from_utf8_lossy(&record);
        if published_group(&record).is_some() {
            return;
        }
        assert!(
            written.wait(),
            "no group was published once the wrapper was gone: {record}"
        );
    }
}

/// The group a record names, if the slot in it has been filled in.
///
/// A reserved slot and a published one are the same width and the same field,
/// so the field being present says nothing; only its contents do.
fn published_group(record: &str) -> Option<u32> {
    let (_, slot) = record.split_once(r#""protected_group":""#)?;
    let (slot, _) = slot.split_once('"')?;
    slot.parse().ok().filter(|group| *group != 0)
}

/// What signal zero says about one kernel identity, named rather than numbered.
fn signal_zero(pid: libc::pid_t) -> String {
    // SAFETY: signal zero only probes whether the identity remains present.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return format!("{pid} present");
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::ESRCH) => format!("{pid} absent"),
        other => format!("{pid} unreadable ({other:?})"),
    }
}

impl Drop for AbruptProtected {
    fn drop(&mut self) {
        drop(self.release.take());
        if !self.reaped {
            // SAFETY: cleanup targets only this fixture's wrapper.
            unsafe {
                libc::kill(self.wrapper_pid as libc::pid_t, libc::SIGKILL);
            }
            let _ = self.wrapper.wait();
        }
    }
}

fn wait_for_close_fds_ready(output: &mut impl BufRead) {
    loop {
        let mut ready = String::new();
        assert_ne!(
            output
                .read_line(&mut ready)
                .expect("abrupt child reaches execution"),
            0,
            "close-fds helper exited before readiness"
        );
        if ready.contains(CLOSE_FDS_READY) {
            return;
        }
    }
}

fn retain_unreaped_children() {
    // SAFETY: the isolated fixture has no concurrent child lifecycle and installs
    // ordinary waitable-child semantics before it spawns the wrapper.
    let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
    action.sa_sigaction = libc::SIG_DFL;
    // SAFETY: the signal mask belongs exclusively to this local action.
    assert_eq!(unsafe { libc::sigemptyset(&mut action.sa_mask) }, 0);
    // SAFETY: this process-local fixture intentionally clears SA_NOCLDWAIT.
    assert_eq!(
        unsafe { libc::sigaction(libc::SIGCHLD, &action, std::ptr::null_mut()) },
        0
    );
}

fn run_isolated_lifecycle_fixture(name: &str) {
    let output = Command::new(std::env::current_exe().expect("locate lock CLI test executable"))
        .args(["--exact", name, "--ignored", "--nocapture"])
        .output()
        .expect("launch isolated lock lifecycle fixture");
    assert!(
        output.status.success(),
        "lock lifecycle fixture: {output:?}"
    );
}

fn assert_process_terminal(pid: libc::pid_t) {
    // SAFETY: signal zero only probes whether the process remains present.
    if unsafe { libc::kill(pid, 0) } == -1 {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    } else {
        assert_process_zombie(pid as u32);
    }
}

fn assert_process_zombie(pid: u32) {
    #[cfg(target_os = "linux")]
    assert_linux_process_zombie(pid);
    #[cfg(target_os = "macos")]
    assert_macos_process_zombie(pid);
}

struct Spoken {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

#[test]
fn status_is_a_stable_read_only_direct_report_in_both_forms() {
    let fixture = Fixture::new("status");
    let text = fixture.invoke(&["lock"]);
    let json = fixture.invoke(&["lock", "--json"]);

    assert_eq!(text.code, Some(0));
    assert!(text.err.is_empty());
    let text: LockReport = wire::decode(&text.out).expect("text body is the typed report");
    let json: Outcome<LockReport> = wire::decode(&json.out).expect("JSON is a typed outcome");
    let Outcome::Reply(json) = json else {
        panic!("lock status must reply: {json:?}");
    };
    assert_eq!(json.position, wire::ReplyPosition::Direct {});
    assert_eq!(text, json.body);
    assert_eq!(text.root, fixture.root.display().to_string());
    assert_eq!(text.exits.busy, BUSY_EXIT);
    assert_eq!(text.exits.taken, TAKEN_EXIT);
    assert_eq!(text.exits.full, FULL_EXIT);
    assert_eq!(text.pools.iter().map(|pool| pool.shards).sum::<u32>(), 3);
    assert!(
        text.pools
            .iter()
            .flat_map(|pool| &pool.entries)
            .all(|entry| { entry.availability == LockAvailability::Available })
    );
    assert!(
        !fixture.lock_root().exists(),
        "a status read does not create lock state"
    );
}

#[test]
fn run_preserves_exact_argv_environment_cwd_and_child_status() {
    let fixture = Fixture::new("child");
    let output = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args([
            "lock",
            "run",
            "solo",
            "--",
            "/bin/sh",
            "-c",
            "printf '%s|%s|%s' \"$LOCK_PROBE\" \"$1\" \"$PWD\"",
            "child-zero",
            "--child-flag",
        ])
        .current_dir(&fixture.root)
        .env("LOCK_PROBE", "exact")
        .output()
        .expect("lock child process launches");
    assert!(output.status.success(), "lock child: {output:?}");
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).expect("child output is UTF-8"),
        format!("exact|--child-flag|{}", fixture.root.display())
    );
    assert_eq!(
        fs::read(fixture.lock_root().join("solo.lock")).expect("read released lock metadata"),
        b""
    );

    let exited = fixture.invoke(&["lock", "run", "solo", "--", "/bin/sh", "-c", "exit 23"]);
    assert_eq!(exited.code, Some(23));
    assert!(exited.out.is_empty());
    assert!(exited.err.is_empty());

    let missing = fixture.invoke(&[
        "--json",
        "lock",
        "run",
        "solo",
        "--",
        "/definitely/missing/spice-lock-child",
    ]);
    assert_eq!(missing.code, Some(127));
    let outcome: Outcome<()> = wire::decode(&missing.err).expect("launch fault decodes");
    assert!(matches!(
        outcome,
        Outcome::Fault(fault) if fault.reason == "lock::child-launch-failed"
    ));
    let reacquired = fixture.invoke(&["lock", "run", "solo", "--", "/usr/bin/true"]);
    assert_eq!(reacquired.code, Some(0));
}

#[test]
fn explicit_absolute_file_and_pool_locations_are_reported_before_creation() {
    let fixture = Fixture::new("absolute");
    let file = fixture.root.join("absolute-state/single.lock");
    let directory = fixture.root.join("absolute-state/pool");
    fs::write(
        fixture.root.join("spice-rust.toml"),
        format!(
            concat!(
                "[lock]\nbusy = 70\ntaken = 71\nfull = 72\n",
                "[lock.solo]\npath = {:?}\n",
                "[lock.pool]\ndirectory = {:?}\nshards = 2\n",
            ),
            file.display().to_string(),
            directory.display().to_string(),
        ),
    )
    .expect("absolute lock configuration writes");

    let report = reply_body(fixture.invoke(&["lock", "--json"]));
    let solo = report
        .pools
        .iter()
        .find(|pool| pool.name == "solo")
        .expect("single file is reported");
    let pool = report
        .pools
        .iter()
        .find(|pool| pool.name == "pool")
        .expect("pool is reported");
    assert_eq!(solo.kind, sill::LockTargetKind::File);
    assert_eq!(solo.entries[0].path, file.display().to_string());
    assert_eq!(pool.kind, sill::LockTargetKind::Pool);
    assert_eq!(
        pool.entries[0].path,
        directory.join("0.lock").display().to_string()
    );
    assert!(!file.exists(), "status created an absolute lock file");
    assert!(!directory.exists(), "status created an absolute pool");

    assert_eq!(
        fixture
            .invoke(&["lock", "run", "solo", "--", "/usr/bin/true"])
            .code,
        Some(0)
    );
    assert_eq!(
        fixture
            .invoke(&["lock", "run", "pool", "--shard", "1", "--", "/usr/bin/true",])
            .code,
        Some(0)
    );
    assert!(file.is_file());
    assert!(directory.join("1.lock").is_file());
}

#[test]
fn all_three_contention_classes_and_held_status_are_observable() {
    let fixture = Fixture::new("contention");
    let solo = fixture.hold("solo", None);

    let status = fixture.invoke(&["lock", "--json"]);
    let report = reply_body(status);
    assert_eq!(availability(&report, "solo", 0), LockAvailability::Held);

    assert_contention(
        fixture.invoke(&["--json", "lock", "run", "solo", "--", "/usr/bin/true"]),
        BUSY_EXIT,
        "lock::busy",
    );

    let pool_zero = fixture.hold("pool", Some(0));
    assert_contention(
        fixture.invoke(&[
            "--json",
            "lock",
            "run",
            "pool",
            "--shard",
            "0",
            "--",
            "/usr/bin/true",
        ]),
        TAKEN_EXIT,
        "lock::taken",
    );
    let pool_one = fixture.hold("pool", Some(1));
    assert_contention(
        fixture.invoke(&["--json", "lock", "run", "pool", "--", "/usr/bin/true"]),
        FULL_EXIT,
        "lock::full",
    );

    pool_one.release();
    pool_zero.release();
    solo.release();
    let report = reply_body(fixture.invoke(&["lock", "--json"]));
    assert!(
        report
            .pools
            .iter()
            .flat_map(|pool| &pool.entries)
            .all(|entry| { entry.availability == LockAvailability::Available })
    );
}

#[test]
fn a_signal_to_the_wrapper_terminates_the_child_group_and_releases_the_lock() {
    let fixture = Fixture::new("signal");
    let mut child = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args([
            "lock",
            "run",
            "solo",
            "--",
            "/bin/sh",
            "-c",
            "printf 'READY\\n'; exec /usr/bin/tail -f /dev/null",
        ])
        .current_dir(&fixture.root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn signal fixture");
    let stdout = child.stdout.take().expect("signal fixture stdout is piped");
    let mut reader = BufReader::new(stdout);
    let mut ready = String::new();
    reader
        .read_line(&mut ready)
        .expect("signal fixture reaches child execution");
    assert_eq!(ready, "READY\n");

    let killed = Command::new("/bin/kill")
        .args(["-TERM", &child.id().to_string()])
        .output()
        .expect("signal lock wrapper");
    assert!(killed.status.success(), "kill wrapper: {killed:?}");
    let output = child.wait_with_output().expect("signalled wrapper exits");
    assert_eq!(output.status.code(), Some(143));
    assert!(output.stderr.is_empty(), "signal fixture: {output:?}");
    assert_eq!(
        fs::read(fixture.lock_root().join("solo.lock")).expect("read lock after signal"),
        b""
    );

    let reacquired = fixture.invoke(&["lock", "run", "solo", "--", "/usr/bin/true"]);
    assert_eq!(reacquired.code, Some(0));
}

#[test]
fn unreaped_wrapper_synchronizes_first_topology_acquisition_after_group_completion() {
    run_isolated_lifecycle_fixture(ACQUIRE_FIXTURE);
}

#[test]
#[ignore = "subprocess fixture isolates unreaped-child signal policy"]
fn unreaped_wrapper_synchronizes_first_topology_acquisition_fixture() {
    retain_unreaped_children();
    let fixture = Fixture::new("abrupt-acquire");
    let topology = fixture.topology();
    let mut protected = AbruptProtected::start(&fixture);
    // The wrapper is already dead, so this record is the whole of what the
    // acquisitions below get to reason from. Read against the same evidence at
    // the moment of a failure, it separates a topology that was never published
    // from one that was published and then stopped answering.
    let published = protected.synchronization_evidence(&fixture);

    assert!(matches!(
        topology.try_acquire("solo", None).expect("live group contention is observable"),
        LockAttempt::Contended { kind: Contention::Busy, code } if code.get() == BUSY_EXIT
    ));
    protected.terminate_group();

    let acquired = topology
        .try_acquire("solo", None)
        .expect("terminal group completion synchronizes");
    let LockAttempt::Acquired(held) = acquired else {
        panic!(
            "first terminal-group acquisition remained contended: {}\n  as published: {published}",
            protected.synchronization_evidence(&fixture)
        );
    };
    assert_eq!(held.shard(), 0);
    drop(held);
    protected.reap();
}

#[test]
fn terminal_zombie_owner_never_exposes_held_availability() {
    run_isolated_lifecycle_fixture(REPORT_FIXTURE);
}

#[test]
#[ignore = "subprocess fixture isolates unreaped-child signal policy"]
fn terminal_zombie_owner_never_exposes_held_availability_fixture() {
    retain_unreaped_children();
    let fixture = Fixture::new("abrupt-report");
    let topology = fixture.topology();
    let mut protected = AbruptProtected::start(&fixture);
    protected.terminate_group();

    let report = topology
        .report()
        .expect("terminal zombie owner synchronizes");
    let entry = &report
        .pools
        .iter()
        .find(|pool| pool.name == "solo")
        .expect("single lock is reported")
        .entries[0];
    assert_eq!(entry.availability, LockAvailability::Available);
    assert_eq!(
        entry.owner.as_ref().map(|owner| owner.pid),
        Some(protected.wrapper_pid)
    );
    assert!(entry.owner_stale);
    protected.reap();
}

#[test]
fn a_surviving_group_member_retains_possession_after_the_leader_exits() {
    let fixture = Fixture::new("descendant");
    let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
    command
        .args([
            "lock",
            "run",
            "solo",
            "--",
            "/bin/sh",
            "-c",
            "exec 3<&0; /bin/cat <&3 & printf 'READY\\n'",
        ])
        .current_dir(&fixture.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut wrapper = command.spawn().expect("descendant wrapper launches");
    let release = wrapper.stdin.take().expect("wrapper stdin is piped");
    let mut output = BufReader::new(wrapper.stdout.take().expect("wrapper stdout is piped"));
    let mut ready = String::new();
    output
        .read_line(&mut ready)
        .expect("group leader reaches execution");
    assert_eq!(ready, "READY\n");
    assert!(
        wrapper
            .wait()
            .expect("wrapper exits with its leader")
            .success()
    );

    let report = reply_body(fixture.invoke(&["lock", "--json"]));
    let entry = &report
        .pools
        .iter()
        .find(|pool| pool.name == "solo")
        .expect("single lock is reported")
        .entries[0];
    assert_eq!(entry.availability, LockAvailability::Held);
    assert!(entry.owner.is_none());
    assert_contention(
        fixture.invoke(&["--json", "lock", "run", "solo", "--", "/usr/bin/true"]),
        BUSY_EXIT,
        "lock::busy",
    );

    drop(release);
    let mut trailing = String::new();
    output
        .read_to_string(&mut trailing)
        .expect("surviving group member closes stdout on exit");
    assert!(trailing.is_empty(), "group member output: {trailing:?}");
    assert_eq!(
        fixture
            .invoke(&["lock", "run", "solo", "--", "/usr/bin/true"])
            .code,
        Some(0)
    );
}

#[test]
fn malformed_names_shards_and_configuration_refuse_without_running_a_child() {
    let fixture = Fixture::new("refusals");
    for (argv, reason) in [
        (
            &["--json", "lock", "run", "../escape", "--", "/usr/bin/true"][..],
            "lock::name-refused",
        ),
        (
            &[
                "--json",
                "lock",
                "run",
                "pool",
                "--shard",
                "2",
                "--",
                "/usr/bin/true",
            ][..],
            "lock::shard-refused",
        ),
        (
            &["--json", "lock", "run", "absent", "--", "/usr/bin/true"][..],
            "lock::unknown",
        ),
    ] {
        let spoken = fixture.invoke(argv);
        assert_eq!(spoken.code, Some(2));
        let outcome: Outcome<()> = wire::decode(&spoken.err).expect("typed refusal decodes");
        let Outcome::Refusal(refusal) = outcome else {
            panic!("invalid lock request must refuse: {outcome:?}");
        };
        assert_eq!(refusal.reason(), reason);
    }
    assert!(!fixture.lock_root().exists());

    for document in [
        "[lock.solo]\ndirectory = 'locks/solo'\nshards = 0\n",
        "[lock.solo]\npath = ''\n",
        "[lock.solo]\npath = '../outside.lock'\n",
        "[lock.solo]\npath = 'locks/solo.lock'\ndirectory = 'locks/solo'\n",
        "[lock.solo]\nshards = 1\n",
    ] {
        fs::write(fixture.root.join("spice-rust.toml"), document)
            .expect("write invalid lock configuration");
        let invalid = fixture.invoke(&["lock", "--json"]);
        assert_eq!(invalid.code, Some(2));
        let invalid: Outcome<()> = wire::decode(&invalid.err).expect("config refusal decodes");
        let Outcome::Refusal(invalid) = invalid else {
            panic!("invalid config must refuse: {invalid:?}");
        };
        assert_eq!(invalid.reason(), "lock::configuration-refused");
        assert!(!fixture.root.join("outside.lock").exists());
    }
}

fn reply_body(spoken: Spoken) -> LockReport {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    let outcome: Outcome<LockReport> = wire::decode(&spoken.out).expect("typed reply decodes");
    let Outcome::Reply(reply) = outcome else {
        panic!("lock status must reply: {outcome:?}");
    };
    reply.body
}

fn availability(report: &LockReport, name: &str, shard: u32) -> LockAvailability {
    report
        .pools
        .iter()
        .find(|pool| pool.name == name)
        .expect("configured lock pool is reported")
        .entries
        .iter()
        .find(|entry| entry.shard == shard)
        .expect("configured shard is reported")
        .availability
}

fn assert_contention(spoken: Spoken, code: i32, reason: &str) {
    assert_eq!(spoken.code, Some(code));

    assert!(spoken.out.is_empty());
    let outcome: Outcome<()> = wire::decode(&spoken.err).expect("contention refusal decodes");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("lock contention must refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason(), reason);
}

#[test]
fn final_and_intermediate_symlinks_never_escape_the_configured_topology() {
    use std::os::unix::fs::symlink;

    let final_link = Fixture::new("final-symlink");
    let final_external = final_link.root.join("external-final");
    fs::write(&final_external, b"external final lock").expect("external lock writes");
    let final_hold = hold_external(&final_external, "external-final");
    fs::create_dir_all(final_link.lock_root()).expect("final-link parent creates");
    symlink(&final_external, final_link.lock_root().join("solo.lock"))
        .expect("final lock symlink creates");
    assert_symlink_refusal(&final_link, "solo", &final_hold);

    let intermediate = Fixture::new("intermediate-symlink");
    let external_pool = intermediate.root.join("external-pool");
    fs::create_dir(&external_pool).expect("external pool creates");
    let external_shard = external_pool.join("0.lock");
    fs::write(&external_shard, b"external intermediate lock").expect("external shard writes");
    let intermediate_hold = hold_external(&external_shard, "external-intermediate");
    fs::create_dir_all(intermediate.lock_root()).expect("lock root creates");
    symlink(&external_pool, intermediate.lock_root().join("pool"))
        .expect("intermediate pool symlink creates");
    assert_symlink_refusal(&intermediate, "pool", &intermediate_hold);
}

fn hold_external(path: &Path, name: &str) -> Possession {
    let lock = LockFile::new(
        LockName::new(name.to_owned()).expect("external lock name is valid"),
        path,
    );
    match lock.try_acquire().expect("external lock is observable") {
        Acquisition::Acquired(possession) => possession,
        Acquisition::Contended => panic!("fresh external lock was contended"),
    }
}

fn assert_symlink_refusal(repository: &Fixture, name: &str, external: &Possession) {
    let child_marker = repository.root.join("child-launched");
    let child_marker = child_marker.to_str().expect("fixture path is UTF-8");
    let before = tree(&repository.root);

    assert_state_fault(&repository.invoke(&["--json", "lock"]));
    let mut arguments = vec!["--json", "lock", "run", name];
    if name == "pool" {
        arguments.extend(["--shard", "0"]);
    }
    arguments.extend([
        "--",
        "/bin/sh",
        "-c",
        "printf launched > \"$1\"",
        "lock-child",
        child_marker,
    ]);
    assert_state_fault(&repository.invoke(&arguments));

    assert!(
        !Path::new(child_marker).exists(),
        "a refused symlink path launched its child"
    );
    assert_eq!(tree(&repository.root), before, "a refusal changed state");
    let contender = external
        .lock()
        .try_acquire()
        .expect("external lock remains observable");
    assert!(matches!(contender, Acquisition::Contended));
}

fn assert_state_fault(spoken: &Spoken) {
    assert_eq!(spoken.code, Some(1));
    assert!(spoken.out.is_empty());
    let outcome: Outcome<()> = wire::decode(&spoken.err).expect("state fault is typed JSON");
    assert!(matches!(
        outcome,
        Outcome::Fault(fault) if fault.reason == "lock::state-unavailable"
    ));
}

fn tree(root: &Path) -> BTreeMap<String, (u32, String, Vec<u8>)> {
    let mut state = BTreeMap::new();
    walk_tree(root, root, &mut state);
    state
}

fn walk_tree(root: &Path, directory: &Path, state: &mut BTreeMap<String, (u32, String, Vec<u8>)>) {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;

    let mut entries = fs::read_dir(directory)
        .expect("snapshot directory reads")
        .collect::<Result<Vec<_>, _>>()
        .expect("snapshot entries read");
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .expect("snapshot path is rooted")
            .to_string_lossy()
            .into_owned();
        let metadata = fs::symlink_metadata(&path).expect("snapshot metadata reads");
        let mode = metadata.permissions().mode() & 0o7777;
        if metadata.is_dir() {
            state.insert(relative, (mode, "directory".to_owned(), Vec::new()));
            walk_tree(root, &path, state);
        } else if metadata.is_file() {
            state.insert(
                relative,
                (mode, "file".to_owned(), fs::read(path).expect("file reads")),
            );
        } else if metadata.file_type().is_symlink() {
            state.insert(
                relative,
                (
                    mode,
                    "symlink".to_owned(),
                    fs::read_link(path)
                        .expect("symlink reads")
                        .as_os_str()
                        .as_bytes()
                        .to_vec(),
                ),
            );
        } else {
            panic!("snapshot encountered an unsupported file type at {relative}");
        }
    }
}

#[cfg(target_os = "linux")]
fn assert_linux_process_zombie(pid: u32) {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).expect("zombie stat reads");
    let (_, fields) = stat.rsplit_once(") ").expect("zombie stat fields split");
    assert_eq!(fields.as_bytes().first(), Some(&b'Z'));
}

#[cfg(target_os = "macos")]
fn assert_macos_process_zombie(pid: u32) {
    let mut information = std::mem::MaybeUninit::<libc::proc_bsdshortinfo>::uninit();
    let size = libc::c_int::try_from(size_of::<libc::proc_bsdshortinfo>())
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
    assert_eq!(bytes, 0);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}
