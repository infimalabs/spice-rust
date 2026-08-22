#![cfg(unix)]

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use sill::{Acquisition, Availability, LockFile, LockName, Possession};
use xtask::{
    BuildCacheContext, BuildCacheDisposition, BuildCacheError, BuildCacheRequest,
    BuildCacheRootKind, BuildCacheRootReport, BuildCacheTargetDirectory, collect_build_cache_at,
    parse_byte_quantity,
};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
const CACHE_TAG: &str = "CACHEDIR.TAG";
const LOCK_NAME: &str = ".cargo-build-lock";
const TARGET_LOCK_NAME: &str = ".cargo-lock";
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

#[test]
fn build_cache_dry_run_inventories_and_selects_oldest_without_removing() {
    let scratch = Scratch::new("dry-run-lru");
    let cache = scratch.join("CargoBuild");
    let old = write_cache_root(&cache, "00", "00000000000000", "old");
    let newer = write_cache_root(&cache, "01", "00000000000000", "newer");
    let current = write_cache_root(&cache, "ff", "ffffffffffffff", "current");
    let inventory = collect_cache(&current, BuildCacheRequest::default(), Vec::new(), None);
    let old_bytes = root_report(&inventory, &old).bytes;
    let report = collect_cache(
        &current,
        BuildCacheRequest {
            max_bytes: Some(inventory.total_bytes - old_bytes),
            ..BuildCacheRequest::default()
        },
        Vec::new(),
        None,
    );

    assert_eq!(
        root_report(&report, &old).disposition,
        BuildCacheDisposition::WouldReclaim
    );
    assert_eq!(
        root_report(&report, &newer).disposition,
        BuildCacheDisposition::Kept
    );
    assert_eq!(
        root_report(&report, &current).disposition,
        BuildCacheDisposition::RetainedCurrent
    );
    assert!(old.is_dir(), "dry-run removed the selected old root");
    assert_eq!(report.reclaimed_bytes, 0);
    assert_eq!(report.selected_bytes, old_bytes);
}

#[test]
fn build_cache_applies_lru_cap_and_is_idempotent_on_an_eight_hundred_root_fixture() {
    let scratch = Scratch::new("large-lru");
    let cache = scratch.join("CargoBuild");
    let mut roots = Vec::new();
    for index in 0..800_u16 {
        roots.push(write_cache_root(
            &cache,
            &format!("{:02x}", index % 256),
            &format!("{index:014x}"),
            "payload",
        ));
    }
    let current = roots.last().expect("fixture root exists").clone();
    let inventory = collect_cache(&current, BuildCacheRequest::default(), Vec::new(), None);
    let current_bytes = root_report(&inventory, &current).bytes;
    let request = BuildCacheRequest {
        apply: true,
        max_bytes: Some(current_bytes),
        retain_live_worktrees: 0,
        ..BuildCacheRequest::default()
    };

    let started = Instant::now();
    let first = collect_cache(&current, request.clone(), Vec::new(), None);
    assert!(
        started.elapsed() < Duration::from_secs(120),
        "800-root collection exceeded the task bound"
    );
    assert!(first.reclaimed_bytes > 0, "large fixture reclaimed nothing");
    assert!(
        roots[..roots.len() - 1].iter().all(|root| !root.exists()),
        "collector left an old root behind"
    );
    assert!(current.is_dir(), "collector removed the current root");

    let second = collect_cache(&current, request, Vec::new(), None);
    assert_eq!(second.reclaimed_bytes, 0, "second run was not idempotent");
    assert_eq!(second.selected_bytes, 0, "second run selected a root");
}

