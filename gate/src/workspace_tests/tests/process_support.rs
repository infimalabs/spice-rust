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
        concat!("#!/bin/sh\n", "exec /usr/bin/tail -f /dev/null\n",),
    );
    let waiter = executable(
        &root,
        "report-waiter",
        concat!("#!/bin/sh\n", "exec /bin/cat \"$PID_ROOT/resident.fifo\"\n",),
    );
    let test_binary = executable(
        &root,
        "report-test",
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' \"$$\" > \"$PID_ROOT/$ROLE.test.pid\"\n",
            "ROLE=\"$ROLE\" \"$VENDOR\" &\n",
            "printf '%s\\n' \"$!\" > \"$PID_ROOT/$ROLE.vendor.pid\"\n",
            "ROLE=\"$ROLE\" \"$WAITER\" &\n",
            "printf '%s\\n' \"$!\" > \"$PID_ROOT/$ROLE.waiter.pid\"\n",
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
            "printf '%s\\n' \"$!\" > \"$PID_ROOT/$role.test.pid\"\n",
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
            declared_path_sources: Vec::new(),
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
                    declared_path_sources: Vec::new(),
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
                    declared_path_sources: Vec::new(),
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
        declared_path_sources: Vec::new(),
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
