#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use sill::{Acquisition, Availability, LockFile, LockName, Possession};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
const LOCK_NAME: &str = ".cargo-build-lock";
const WAIT_LIMIT: Duration = Duration::from_secs(15);

/// Cargo's own profile lock is an externally observable active-build marker.
///
/// The fixture waits inside a genuine Cargo build script, after Cargo has
/// acquired the build-directory lock. A second process's nonblocking kernel
/// probe sees that file held, then sees the same permanent file available once
/// the build exits. Presence alone cannot satisfy either assertion.
#[test]
fn a_cargo_build_lock_distinguishes_an_active_root_from_an_idle_one() {
    let scratch = Scratch::new("active");
    let fixture = CargoFixture::new(&scratch);
    let mut build = fixture.start_waiting_build();

    await_path(&fixture.ready);
    await_path(&fixture.lock_path());
    assert!(
        fixture.build.join("CACHEDIR.TAG").is_file(),
        "Cargo did not identify its build root as a cache"
    );
    assert_eq!(
        fixture
            .lock()
            .availability()
            .expect("probe active Cargo lock"),
        Availability::Held,
        "a real build did not publish a held lock outside its process"
    );

    build.finish();
    assert_eq!(
        fixture
            .lock()
            .availability()
            .expect("probe idle Cargo lock"),
        Availability::Available,
        "the permanent Cargo lock file could not distinguish an idle root"
    );
}

/// The pinned Cargo collector does not manage build directories at all.
///
/// This is deliberately a negative result rather than an inference from
/// documentation. Stable automatic collection is forced once while the build
/// lock is held and once after release, and Cargo's own trace must say it ran.
/// It leaves the witness in both states, establishing that it neither honours
/// nor violates this marker: it never visits the build root.
///
/// The negative only means that if the collector was told about the root it
/// spared. It was: every invocation here is configured with the same
/// `build.build-dir` the witness and the marker live in, and that root is
/// Cargo's own creation rather than a directory the fixture made up. The
/// collector runs under a different *profile*, which is what keeps it from
/// blocking on the marker this fixture holds -- `.cargo-build-lock` is per
/// profile, so a release-profile collector and a held dev-profile marker share
/// a root without contending. Measured: holding `debug` lets a `--release`
/// invocation finish in 0.16s, while holding `release` blocks the same
/// invocation past 20s, so the separation is real and so is the lock.
#[test]
fn cargo_gc_visits_neither_a_held_nor_an_idle_build_directory() {
    let scratch = Scratch::new("collector");
    let fixture = CargoFixture::new(&scratch);
    fixture.prime_gc();
    fixture.assert_collector_root();

    let witness = fixture.build.join("collector-witness");
    fs::write(&witness, b"build directory is outside Cargo home").expect("write collector witness");

    let held = acquired(fixture.lock().try_acquire().expect("hold Cargo lock"));
    assert_eq!(
        fixture.lock().availability().expect("probe held marker"),
        Availability::Held,
        "the collector ran against an idle root, so a held root was never tested"
    );
    fixture.run_gc();
    assert!(witness.is_file(), "Cargo gc removed a held build directory");

    drop(held);
    assert_eq!(
        fixture.lock().availability().expect("probe idle marker"),
        Availability::Available,
        "the marker stayed held, so an idle root was never tested"
    );
    fixture.run_gc();
    assert!(
        witness.is_file(),
        "Cargo gc removed an idle build directory"
    );
    fixture.assert_collector_root();
}

fn acquired(acquisition: Acquisition) -> Possession {
    match acquisition {
        Acquisition::Acquired(possession) => possession,
        Acquisition::Contended => panic!("fixture Cargo lock is unexpectedly held"),
    }
}