#[test]
fn build_cache_skips_held_native_release_and_wasm_locks_until_release() {
    let scratch = Scratch::new("held-locks");
    let cache = scratch.join("CargoBuild");
    let current = write_cache_root(&cache, "ff", "ffffffffffffff", "current");
    let native = write_cache_root(&cache, "00", "00000000000000", "native");
    let release = write_cache_root(&cache, "01", "00000000000000", "release");
    let wasm = write_cache_root(&cache, "02", "00000000000000", "wasm");
    let held = vec![
        hold_lock(&add_lock(&native, "debug/.cargo-build-lock")),
        hold_lock(&add_lock(&release, "release/.cargo-build-lock")),
        hold_lock(&add_lock(
            &wasm,
            "wasm32-unknown-unknown/debug/.cargo-build-lock",
        )),
    ];
    let request = BuildCacheRequest {
        apply: true,
        max_bytes: Some(0),
        retain_live_worktrees: 0,
        ..BuildCacheRequest::default()
    };

    let blocked = collect_cache(&current, request.clone(), Vec::new(), None);
    for root in [&native, &release, &wasm] {
        assert_eq!(
            root_report(&blocked, root).disposition,
            BuildCacheDisposition::SkippedActive,
            "{} was not reported as active",
            root.display()
        );
        assert!(root.is_dir(), "{} was removed while locked", root.display());
    }

    drop(held);
    let collected = collect_cache(&current, request, Vec::new(), None);
    for root in [&native, &release, &wasm] {
        assert_eq!(
            root_report(&collected, root).disposition,
            BuildCacheDisposition::Reclaimed,
            "{} was not reclaimed after release",
            root.display()
        );
        assert!(!root.exists(), "{} survived after release", root.display());
    }
}

#[test]
fn build_cache_retains_current_and_configured_hottest_live_worktree_roots() {
    let scratch = Scratch::new("live-retention");
    let cache = scratch.join("CargoBuild");
    let victim = write_cache_root(&cache, "00", "00000000000000", "victim");
    let cold_live = write_cache_root(&cache, "01", "00000000000000", "cold-live");
    thread::sleep(Duration::from_millis(5));
    let hot_worktree = scratch.join("hot-worktree");
    let hot_live = write_target_root(&hot_worktree, "hot-live");
    let current = write_cache_root(&cache, "ff", "ffffffffffffff", "current");
    let request = BuildCacheRequest {
        apply: true,
        max_bytes: Some(0),
        retain_live_worktrees: 1,
        ..BuildCacheRequest::default()
    };

    let (report, _) = collect_cache_with_targets(
        &current,
        request,
        vec![cold_live.clone()],
        None,
        vec![BuildCacheTargetDirectory::new(&hot_worktree, &hot_live)],
        None,
    );

    assert_eq!(
        root_report(&report, &current).disposition,
        BuildCacheDisposition::RetainedCurrent
    );
    assert_eq!(
        root_report(&report, &hot_live).disposition,
        BuildCacheDisposition::RetainedLiveWorktree
    );
    assert_eq!(
        root_report(&report, &cold_live).disposition,
        BuildCacheDisposition::Reclaimed
    );
    assert_eq!(
        root_report(&report, &victim).disposition,
        BuildCacheDisposition::Reclaimed
    );
    assert!(current.is_dir(), "current worktree root was removed");
    assert!(hot_live.is_dir(), "hottest live worktree root was removed");
    assert!(
        !cold_live.exists(),
        "unretained live worktree root survived"
    );
}

#[test]
fn build_cache_uses_free_space_floor_as_a_reclaim_target() {
    let scratch = Scratch::new("free-floor");
    let cache = scratch.join("CargoBuild");
    let old = write_cache_root(&cache, "00", "00000000000000", "old");
    let current = write_cache_root(&cache, "ff", "ffffffffffffff", "current");
    let inventory = collect_cache(
        &current,
        BuildCacheRequest::default(),
        Vec::new(),
        Some(100),
    );
    let old_bytes = root_report(&inventory, &old).bytes;

    let report = collect_cache(
        &current,
        BuildCacheRequest {
            free_space_floor: Some(100 + old_bytes),
            ..BuildCacheRequest::default()
        },
        Vec::new(),
        Some(100),
    );

    assert_eq!(
        root_report(&report, &old).disposition,
        BuildCacheDisposition::WouldReclaim
    );
    assert_eq!(report.target_bytes, inventory.total_bytes - old_bytes);
}

