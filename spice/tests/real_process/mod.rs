use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, SystemTime};

const FIXTURE_RERUN_VARIABLE: &str = "SPICE_FIXTURE_RERUN"; // env-policy: allow

struct Fixture {
    root: PathBuf,
    bootstrap: PathBuf,
    first: PathBuf,
    second: PathBuf,
    cargo: PathBuf,
    seed_count: PathBuf,
    hold: PathBuf,
    ready: PathBuf,
    release: PathBuf,
    finished: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-cargo-real-process-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        let bootstrap = root.join("cargo-home");
        let dependency = root.join("dependency");
        let first = root.join("first");
        let second = root.join("second");
        let cargo = root.join("native-cargo");
        let seed_count = root.join("seed-count");
        let hold = root.join("hold-build-script");
        let ready = root.join("build-script-ready");
        let release = root.join("release-build-script");
        let finished = root.join("build-script-finished");
        fs::create_dir(&root).expect("create real-process fixture root");
        fs::create_dir(&bootstrap).expect("create real bootstrap Cargo home");
        fs::create_dir_all(first.join("src")).expect("create real first worktree");
        let revision = initialize_dependency(
            &dependency,
            BuildScriptSignals {
                hold: &hold,
                ready: &ready,
                release: &release,
                finished: &finished,
            },
        );