fn await_path(path: &Path) {
    let started = Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < WAIT_LIMIT,
            "timed out waiting for {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn assert_success(output: &Output, operation: &str) {
    assert!(
        output.status.success(),
        "{operation} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

struct CargoFixture {
    cargo_home: PathBuf,
    workspace: PathBuf,
    target: PathBuf,
    build: PathBuf,
    ready: PathBuf,
    release: PathBuf,
}

impl CargoFixture {
    fn new(scratch: &Scratch) -> Self {
        let cargo_home = scratch.join("cargo-home");
        let workspace = scratch.join("workspace");
        let target = scratch.join("target");
        let build = scratch.join("build");
        let ready = scratch.join("build-ready");
        let release = scratch.join("build-release");
        fs::create_dir_all(workspace.join("src")).expect("create Cargo fixture source");
        fs::create_dir_all(&cargo_home).expect("create isolated Cargo home");
        fs::write(
            workspace.join("Cargo.toml"),
            concat!(
                "[package]\n",
                "name = \"build-lock-fixture\"\n",
                "version = \"0.0.0\"\n",
                "edition = \"2024\"\n",
                "build = \"build.rs\"\n",
                "\n[workspace]\n",
            ),
        )
        .expect("write Cargo fixture manifest");
        fs::write(workspace.join("src/lib.rs"), "pub fn fixture() {}\n")
            .expect("write Cargo fixture library");
        fs::write(
            workspace.join("build.rs"),
            concat!(
                "use std::{env, fs, path::Path, thread, time::Duration};\n",
                "fn main() {\n",
                "    let ready = env::var_os(\"BUILD_LOCK_READY\").expect(\"ready path\");\n",
                "    let release = env::var_os(\"BUILD_LOCK_RELEASE\").expect(\"release path\");\n",
                "    fs::write(ready, b\"ready\").expect(\"publish readiness\");\n",
                "    while !Path::new(&release).exists() {\n",
                "        thread::sleep(Duration::from_millis(10));\n",
                "    }\n",
                "}\n",
            ),
        )
        .expect("write blocking Cargo build script");
        Self {
            cargo_home,
            workspace,
            target,
            build,
            ready,
            release,
        }
    }

    fn lock_path(&self) -> PathBuf {
        self.build.join("debug").join(LOCK_NAME)
    }

    /// The marker the collector's own profile takes, which is what lets the
    /// collector share a build root with a held marker without blocking on it.
    fn collector_lock_path(&self) -> PathBuf {
        self.build.join("release").join(LOCK_NAME)
    }

    /// Prove the observed root is the one Cargo was configured with and built
    /// into, rather than any directory that merely happens to sit nearby.
    ///
    /// An arbitrary unconfigured sibling satisfies none of these: Cargo tags
    /// only roots it creates, and the two profile markers exist only because
    /// this fixture's own invocations were told to build here.
    fn assert_collector_root(&self) {
        assert!(
            self.build.join("CACHEDIR.TAG").is_file(),
            "the observed root is not a Cargo-created build directory"
        );
        assert!(
            self.lock_path().is_file(),
            "the observed root holds no dev-profile marker for this fixture to take"
        );
        assert!(
            self.collector_lock_path().is_file(),
            "the collector never built into the observed root"
        );
        assert_ne!(
            self.lock_path(),
            self.collector_lock_path(),
            "the collector would block on the very marker this fixture holds"
        );
    }

    fn lock(&self) -> LockFile {
        LockFile::new(
            LockName::new(LOCK_NAME).expect("Cargo lock name is one component"),
            self.lock_path(),
        )
    }

    fn start_waiting_build(&self) -> RunningBuild {
        let child = self
            .cargo_command()
            .arg("build")
            .arg("--quiet")
            .env("BUILD_LOCK_READY", &self.ready)
            .env("BUILD_LOCK_RELEASE", &self.release)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start genuine Cargo build");
        RunningBuild {
            child: Some(child),
            release: self.release.clone(),
        }
    }

    /// Build both profiles into the one root every collector run observes, so
    /// the witness sits in a genuine Cargo build directory and both profile
    /// markers are Cargo's own files rather than ones the fixture invented.
    fn prime_gc(&self) {
        fs::write(&self.release, b"release").expect("release collector build script");
        let output = self
            .collector_command()
            .output()
            .expect("prime the fixture's dev profile");
        assert_success(&output, "prime the fixture's dev profile");
        let output = self
            .collector_command()
            .arg("--release")
            .output()
            .expect("prime installed Cargo collector");
        assert_success(&output, "prime installed Cargo collector");
    }

    fn run_gc(&self) {
        // Cargo records collection time at whole-second precision and tests
        // `last < now`, so cross one tick before asking `always` to run.
        thread::sleep(Duration::from_millis(1_100));
        let output = self
            .collector_command()
            .arg("--release")
            .env("CARGO_CACHE_AUTO_CLEAN_FREQUENCY", "always")
            .env("CARGO_LOG", "gc=trace")
            .output()
            .expect("run installed Cargo automatic collector");
        assert_success(&output, "installed Cargo automatic collector");
        let trace = String::from_utf8_lossy(&output.stderr);
        assert!(
            trace.contains("last auto gc was") && trace.contains("running"),
            "Cargo did not prove automatic collection ran: {trace}"
        );
        assert!(
            trace.contains("cleaning GcOpts"),
            "Cargo did not enter its global-cache collector: {trace}"
        );
    }

    fn cargo_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO"));
        command
            .current_dir(&self.workspace)
            .env("CARGO_HOME", &self.cargo_home)
            .env("CARGO_TARGET_DIR", &self.target)
            .env("CARGO_BUILD_BUILD_DIR", &self.build);
        command
    }

    /// A Cargo invocation configured with the build root under observation.
    ///
    /// Deliberately *not* given a separate `build.build-dir`: a collector run
    /// pointed somewhere else proves nothing about the root whose survival is
    /// asserted, which is the defect this fixture was rebuilt to close.
    fn collector_command(&self) -> Command {
        let mut command = self.cargo_command();
        command
            .arg("build")
            .arg("--quiet")
            .env("BUILD_LOCK_READY", &self.ready)
            .env("BUILD_LOCK_RELEASE", &self.release);
        command
    }
}

struct RunningBuild {
    child: Option<Child>,
    release: PathBuf,
}

impl RunningBuild {
    fn finish(&mut self) {
        fs::write(&self.release, b"release").expect("release blocking build script");
        let child = self.child.take().expect("build child remains live");
        let output = child.wait_with_output().expect("wait for Cargo build");
        assert_success(&output, "genuine Cargo build");
    }
}

impl Drop for RunningBuild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let released = fs::write(&self.release, b"release").is_ok();
            if !released {
                let _kill = child.kill();
            }
            let _wait = child.wait();
        }
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "spice-cargo-build-cache-{label}-{}-{}",
            std::process::id(),
            NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create build-cache scratch");
        Self(path)
    }

    fn join(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.0.join(relative)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove build-cache scratch");
    }
}
