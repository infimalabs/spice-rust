use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

#[test]
fn partition_validation_names_omitted_and_duplicated_packages_and_targets() {
    let inventory = fixture_inventory();
    let exact = partition(&inventory);
    validate_partition(&inventory, &exact).expect("exact partition passes");

    let mut omitted = exact.clone();
    omitted[0].targets.pop();
    assert!(
        validate_partition(&inventory, &omitted)
            .expect_err("omitted target refuses")
            .to_string()
            .contains("observed 0")
    );

    let mut duplicated = exact.clone();
    let duplicate = duplicated[0].targets[0].clone();
    duplicated[0].targets.push(duplicate);
    assert!(
        validate_partition(&inventory, &duplicated)
            .expect_err("duplicated target refuses")
            .to_string()
            .contains("observed 2")
    );

    let mut package_missing = exact;
    package_missing.pop();
    let error = validate_partition(&inventory, &package_missing)
        .expect_err("omitted package refuses")
        .to_string();
    assert!(error.contains("package"));
    assert!(error.contains("observed 0"));

    let mut wrong_owner = partition(&inventory);
    wrong_owner[0].targets[0].package = wrong_owner[1].id.clone();
    assert!(
        validate_partition(&inventory, &wrong_owner)
            .expect_err("a target assigned to another package refuses")
            .to_string()
            .contains("target")
    );
}

#[test]
fn partition_splits_large_packages_in_stable_target_order() {
    let mut inventory = fixture_inventory();
    inventory.packages[0].targets = (0..TARGETS_PER_SHARD + 1)
        .map(|index| TargetIdentity {
            package: "fixture-a-id".to_owned(),
            kind: "test".to_owned(),
            name: format!("target-{index:02}"),
        })
        .collect();

    let shards = partition(&inventory);

    assert_eq!(shards.len(), 3);
    assert_eq!(shards[0].targets.len(), TARGETS_PER_SHARD);
    assert_eq!(shards[1].targets.len(), 1);
    assert_eq!(shards[2].name, "fixture-b");
    validate_partition(&inventory, &shards).expect("split partition remains exact");
}

#[test]
fn cargo_target_arguments_preserve_library_binary_and_integration_semantics() {
    let targets = [
        target_identity("lib", "fixture"),
        target_identity("bin", "fixture"),
        target_identity("test", "resident"),
    ];

    let arguments = targets
        .iter()
        .map(target_arguments)
        .collect::<Result<Vec<_>, _>>()
        .expect("current target kinds map to Cargo selectors")
        .concat();

    assert_eq!(
        arguments,
        ["--lib", "--bin", "fixture", "--test", "resident"]
            .map(OsString::from)
            .to_vec()
    );
}

#[test]
fn metadata_inventory_keeps_library_binary_and_integration_targets() {
    let inventory = inventory_from_metadata(Metadata {
        workspace_members: vec!["fixture-id".to_owned()],
        packages: vec![MetadataPackage {
            id: "fixture-id".to_owned(),
            name: "fixture".to_owned(),
            targets: vec![
                target("fixture", &["lib"], true),
                target("fixture", &["bin"], true),
                target("resident", &["test"], true),
                target("build-script-build", &["custom-build"], false),
            ],
        }],
    })
    .expect("fixture metadata admits");

    assert_eq!(
        inventory.packages[0]
            .targets
            .iter()
            .map(TargetIdentity::display)
            .collect::<Vec<_>>(),
        ["bin:fixture", "lib:fixture", "test:resident"]
    );
}

#[test]
fn last_test_observation_ignores_the_harness_summary() {
    let observer = LastTest::default();
    observer
        .observe_line("test resident::one_case ... ok".to_owned())
        .expect("test record observes");
    observer
        .observe_line("test result: ok. 1 passed; 0 failed".to_owned())
        .expect("summary record observes");

    assert_eq!(observer.last(), "test resident::one_case ... ok");
}

#[test]
fn shard_timeout_reaps_cargo_test_vendor_and_fifo_waiter() {
    let root = fixture_root("workspace-test-timeout");
    fs::create_dir_all(&root).expect("timeout fixture root creates");
    let fifo = root.join("resident.fifo");
    let vendor = executable(
        &root,
        "vendor",
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' \"$$\" > \"$PID_ROOT/vendor.pid\"\n",
            "exec /usr/bin/tail -f /dev/null\n",
        ),
    );
    let waiter = executable(
        &root,
        "fifo-waiter",
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' \"$$\" > \"$PID_ROOT/waiter.pid\"\n",
            "exec /bin/cat \"$PID_ROOT/resident.fifo\"\n",
        ),
    );
    let test_binary = executable(
        &root,
        "resident-test",
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' \"$$\" > \"$PID_ROOT/test.pid\"\n",
            "\"$VENDOR\" &\n",
            "\"$WAITER\" &\n",
            "wait\n",
        ),
    );
    let cargo = executable(
        &root,
        "cargo",
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' \"$$\" > \"$PID_ROOT/cargo.pid\"\n",
            "printf '     Running tests/resident.rs (fixture)\\n' >&2\n",
            "printf 'test resident::fifo_wait ... '\n",
            "\"$TEST_BINARY\" &\n",
            "wait\n",
        ),
    );
    std::process::Command::new("/usr/bin/mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo launches")
        .success()
        .then_some(())
        .expect("mkfifo succeeds");
    let environment = Environment::new([
        (OsString::from("PID_ROOT"), root.clone().into_os_string()),
        (OsString::from("TEST_BINARY"), test_binary.into_os_string()),
        (OsString::from("VENDOR"), vendor.into_os_string()),
        (OsString::from("WAITER"), waiter.into_os_string()),
        (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
    ])
    .expect("fixture environment admits");
    let shard = fixture_inventory().packages.remove(0);
    let policy = RunPolicy {
        concurrency: 1,
        shard_deadline: Duration::from_secs(5),
        coordinator_deadline: Duration::from_secs(7),
        progress_interval: Duration::from_secs(1),
        output_drain: Duration::from_secs(1),
        reap: Duration::from_secs(1),
        grace: Duration::from_millis(50),
    };
    let mut output = Vec::new();

    let error = Coordinator::new(
        vec![shard],
        root.clone(),
        environment,
        cargo.into_os_string(),
        policy,
        &mut output,
    )
    .run()
    .expect_err("hung shard times out");

    assert!(error.to_string().contains("timeout"), "{error}");
    let output = String::from_utf8(output).expect("timeout report is UTF-8");
    assert!(output.contains("disposition=timeout"), "{output}");
    assert!(output.contains("tests/resident.rs (fixture)"), "{output}");
    assert!(output.contains("test resident::fifo_wait ..."), "{output}");
    for name in ["cargo", "test", "vendor", "waiter"] {
        let pid = read_pid(&root.join(format!("{name}.pid")));
        assert_process_terminal(pid, name);
    }
    fs::remove_dir_all(root).expect("timeout fixture removes");
}

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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReportFailure {
    Write,
    Flush,
}

