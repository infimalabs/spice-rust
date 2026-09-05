use super::*;
use std::sync::Barrier;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
#[cfg(unix)]
use std::process::{Command, Stdio};

#[cfg(unix)]
const OUTER_SUPERVISOR_PREBUILD_FIXTURE: &str = concat!(
    "workspace_tests::prebuild::tests::",
    "outer_supervisor_owns_selected_prebuild_runtime_fixture",
);
#[cfg(unix)]
const RETIRED_PREBUILD_FIXTURE_DEADLINE: Duration = Duration::from_millis(25);

#[cfg(unix)]
struct DiscoveryFixture {
    root: PathBuf,
}

#[cfg(unix)]
impl DiscoveryFixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);

        for _ in 0..100 {
            let nonce = NEXT.fetch_add(1, AtomicOrdering::Relaxed);
            let root = env::temp_dir().join(format!(
                "spice-gate-discovery-{}-{nonce}",
                std::process::id(),
            ));
            match fs::create_dir(&root) {
                Ok(()) => return Self { root },
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("create discovery fixture root: {error}"),
            }
        }
        panic!("create unique discovery fixture root");
    }

    fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}")).expect("write discovery fixture script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("make discovery fixture script executable");
        path
    }

    fn test(&self, name: &str, executable: PathBuf) -> PrebuiltTest {
        PrebuiltTest {
            package: "fixture".to_owned(),
            package_root: self.root.clone(),
            target: TargetIdentity {
                package: "fixture".to_owned(),
                kind: "test".to_owned(),
                name: name.to_owned(),
                required_features: Vec::new(),
                source: executable.clone(),
                declared_path_sources: Vec::new(),
                source_bytes: 1,
            },
            executable,
            partition: None,
        }
    }
}

#[cfg(unix)]
impl Drop for DiscoveryFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove discovery fixture root");
    }
}

#[cfg(unix)]
fn empty_environment() -> Environment {
    Environment::new(std::iter::empty::<(OsString, OsString)>())
        .expect("empty fixture environment is valid")
}

