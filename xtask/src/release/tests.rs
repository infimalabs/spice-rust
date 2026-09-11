use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

static NEXT: AtomicU64 = AtomicU64::new(0);
const COMMIT: &str = "1111111111111111111111111111111111111111";
const MOVED: &str = "2222222222222222222222222222222222222222";
const TEST_WASM: &[u8] = b"lane-built wasm";
const TEST_HTML: &[u8] = b"lane-built shell";
const GENERATION: &str = "6666666666666666666666666666666666666666666666666666666666666666";

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
        let app = root
            .join(spice::canvas::APP_STAGE)
            .join(spice::canvas::APP_STAGE_GENERATIONS)
            .join(GENERATION);
        fs::create_dir_all(&app).expect("fixture app generation");
        for asset in spice::canvas::APP.assets() {
            let contents = match asset.file() {
                "app_bg.wasm" => TEST_WASM,
                "canvas-app.html" => TEST_HTML,
                _ => b"lane-built app asset",
            };
            fs::write(app.join(asset.file()), contents).expect("write lane-built app asset");
        }
        fs::write(
            root.join(spice::canvas::APP_STAGE)
                .join(spice::canvas::APP_STAGE_CURRENT),
            format!("{GENERATION}\n"),
        )
        .expect("write lane app pointer");
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
        assert_eq!(build_records[0].arguments, [OsString::from("app")]);
        assert_eq!(build_records[0].program, programs.xtask);
        assert_eq!(
            build_records[1].arguments,
            cargo_arguments(&fixture.root, "canvas", target)
        );
        assert_eq!(build_records[1].program, programs.cargo);
        assert_eq!(
            build_records[2].arguments,
            cargo_arguments(&fixture.root, "spice", target)
        );
        assert_eq!(build_records[2].program, programs.cargo);
        let rust_observations = build
            .commands
            .iter()
            .filter(|record| record.label == "rust-version")
            .collect::<Vec<_>>();
        assert_eq!(rust_observations.len(), 2);
        assert!(
            rust_observations
                .iter()
                .all(|record| record.program == programs.rustc),
            "preflight and publisher recheck must execute the same resolved compiler"
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
            GENERATION.parse().expect("fixture generation is canonical")
        );
        assert_complete_app_inventory(&build.staged.payload);
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

