//! Detached and foreground launcher contract cases.

use super::*;

#[cfg(unix)]
struct DetachedResidentGuard<'a> {
    fixture: &'a Fixture,
    process: DetachedProcess,
    armed: bool,
}

#[cfg(unix)]
impl Drop for DetachedResidentGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = invoke(self.fixture, "down");
            self.process.terminate_for_cleanup();
        }
    }
}

#[cfg(unix)]
#[derive(Clone, Copy)]
struct DetachedProcess {
    pid: u32,
    group: i32,
}

#[cfg(unix)]
impl DetachedProcess {
    fn read(fixture: &Fixture) -> Option<Self> {
        let holder = fs::read_to_string(fixture.resident()).ok()?;
        let pid = holder
            .strip_prefix("pid=")?
            .split_once(';')?
            .0
            .parse::<u32>()
            .ok()?;
        Some(Self {
            pid,
            group: i32::try_from(pid).ok()?,
        })
    }

    fn wait_for_holder(fixture: &Fixture) -> Self {
        let deadline = Monotonic::now() + Duration::from_secs(5);
        loop {
            if let Some(process) = Self::read(fixture) {
                return process;
            }
            assert!(
                Monotonic::now() < deadline,
                "detached child did not publish a holder record"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn assert_running_in_unique_group(self) {
        assert!(process_alive(self.pid), "detached resident is not alive");
        // SAFETY: getpgid and getpgrp only inspect process identities.
        let observed = unsafe { libc::getpgid(i32::try_from(self.pid).expect("pid fits i32")) };
        assert_eq!(observed, self.group);
        assert_ne!(observed, unsafe { libc::getpgrp() });
        let rows = self.rows();
        assert!(
            rows.iter().any(|row| row.pid == self.pid && !row.zombie),
            "detached resident is absent or zombie: {rows:?}"
        );
    }

    fn assert_gone(self, operation: &str) {
        let deadline = Monotonic::now() + Duration::from_secs(5);
        loop {
            let rows = self.rows();
            if rows.is_empty() {
                return;
            }
            assert!(
                Monotonic::now() < deadline,
                "{operation} left process group {} live or zombie: {rows:?}",
                self.group
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn rows(self) -> Vec<ProcessRow> {
        process_rows()
            .into_iter()
            .filter(|row| row.pid == self.pid || row.group == self.group)
            .collect()
    }

    fn terminate_for_cleanup(self) {
        if self.rows().is_empty() {
            return;
        }
        // SAFETY: this test minted a unique process group whose positive id is
        // recorded above; negating it addresses only that exact fixture group.
        let _ = unsafe { libc::kill(-self.group, libc::SIGKILL) };
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct ProcessRow {
    pid: u32,
    parent: u32,
    group: i32,
    zombie: bool,
}

#[cfg(unix)]
impl ProcessRow {
    fn parse(line: &str) -> Option<Self> {
        let mut fields = line.split_whitespace();
        let pid = fields.next()?.parse().ok()?;
        let parent = fields.next()?.parse().ok()?;
        let group = fields.next()?.parse().ok()?;
        let status = fields.next()?;
        Some(Self {
            pid,
            parent,
            group,
            zombie: status.starts_with('Z'),
        })
    }
}

#[cfg(unix)]
fn process_rows() -> Vec<ProcessRow> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,pgid=,stat=,command="])
        .output()
        .expect("detached process audit runs");
    assert!(output.status.success(), "detached process audit succeeds");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(ProcessRow::parse)
        .collect()
}

#[cfg(unix)]
struct LauncherGuard {
    child: Option<Child>,
    detached: Option<DetachedProcess>,
}

#[cfg(unix)]
impl LauncherGuard {
    fn spawn(fixture: &Fixture, arguments: &[&str]) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_spice"))
            .arg("--json")
            .args(arguments)
            .current_dir(&fixture.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("detached launcher starts");
        Self {
            child: Some(child),
            detached: None,
        }
    }

    fn pid(&self) -> u32 {
        self.child.as_ref().expect("detached launcher").id()
    }

    fn observe(&mut self, process: DetachedProcess) {
        self.detached = Some(process);
    }

    fn wait(&mut self) -> Output {
        self.child
            .take()
            .expect("detached launcher")
            .wait_with_output()
            .expect("detached launcher exits")
    }

    fn disarm(&mut self) {
        self.detached = None;
    }
}

#[cfg(unix)]
impl Drop for LauncherGuard {
    fn drop(&mut self) {
        if let Some(process) = self.detached {
            process.terminate_for_cleanup();
        } else if let Some(child) = self.child.as_ref() {
            for process in process_rows()
                .into_iter()
                .filter(|process| process.parent == child.id())
                .filter(|process| i32::try_from(process.pid) == Ok(process.group))
                .map(|process| DetachedProcess {
                    pid: process.pid,
                    group: process.group,
                })
            {
                process.terminate_for_cleanup();
            }
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(unix)]
fn assert_pid_gone(pid: u32, operation: &str) {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .expect("launcher process audit runs");
    assert!(
        output.stdout.is_empty(),
        "{operation} left launcher process {pid} live or zombie"
    );
}

#[cfg(unix)]
fn block_app_pointer(fixture: &Fixture) -> (PathBuf, Vec<u8>) {
    let pointer = fixture
        .root
        .join(canvas::APP_STAGE)
        .join(canvas::APP_STAGE_CURRENT);
    let current = fs::read(&pointer).expect("read current app pointer");
    fs::remove_file(&pointer).expect("replace app pointer with a blocking fixture");
    let fifo = Command::new("mkfifo")
        .arg(&pointer)
        .status()
        .expect("create blocking app pointer");
    assert!(fifo.success(), "blocking app pointer is a FIFO");
    (pointer, current)
}

#[cfg(unix)]
#[test]
fn default_up_detaches_proves_authentication_and_retains_no_credential_surface() {
    const TOKEN: &str = "correct-horse-battery-staple";

    let fixture = Fixture::new("daemon-default");
    let mut launcher =
        LauncherGuard::spawn(&fixture, &["up", "--port", "0", "--auth-token", TOKEN]);
    let launcher_pid = launcher.pid();
    let launched = launcher.wait();
    assert!(
        launched.status.success(),
        "launcher failed: {}",
        String::from_utf8_lossy(&launched.stderr)
    );
    assert_pid_gone(launcher_pid, "successful parent exit");
    let process = DetachedProcess::wait_for_holder(&fixture);
    launcher.observe(process);
    let mut cleanup = DetachedResidentGuard {
        fixture: &fixture,
        process,
        armed: true,
    };
    let Outcome::Reply(ready) =
        wire::decode::<Outcome<wire::CanvasOpen>>(&launched.stdout).expect("readiness decodes")
    else {
        panic!("default up did not return a readiness reply")
    };
    let discovery_bytes = fs::read(fixture.canvas()).expect("daemon discovery reads");
    let discovery: canvas::Discovery =
        wire::decode(&discovery_bytes).expect("daemon discovery decodes");
    assert_eq!(ready.body.endpoint, discovery.endpoint);
    assert_eq!(ready.body.epoch, discovery.epoch);

    process.assert_running_in_unique_group();

    let listed = Command::new("ps")
        .args(["eww", "-p", &process.pid.to_string()])
        .output()
        .expect("resident process listing runs");
    assert!(listed.status.success(), "resident process listing succeeds");
    let log = fs::read(fixture.runtime.join("resident.log")).expect("resident log reads");
    for surface in [
        &launched.stdout[..],
        &discovery_bytes[..],
        &log[..],
        &listed.stdout[..],
    ] {
        assert!(
            !surface
                .windows(TOKEN.len())
                .any(|window| window == TOKEN.as_bytes()),
            "auth token escaped its one-shot handoff"
        );
    }
    let down = invoke(&fixture, "down");
    assert!(
        down.status.success(),
        "daemon down failed: {}",
        String::from_utf8_lossy(&down.stderr)
    );
    process.assert_gone("down");
    assert!(!fixture.canvas().exists(), "down withdrew daemon discovery");
    cleanup.armed = false;
    launcher.disarm();
}

#[cfg(unix)]
#[test]
fn failed_daemon_start_reaps_its_unique_process_group_and_releases_runtime_state() {
    let fixture = Fixture::new("daemon-bind-failure");
    let reservation = TcpListener::bind(("127.0.0.1", 0)).expect("reserve daemon failure port");
    let port = reservation
        .local_addr()
        .expect("reserved daemon failure address")
        .port();
    let port_text = port.to_string();
    let (pointer, current) = block_app_pointer(&fixture);

    let mut launcher = LauncherGuard::spawn(&fixture, &["up", "--port", port_text.as_str()]);
    let launcher_pid = launcher.pid();
    let process = DetachedProcess::wait_for_holder(&fixture);
    launcher.observe(process);
    process.assert_running_in_unique_group();
    fs::write(pointer, current).expect("release child into its early bind failure");

    let launched = launcher.wait();

    assert_eq!(launched.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&launched) else {
        panic!("taken daemon port must return its typed bind fault")
    };
    assert_eq!(fault.reason, "canvas::bind-failed");
    assert_pid_gone(launcher_pid, "early bind failure");
    process.assert_gone("early bind failure");
    assert!(
        !fixture.canvas().exists(),
        "failed start withdrew discovery"
    );
    let possession = hold(&fixture.resident(), "post-failure-audit\n");
    drop(possession);
    launcher.disarm();
}