#[test]
fn build_cache_removal_never_follows_symlinks_or_collects_malformed_root_links() {
    let scratch = Scratch::new("symlink-boundary");
    let cache = scratch.join("CargoBuild");
    let outside = scratch.join("outside");
    fs::create_dir(&outside).expect("create outside directory");
    fs::write(outside.join("kept"), b"outside").expect("write outside witness");
    let current = write_cache_root(&cache, "ff", "ffffffffffffff", "current");
    let linked_inside = write_cache_root(&cache, "00", "00000000000000", "linked-inside");
    fs::create_dir_all(linked_inside.join("debug")).expect("create symlink parent");
    std::os::unix::fs::symlink(&outside, linked_inside.join("debug/outside-link"))
        .expect("create inside symlink");
    let malformed_bucket = cache.join("01");
    fs::create_dir_all(&malformed_bucket).expect("create malformed bucket");
    let malformed = malformed_bucket.join("11111111111111");
    std::os::unix::fs::symlink(&outside, &malformed).expect("create root symlink");

    let report = collect_cache(
        &current,
        BuildCacheRequest {
            apply: true,
            max_bytes: Some(0),
            retain_live_worktrees: 0,
            ..BuildCacheRequest::default()
        },
        Vec::new(),
        None,
    );

    assert_eq!(
        root_report(&report, &linked_inside).disposition,
        BuildCacheDisposition::Reclaimed
    );
    assert!(
        outside.join("kept").is_file(),
        "collector followed a symlink outside the hierarchy"
    );
    assert!(
        malformed.exists(),
        "collector removed a symlink candidate root"
    );
    assert!(matches!(
        root_report(&report, &malformed).disposition,
        BuildCacheDisposition::SkippedMalformed(_)
    ));
}

#[test]
fn build_cache_dry_run_reports_managed_and_worktree_targets_in_one_inventory() {
    let scratch = Scratch::new("mixed-inventory");
    let cache = scratch.join("CargoBuild");
    let linked_build = write_cache_root(&cache, "00", "00000000000000", "linked-build");
    let current_build = write_cache_root(&cache, "ff", "ffffffffffffff", "current-build");
    let current_worktree = scratch.join("current-worktree");
    let linked_worktree = scratch.join("linked-worktree");
    let current_target = write_target_root(&current_worktree, "current-target");
    let linked_target = write_target_root(&linked_worktree, "linked-target");
    let (report, output) = collect_cache_with_targets(
        &current_build,
        BuildCacheRequest {
            max_bytes: Some(0),
            retain_live_worktrees: 0,
            ..BuildCacheRequest::default()
        },
        vec![linked_build.clone()],
        Some(BuildCacheTargetDirectory::new(
            &current_worktree,
            &current_target,
        )),
        vec![BuildCacheTargetDirectory::new(
            &linked_worktree,
            &linked_target,
        )],
        None,
    );

    assert_eq!(
        root_report(&report, &current_build).kind,
        BuildCacheRootKind::ManagedBuild
    );
    assert_eq!(
        root_report(&report, &current_target).kind,
        BuildCacheRootKind::WorktreeTarget
    );
    for current in [&current_build, &current_target] {
        assert_eq!(
            root_report(&report, current).disposition,
            BuildCacheDisposition::RetainedCurrent,
            "{} was not retained as a current root",
            current.display()
        );
    }
    for linked in [&linked_build, &linked_target] {
        assert_eq!(
            root_report(&report, linked).disposition,
            BuildCacheDisposition::WouldReclaim,
            "{} was not selected in the global LRU inventory",
            linked.display()
        );
    }
    assert_eq!(report.current_target, Some(current_target.clone()));
    assert_eq!(
        report.total_bytes,
        report.roots.iter().map(|root| root.bytes).sum::<u64>()
    );
    let target = root_report(&report, &linked_target);
    assert!(
        target
            .locks
            .iter()
            .any(|lock| lock.file_name() == Some(OsStr::new(TARGET_LOCK_NAME)))
    );
    assert!(
        target
            .locks
            .iter()
            .any(|lock| lock.file_name() == Some(OsStr::new(LOCK_NAME)))
    );
    assert!(
        output.contains("scope=managed-build-and-worktree-targets"),
        "{output}"
    );
    assert!(output.contains("kind=managed-build"), "{output}");
    assert!(output.contains("kind=worktree-target"), "{output}");
}