fn reporting_failure_case(point: ReportPoint, failure: ReportFailure) {
    let fixture = reporting_process_fixture(&format!("{point:?}-{failure:?}"));
    let ready = if point.has_hung_shard() {
        process_pid_paths(&fixture.root, "a")
    } else {
        Vec::new()
    };
    let mut output = FailingOutput::new(point.marker(), failure, ready.clone());
    let policy = RunPolicy {
        concurrency: 2,
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
    for path in ready {
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
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' \"$$\" > \"$PID_ROOT/$ROLE.vendor.pid\"\n",
            "exec /usr/bin/tail -f /dev/null\n",
        ),
    );
    let waiter = executable(
        &root,
        "report-waiter",
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' \"$$\" > \"$PID_ROOT/$ROLE.waiter.pid\"\n",
            "exec /bin/cat \"$PID_ROOT/resident.fifo\"\n",
        ),
    );
    let test_binary = executable(
        &root,
        "report-test",
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' \"$$\" > \"$PID_ROOT/$ROLE.test.pid\"\n",
            "ROLE=\"$ROLE\" \"$VENDOR\" &\n",
            "ROLE=\"$ROLE\" \"$WAITER\" &\n",
            "wait\n",
        ),
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
            "printf '     Running tests/%s.rs (fixture)\\n' \"$role\" >&2\n",
            "printf 'test %s::fifo_wait ... ' \"$role\"\n",
            "ROLE=\"$role\" \"$TEST_BINARY\" &\n",
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

fn quick_package() -> PackageInventory {
    PackageInventory {
        id: "quick-id".to_owned(),
        name: "quick".to_owned(),
        targets: vec![TargetIdentity {
            package: "quick-id".to_owned(),
            kind: "test".to_owned(),
            name: "quick".to_owned(),
        }],
    }
}

fn fixture_inventory() -> Inventory {
    Inventory {
        packages: vec![
            PackageInventory {
                id: "fixture-a-id".to_owned(),
                name: "fixture-a".to_owned(),
                targets: vec![TargetIdentity {
                    package: "fixture-a-id".to_owned(),
                    kind: "test".to_owned(),
                    name: "resident".to_owned(),
                }],
            },
            PackageInventory {
                id: "fixture-b-id".to_owned(),
                name: "fixture-b".to_owned(),
                targets: vec![TargetIdentity {
                    package: "fixture-b-id".to_owned(),
                    kind: "bin".to_owned(),
                    name: "fixture-b".to_owned(),
                }],
            },
        ],
    }
}

fn target(name: &str, kind: &[&str], test: bool) -> MetadataTarget {
    MetadataTarget {
        name: name.to_owned(),
        kind: kind.iter().map(|kind| (*kind).to_owned()).collect(),
        test,
    }
}

fn target_identity(kind: &str, name: &str) -> TargetIdentity {
    TargetIdentity {
        package: "fixture-id".to_owned(),
        kind: kind.to_owned(),
        name: name.to_owned(),
    }
}

fn fixture_root(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    env::temp_dir().join(format!(
        "spice-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn executable(root: &Path, name: &str, body: &str) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, body).expect("fixture executable writes");
    let mut permissions = fs::metadata(&path)
        .expect("fixture executable metadata reads")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("fixture executable becomes executable");
    path
}

fn read_pid(path: &Path) -> u32 {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("fixture pid {} reads: {error}", path.display()))
        .trim()
        .parse()
        .expect("fixture pid parses")
}

fn assert_process_terminal(pid: u32, role: &str) {
    let started = Instant::now();
    while process_live(pid) && started.elapsed() < Duration::from_secs(2) {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(!process_live(pid), "{role} process {pid} survived timeout");
}

fn process_live(pid: u32) -> bool {
    let output = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
        .expect("process state probe launches");
    output.status.success()
        && String::from_utf8_lossy(&output.stdout)
            .trim()
            .chars()
            .next()
            .is_some_and(|state| state != 'Z')
}