#[cfg(unix)]
fn wait_for_paths(paths: &[PathBuf]) {
    let started = Instant::now();
    while !paths.iter().all(|path| path.is_file()) {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "fixture children did not publish {:?}",
            paths,
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
fn fixture_pid(path: &Path) -> u32 {
    fs::read_to_string(path)
        .expect("read fixture PID")
        .trim()
        .parse()
        .expect("fixture PID is numeric")
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("inspect fixture process")
        .success()
}

#[test]
fn standalone_prebuild_retains_its_local_shard_bound() {
    let spec = prebuild_process_spec(
        OsStr::new("cargo"),
        std::iter::empty::<OsString>(),
        Path::new("."),
        Environment::empty(),
        SHARD_DEADLINE,
        Cancellation::Never,
        "standalone prebuild fixture",
    )
    .expect("standalone prebuild spec is valid");

    assert_eq!(SHARD_DEADLINE, Duration::from_secs(180));
    assert_eq!(spec.group, ProcessGroup::New);
    assert_eq!(spec.deadlines.runtime(), SHARD_DEADLINE);
}

#[cfg(unix)]
#[test]
fn outer_supervisor_owns_selected_prebuild_runtime() {
    let output = Command::new(std::env::current_exe().expect("current gate test executable"))
        .args([
            "--ignored",
            "--exact",
            OUTER_SUPERVISOR_PREBUILD_FIXTURE,
            "--nocapture",
        ])
        .output()
        .expect("launch isolated outer-supervisor prebuild fixture");

    assert!(
        output.status.success(),
        "outer-supervisor prebuild fixture failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[cfg(unix)]
#[test]
#[ignore = "subprocess fixture isolates process-global pre-commit containment"]
fn outer_supervisor_owns_selected_prebuild_runtime_fixture() {
    let _containment = crate::measure::inherit_process_group()
        .expect("isolated fixture enters pre-commit containment");
    let fixture = DiscoveryFixture::new();
    let cargo = fixture.script(
        "cold-cargo",
        concat!(
            "/bin/sleep 0.1\n",
            "printf '%s\\n' '{\"reason\":\"compiler-artifact\"}'\n",
        ),
    );
    let started = Instant::now();
    let spec = prebuild_process_spec(
        cargo.as_os_str(),
        std::iter::empty::<OsString>(),
        &fixture.root,
        empty_environment(),
        RETIRED_PREBUILD_FIXTURE_DEADLINE,
        Cancellation::Never,
        "cold selected prebuild fixture",
    )
    .expect("supervised prebuild spec is valid");
    assert_eq!(spec.group, ProcessGroup::Inherit);
    assert_eq!(spec.deadlines.runtime(), Duration::MAX);

    let outcome = sill::spawn(spec)
        .expect("cold selected prebuild launches")
        .wait()
        .expect("outer supervisor owns the complete wait");

    assert!(started.elapsed() > RETIRED_PREBUILD_FIXTURE_DEADLINE);
    assert!(matches!(outcome.completion(), Completion::Exited(status) if status.success()));
    assert!(outcome.outputs_drained());
    assert_eq!(
        outcome.stdout().map(|output| output.bytes()),
        Some(b"{\"reason\":\"compiler-artifact\"}\n".as_slice()),
    );
    assert_eq!(
        outcome.stderr().map(|output| output.bytes()),
        Some(b"".as_slice()),
    );
}

#[test]
fn single_lane_partition_reports_its_terminal_timing() {
    let environment = Environment::new(std::iter::empty::<(OsString, OsString)>())
        .expect("empty fixture environment is valid");
    let mut output = Vec::new();

    let partitioned = partition_largest_prebuilt_tests(Vec::new(), &environment, 1, 1, &mut output)
        .expect("single-lane partition succeeds");

    assert!(partitioned.is_empty());
    let report = String::from_utf8(output).expect("partition report is UTF-8");
    assert!(report.contains("stage=partition artifacts=0 candidates=0 shards=0"));
    assert!(report.contains("stage=partition-terminal elapsed_ms="));
}

#[cfg(unix)]
#[test]
fn unpartitioned_artifact_is_preserved_without_discovery() {
    let fixture = DiscoveryFixture::new();
    let invoked = fixture.root.join("invoked");
    let executable = fixture.script(
        "must-not-run",
        &format!("printf 'invoked\\n' > '{}'\nexit 19\n", invoked.display()),
    );
    let test = fixture.test("must-not-run", executable);
    let mut output = Vec::new();

    let partitioned = partition_largest_prebuilt_tests(
        vec![test.clone()],
        &empty_environment(),
        ROUTINE_PARTITIONS_PER_TARGET,
        1,
        &mut output,
    )
    .expect("unpartitioned target needs no discovery process");

    assert_eq!(partitioned.len(), 1);
    assert_eq!(partitioned[0].package, test.package);
    assert_eq!(partitioned[0].target, test.target);
    assert_eq!(partitioned[0].executable, test.executable);
    assert!(partitioned[0].partition.is_none());
    assert!(!invoked.exists());
    let report = String::from_utf8(output).expect("partition report is UTF-8");
    assert!(report.contains("stage=partition artifacts=1 candidates=0 shards=1"));
}

#[cfg(unix)]
#[test]
fn policy_artifacts_produce_the_exact_selected_executable() {
    let fixture = DiscoveryFixture::new();
    let executable = fixture.script("selected-test", "exit 0\n");
    let inventory = Inventory {
        packages: vec![PackageInventory {
            id: "fixture-id".to_owned(),
            name: "fixture".to_owned(),
            root: fixture.root.clone(),
            dependencies: Vec::new(),
            declared_targets: Vec::new(),
            removed_targets: Vec::new(),
            targets: vec![fixture.test("selected-test", executable.clone()).target],
        }],
        binary_outputs: Vec::new(),
        target_directory: fixture.root.clone(),
        build_directory: fixture.root.clone(),
    };
    let artifacts = serde_json::to_vec(&serde_json::json!({
        "reason": "compiler-artifact",
        "package_id": "fixture-id",
        "target": { "name": "selected-test", "kind": ["test"] },
        "profile": { "test": true },
        "executable": executable,
    }))
    .expect("serialize fixture Cargo artifact");

    let plan = prebuilt_test_plan(&inventory, &artifacts)
        .expect("policy artifact satisfies selected test inventory");

    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].package, "fixture");
    assert_eq!(plan[0].target.name, "selected-test");
    assert_eq!(plan[0].executable, executable);

    let error = prebuilt_test_plan(&inventory, b"")
        .expect_err("empty reusable policy artifacts must refuse");
    assert!(error.to_string().contains("omitted test artifact"));
}

#[cfg(unix)]
#[test]
fn one_discovery_lane_can_produce_multiple_sequential_execution_shards() {
    let fixture = DiscoveryFixture::new();
    let executable = fixture.script(
        "partitionable",
        "for case in 0 1 2 3 4 5 6 7 8 9; do printf 'case_%s: test\\n' \"$case\"; done\n",
    );
    let environment = empty_environment();
    let mut output = Vec::new();

    let partitioned = partition_largest_prebuilt_tests(
        vec![fixture.test("partitionable", executable)],
        &environment,
        2,
        1,
        &mut output,
    )
    .expect("one discovery lane partitions the selected artifact");

    assert_eq!(partitioned.len(), 2);
    assert_eq!(
        partitioned
            .iter()
            .map(PrebuiltTest::partition_count)
            .collect::<Vec<_>>(),
        vec![2, 2],
    );
    assert_eq!(
        partitioned
            .iter()
            .map(PrebuiltTest::partition_index)
            .collect::<Vec<_>>(),
        vec![0, 1],
    );
    let report = String::from_utf8(output).expect("partition report is UTF-8");
    assert!(report.contains("candidates=1 completed=1 lanes=1"));
    assert!(report.contains("stage=partition artifacts=1 candidates=1 shards=2"));
}

#[test]
fn bounded_discovery_preserves_candidate_order_and_lane_bound() {
    let candidates = (0..9).collect::<Vec<_>>();
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);

    let discoveries = run_bounded_discovery(&candidates, 3, |candidate, _cancellation| {
        let now_active = active.fetch_add(1, AtomicOrdering::AcqRel) + 1;
        peak.fetch_max(now_active, AtomicOrdering::AcqRel);
        thread::sleep(Duration::from_millis(2));
        active.fetch_sub(1, AtomicOrdering::AcqRel);
        Ok(vec![candidate.to_string()])
    })
    .expect("bounded discovery succeeds");

    assert_eq!(
        discoveries
            .completed
            .iter()
            .map(|discovery| discovery.ordinal)
            .collect::<Vec<_>>(),
        (0..candidates.len()).collect::<Vec<_>>(),
    );
    assert_eq!(discoveries.causal_failure, None);
    assert!(peak.load(AtomicOrdering::Acquire) <= 3);
    assert_eq!(active.load(AtomicOrdering::Acquire), 0);
}

#[test]
fn bounded_discovery_stops_admission_after_failure() {
    let candidates = (0..100).collect::<Vec<_>>();
    let first_wave = Barrier::new(2);
    let invoked = AtomicUsize::new(0);

    let discoveries = run_bounded_discovery(&candidates, 2, |candidate, _cancellation| {
        invoked.fetch_add(1, AtomicOrdering::AcqRel);
        if *candidate < 2 {
            first_wave.wait();
        }
        if *candidate == 0 {
            Err(WorkspaceTestError::new("fixture discovery failed"))
        } else {
            if *candidate == 1 {
                thread::sleep(Duration::from_millis(2));
            }
            Ok(vec![candidate.to_string()])
        }
    })
    .expect("worker pool reports child failures through ordered results");

    assert_eq!(
        discoveries
            .completed
            .iter()
            .map(|discovery| discovery.ordinal)
            .collect::<Vec<_>>(),
        vec![0, 1],
    );
    assert!(discoveries.completed[0].cases.is_err());
    assert_eq!(discoveries.causal_failure, Some(0));
    assert_eq!(invoked.load(AtomicOrdering::Acquire), 2);
}

#[test]
fn bounded_discovery_refuses_unbounded_concurrency() {
    let error = run_bounded_discovery(&[0], usize::MAX, |_, _cancellation| Ok(Vec::new()))
        .expect_err("unbounded discovery concurrency is refused");

    assert!(error.to_string().contains("concurrency must be between 1"));
}

#[cfg(unix)]
#[test]
fn real_discovery_failure_cancels_and_reaps_in_flight_sibling() {
    let fixture = DiscoveryFixture::new();
    let ready = fixture.root.join("sleeper-ready");
    let pid = fixture.root.join("sleeper-pid");
    let sleeper = fixture.script(
        "sleeper",
        &format!(
            "printf '%s\\n' \"$$\" > '{}'\nprintf 'ready\\n' > '{}'\nexec /bin/sleep 30\n",
            pid.display(),
            ready.display(),
        ),
    );
    let failure = fixture.script(
        "failure",
        &format!(
            "while [ ! -f '{}' ]; do /bin/sleep 0.01; done\nexit 19\n",
            ready.display(),
        ),
    );
    let tests = vec![
        fixture.test("failure", failure),
        fixture.test("sleeper", sleeper),
    ];
    let environment = empty_environment();
    let started = Instant::now();

    let discoveries = run_bounded_discovery(&tests, 2, |test, cancellation| {
        prebuilt_test_cases(test, &environment, cancellation)
    })
    .expect("worker pool captures real child failures");

    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        discoveries
            .completed
            .iter()
            .map(|discovery| discovery.ordinal)
            .collect::<Vec<_>>(),
        vec![0, 1],
    );
    assert!(
        discoveries.completed[0]
            .cases
            .as_ref()
            .expect_err("first child fails")
            .to_string()
            .contains("exit code=19"),
    );
    assert!(
        discoveries.completed[1]
            .cases
            .as_ref()
            .expect_err("sibling is cancelled")
            .to_string()
            .contains("cancelled"),
    );
    assert_eq!(discoveries.causal_failure, Some(0));
    assert!(!process_is_alive(fixture_pid(&pid)));
}