        fs::write(
            first.join("Cargo.toml"),
            format!(
                concat!(
                    "[package]\n",
                    "name = \"fixture-root\"\n",
                    "version = \"0.0.0\"\n",
                    "edition = \"2024\"\n\n",
                    "[dependencies]\n",
                    "fixture-dependency = {{ git = \"file://{}\", rev = \"{}\" }}\n",
                ),
                dependency.display(),
                revision,
            ),
        )
        .expect("write real repository manifest");
        fs::write(
            first.join("src/lib.rs"),
            "pub fn answer() -> u8 { fixture_dependency::answer() }\n",
        )
        .expect("write real repository source");
        let native = native_cargo();
        assert_success(
            Command::new(&native)
                .arg("generate-lockfile")
                .current_dir(&first)
                .env("CARGO_HOME", &bootstrap)
                .output()
                .expect("generate real lockfile"),
        );
        initialize_repository(&first);
        git(
            &first,
            &[
                "worktree",
                "add",
                "-b",
                "fixture-second",
                second.to_str().expect("UTF-8 real fixture path"),
            ],
        );
        fs::write(
            &cargo,
            format!(
                concat!(
                    "#!/bin/sh\n",
                    "set -eu\n",
                    "if [ \"${{1-}}\" = vendor ]; then printf 'seed\\n' >> {}; fi\n",
                    "exec {} \"$@\"\n",
                ),
                shell_word(&seed_count),
                shell_word(&native),
            ),
        )
        .expect("write real Cargo counting wrapper");
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o700))
            .expect("make real Cargo wrapper executable");

        Self {
            root,
            bootstrap,
            first,
            second,
            cargo,
            seed_count,
            hold,
            ready,
            release,
            finished,
        }
    }

    fn cargo_check(&self, worktree: &Path) -> Command {
        let mut command = self.spice(worktree);
        command
            .args(["dev", "cargo", "--", "check", "--locked", "--offline"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn spice(&self, worktree: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        command
            .current_dir(worktree)
            .env(
                spice::cargo_isolation::BOOTSTRAP_HOME_VARIABLE,
                &self.bootstrap,
            )
            .env(spice::cargo_isolation::NATIVE_CARGO_VARIABLE, &self.cargo)
            .env_remove("CARGO_HOME")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_BUILD_DIR");
        command
    }

    fn cargo_metadata(&self, worktree: &Path) -> Output {
        self.spice(worktree)
            .args([
                "dev",
                "cargo",
                "--",
                "metadata",
                "--locked",
                "--offline",
                "--format-version",
                "1",
                "--no-deps",
            ])
            .output()
            .expect("run metadata through refreshed private home")
    }
}

struct BuildScriptSignals<'a> {
    hold: &'a Path,
    ready: &'a Path,
    release: &'a Path,
    finished: &'a Path,
}

fn initialize_dependency(dependency: &Path, signals: BuildScriptSignals<'_>) -> String {
    fs::create_dir_all(dependency.join("src")).expect("create real dependency fixture");
    fs::write(
        dependency.join("Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"fixture-dependency\"\n",
            "version = \"1.0.0\"\n",
            "edition = \"2024\"\n",
            "build = \"build.rs\"\n",
        ),
    )
    .expect("write real dependency manifest");
    fs::write(
        dependency.join("src/lib.rs"),
        "pub fn answer() -> u8 { 42 }\n",
    )
    .expect("write real dependency source");
    fs::write(
        dependency.join("template.rs"),
        "pub const COPIED: u8 = 42;\n",
    )
    .expect("write copied build-script input");
    fs::write(
        dependency.join("build.rs"),
        format!(
            concat!(
                "fn main() {{\n",
                "    println!(\"cargo:rerun-if-env-changed=SPICE_FIXTURE_RERUN\");\n",
                "    if std::path::Path::new({hold:?}).exists() {{\n",
                "        std::fs::write({ready:?}, b\"ready\").unwrap();\n",
                "        while !std::path::Path::new({release:?}).exists() {{\n",
                "            std::thread::sleep(std::time::Duration::from_millis(10));\n",
                "        }}\n",
                "    }}\n",
                "    let output = std::path::PathBuf::from(std::env::var_os(\"OUT_DIR\").unwrap());\n",
                "    std::fs::copy(\"template.rs\", output.join(\"copied.rs\")).unwrap();\n",
                "    std::fs::write({finished:?}, b\"finished\").unwrap();\n",
                "    std::thread::sleep(std::time::Duration::from_millis(300));\n",
                "}}\n",
            ),
            hold = signals.hold.to_str().expect("UTF-8 hold path"),
            ready = signals.ready.to_str().expect("UTF-8 readiness path"),
            release = signals.release.to_str().expect("UTF-8 release path"),
            finished = signals.finished.to_str().expect("UTF-8 finished path"),
        ),
    )
    .expect("write real dependency build script");
    initialize_repository(dependency);
    git_output(dependency, &["rev-parse", "HEAD"])
}

impl Drop for Fixture {
    fn drop(&mut self) {
        make_writable(&self.root);
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn linked_worktrees_run_genuine_cargo_without_a_shared_home_lock() {
    let fixture = Fixture::new();
    let cold = run_together(&fixture);
    assert_clean_pair(&cold);
    assert_eq!(seed_count(&fixture), 1, "one cold vendor transaction");
    let homes = private_homes(&fixture);
    assert_eq!(homes.len(), 2);
    assert_private_homes(&homes);
    assert_shared_generation(&fixture);

    for worktree in [&fixture.first, &fixture.second] {
        fs::remove_dir_all(worktree.join("target")).expect("remove genuine Cargo build output");
    }
    let first = fixture
        .cargo_check(&fixture.first)
        .env(FIXTURE_RERUN_VARIABLE, "warm")
        .spawn()
        .expect("launch first warm genuine Cargo");
    let second = fixture
        .cargo_check(&fixture.second)
        .env(FIXTURE_RERUN_VARIABLE, "warm")
        .spawn()
        .expect("launch second warm genuine Cargo");
    std::thread::sleep(Duration::from_millis(100));
    let cleanup = fixture
        .spice(&fixture.first)
        .args(["dev", "cargo-store", "--cleanup"])
        .output()
        .expect("run cleanup beside genuine Cargo readers");
    assert_success(cleanup);
    let warm = [
        first.wait_with_output().expect("wait for first warm Cargo"),
        second
            .wait_with_output()
            .expect("wait for second warm Cargo"),
    ];
    assert_clean_pair(&warm);
    assert_eq!(seed_count(&fixture), 1, "warm launches reuse generation");
    assert_shared_generation(&fixture);
}

#[test]
fn cleanup_pins_the_generation_of_a_live_cargo_child_across_home_advance() {
    let fixture = Fixture::new();
    fs::write(&fixture.hold, b"hold").expect("arm blocking dependency build script");
    let mut first = fixture
        .cargo_check(&fixture.first)
        .spawn()
        .expect("launch Cargo on generation A");
    wait_for_path(&fixture.ready);
    first.kill().expect("crash the generation A wrapper");
    assert!(!first.wait().expect("reap crashed wrapper").success());

    let store = fixture.bootstrap.join("spice/source-store-v2");
    let generation_a = only_directory(&store.join("generations"));
    let mut refreshed = b"# generation B\n".to_vec();
    refreshed.extend(fs::read(fixture.first.join("Cargo.lock")).expect("read generation A lock"));
    fs::write(fixture.first.join("Cargo.lock"), refreshed).expect("advance exact lock bytes");
    assert_success(fixture.cargo_metadata(&fixture.first));
    let generations = directory_paths(&store.join("generations"));
    assert_eq!(generations.len(), 2);
    let generation_b = generations
        .into_iter()
        .find(|path| path != &generation_a)
        .expect("generation B was published");

    fs::File::open(&generation_a)
        .expect("open generation A for deterministic aging")
        .set_times(
            fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        )
        .expect("age generation A beyond cleanup grace");
    assert_success(
        fixture
            .spice(&fixture.first)
            .args(["dev", "cargo-store", "--cleanup"])
            .output()
            .expect("clean while Cargo A is live"),
    );
    assert!(
        generation_a.exists(),
        "the child-inherited pin retains generation A after its wrapper crashes",
    );
    assert!(generation_b.exists());

    fs::write(&fixture.release, b"release").expect("release Cargo A source read");
    wait_for_path(&fixture.finished);
    wait_for_reclamation(&fixture, &generation_a);
    assert!(
        generation_b.exists(),
        "the current home retains generation B"
    );
}

fn run_together(fixture: &Fixture) -> [Output; 2] {
    let first = fixture
        .cargo_check(&fixture.first)
        .env(FIXTURE_RERUN_VARIABLE, "cold")
        .spawn()
        .expect("launch first cold genuine Cargo");
    let second = fixture
        .cargo_check(&fixture.second)
        .env(FIXTURE_RERUN_VARIABLE, "cold")
        .spawn()
        .expect("launch second cold genuine Cargo");
    [
        first.wait_with_output().expect("wait for first cold Cargo"),
        second
            .wait_with_output()
            .expect("wait for second cold Cargo"),
    ]
}

fn assert_clean_pair(outputs: &[Output; 2]) {
    for output in outputs {
        assert_success_ref(output);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("Blocking waiting for file lock"),
            "{stderr}"
        );
        assert!(!stderr.contains("package cache lock"), "{stderr}");
        assert!(!stderr.contains("global-cache"), "{stderr}");
    }
}

fn private_homes(fixture: &Fixture) -> Vec<PathBuf> {
    let mut homes = fs::read_dir(fixture.bootstrap.join("spice/source-store-v2/homes"))
        .expect("read real private homes")
        .map(|entry| entry.expect("read real home entry").path())
        .collect::<Vec<_>>();
    homes.sort();
    homes
}

fn assert_private_homes(homes: &[PathBuf]) {
    for home in homes {
        assert_eq!(mode(home), 0o700);
        assert!(tree_bytes(home) <= 1024 * 1024);
        assert!(!home.join("registry").exists());
        assert!(!home.join("git").exists());
    }
    let databases = homes
        .iter()
        .map(|home| home.join(".global-cache"))
        .filter(|database| database.exists())
        .collect::<Vec<_>>();
    assert!(databases.is_empty() || databases.len() == homes.len());
    if let [first, second] = databases.as_slice() {
        assert_ne!(
            fs::metadata(first).expect("first Cargo database").ino(),
            fs::metadata(second).expect("second Cargo database").ino(),
        );
    }
}

fn assert_shared_generation(fixture: &Fixture) {
    let store = fixture.bootstrap.join("spice/source-store-v2");
    assert_eq!(normal_directories(&store.join("generations")), 1);
    assert_eq!(normal_directories(&store.join("packages")), 1);
    assert_eq!(normal_directories(&store.join("homes")), 2);
}

fn seed_count(fixture: &Fixture) -> usize {
    fs::read_to_string(&fixture.seed_count)
        .expect("read real seed counter")
        .lines()
        .count()
}

fn wait_for_path(path: &Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {path:?}",
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_reclamation(fixture: &Fixture, generation: &Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while generation.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out reclaiming {generation:?}",
        );
        assert_success(
            fixture
                .spice(&fixture.first)
                .args(["dev", "cargo-store", "--cleanup"])
                .output()
                .expect("clean after Cargo A exits"),
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn only_directory(path: &Path) -> PathBuf {
    let paths = directory_paths(path);
    assert_eq!(paths.len(), 1, "expected one directory below {path:?}");
    paths.into_iter().next().expect("one directory")
}

fn directory_paths(path: &Path) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(path)
        .expect("read owned directories")
        .map(|entry| entry.expect("read owned directory").path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn native_cargo() -> PathBuf {
    let cargo = PathBuf::from(env!("CARGO"));
    let metadata = fs::metadata(&cargo).expect("Cargo supplies its executable to the test");
    assert!(
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0,
        "Cargo's test executable is runnable: {cargo:?}",
    );
    cargo
}

fn initialize_repository(repository: &Path) {
    git(repository, &["init"]);
    git(repository, &["config", "user.name", "Spice Test"]);
    git(
        repository,
        &["config", "user.email", "spice@example.invalid"],
    );
    git(repository, &["config", "core.hooksPath", "/dev/null"]);
    git(repository, &["add", "."]);
    git(repository, &["commit", "-m", "fixture"]);
}

fn git(repository: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .expect("launch fixture Git");
    assert_success(output);
}

fn git_output(repository: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .expect("launch fixture Git query");
    assert_success_ref(&output);
    String::from_utf8(output.stdout)
        .expect("fixture Git emits UTF-8")
        .trim()
        .to_owned()
}

fn assert_success(output: Output) {
    assert_success_ref(&output);
}

fn assert_success_ref(output: &Output) {
    assert!(
        output.status.success(),
        "process failed: status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn normal_directories(path: &Path) -> usize {
    fs::read_dir(path)
        .expect("read owned directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .count()
}

fn tree_bytes(path: &Path) -> u64 {
    fs::read_dir(path)
        .expect("read private home")
        .filter_map(Result::ok)
        .map(|entry| {
            let metadata = fs::symlink_metadata(entry.path()).expect("measure home entry");
            if metadata.is_dir() {
                tree_bytes(&entry.path())
            } else {
                metadata.len()
            }
        })
        .sum()
}

fn shell_word(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .expect("read fixture mode")
        .permissions()
        .mode()
        & 0o777
}

fn make_writable(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    if metadata.is_dir() {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                make_writable(&entry.path());
            }
        }
    } else {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
}