fn assert_complete_app_inventory(payload: &wire::manifest::TargetPayload) {
    assert_eq!(
        payload
            .inventory
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        [
            "app/app.module",
            "app/app_bg.wasm",
            "app/canvas-app.html",
            "app/chrome.css",
            "app/composer.css",
            "app/mosaic.css",
            "app/responsive.css",
            "app/style.css",
            "bin/spice",
        ],
    );
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

/// The cleared release environment freezes the compiler discovery already made.
///
/// `programs.rustc` is the workspace-root, selected-`CARGO_HOME`, five-rank
/// answer. Installing it as `RUSTC` puts that answer at Cargo's highest rank
/// for app-build, embed-build, and native-build; copying `CARGO_BUILD_RUSTC`
/// would be a second, lower-ranked answer rather than additional authority.
#[test]
fn system_executor_overrides_machine_local_cargo_build_state() {
    let fixture = Fixture::new();
    let release = release_environment(None);
    let programs = fixture.programs();

    let executor =
        SystemExecutor::new(&fixture.root, &release, &programs.rustc).expect("release executor");
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
    assert_eq!(value("RUSTC"), programs.rustc.as_os_str());
    assert!(
        entries.iter().all(|(key, _)| key != "CARGO_BUILD_RUSTC"),
        "the resolved RUSTC already outranks CARGO_BUILD_RUSTC"
    );
    // Pinned to the resolved answer rather than left absent: absence would let
    // the sole app-build child, `xtask app`, pick a build root of its own.
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

    let executor = SystemExecutor::new(
        root,
        &release_environment(Some("/cargo-target")),
        Path::new("/tool/bin/rustc"),
    )
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
        "?? dist/stage/objects/1f/2e\n?? dist/stage/targets/aarch64-apple-darwin\n?? dist/stage/release/manifest.json\n".to_owned();
    verify_checkout(&executor, &git).expect("release output is the build's own product");

    *executor.untracked.borrow_mut() =
        "?? dist/stage/objects/1f/2e\n?? dist/stage/release/manifest.extra\n".to_owned();
    let stray = verify_checkout(&executor, &git).expect_err("a stray untracked path refuses");
    assert!(
        matches!(&stray, TargetBuildError::SourceDirty(reason) if reason.ends_with("manifest.extra")),
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
        read_lane_app_asset(&fixture.root, "/app_bg.wasm").is_ok(),
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

/// A throwaway tree whose Cargo configs a resolver walk can be aimed at.
///
/// Built under the temporary directory rather than beside the workspace, so
/// the walk this drives climbs directories this fixture owns outright and no
/// lane's checkout is read on the way up.
#[cfg(unix)]
struct ConfigTree {
    root: PathBuf,
}

#[cfg(unix)]
impl ConfigTree {
    fn new() -> Self {
        let root = env::temp_dir().join(format!(
            "spice-release-cargo-config-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("fixture root");
        Self { root }
    }

    /// Write one Cargo config into the `.cargo` directory of `directory`.
    ///
    /// The name is passed rather than assumed, because which of the two names
    /// a directory carries is itself one of the things under test.
    fn config(&self, directory: &str, name: &str, body: &str) -> PathBuf {
        let cargo = self.root.join(directory).join(".cargo");
        fs::create_dir_all(&cargo).expect("fixture cargo directory");
        let path = cargo.join(name);
        fs::write(&path, body).expect("fixture cargo config");
        path
    }

    /// A Cargo home holding a config, which is not inside a `.cargo` directory.
    ///
    /// Cargo reads `$CARGO_HOME/config.toml` directly, so a fixture that put it
    /// under a `.cargo` child would be measuring a file Cargo never opens.
    fn cargo_home(&self, name: &str, body: &str) -> OsString {
        let home = self.root.join("cargo-home");
        fs::create_dir_all(&home).expect("fixture cargo home");
        fs::write(home.join(name), body).expect("fixture cargo home config");
        home.into_os_string()
    }

    /// Install one executable at an absolute fixture-owned path.
    fn executable(&self, relative: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;

        let executable = self.root.join(relative);
        fs::create_dir_all(executable.parent().expect("fixture executable parent"))
            .expect("fixture executable directory");
        fs::write(&executable, b"#!/bin/sh\n").expect("fixture executable");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
            .expect("fixture executable mode");
        executable
    }

    /// A `PATH` whose walk answers, so a config that decides has beaten one.
    ///
    /// Without this every config assertion below would also pass for a
    /// resolver that simply found nothing on `PATH`, which is not the claim.
    fn path_with_rustc(&self) -> OsString {
        let directory = self.root.join("path");
        self.executable("path/rustc");
        directory.into_os_string()
    }

    /// A complete release-tool `PATH`, including a compiler that must lose to
    /// the workspace-root Cargo configuration in the discovery fixture.
    fn release_path(&self) -> (OsString, PathBuf) {
        let directory = self.root.join("release-path");
        for program in ["cargo", "rustc", "rustup", "trunk", "cargo-mutants", "git"] {
            self.executable(&format!("release-path/{program}"));
        }
        let rustc = directory.join("rustc");
        (directory.into_os_string(), rustc)
    }

    fn build_dir(&self, relative: &str) -> PathBuf {
        let directory = self.root.join(relative);
        fs::create_dir_all(&directory).expect("fixture build directory");
        directory
    }
}

#[cfg(unix)]
impl Drop for ConfigTree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("config fixture cleanup");
    }
}

/// An environment naming a `PATH` and a Cargo home, and no compiler override.
#[cfg(unix)]
fn resolving_environment(path: OsString, cargo_home: Option<OsString>) -> ReleaseEnvironment {
    ReleaseEnvironment::new(
        Some(path),
        None,
        cargo_home,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
}

/// Release discovery resolves the compiler from the build's exact hierarchy.
///
/// All three lower-ranked answers are live and distinct: the workspace-root
/// config, the selected non-ambient Cargo home, and `PATH` each name an
/// executable compiler. The workspace answer must win because the release
/// executor runs Cargo from that root. Falling back to the old direct `PATH`
/// walk selects `path_rustc`; resolving from the fixture parent instead of the
/// build cwd skips the workspace config and selects `cargo_home_rustc`.
/// The same discovery door is then offered a malformed config and no compiler
/// at all, proving both typed resolver outcomes cross it unchanged.
#[cfg(unix)]
#[test]
fn program_discovery_resolves_rustc_from_the_workspace_build_hierarchy() {
    let tree = ConfigTree::new();
    let workspace = tree.build_dir("workspace");
    let configured_rustc = tree.executable("configured/rustc");
    let cargo_home_rustc = tree.executable("cargo-home-rustc");
    let (path, path_rustc) = tree.release_path();
    let workspace_config = tree.config(
        "workspace",
        "config.toml",
        &format!(
            "[build]\nrustc = {:?}\n",
            configured_rustc.display().to_string()
        ),
    );
    let cargo_home = tree.cargo_home(
        "config.toml",
        &format!(
            "[build]\nrustc = {:?}\n",
            cargo_home_rustc.display().to_string()
        ),
    );
    let environment = resolving_environment(path, Some(cargo_home));

    let programs = Programs::discover(&workspace, &environment)
        .expect("the workspace release programs resolve");

    assert_eq!(programs.rustc, configured_rustc);
    assert_ne!(programs.rustc, path_rustc);
    assert_ne!(programs.rustc, cargo_home_rustc);
    assert_eq!(
        programs.cargo,
        fs::canonicalize(tree.root.join("release-path"))
            .expect("release path canonicalizes")
            .join("cargo")
    );

    fs::write(&workspace_config, "[build]\nrustc = 7\n")
        .expect("malformed workspace config installs");
    assert!(matches!(
        Programs::discover(&workspace, &environment),
        Err(TargetBuildError::CargoConfig { ref path, .. }) if path == &workspace_config
    ));

    fs::remove_file(&workspace_config).expect("workspace config removes");
    fs::remove_file(tree.root.join("cargo-home/config.toml")).expect("Cargo home config removes");
    fs::remove_file(path_rustc).expect("PATH compiler removes");
    assert!(matches!(
        Programs::discover(&workspace, &environment),
        Err(TargetBuildError::MissingProgram("rustc"))
    ));
}

/// A Cargo config naming `build.rustc` decides ahead of the `PATH` walk.
///
/// The `PATH` here really does answer -- it holds an executable `rustc` -- so
/// the config winning is the config outranking a live alternative rather than
/// filling a gap. Measured against the installed Cargo: a build with
/// `build.rustc` in a config and a healthy compiler on `PATH` dies on the one
/// the config names.
#[cfg(unix)]
#[test]
fn a_cargo_config_names_the_compiler_ahead_of_the_path_walk() {
    let tree = ConfigTree::new();
    tree.config("", "config.toml", "[build]\nrustc = \"/named/by/config\"\n");
    let environment = resolving_environment(tree.path_with_rustc(), None);

    assert_eq!(
        environment
            .rustc(&tree.build_dir("app"))
            .expect("a readable config resolves"),
        Some(PathBuf::from("/named/by/config")),
    );
}

/// The nearer config decides, and any of them decides over the Cargo home.
///
/// Three ranks in one assertion set because they are one ordering: the walk
/// runs outward from the build directory and the Cargo home is asked only
/// after it runs out. Splitting them into separate fixtures would let a
/// resolver that asked the Cargo home first pass two of the three.
#[cfg(unix)]
#[test]
fn the_nearest_cargo_config_decides_and_the_cargo_home_is_asked_last() {
    let tree = ConfigTree::new();
    tree.config("outer", "config.toml", "[build]\nrustc = \"/outer\"\n");
    tree.config("outer/mid", "config.toml", "[build]\nrustc = \"/mid\"\n");
    let cargo_home = tree.cargo_home("config.toml", "[build]\nrustc = \"/cargo-home\"\n");
    let path = tree.path_with_rustc();
    let build_dir = tree.build_dir("outer/mid/app");

    assert_eq!(
        resolving_environment(path.clone(), Some(cargo_home.clone()))
            .rustc(&build_dir)
            .expect("a readable config resolves"),
        Some(PathBuf::from("/mid")),
        "the nearer config did not decide",
    );
    assert_eq!(
        resolving_environment(path.clone(), Some(cargo_home.clone()))
            .rustc(&tree.build_dir("outer/app"))
            .expect("a readable config resolves"),
        Some(PathBuf::from("/outer")),
        "an ancestor config did not outrank the Cargo home",
    );
    assert_eq!(
        resolving_environment(path, Some(cargo_home))
            .rustc(&tree.build_dir("elsewhere/app"))
            .expect("a readable config resolves"),
        Some(PathBuf::from("/cargo-home")),
        "the Cargo home did not decide when nothing in the walk named a compiler",
    );
}

/// A config that leaves the key unset does not end the walk.
///
/// Cargo merges these files key by key, so a nearer config that says nothing
/// about `build.rustc` leaves the question to its parent. A resolver that
/// stopped at the first file it found would answer from `PATH` here, which is
/// a different compiler than the one Cargo would run.
#[cfg(unix)]
#[test]
fn a_cargo_config_without_the_key_leaves_the_question_to_its_parent() {
    let tree = ConfigTree::new();
    tree.config("outer", "config.toml", "[build]\nrustc = \"/outer\"\n");
    tree.config("outer/mid", "config.toml", "[net]\noffline = true\n");
    let environment = resolving_environment(tree.path_with_rustc(), None);

    assert_eq!(
        environment
            .rustc(&tree.build_dir("outer/mid/app"))
            .expect("a readable config resolves"),
        Some(PathBuf::from("/outer")),
    );
}

/// Within one directory the extension-less name is the one Cargo reads.
///
/// Measured against the installed Cargo: a `.cargo` holding both warns that
/// `config` is deprecated in favour of `config.toml` and then uses `config`.
/// The two files name different compilers here, because with the same name in
/// both a reader that opened the wrong one would answer correctly anyway.
#[cfg(unix)]
#[test]
fn the_extension_less_cargo_config_is_the_one_read() {
    let tree = ConfigTree::new();
    tree.config("", "config", "[build]\nrustc = \"/extension-less\"\n");
    tree.config("", "config.toml", "[build]\nrustc = \"/dot-toml\"\n");
    let environment = resolving_environment(tree.path_with_rustc(), None);

    assert_eq!(
        environment
            .rustc(&tree.build_dir("app"))
            .expect("a readable config resolves"),
        Some(PathBuf::from("/extension-less")),
    );
}

/// A config that cannot be understood refuses rather than answering.
///
/// Five shapes are driven because they stop the reader in five different
/// places, and two of them hide the key rather than malform it: an inline
/// `build` table and a quoted dotted key both carry a compiler name that a
/// reader placing only the two spellings it likes would never see. The danger
/// is identical in every case -- the `PATH` here holds a working compiler, so a
/// resolver that stepped over the file would hand back a real path and the
/// caller would admit a release for a build that was never going to start.
#[cfg(unix)]
#[test]
fn a_cargo_config_that_cannot_be_understood_refuses_the_resolution() {
    for (label, body, expected) in [
        (
            "unclosed header",
            "[build\nrustc = \"/x\"\n",
            "a table header does not close",
        ),
        (
            "not a string",
            "[build]\nrustc = 7\n",
            "build.rustc is not one quoted string",
        ),
        (
            "build is one value",
            "build = 7\n",
            "build is given as one value",
        ),
        (
            "the key inside an inline table",
            "build = { rustc = \"/hidden\" }\n",
            "build is given as one value",
        ),
        (
            "the key spelled as one quoted dotted key",
            "\"build.rustc\" = \"/hidden\"\n",
            "a key names rustc somewhere this reader cannot place it",
        ),
    ] {
        let tree = ConfigTree::new();
        let written = tree.config("", "config.toml", body);
        let environment = resolving_environment(tree.path_with_rustc(), None);

        let error = environment
            .rustc(&tree.build_dir("app"))
            .expect_err("a config that cannot be understood is refused");
        let refusal = error.to_string();
        assert!(
            refusal.contains(&written.display().to_string()),
            "the {label} refusal never names the file it read: {refusal}",
        );
        assert!(
            refusal.contains(expected),
            "the {label} refusal never says what was wrong: {refusal}",
        );
    }
}

/// A config that exists and cannot be opened refuses rather than answering.
///
/// The unreadable half of the law the shapes above cover from the other side.
/// A file Cargo would open and this one cannot is not an absent file, and
/// answering as though it were absent hands back the `PATH` compiler for a
/// build Cargo was never going to start. The body names a compiler, so a
/// resolver that opened the file successfully would answer with it rather than
/// refuse, and the mode is put back before the assertions run so a failing
/// assertion still leaves a readable tree to clean up.
#[cfg(unix)]
#[test]
fn a_cargo_config_that_cannot_be_opened_refuses_the_resolution() {
    use std::os::unix::fs::PermissionsExt as _;

    let tree = ConfigTree::new();
    let body = "[build]\nrustc = \"/named/by/config\"\n";
    let written = tree.config("", "config.toml", body);
    fs::set_permissions(&written, fs::Permissions::from_mode(0o000)).expect("deny the config");
    let environment = resolving_environment(tree.path_with_rustc(), None);

    let refused = environment.rustc(&tree.build_dir("app"));
    fs::set_permissions(&written, fs::Permissions::from_mode(0o644)).expect("restore the config");

    let error = refused.expect_err("a config that cannot be opened is refused");
    let refusal = error.to_string();
    assert!(
        refusal.contains(&written.display().to_string()),
        "the refusal never names the file it could not open: {refusal}",
    );
    assert!(
        refusal.contains("Permission denied"),
        "the refusal never says why the file could not be read: {refusal}",
    );
}

/// Both spellings resolve, and the ordinary company they keep is read through.
///
/// Every body here puts something above the key that a reader guarding itself
/// too broadly would stop on, so stopping costs the answer instead of passing
/// unnoticed. `rustc-wrapper` names what wraps the compiler rather than which
/// compiler runs and is exactly what this machine's own
/// `~/.cargo/config.toml` sets; a multi-line `rustflags` list is the other
/// everyday shape, and it is the reason lines without a `=` are walked through
/// rather than refused. Cargo reads all three without complaint, and a release
/// tool that refused them would refuse every stage on a developer's machine.
#[cfg(unix)]
#[test]
fn both_spellings_resolve_through_the_company_a_config_keeps() {
    for (label, body) in [
        (
            "under a header",
            "[build]\nrustc-wrapper = \"/sccache\"\nrustc = \"/named/by/config\"\n",
        ),
        (
            "as dotted keys",
            "build.rustc-wrapper = \"/sccache\"\nbuild.rustc = \"/named/by/config\"\n",
        ),
        (
            "below a multi-line list",
            concat!(
                "[build]\nrustflags = [\n  \"-C\",\n  \"target-cpu=native\",\n]\n",
                "rustc = \"/named/by/config\"\n"
            ),
        ),
    ] {
        let tree = ConfigTree::new();
        tree.config("", "config.toml", body);
        let environment = resolving_environment(tree.path_with_rustc(), None);

        assert_eq!(
            environment
                .rustc(&tree.build_dir("app"))
                .unwrap_or_else(|error| panic!("the config written {label} is readable: {error}")),
            Some(PathBuf::from("/named/by/config")),
            "the config written {label} did not decide the compiler",
        );
    }
}

/// Either compiler override outranks every config file.
///
/// Both spellings are driven against the same config, because a resolver that
/// asked the files before the environment would answer identically for the one
/// it still happened to ask first.
#[cfg(unix)]
#[test]
fn a_compiler_override_outranks_every_cargo_config() {
    for (label, rustc, build_rustc) in [
        ("RUSTC", Some(OsString::from("/by-rustc")), None),
        ("CARGO_BUILD_RUSTC", None, Some(OsString::from("/by-rustc"))),
    ] {
        let tree = ConfigTree::new();
        tree.config("", "config.toml", "[build]\nrustc = \"/by-config\"\n");
        let environment = ReleaseEnvironment::new(
            Some(tree.path_with_rustc()),
            None,
            Some(tree.cargo_home("config.toml", "[build]\nrustc = \"/by-cargo-home\"\n")),
            None,
            None,
            None,
            None,
            None,
            None,
            rustc,
            build_rustc,
        );

        assert_eq!(
            environment
                .rustc(&tree.build_dir("app"))
                .expect("an override resolves without reading a config"),
            Some(PathBuf::from("/by-rustc")),
            "{label} did not outrank the config files",
        );
    }
}
