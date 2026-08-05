use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

static NEXT: AtomicU64 = AtomicU64::new(0);
const COMMIT: &str = "1111111111111111111111111111111111111111";
const MOVED: &str = "2222222222222222222222222222222222222222";

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = env::temp_dir().join(format!(
            "spice-release-builder-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("dist/stage")).expect("fixture stage directory");
        fs::write(
            root.join(APP_ARTIFACT),
            include_bytes!("../../../dist/stage/app_bg.wasm"),
        )
        .expect("copy real staged Wasm");
        fs::write(
            root.join(EMBEDDED_ARTIFACT),
            include_bytes!("../../../dist/stage/canvas-app.html"),
        )
        .expect("copy real embedded shell");
        Self { root }
    }

    fn programs(&self) -> Programs {
        let program = |name: &str| self.root.join("programs").join(name);
        Programs {
            cargo: program("cargo"),
            rustc: program("rustc"),
            rustup: program("rustup"),
            trunk: program("trunk"),
            mutants: program("cargo-mutants"),
            git: program("git"),
            xtask: program("xtask"),
        }
    }

    fn pointer(&self, target: ReleaseTarget) -> PathBuf {
        self.root
            .join("dist/stage/targets")
            .join(target.as_str())
            .join("payload.json")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("fixture cleanup");
    }
}

struct FakeExecutor {
    root: PathBuf,
    target: ReleaseTarget,
    trunk: RefCell<String>,
    trunk_drift_after_native: Cell<bool>,
    refusing: Cell<Option<&'static str>>,
    tracked_dirty: Cell<bool>,
    untracked: RefCell<String>,
    moved_after_native: Cell<bool>,
    native_ran: Cell<bool>,
    records: RefCell<Vec<CommandRecord>>,
}

impl FakeExecutor {
    fn new(root: &Path, target: ReleaseTarget) -> Self {
        Self {
            root: root.to_path_buf(),
            target,
            trunk: RefCell::new(PINNED_TRUNK.to_owned()),
            trunk_drift_after_native: Cell::new(false),
            refusing: Cell::new(None),
            tracked_dirty: Cell::new(false),
            untracked: RefCell::new(String::new()),
            moved_after_native: Cell::new(false),
            native_ran: Cell::new(false),
            records: RefCell::new(Vec::new()),
        }
    }

    fn labels(&self) -> Vec<&'static str> {
        self.records.borrow().iter().map(|row| row.label).collect()
    }
}

impl Executor for FakeExecutor {
    fn execute(&self, command: CommandSpec) -> Result<CommandRecord, TargetBuildError> {
        if command.label == "native-build" {
            let destination = self
                .root
                .join("target/release-appliance")
                .join(self.target.as_str())
                .join("release/spice");
            fs::create_dir_all(destination.parent().expect("native parent"))
                .expect("native directory");
            fs::copy(env::current_exe().expect("test executable"), &destination)
                .expect("copy real compiled executable");
            self.native_ran.set(true);
        }
        let stdout = match command.label {
            "source-commit" if self.moved_after_native.get() && self.native_ran.get() => {
                format!("{MOVED}\n").into_bytes()
            }
            "source-commit" => format!("{COMMIT}\n").into_bytes(),
            "rust-version" => b"rustc 1.97.1 (fixture)\n".to_vec(),
            "installed-targets" => format!("{WASM_TARGET}\n{}\n", self.target).into_bytes(),
            "trunk-version" if self.trunk_drift_after_native.get() && self.native_ran.get() => {
                b"trunk 0.21.13\n".to_vec()
            }
            "trunk-version" => format!("{}\n", self.trunk.borrow()).into_bytes(),
            "mutants-version" => format!("{PINNED_MUTANTS}\n").into_bytes(),
            "app-build" | "embed-build" | "native-build" => {
                format!("{} complete\n", command.label).into_bytes()
            }
            "source-status" if self.tracked_dirty.get() => b" M xtask/src/lib.rs\n".to_vec(),
            "source-status" => self.untracked.borrow().clone().into_bytes(),
            label => panic!("unexpected command label {label}"),
        };
        let refused = self.refusing.get() == Some(command.label);
        let record = CommandRecord {
            label: command.label,
            program: command.program,
            arguments: command.arguments,
            code: Some(i32::from(refused)),
            stdout,
            stderr: Vec::new(),
        };
        self.records.borrow_mut().push(record.clone());
        Ok(record)
    }