#[cfg(unix)]
#[test]
fn reversed_real_failure_survives_lower_ordinal_sibling_cancellation() {
    let fixture = DiscoveryFixture::new();
    let ready = fixture.root.join("reversed-sleeper-ready");
    let pid = fixture.root.join("reversed-sleeper-pid");
    let sleeper = fixture.script(
        "ordinal-0-sleeper",
        &format!(
            "printf '%s\\n' \"$$\" > '{}'\nprintf 'ready\\n' > '{}'\nexec /bin/sleep 30\n",
            pid.display(),
            ready.display(),
        ),
    );
    let failure = fixture.script(
        "ordinal-1-failure",
        &format!(
            "while [ ! -f '{}' ]; do /bin/sleep 0.01; done\nexit 19\n",
            ready.display(),
        ),
    );
    let tests = vec![
        fixture.test("ordinal-0-sleeper", sleeper),
        fixture.test("ordinal-1-failure", failure),
    ];
    let mut output = Vec::new();
    let started = Instant::now();

    let error = partition_largest_prebuilt_tests(tests, &empty_environment(), 2, 2, &mut output)
        .expect_err("ordinal 1 exit fails discovery");

    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(error.to_string().contains("ordinal-1-failure"), "{error}");
    assert!(error.to_string().contains("exit code=19"), "{error}");
    assert!(!error.to_string().contains("cancelled"), "{error}");
    assert!(!process_is_alive(fixture_pid(&pid)));
    let output = String::from_utf8(output).expect("discovery progress is UTF-8");
    let ordinal_0 = output.find("ordinal-0-sleeper").expect("ordinal 0 reports");
    let ordinal_1 = output.find("ordinal-1-failure").expect("ordinal 1 reports");
    assert!(ordinal_0 < ordinal_1, "{output}");
}

