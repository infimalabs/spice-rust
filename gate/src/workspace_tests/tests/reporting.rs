use std::io::{self, Write};

use super::*;

#[test]
fn every_reporting_failure_cancels_and_reaps_started_process_groups() {
    for point in [
        ReportPoint::Start,
        ReportPoint::Progress,
        ReportPoint::Terminal,
        ReportPoint::Coordinator,
    ] {
        for failure in [ReportFailure::Write, ReportFailure::Flush] {
            reporting_failure_case(point, failure);
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ReportPoint {
    Start,
    Progress,
    Terminal,
    Coordinator,
}

impl ReportPoint {
    const fn marker(self) -> &'static str {
        match self {
            Self::Start => "stage=start shard=2/2",
            Self::Progress => "stage=progress shard=1/1",
            Self::Terminal => "stage=terminal shard=1/2",
            Self::Coordinator => "[rust-test] coordinator",
        }
    }

    fn shards(self) -> Vec<PackageInventory> {
        let mut hung = fixture_inventory().packages;
        match self {
            Self::Start => hung,
            Self::Progress => vec![hung.remove(0)],
            Self::Terminal => vec![quick_package(), hung.remove(0)],
            Self::Coordinator => vec![quick_package()],
        }
    }

    const fn has_hung_shard(self) -> bool {
        !matches!(self, Self::Coordinator)
    }

    const fn requires_started_hung_shard(self) -> bool {
        matches!(self, Self::Progress | Self::Terminal)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReportFailure {
    Write,
    Flush,
}

fn reporting_failure_case(point: ReportPoint, failure: ReportFailure) {
    let fixture = reporting_process_fixture(&format!("{point:?}-{failure:?}"));
    let pid_paths = if point.has_hung_shard() {
        process_pid_paths(&fixture.root, "a")
    } else {
        Vec::new()
    };
    // A start-report failure may legitimately beat the first asynchronous
    // shard's PID publication. Progress and terminal reports, by contrast,
    // must observe a complete live process tree before failure is injected.
    let ready = if point.requires_started_hung_shard() {
        pid_paths.clone()
    } else {
        Vec::new()
    };
    let mut output = FailingOutput::new(point.marker(), failure, ready.clone());
    let policy = RunPolicy {
        concurrency: 2,
        shared_outputs: false,
        cargo_jobs: None,
        test_threads: None,
        shard_deadline: Duration::from_secs(15),
        coordinator_deadline: Duration::from_secs(17),
        progress_interval: Duration::from_millis(40),
        output_drain: Duration::from_secs(1),
        reap: Duration::from_secs(1),
        grace: Duration::from_millis(50),
    };
    let started = Instant::now();

    let error = Coordinator::new(
        point.shards(),
        fixture.root.clone(),
        fixture.environment.clone(),
        fixture.cargo.clone().into_os_string(),
        policy,
        &mut output,
    )
    .run()
    .expect_err("forced reporting failure refuses");

    assert!(
        error.to_string().contains(FORCED_REPORT_FAILURE),
        "{point:?} {failure:?} replaced the initiating output error: {error}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(12),
        "{point:?} {failure:?} did not cancel promptly"
    );
    for path in pid_paths.into_iter().filter(|path| pid_path_ready(path)) {
        let role = path
            .file_stem()
            .and_then(OsStr::to_str)
            .expect("pid path has a UTF-8 role");
        assert_process_terminal(read_pid(&path), role);
    }
}

const FORCED_REPORT_FAILURE: &str = "forced coordinator report failure";

struct FailingOutput {
    marker: &'static str,
    failure: ReportFailure,
    ready: Vec<PathBuf>,
    bytes: Vec<u8>,
    armed: bool,
}

impl FailingOutput {
    fn new(marker: &'static str, failure: ReportFailure, ready: Vec<PathBuf>) -> Self {
        Self {
            marker,
            failure,
            ready,
            bytes: Vec::new(),
            armed: false,
        }
    }

    fn arm_if_marker_arrived(&mut self, bytes: &[u8]) -> io::Result<bool> {
        if self.armed {
            return Ok(false);
        }
        let mut candidate = self.bytes.clone();
        candidate.extend_from_slice(bytes);
        if !String::from_utf8_lossy(&candidate).contains(self.marker) {
            return Ok(false);
        }
        wait_for_pid_paths(&self.ready)?;
        self.armed = true;
        Ok(true)
    }

    fn forced_error() -> io::Error {
        io::Error::new(io::ErrorKind::BrokenPipe, FORCED_REPORT_FAILURE)
    }
}

impl Write for FailingOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.arm_if_marker_arrived(bytes)? && self.failure == ReportFailure::Write {
            return Err(Self::forced_error());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.armed && self.failure == ReportFailure::Flush {
            Err(Self::forced_error())
        } else {
            Ok(())
        }
    }
}

struct ReportingProcessFixture {
    root: PathBuf,
    cargo: PathBuf,
    environment: Environment,
}

impl Drop for ReportingProcessFixture {
    fn drop(&mut self) {
        // The shards this fixture launches outlive their coordinator by design:
        // that abandonment is the defect under test. An assertion that fires
        // before the coordinator reaps them would otherwise strand `tail -f` and
        // a FIFO reader forever, so the fixture owns every pid it recorded and
        // signals them all here, where a panic unwinding through also arrives.
        for role in ["a", "b"] {
            for path in process_pid_paths(&self.root, role) {
                // A role that never started left no pid file, and one the
                // coordinator already reaped is gone. The second case is the
                // normal one, so the signal is sent through `output` to capture
                // the "no such process" the passing run would otherwise print.
                if let Some(pid) = fs::read_to_string(&path)
                    .ok()
                    .and_then(|pid| pid.trim().parse::<u32>().ok())
                {
                    let _ = Command::new("/bin/kill")
                        .args(["-KILL", &pid.to_string()])
                        .output();
                }
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn reporting_process_fixture(label: &str) -> ReportingProcessFixture {
    let root = fixture_root(&format!("report-failure-{label}"));
    fs::create_dir_all(&root).expect("reporting fixture root creates");
    std::process::Command::new("/usr/bin/mkfifo")
        .arg(root.join("resident.fifo"))
        .status()
        .expect("reporting fixture mkfifo launches")
        .success()
        .then_some(())
        .expect("reporting fixture mkfifo succeeds");
    let vendor = executable(
        &root,
        "report-vendor",
        "#!/bin/sh\nexec /usr/bin/tail -f /dev/null\n",
    );
    let waiter = executable(
        &root,
        "report-waiter",
        "#!/bin/sh\nexec /bin/cat \"$PID_ROOT/resident.fifo\"\n",
    );
    let test_binary = executable(
        &root,
        "report-test",
        "#!/bin/sh\nexec /usr/bin/tail -f /dev/null\n",
    );
    let cargo = executable(
        &root,
        "report-cargo",
        concat!(
            "#!/bin/sh\n",
            "case \"$*\" in\n",
            "  *quick-id*)\n",
            "    printf '     Running tests/quick.rs (fixture)\\n' >&2\n",
            "    printf 'test quick::done ... ok\\n'\n",
            "    exit 0\n",
            "    ;;\n",
            "  *fixture-a-id*) role=a ;;\n",
            "  *fixture-b-id*) role=b ;;\n",
            "  *) exit 64 ;;\n",
            "esac\n",
            "printf '%s\\n' \"$$\" > \"$PID_ROOT/$role.cargo.pid\"\n",
            "\"$TEST_BINARY\" & test_pid=$!\n",
            "printf '%s\\n' \"$test_pid\" > \"$PID_ROOT/$role.test.pid\"\n",
            "\"$VENDOR\" & vendor_pid=$!\n",
            "printf '%s\\n' \"$vendor_pid\" > \"$PID_ROOT/$role.vendor.pid\"\n",
            "\"$WAITER\" & waiter_pid=$!\n",
            "printf '%s\\n' \"$waiter_pid\" > \"$PID_ROOT/$role.waiter.pid\"\n",
            "printf '     Running tests/%s.rs (fixture)\\n' \"$role\" >&2\n",
            "printf 'test %s::fifo_wait ... ' \"$role\"\n",
            "wait\n",
        ),
    );
    let environment = Environment::new([
        (OsString::from("PID_ROOT"), root.clone().into_os_string()),
        (OsString::from("TEST_BINARY"), test_binary.into_os_string()),
        (OsString::from("VENDOR"), vendor.into_os_string()),
        (OsString::from("WAITER"), waiter.into_os_string()),
        (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
    ])
    .expect("reporting fixture environment admits");
    ReportingProcessFixture {
        root,
        cargo,
        environment,
    }
}

fn process_pid_paths(root: &Path, role: &str) -> Vec<PathBuf> {
    ["cargo", "test", "vendor", "waiter"]
        .map(|process| root.join(format!("{role}.{process}.pid")))
        .to_vec()
}

fn wait_for_pid_paths(paths: &[PathBuf]) -> io::Result<()> {
    let started = Instant::now();
    while paths.iter().any(|path| !pid_path_ready(path))
        && started.elapsed() < Duration::from_secs(10)
    {
        thread::sleep(Duration::from_millis(5));
    }
    let missing = paths
        .iter()
        .filter(|path| !pid_path_ready(path))
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("reporting fixture did not start {}", missing.join(", ")),
        ))
    }
}

fn pid_path_ready(path: &Path) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|pid| pid.trim().parse::<u32>().ok())
        .is_some()
}
