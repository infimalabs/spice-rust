use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

#[test]
fn production_policy_uses_only_the_scheduler_granted_shard_workers() {
    for concurrency in 1..=WORKSPACE_TEST_CONCURRENCY_CAP {
        assert_eq!(
            RunPolicy::production(concurrency)
                .expect("every bounded scheduler grant admits")
                .concurrency,
            concurrency
        );
    }
    assert_eq!(
        RunPolicy::production(WORKSPACE_TEST_CONCURRENCY_CAP)
            .expect("bounded scheduler grant admits")
            .for_shards(2)
            .concurrency,
        2,
        "the coordinator never creates more workers than actual shards"
    );
    for concurrency in [0, WORKSPACE_TEST_CONCURRENCY_CAP + 1] {
        let error = RunPolicy::production(concurrency)
            .expect_err("out-of-contract scheduler grant refuses")
            .to_string();
        assert!(error.contains("must be between 1 and 4"), "{error}");
    }
}

#[test]
fn selected_test_environment_cannot_inherit_the_parent_git_repository() {
    let mut entries = BTreeMap::from([
        (OsString::from("PATH"), OsString::from("/fixture/bin")),
        (
            OsString::from(CARGO_TARGET_DIR),
            OsString::from("/fixture/target"),
        ),
    ]);
    for variable in GIT_LOCAL_ENVIRONMENT {
        entries.insert(
            OsString::from(variable),
            OsString::from("parent-repository"),
        );
    }
    let environment = without_parent_repository(
        Environment::new(entries).expect("declare inherited hook environment"),
    )
    .expect("selected test environment admits after isolation");
    let value = |name: &str| {
        environment
            .entries()
            .iter()
            .find_map(|(key, value)| (key == OsStr::new(name)).then_some(value))
    };

    assert_eq!(
        value("PATH"),
        Some(&OsString::from("/fixture/bin")),
        "ordinary process environment survives repository isolation"
    );
    assert_eq!(
        value(CARGO_TARGET_DIR),
        Some(&OsString::from("/fixture/target")),
        "configured Cargo cache authority survives repository isolation"
    );
    for variable in GIT_LOCAL_ENVIRONMENT {
        assert!(
            value(variable).is_none(),
            "selected tests retained parent repository variable {variable}"
        );
    }
}

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
            source: PathBuf::from(format!("/workspace/tests/target-{index:02}.rs")),
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
    let root = Path::new("/workspace");
    let inventory = inventory_from_metadata(
        Metadata {
            workspace_members: vec!["fixture-id".to_owned()],
            target_directory: root.join("target"),
            build_directory: root.join("target/cargo-build"),
            packages: vec![MetadataPackage {
                id: "fixture-id".to_owned(),
                name: "fixture".to_owned(),
                manifest_path: root.join("Cargo.toml"),
                targets: vec![
                    target("fixture", &["lib"], true),
                    target("fixture", &["bin"], true),
                    target("resident", &["test"], true),
                    target("build-script-build", &["custom-build"], false),
                ],
            }],
        },
        root,
    )
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
fn successful_execution_publishes_one_exact_target_inventory_without_parsing_logs() {
    let execution = WorkspaceTestExecution::from_inventory(&fixture_inventory());
    assert_eq!(
        execution.tests(),
        ["fixture-a::lib:fixture-a", "fixture-b::bin:fixture-b"]
    );

    let source = wire::Digest::compute(wire::Domain::GateTree, b"workspace tests");
    let unknown = execution
        .publish_unknown(
            source,
            vec!["src/lib.rs".to_owned()],
            "coverage publisher emitted no complete receipt",
        )
        .expect("unknown coverage remains typed");
    assert_eq!(unknown.tests_run().len(), 2);
    assert!(unknown.test_coverage().is_empty());
    assert!(matches!(
        unknown.coverage(),
        crate::CoverageCompleteness::Unknown { .. }
    ));
}

#[test]
fn staged_rust_source_selects_its_direct_cargo_test_target_with_exact_edge() {
    let inventory = fixture_inventory();
    let source = wire::Digest::compute(wire::Domain::GateTree, b"selected Rust tree");

    let report = selection_report(
        &inventory,
        Path::new("/workspace"),
        source.clone(),
        &["fixture-a/src/behavior.rs".to_owned()],
    )
    .expect("workspace reach graph selects tests");

    assert!(report.admits());
    assert_eq!(report.source(), &source);
    assert_eq!(report.changed_sources(), ["fixture-a/src/behavior.rs"]);
    assert_eq!(report.tests(), ["fixture-a::lib:fixture-a"]);
    assert_eq!(report.edges().len(), 1);
    assert!(
        report
            .edges()
            .iter()
            .all(|edge| edge.source() == "fixture-a/src/behavior.rs")
    );
    assert_eq!(
        crate::TestSelectionReport::decode(&report.encode().expect("encode selection"))
            .expect("decode selection"),
        report
    );
}

