use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{OutputLease, Pass, RECENT_USE, RECLAIM_INTERVAL, measure, reclaim};

/// One scratch directory, removed again when the test that made it ends.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn scratch(label: &str) -> Scratch {
    let root = std::env::temp_dir().join(format!(
        "spice-cargo-output-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    fs::create_dir(&root).expect("scratch directory creates");
    Scratch(root)
}

/// A Cargo profile directory holding `lock` and one compiled artifact.
///
/// Units are written in the order a test names them, so a unit written
/// earlier is the less recently used one.
fn unit(directory: &Path, lock: &str) {
    fs::create_dir_all(directory.join("deps")).expect("unit deps create");
    fs::create_dir_all(directory.join(".fingerprint/crate-0123")).expect("fingerprints create");
    fs::write(directory.join(lock), b"").expect("unit lock writes");
    fs::write(
        directory.join("deps/libcrate-0123.rlib"),
        vec![7_u8; 64 * 1024],
    )
    .expect("artifact writes");
    fs::write(
        directory.join(".fingerprint/crate-0123/lib-crate"),
        b"fresh",
    )
    .expect("fingerprint writes");
}

fn emptied(directory: &Path, lock: &str) -> bool {
    let mut entries = fs::read_dir(directory)
        .expect("unit directory remains")
        .map(|entry| entry.expect("entry reads").file_name())
        .collect::<Vec<_>>();
    entries.sort();
    entries == [lock]
}

fn intact(directory: &Path) -> bool {
    directory.join("deps/libcrate-0123.rlib").is_file()
        && directory
            .join(".fingerprint/crate-0123/lib-crate")
            .is_file()
}

/// A pass time at which every unit a test just wrote is past [`RECENT_USE`].
fn after_recent_use() -> SystemTime {
    SystemTime::now() + RECENT_USE * 2
}

#[test]
fn reclamation_empties_least_recent_units_until_the_footprint_fits() {
    let scratch = scratch("lru");
    let target = scratch.0.join("target");
    let oldest = target.join("debug");
    let nested = oldest.join("build/tool/out/release");
    let newer = target.join("cargo-build/debug");
    let newest = target.join("rust-analyzer/debug");
    unit(&oldest, ".cargo-lock");
    unit(&nested, ".cargo-lock");
    unit(&newer, ".cargo-build-lock");
    unit(&newest, ".cargo-lock");
    fs::create_dir_all(target.join("xtask-app-source")).expect("loose output creates");
    fs::write(
        target.join("xtask-app-source/index.js"),
        vec![1_u8; 64 * 1024],
    )
    .expect("loose output writes");

    let whole = measure(&target, u64::MAX).expect("target measures");
    let oldest_bytes = whole
        .units
        .iter()
        .find(|unit| unit.path == oldest)
        .expect("profile directory is a unit")
        .bytes;
    let bound = whole.bytes - oldest_bytes - 1;
    let footprint = measure(&target, bound).expect("target measures");
    let reclamation = reclaim(&footprint, after_recent_use()).expect("reclamation runs");

    assert_eq!(
        reclamation
            .removed
            .iter()
            .map(|removed| removed.path.as_path())
            .collect::<Vec<_>>(),
        [oldest.as_path(), nested.as_path()],
        "the two least recently used units go, oldest first, and no more"
    );
    assert!(reclamation.after <= bound);
    let remeasured = measure(&target, bound).expect("target remeasures");
    assert!(
        remeasured.bytes <= bound,
        "du agrees with the reported footprint"
    );
    assert!(emptied(&nested, ".cargo-lock"));
    assert!(
        oldest.join(".cargo-lock").is_file() && !oldest.join("deps").exists(),
        "an emptied unit keeps only the lock Cargo waits on"
    );
    assert!(intact(&newer) && intact(&newest));
    assert!(
        target.join("xtask-app-source/index.js").is_file(),
        "output outside every Cargo unit is never reclaimed"
    );
}

#[test]
fn a_unit_used_within_the_recent_window_survives_an_over_bound_pass() {
    let scratch = scratch("recent");
    let target = scratch.0.join("target");
    let stale = target.join("debug");
    let recent = target.join("cargo-build/debug");
    unit(&stale, ".cargo-lock");
    unit(&recent, ".cargo-build-lock");
    let now = SystemTime::now();
    let mut footprint = measure(&target, 0).expect("target measures");
    for measured in &mut footprint.units {
        if measured.path == stale {
            measured.last_use = now - RECENT_USE - Duration::from_secs(1);
        }
    }

    let reclamation = reclaim(&footprint, now).expect("reclamation runs");

    assert!(emptied(&stale, ".cargo-lock"), "an idle unit is reclaimed");
    assert!(
        intact(&recent),
        "a unit used within the window keeps its artifacts over the bound"
    );
    assert_eq!(reclamation.recent, std::slice::from_ref(&recent));
    assert!(
        reclamation.after > reclamation.bound,
        "the pass reports the footprint it had to leave over the bound"
    );
}

#[test]
fn a_unit_whose_cargo_lock_is_held_is_left_whole() {
    let scratch = scratch("busy");
    let target = scratch.0.join("target");
    let busy = target.join("debug");
    let idle = target.join("wasm32-unknown-unknown/debug");
    unit(&busy, ".cargo-lock");
    fs::write(busy.join(".cargo-artifact-lock"), b"").expect("second lock writes");
    unit(&idle, ".cargo-lock");
    let cargo = File::open(busy.join(".cargo-artifact-lock")).expect("lock opens");
    cargo.try_lock().expect("the live Cargo holds its lock");

    let footprint = measure(&target, 0).expect("target measures");
    let reclamation = reclaim(&footprint, after_recent_use()).expect("reclamation runs");

    assert_eq!(reclamation.busy, std::slice::from_ref(&busy));
    assert!(intact(&busy), "a held unit keeps every artifact");
    assert!(emptied(&idle, ".cargo-lock"));
    drop(cargo);
    let footprint = measure(&target, 0).expect("target remeasures");
    let reclamation = reclaim(&footprint, after_recent_use()).expect("reclamation reruns");
    assert!(reclamation.busy.is_empty());
    assert!(!busy.join("deps").exists());
}

#[test]
fn the_lease_reclaims_only_when_exclusive_and_due() {
    let scratch = scratch("lease");
    let locks = scratch.0.join("locks");
    let target = scratch.0.join("target");
    let profile = target.join("debug");
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    unit(&profile, ".cargo-lock");

    let (first, pass) = OutputLease::enter(&locks, &target, 0, start).expect("lease enters");
    let Pass::Ran(Ok(reclamation)) = pass else {
        panic!("a never-reclaimed worktree is due: {pass:?}");
    };
    assert_eq!(reclamation.removed.len(), 1);
    assert!(emptied(&profile, ".cargo-lock"));

    unit(&profile, ".cargo-lock");
    let later = start + RECLAIM_INTERVAL * 2;
    let (second, pass) = OutputLease::enter(&locks, &target, 0, later).expect("lease shares");
    assert!(
        matches!(pass, Pass::Shared),
        "a run holding the lease excludes reclamation: {pass:?}"
    );
    assert!(intact(&profile));
    drop((first, second));

    let soon = start + RECLAIM_INTERVAL - Duration::from_secs(1);
    let (lease, pass) = OutputLease::enter(&locks, &target, 0, soon).expect("lease enters");
    assert!(matches!(pass, Pass::NotDue), "{pass:?}");
    assert!(intact(&profile));
    drop(lease);

    let (_lease, pass) =
        OutputLease::enter(&locks, &target, 0, start + RECLAIM_INTERVAL).expect("lease enters");
    assert!(matches!(pass, Pass::Ran(Ok(_))), "{pass:?}");
    assert!(emptied(&profile, ".cargo-lock"));
}