    fn records(&self) -> Vec<CommandRecord> {
        self.records.borrow().clone()
    }
}

#[test]
fn both_targets_use_the_fixed_commands_and_real_artifact_bytes() {
    for target in [
        ReleaseTarget::Aarch64AppleDarwin,
        ReleaseTarget::X86_64UnknownLinuxMusl,
    ] {
        let fixture = Fixture::new();
        let programs = fixture.programs();
        let executor = FakeExecutor::new(&fixture.root, target);

        let build = stage_with(
            &fixture.root,
            &fixture.root.join("target"),
            target,
            &programs,
            &executor,
        )
        .expect("target stage succeeds");

        assert_eq!(
            executor.labels(),
            [
                "source-commit",
                "source-status",
                "rust-version",
                "installed-targets",
                "trunk-version",
                "mutants-version",
                "app-build",
                "embed-build",
                "native-build",
                "source-commit",
                "source-status",
                "rust-version",
                "installed-targets",
                "trunk-version",
                "mutants-version",
            ]
        );
        let build_records = build
            .commands
            .iter()
            .filter(|record| record.label.ends_with("-build"))
            .collect::<Vec<_>>();
        assert_eq!(
            build_records
                .iter()
                .map(|record| record.label)
                .collect::<Vec<_>>(),
            ["app-build", "embed-build", "native-build"]
        );
        assert_eq!(build_records[0].arguments, [OsString::from("check-app")]);
        assert_eq!(
            build_records[1].arguments,
            cargo_arguments(&fixture.root, "canvas", target)
        );
        assert_eq!(
            build_records[2].arguments,
            cargo_arguments(&fixture.root, "spice", target)
        );
        let mutants = build
            .commands
            .iter()
            .find(|record| record.label == "mutants-version")
            .expect("cargo-mutants pin observation");
        assert_eq!(
            mutants.arguments,
            [OsString::from("mutants"), OsString::from("--version")]
        );
        assert!(build_records.iter().all(|record| {
            record.success() && !record.stdout.is_empty() && record.stderr.is_empty()
        }));
        assert_eq!(
            build.staged.payload.app,
            Digest::compute(
                Domain::DistArtifact,
                include_bytes!("../../../dist/stage/app_bg.wasm")
            )
        );
        assert_eq!(
            build.staged.payload.embedded_app,
            Digest::compute(
                Domain::DistArtifact,
                include_bytes!("../../../dist/stage/canvas-app.html")
            )
        );
        let native = fs::read(env::current_exe().expect("test executable"))
            .expect("read real compiled executable");
        assert_eq!(
            build.staged.payload.native,
            Digest::compute(Domain::DistArtifact, &native)
        );
        assert_eq!(
            fs::read(fixture.pointer(target)).expect("target pointer reads"),
            build.staged.payload_bytes
        );
    }
}

fn release_environment(cargo_target_dir: Option<&str>) -> ReleaseEnvironment {
    ReleaseEnvironment::new(
        Some(OsString::from("/tool/bin")),
        Some(OsString::from("/home/release")),
        Some(OsString::from("/cargo/home")),
        None,
        None,
        None,
        None,
        None,
        cargo_target_dir.map(OsString::from),
        None,
        None,
    )
}