#[test]
fn build_cache_reprobes_target_and_build_locks_before_removal() {
    let scratch = Scratch::new("target-locks");
    let cache = scratch.join("CargoBuild");
    let current = write_cache_root(&cache, "ff", "ffffffffffffff", "current");
    let linked_worktree = scratch.join("linked-worktree");
    let target = write_target_root(&linked_worktree, "linked-target");
    let target_lock = target.join("debug").join(TARGET_LOCK_NAME);
    let held = hold_named_lock(&target_lock);
    let request = BuildCacheRequest {
        apply: true,
        max_bytes: Some(0),
        retain_live_worktrees: 0,
        ..BuildCacheRequest::default()
    };

    let (blocked, _) = collect_cache_with_targets(
        &current,
        request.clone(),
        Vec::new(),
        None,
        vec![BuildCacheTargetDirectory::new(&linked_worktree, &target)],
        None,
    );
    assert_eq!(
        root_report(&blocked, &target).disposition,
        BuildCacheDisposition::SkippedActive
    );
    assert!(target.is_dir(), "held worktree target was removed");

    drop(held);
    let (collected, _) = collect_cache_with_targets(
        &current,
        request,
        Vec::new(),
        None,
        vec![BuildCacheTargetDirectory::new(&linked_worktree, &target)],
        None,
    );
    assert_eq!(
        root_report(&collected, &target).disposition,
        BuildCacheDisposition::Reclaimed
    );
    assert!(!target.exists(), "released worktree target survived");
}

#[test]
fn build_cache_skips_malformed_symlinked_and_outside_worktree_targets() {
    let scratch = Scratch::new("target-boundaries");
    let cache = scratch.join("CargoBuild");
    let current = write_cache_root(&cache, "ff", "ffffffffffffff", "current");
    let worktree = scratch.join("worktree");
    fs::create_dir_all(&worktree).expect("create owning worktree");
    let outside = scratch.join("outside-target");
    write_target_at(&outside, "outside");
    let symlink = worktree.join("symlink-target");
    std::os::unix::fs::symlink(&outside, &symlink).expect("link target outside worktree");
    let untagged = worktree.join("untagged-target");
    fs::create_dir(&untagged).expect("create untagged target");
    fs::write(untagged.join("payload"), b"untagged").expect("write untagged target");

    let (report, _) = collect_cache_with_targets(
        &current,
        BuildCacheRequest {
            apply: true,
            max_bytes: Some(0),
            retain_live_worktrees: 0,
            ..BuildCacheRequest::default()
        },
        Vec::new(),
        None,
        [&outside, &symlink, &untagged]
            .map(|target| BuildCacheTargetDirectory::new(&worktree, target))
            .to_vec(),
        None,
    );

    for target in [&outside, &symlink, &untagged] {
        assert!(matches!(
            root_report(&report, target).disposition,
            BuildCacheDisposition::SkippedMalformed(_)
        ));
        assert!(target.exists(), "{} was removed", target.display());
    }
    assert!(outside.join("payload").is_file());
}

#[test]
fn build_cache_names_the_exact_owner_read_only_target_path() {
    let scratch = Scratch::new("read-only-target");
    let cache = scratch.join("CargoBuild");
    let current = write_cache_root(&cache, "ff", "ffffffffffffff", "current");
    let worktree = scratch.join("worktree");
    let target = write_target_root(&worktree, "read-only");
    let read_only = target.join("xtask-app-source/journal");
    fs::create_dir_all(&read_only).expect("create read-only snapshot");
    let refused = read_only.join("Cargo.toml");
    fs::write(&refused, b"[workspace]\n").expect("write refused snapshot file");
    fs::set_permissions(&read_only, fs::Permissions::from_mode(0o500))
        .expect("make snapshot owner-read-only");
    let mut context = BuildCacheContext::new(&current);
    context
        .live_target_directories
        .push(BuildCacheTargetDirectory::new(&worktree, &target));
    let mut output = Vec::new();
    let error = collect_build_cache_at(
        &BuildCacheRequest {
            apply: true,
            max_bytes: Some(0),
            retain_live_worktrees: 0,
            ..BuildCacheRequest::default()
        },
        &context,
        &mut output,
    )
    .expect_err("owner-read-only target subtree refuses removal");
    fs::set_permissions(&read_only, fs::Permissions::from_mode(0o700))
        .expect("restore snapshot permissions");

    let BuildCacheError::Io { path, .. } = error else {
        panic!("read-only target returned the wrong refusal: {error}");
    };
    assert_eq!(path, refused);
}