#[test]
fn staged_library_source_selects_every_test_enabled_target_that_may_link_it() {
    let mut inventory = fixture_inventory();
    inventory.packages[0].targets.push(TargetIdentity {
        package: "fixture-a-id".to_owned(),
        kind: "test".to_owned(),
        name: "behavior".to_owned(),
        source: PathBuf::from("/workspace/fixture-a/tests/behavior.rs"),
    });

    let report = selection_report(
        &inventory,
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"library-dependent tests"),
        &["fixture-a/src/behavior.rs".to_owned()],
    )
    .expect("library reach expands through its package test targets");

    assert!(report.admits());
    assert_eq!(
        report.tests(),
        ["fixture-a::lib:fixture-a", "fixture-a::test:behavior"]
    );
    assert_eq!(report.edges().len(), 2);
    assert!(
        report
            .edges()
            .iter()
            .all(|edge| edge.source() == "fixture-a/src/behavior.rs")
    );
}

#[test]
fn rust_source_inside_a_package_but_outside_target_roots_refuses_narrowing() {
    let report = selection_report(
        &fixture_inventory(),
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"unreachable package source"),
        &["fixture-a/tools/unreachable.rs".to_owned()],
    )
    .expect("unreachable package source remains reportable");

    assert!(!report.admits());
    assert!(report.tests().is_empty());
    assert!(report.edges().is_empty());
    let crate::CoverageCompleteness::Unknown { reason } = report.coverage() else {
        panic!("unreachable package source must remain unknown");
    };
    assert!(reason.contains("reaches no direct Cargo test target"));
}

#[test]
fn rust_source_outside_workspace_packages_refuses_narrowing_loudly() {
    let report = selection_report(
        &fixture_inventory(),
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"unknown Rust tree"),
        &["tools/outside.rs".to_owned()],
    )
    .expect("unknown selection remains reportable");

    assert!(!report.admits());
    assert!(report.tests().is_empty());
    assert!(report.edges().is_empty());
    let crate::CoverageCompleteness::Unknown { reason } = report.coverage() else {
        panic!("unmapped Rust source must remain unknown");
    };
    assert!(reason.contains("outside every workspace package"));
    let document = String::from_utf8(report.encode().expect("encode refusal")).unwrap();
    assert!(document.contains("tools/outside.rs"));
    assert!(document.contains("outside every workspace package"));
}

#[test]
fn partially_mapped_rust_sources_retain_edges_but_refuse_as_incomplete() {
    let report = selection_report(
        &fixture_inventory(),
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"incomplete Rust tree"),
        &[
            "fixture-a/src/behavior.rs".to_owned(),
            "tools/outside.rs".to_owned(),
        ],
    )
    .expect("incomplete selection remains reportable");

    assert!(!report.admits());
    assert_eq!(report.tests(), ["fixture-a::lib:fixture-a"]);
    assert_eq!(report.edges().len(), 1);
    assert_eq!(report.edges()[0].source(), "fixture-a/src/behavior.rs");
    let crate::CoverageCompleteness::Incomplete { reason } = report.coverage() else {
        panic!("partially mapped Rust selection must remain incomplete");
    };
    assert!(reason.contains("tools/outside.rs"));
}

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "the end-to-end canonical runner fixture captures its test process environment"
)]
fn selected_workspace_runner_executes_the_reported_target_from_changed_rust_source() {
    let root = fixture_root("workspace-selected-test");
    fs::create_dir_all(root.join("src")).expect("create selected workspace fixture");
    fs::write(
        root.join("Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"selected-fixture\"\n",
            "version = \"0.0.0\"\n",
            "edition = \"2024\"\n",
        ),
    )
    .expect("write selected fixture manifest");
    fs::write(
        root.join("Cargo.lock"),
        concat!(
            "# This file is automatically @generated by Cargo.\n",
            "# It is not intended for manual editing.\n",
            "version = 4\n\n",
            "[[package]]\n",
            "name = \"selected-fixture\"\n",
            "version = \"0.0.0\"\n",
        ),
    )
    .expect("write selected fixture lock");
    fs::write(
        root.join("src/lib.rs"),
        concat!(
            "pub fn selected() -> bool { true }\n",
            "#[test]\n",
            "fn selected_runs() { assert!(selected()); }\n",
        ),
    )
    .expect("write selected fixture source");
    // This test itself runs inside Cargo. A selected workspace-test parent also
    // supplies its output roots, so inheriting them here would make the nested
    // Cargo wait on the parent test binary's own build-directory lock.
    let environment = Environment::new(std::env::vars_os().filter(|(name, _)| {
        name != OsStr::new(CARGO_TARGET_DIR) && name != OsStr::new(CARGO_BUILD_BUILD_DIR)
    }))
    .expect("capture isolated test environment");
    let mut output = Vec::new();

    let report = run_selected_at(
        WorkspaceTestRoots::new(&root, &root),
        wire::Digest::compute(wire::Domain::GateTree, b"selected fixture"),
        &["src/lib.rs".to_owned()],
        environment,
        1,
        &mut output,
    )
    .unwrap_or_else(|error| {
        panic!(
            "selected runner failed: {error}; output={}",
            String::from_utf8_lossy(&output)
        )
    });

    assert!(report.admits());
    assert_eq!(report.tests(), ["selected-fixture::lib:selected_fixture"]);
    assert!(
        String::from_utf8(output)
            .unwrap()
            .contains("[rust-test-selection] source=\"src/lib.rs\"")
    );
    fs::remove_dir_all(root).expect("remove selected workspace fixture");
}