#[test]
fn system_executor_overrides_machine_local_cargo_build_state() {
    let fixture = Fixture::new();
    let release = release_environment(None);

    let executor = SystemExecutor::new(&fixture.root, &release).expect("release executor");
    let entries = executor.environment.entries();
    let value = |name: &str| {
        entries
            .iter()
            .find_map(|(key, value)| (key == name).then_some(value.as_os_str()))
            .unwrap_or_else(|| panic!("missing release environment {name}"))
    };

    assert_eq!(
        value("CARGO_BUILD_BUILD_DIR"),
        fixture
            .root
            .join("target/release-appliance-build")
            .as_os_str()
    );
    assert_eq!(value("CARGO_INCREMENTAL"), "0");
    // Pinned to the resolved answer rather than left absent: absence would
    // let the one child that reads this variable — `xtask check-app` — pick
    // a build root of its own.
    assert_eq!(
        value("CARGO_TARGET_DIR"),
        fixture.root.join("target").as_os_str()
    );
    assert_eq!(value("RUSTC_WRAPPER"), "");
    assert_eq!(value("RUSTC_WORKSPACE_WRAPPER"), "");
}

/// The workspace default and the acquired value are asserted against
/// different roots on purpose. A sealed image mounts the workspace
/// read-only, so a build root that silently stayed under it would be the
/// one failure this resolver exists to prevent, and two cases that agreed
/// on a path could not tell the two arms apart.
#[test]
fn the_build_root_is_the_acquired_scratch_or_the_workspace_and_never_both() {
    let root = Path::new("/workspace");

    assert_eq!(
        release_environment(None).build_root(root),
        root.join("target"),
        "an unset CARGO_TARGET_DIR must fall back to the workspace",
    );
    assert_eq!(
        release_environment(Some("/cargo-target")).build_root(root),
        Path::new("/cargo-target"),
        "an acquired CARGO_TARGET_DIR must displace the workspace entirely",
    );

    let executor = SystemExecutor::new(root, &release_environment(Some("/cargo-target")))
        .expect("release executor");
    let entries = executor.environment.entries();
    let value = |name: &str| {
        entries
            .iter()
            .find_map(|(key, value)| (key == name).then_some(value.as_os_str()))
            .unwrap_or_else(|| panic!("missing release environment {name}"))
    };
    assert_eq!(
        value("CARGO_BUILD_BUILD_DIR"),
        OsStr::new("/cargo-target/release-appliance-build"),
    );
    // The `app-build` step runs this same binary, which acquires its build
    // root from `CARGO_TARGET_DIR` at its own rim. Withholding it here
    // would send the app rebuild back under the workspace no matter what
    // the resolver above decided.
    assert_eq!(value("CARGO_TARGET_DIR"), OsStr::new("/cargo-target"));
}