#[test]
fn build_cache_byte_parser_accepts_documented_binary_suffixes() {
    assert_eq!(
        parse_byte_quantity("7G").expect("parse G"),
        7 * 1024 * 1024 * 1024
    );
    assert_eq!(
        parse_byte_quantity("50GiB").expect("parse GiB"),
        50 * 1024 * 1024 * 1024
    );
    assert!(parse_byte_quantity("eight").is_err());
}

fn acquired(acquisition: Acquisition) -> Possession {
    match acquisition {
        Acquisition::Acquired(possession) => possession,
        Acquisition::Contended => panic!("fixture Cargo lock is unexpectedly held"),
    }
}

fn write_cache_root(cache: &Path, bucket: &str, hash: &str, label: &str) -> PathBuf {
    let root = cache.join(bucket).join(hash);
    fs::create_dir_all(root.join("debug")).expect("create cache root");
    fs::write(
        root.join(CACHE_TAG),
        b"Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .expect("write cache tag");
    fs::write(
        root.join("payload"),
        format!("{label}\n{}", "x".repeat(4096)),
    )
    .expect("write payload");
    fs::write(root.join("debug/.cargo-build-lock"), b"").expect("write idle lock");
    root
}

fn add_lock(root: &Path, relative: &str) -> PathBuf {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("lock has a parent")).expect("create lock parent");
    fs::write(&path, b"").expect("write lock file");
    path
}

fn write_target_root(worktree: &Path, label: &str) -> PathBuf {
    let target = worktree.join("target");
    write_target_at(&target, label);
    target
}

fn write_target_at(target: &Path, label: &str) {
    fs::create_dir_all(target.join("debug")).expect("create worktree target");
    fs::write(
        target.join(CACHE_TAG),
        b"Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .expect("write target cache tag");
    fs::write(target.join("payload"), label).expect("write target payload");
    fs::write(target.join("debug").join(TARGET_LOCK_NAME), b"").expect("write target lock");
    fs::write(target.join("debug").join(LOCK_NAME), b"").expect("write build lock");
}

fn hold_lock(path: &Path) -> Possession {
    hold_named_lock(path)
}

fn hold_named_lock(path: &Path) -> Possession {
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .expect("fixture lock has a UTF-8 name");
    let lock = LockFile::new(
        LockName::new(name).expect("Cargo lock name is one component"),
        path,
    );
    match lock.try_acquire().expect("hold lock") {
        Acquisition::Acquired(possession) => possession,
        Acquisition::Contended => panic!("fixture lock is unexpectedly held"),
    }
}

fn collect_cache(
    current: &Path,
    request: BuildCacheRequest,
    live_build_directories: Vec<PathBuf>,
    available_bytes: Option<u64>,
) -> xtask::BuildCacheReport {
    collect_cache_with_targets(
        current,
        request,
        live_build_directories,
        None,
        Vec::new(),
        available_bytes,
    )
    .0
}

fn collect_cache_with_targets(
    current: &Path,
    request: BuildCacheRequest,
    live_build_directories: Vec<PathBuf>,
    current_target: Option<BuildCacheTargetDirectory>,
    live_target_directories: Vec<BuildCacheTargetDirectory>,
    available_bytes: Option<u64>,
) -> (xtask::BuildCacheReport, String) {
    let mut context = BuildCacheContext::new(current);
    context.target_directory = current_target;
    context.live_build_directories = live_build_directories;
    context.live_target_directories = live_target_directories;
    context.available_bytes = available_bytes;
    let mut output = Vec::new();
    let report = collect_build_cache_at(&request, &context, &mut output)
        .expect("collect build and target cache");
    (
        report,
        String::from_utf8(output).expect("build-cache report is UTF-8"),
    )
}

fn root_report<'a>(report: &'a xtask::BuildCacheReport, root: &Path) -> &'a BuildCacheRootReport {
    let canonical = fs::canonicalize(root).ok();
    report
        .roots
        .iter()
        .find(|entry| entry.path == root)
        .or_else(|| {
            report.roots.iter().find(|entry| {
                canonical
                    .as_ref()
                    .is_some_and(|canonical| entry.path == *canonical)
            })
        })
        .unwrap_or_else(|| panic!("{} was not reported", root.display()))
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
        Self(fs::canonicalize(path).expect("canonicalize build-cache scratch"))
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