#[test]
fn complete_coverage_cannot_omit_an_executed_target_but_may_name_the_known_suite() {
    let execution = WorkspaceTestExecution::from_inventory(&fixture_inventory());
    let source = wire::Digest::compute(wire::Domain::GateTree, b"workspace tests");
    let partial = vec![crate::TestCoverage::new("fixture-a::lib:fixture-a", Vec::new()).unwrap()];
    let error = execution
        .publish(
            source,
            Vec::new(),
            partial,
            crate::CoverageCompleteness::Complete,
        )
        .expect_err("partial target coverage refuses");
    assert!(error.to_string().contains("omits executed test"));

    let complete = vec![
        crate::TestCoverage::new("fixture-a::lib:fixture-a", Vec::new()).unwrap(),
        crate::TestCoverage::new("fixture-b::bin:fixture-b", Vec::new()).unwrap(),
        crate::TestCoverage::new("fixture-c::test:unrun", Vec::new()).unwrap(),
    ];
    let published = execution
        .publish(
            wire::Digest::compute(wire::Domain::GateTree, b"workspace tests"),
            Vec::new(),
            complete,
            crate::CoverageCompleteness::Complete,
        )
        .expect("known-suite coverage may include an unexecuted target");
    assert_eq!(published.tests_run().len(), 2);
    assert_eq!(published.test_coverage().len(), 3);
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
        root: PathBuf::from("/workspace/quick"),
        targets: vec![TargetIdentity {
            package: "quick-id".to_owned(),
            kind: "test".to_owned(),
            name: "quick".to_owned(),
            source: PathBuf::from("/workspace/quick/tests/quick.rs"),
        }],
    }
}

fn fixture_inventory() -> Inventory {
    Inventory {
        target_directory: PathBuf::from("/workspace/target"),
        build_directory: PathBuf::from("/workspace/target/cargo-build"),
        packages: vec![
            PackageInventory {
                id: "fixture-a-id".to_owned(),
                name: "fixture-a".to_owned(),
                root: PathBuf::from("/workspace/fixture-a"),
                targets: vec![TargetIdentity {
                    package: "fixture-a-id".to_owned(),
                    kind: "lib".to_owned(),
                    name: "fixture-a".to_owned(),
                    source: PathBuf::from("/workspace/fixture-a/src/lib.rs"),
                }],
            },
            PackageInventory {
                id: "fixture-b-id".to_owned(),
                name: "fixture-b".to_owned(),
                root: PathBuf::from("/workspace/fixture-b"),
                targets: vec![TargetIdentity {
                    package: "fixture-b-id".to_owned(),
                    kind: "bin".to_owned(),
                    name: "fixture-b".to_owned(),
                    source: PathBuf::from("/workspace/fixture-b/src/main.rs"),
                }],
            },
        ],
    }
}

fn target(name: &str, kind: &[&str], test: bool) -> MetadataTarget {
    let source = match kind.first().copied() {
        Some("lib") => "/workspace/src/lib.rs".to_owned(),
        Some("bin") => "/workspace/src/main.rs".to_owned(),
        _ => format!("/workspace/tests/{name}.rs"),
    };
    MetadataTarget {
        name: name.to_owned(),
        kind: kind.iter().map(|kind| (*kind).to_owned()).collect(),
        test,
        src_path: PathBuf::from(source),
    }
}

fn target_identity(kind: &str, name: &str) -> TargetIdentity {
    TargetIdentity {
        package: "fixture-id".to_owned(),
        kind: kind.to_owned(),
        name: name.to_owned(),
        source: PathBuf::from(format!("/workspace/{kind}/{name}.rs")),
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