/// `HOME` is Cargo's own rule for locating a Cargo home, so the fallback
/// has to reproduce it rather than invent a second answer, and an explicit
/// `CARGO_HOME` has to win over it.
#[test]
fn the_cargo_home_prefers_the_acquired_value_and_falls_back_to_cargos_own_rule() {
    assert_eq!(
        release_environment(None).cargo_home(),
        Some(OsString::from("/cargo/home")),
    );

    let inherited = ReleaseEnvironment::new(
        None,
        Some(OsString::from("/home/release")),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    assert_eq!(
        inherited.cargo_home(),
        Some(OsString::from("/home/release/.cargo")),
    );

    let bare = ReleaseEnvironment::new(
        None, None, None, None, None, None, None, None, None, None, None,
    );
    assert_eq!(
        bare.cargo_home(),
        None,
        "with neither value there is no Cargo home to guess at",
    );
}

#[cfg(unix)]
#[test]
fn program_discovery_preserves_a_multicall_proxy_name() {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    let fixture = Fixture::new();
    let programs = fixture.root.join("programs");
    fs::create_dir(&programs).expect("program directory");
    let rustup = programs.join("rustup");
    fs::write(&rustup, b"#!/bin/sh\n").expect("rustup fixture");
    fs::set_permissions(&rustup, fs::Permissions::from_mode(0o755))
        .expect("rustup executable mode");
    symlink("rustup", programs.join("cargo")).expect("cargo proxy symlink");

    let cargo = find_program(programs.as_os_str(), "cargo").expect("cargo proxy");

    assert_eq!(cargo, fs::canonicalize(&programs).unwrap().join("cargo"));
    assert_eq!(cargo.file_name(), Some(OsStr::new("cargo")));
    assert!(
        fs::symlink_metadata(cargo)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn tool_drift_refuses_before_the_first_build_or_publication() {
    let fixture = Fixture::new();
    let target = ReleaseTarget::Aarch64AppleDarwin;
    let programs = fixture.programs();
    let executor = FakeExecutor::new(&fixture.root, target);
    *executor.trunk.borrow_mut() = "trunk 0.21.13".to_owned();

    let error = stage_with(
        &fixture.root,
        &fixture.root.join("target"),
        target,
        &programs,
        &executor,
    )
    .expect_err("tool drift refuses");

    assert!(matches!(error, TargetBuildError::Pins(_)));
    assert!(
        !executor
            .labels()
            .iter()
            .any(|label| label.ends_with("-build"))
    );
    assert!(!fixture.pointer(target).exists());
}

#[test]
fn dirty_source_refuses_before_pin_observation_build_or_publication() {
    let fixture = Fixture::new();
    let target = ReleaseTarget::Aarch64AppleDarwin;
    let programs = fixture.programs();
    let executor = FakeExecutor::new(&fixture.root, target);
    executor.tracked_dirty.set(true);

    let error = stage_with(
        &fixture.root,
        &fixture.root.join("target"),
        target,
        &programs,
        &executor,
    )
    .expect_err("dirty source refuses");

    assert!(matches!(error, TargetBuildError::SourceDirty(_)));
    assert_eq!(executor.labels(), ["source-commit", "source-status"]);
    assert!(!fixture.pointer(target).exists());
}

/// A refusing source status is a failed reading, not a clean checkout.
///
/// Every other source-check test drives what the status command *said*,
/// which is why none of them reach the exit status. Nothing downstream can
/// stand in for it either: the inventory is read as text, an empty
/// inventory is a checkout with nothing to report, and a command that
/// refuses says nothing. So a status the appliance failed to take would
/// read exactly like a checkout that matched HEAD, and the build it guards
/// would go on to publish a target pointer for source it never verified.
#[test]
fn a_refusing_source_status_refuses_rather_than_reading_a_clean_checkout() {
    let fixture = Fixture::new();
    let target = ReleaseTarget::Aarch64AppleDarwin;
    let programs = fixture.programs();
    let executor = FakeExecutor::new(&fixture.root, target);
    executor.refusing.set(Some("source-status"));

    let error = stage_with(
        &fixture.root,
        &fixture.root.join("target"),
        target,
        &programs,
        &executor,
    )
    .expect_err("a refusing source status refuses the stage");

    assert!(
        matches!(
            &error,
            TargetBuildError::CommandFailed(record) if record.label == "source-status"
        ),
        "{error:?}"
    );
    assert_eq!(
        executor.labels(),
        ["source-commit", "source-status"],
        "an unread source status has to refuse before anything is pinned or built"
    );
    assert!(!fixture.pointer(target).exists());
}

#[test]
fn only_untracked_release_output_survives_the_source_check() {
    let fixture = Fixture::new();
    let executor = FakeExecutor::new(&fixture.root, ReleaseTarget::Aarch64AppleDarwin);
    let git = fixture.root.join("git");

    *executor.untracked.borrow_mut() =
        "?? dist/stage/objects/1f/2e\n?? dist/stage/targets/aarch64-apple-darwin\n".to_owned();
    verify_checkout(&executor, &git).expect("release output is the build's own product");

    *executor.untracked.borrow_mut() = "?? dist/stage/objects/1f/2e\n?? LATER.md\n".to_owned();
    let stray = verify_checkout(&executor, &git).expect_err("a stray untracked path refuses");
    assert!(
        matches!(&stray, TargetBuildError::SourceDirty(reason) if reason.ends_with("LATER.md")),
        "{stray:?}"
    );

    executor.tracked_dirty.set(true);
    let modified = verify_checkout(&executor, &git).expect_err("a modified tracked path refuses");
    assert!(
        matches!(&modified, TargetBuildError::SourceDirty(reason)
            if reason.starts_with("tracked source differs from HEAD")),
        "{modified:?}"
    );
}

#[test]
fn source_movement_after_build_preserves_the_previous_target_pointer() {
    let fixture = Fixture::new();
    let target = ReleaseTarget::Aarch64AppleDarwin;
    let programs = fixture.programs();
    let executor = FakeExecutor::new(&fixture.root, target);
    executor.moved_after_native.set(true);
    let pointer = fixture.pointer(target);
    fs::create_dir_all(pointer.parent().expect("pointer parent")).expect("target directory");
    fs::write(&pointer, b"previous complete target").expect("previous pointer");

    let error = stage_with(
        &fixture.root,
        &fixture.root.join("target"),
        target,
        &programs,
        &executor,
    )
    .expect_err("source movement refuses pointer");

    assert!(matches!(error, TargetBuildError::Stage(_)));
    assert_eq!(
        fs::read(pointer).expect("previous pointer remains"),
        b"previous complete target"
    );
    assert_eq!(
        executor
            .labels()
            .into_iter()
            .filter(|label| label.ends_with("-build"))
            .collect::<Vec<_>>(),
        ["app-build", "embed-build", "native-build"]
    );
}

/// A build command that refuses is the only thing that says the step failed.
///
/// Every artifact this step is followed by reading already exists: the app
/// artifact is tracked, so it is present in any checkout the appliance is
/// allowed to build from, and the embedded shell beside it is too. A build
/// that ignored the exit status would therefore read the bytes that were
/// already there, name their digests in the payload, and publish a target
/// pointer for a build that never ran. The exit status is the whole of the
/// difference, so the refusal has to be asserted where nothing else can
/// stand in for it.
#[test]
fn a_refusing_build_command_publishes_no_pointer_over_the_previous_one() {
    let fixture = Fixture::new();
    let target = ReleaseTarget::Aarch64AppleDarwin;
    let programs = fixture.programs();
    let executor = FakeExecutor::new(&fixture.root, target);
    executor.refusing.set(Some("app-build"));
    let pointer = fixture.pointer(target);
    fs::create_dir_all(pointer.parent().expect("pointer parent")).expect("target directory");
    fs::write(&pointer, b"previous complete target").expect("previous pointer");

    let error = stage_with(
        &fixture.root,
        &fixture.root.join("target"),
        target,
        &programs,
        &executor,
    )
    .expect_err("a refusing build command refuses the stage");

    assert!(matches!(error, TargetBuildError::Stage(_)), "{error:?}");
    assert_eq!(
        fs::read(&pointer).expect("previous pointer remains"),
        b"previous complete target",
        "a refused build replaced the pointer of the last complete one"
    );
    assert_eq!(
        executor.labels().last(),
        Some(&"app-build"),
        "the refusal has to stop the first build step, not be noticed later"
    );
    assert!(
        fs::read(fixture.root.join(APP_ARTIFACT)).is_ok(),
        "the artifact the ignored status would have published must still be readable"
    );
}

#[test]
fn tool_drift_after_build_preserves_the_previous_target_pointer() {
    let fixture = Fixture::new();
    let target = ReleaseTarget::Aarch64AppleDarwin;
    let programs = fixture.programs();
    let executor = FakeExecutor::new(&fixture.root, target);
    executor.trunk_drift_after_native.set(true);
    let pointer = fixture.pointer(target);
    fs::create_dir_all(pointer.parent().expect("pointer parent")).expect("target directory");
    fs::write(&pointer, b"previous complete target").expect("previous pointer");

    let error = stage_with(
        &fixture.root,
        &fixture.root.join("target"),
        target,
        &programs,
        &executor,
    )
    .expect_err("tool drift refuses pointer");

    assert!(matches!(error, TargetBuildError::Stage(_)));
    assert_eq!(
        fs::read(pointer).expect("previous pointer remains"),
        b"previous complete target"
    );
    assert_eq!(
        executor
            .labels()
            .into_iter()
            .filter(|label| label.ends_with("-build"))
            .collect::<Vec<_>>(),
        ["app-build", "embed-build", "native-build"]
    );
}

fn cargo_arguments(root: &Path, package: &str, target: ReleaseTarget) -> Vec<OsString> {
    cargo_build_arguments(package, target, &root.join("target/release-appliance"))
}