#[cfg(unix)]
#[test]
fn real_discovery_timeout_reaps_child() {
    let fixture = DiscoveryFixture::new();
    let sleeper = fixture.script("timeout", "exec /bin/sleep 30\n");
    let test = fixture.test("timeout", sleeper);
    let started = Instant::now();

    let error = prebuilt_test_cases_with_deadline(
        &test,
        &empty_environment(),
        CancellationToken::new(),
        Duration::from_secs(2),
    )
    .expect_err("metadata deadline terminates the child");

    assert!(started.elapsed() < Duration::from_secs(6));
    let message = error.to_string();
    assert!(message.contains("timeout"));
    let pid = message
        .split("process-id=")
        .nth(1)
        .and_then(|suffix| suffix.split_once(' '))
        .and_then(|(pid, _)| pid.parse().ok())
        .expect("timeout identifies the spawned process");
    assert!(!process_is_alive(pid));
}

#[cfg(unix)]
#[test]
fn real_discovery_preserves_order_and_concurrency_bound() {
    let fixture = DiscoveryFixture::new();
    let release = fixture.root.join("release");
    let mut started_paths = Vec::new();
    let mut tests = Vec::new();
    for ordinal in 0..6 {
        let started = fixture.root.join(format!("started-{ordinal}"));
        let delay = match ordinal {
            0 => "0.09",
            1 => "0.06",
            2 => "0.03",
            _ => "0",
        };
        let executable = fixture.script(
            &format!("ordered-{ordinal}"),
            &format!(
                "printf 'started\\n' > '{}'\nwhile [ ! -f '{}' ]; do /bin/sleep 0.01; done\n/bin/sleep {delay}\nprintf 'case_{ordinal}: test\\n'\n",
                started.display(),
                release.display(),
            ),
        );
        started_paths.push(started);
        tests.push(fixture.test(&format!("ordered-{ordinal}"), executable));
    }
    let environment = empty_environment();

    let discoveries = thread::scope(|scope| {
        let worker = scope.spawn(|| {
            run_bounded_discovery(&tests, 3, |test, cancellation| {
                prebuilt_test_cases(test, &environment, cancellation)
            })
        });
        wait_for_paths(&started_paths[..3]);
        assert!(started_paths[3..].iter().all(|path| !path.exists()));
        fs::write(&release, b"release\n").expect("release discovery children");
        worker
            .join()
            .expect("discovery worker thread does not panic")
    })
    .expect("real bounded discovery succeeds");

    assert_eq!(
        discoveries
            .completed
            .iter()
            .map(|discovery| discovery.ordinal)
            .collect::<Vec<_>>(),
        (0..tests.len()).collect::<Vec<_>>(),
    );
    assert_eq!(
        discoveries
            .completed
            .into_iter()
            .map(|discovery| discovery.cases.expect("discovery succeeds"))
            .collect::<Vec<_>>(),
        (0..tests.len())
            .map(|ordinal| vec![format!("case_{ordinal}")])
            .collect::<Vec<_>>(),
    );
}
