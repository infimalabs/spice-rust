use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::*;

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-verifier-archive-{label}-{}-{}",
            std::process::id(),
            line!(),
        ));
        fs::create_dir(&root).expect("create archive fixture root");
        Self { root }
    }

    fn archive(&self) -> PathBuf {
        let archive = self.root.join("archive");
        fs::create_dir_all(&archive).expect("create fixture archive");
        archive
    }

    fn lock(&self) -> PathBuf {
        self.root.join("archive.lock")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove archive fixture root");
    }
}

fn source(ordinal: usize) -> String {
    format!("{ordinal:0width$x}", width = BUNDLE_STEM_HEX)
}

fn plain_bundle(payload: String) -> Vec<u8> {
    MergeParentEvidence {
        version: super::super::MERGE_PARENT_EVIDENCE_VERSION,
        cargo_jobs: 1,
        battery: payload,
        compiler: "compiler".to_owned(),
        state: "state".to_owned(),
        semantic: "semantic".to_owned(),
    }
    .encode()
    .expect("encode fixture merge-parent evidence")
}

fn raw_gzip(plain: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(plain).expect("write raw gzip fixture");
    encoder.finish().expect("finish raw gzip fixture")
}

fn write_bundle(root: &Path, ordinal: usize, stored: &[u8]) -> PathBuf {
    let path = root.join(format!("{}.json", source(ordinal)));
    fs::write(&path, stored).expect("write archive bundle fixture");
    path
}

#[test]
fn current_container_round_trips_and_rejects_version_length_and_integrity_drift() {
    let plain = plain_bundle("payload".repeat(1024));
    let stored = container::encode(&plain).expect("encode current container");
    assert_eq!(container::decode(&stored).expect("decode current"), plain);

    let mut wrong_version = stored.clone();
    wrong_version[8..12].copy_from_slice(&2_u32.to_be_bytes());
    assert!(
        container::decode(&wrong_version)
            .expect_err("unknown version refuses")
            .contains("unsupported archive container version")
    );

    let mut wrong_length = stored.clone();
    wrong_length[20..28].copy_from_slice(&1_u64.to_be_bytes());
    assert!(
        container::decode(&wrong_length)
            .expect_err("stored-length drift refuses")
            .contains("stored length")
    );

    let mut corrupt = stored;
    *corrupt.last_mut().expect("container payload") ^= 1;
    assert!(container::decode(&corrupt).is_err());

    let mut trailing = container::encode(&plain).expect("encode trailing-byte control");
    trailing.push(0);
    let payload_len = u64::try_from(trailing.len() - 60).expect("payload length fits u64");
    trailing[20..28].copy_from_slice(&payload_len.to_be_bytes());
    assert!(
        container::decode(&trailing)
            .expect_err("trailing gzip bytes refuse")
            .contains("consumed")
    );
}

#[test]
fn interrupted_plain_and_raw_gzip_migration_is_idempotently_recovered() {
    let scratch = Scratch::new("migration");
    let root = scratch.archive();
    let first_plain = plain_bundle("plain".repeat(1024));
    let second_plain = plain_bundle("gzip".repeat(1024));
    let first = write_bundle(&root, 1, &first_plain);
    let second = write_bundle(&root, 2, &raw_gzip(&second_plain));
    let first_modified = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let second_modified = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(2_000);
    for (path, modified) in [(&first, first_modified), (&second, second_modified)] {
        File::options()
            .write(true)
            .open(path)
            .expect("open migration fixture for timestamping")
            .set_times(fs::FileTimes::new().set_modified(modified))
            .expect("set migration fixture modification time");
    }
    let archive = LockedArchive::lock_at(&root, &scratch.lock()).expect("lock archive fixture");
    let mut rewrites = 0;
    let error = archive
        .migrate_with(|_| {
            rewrites += 1;
            (rewrites != 1)
                .then_some(())
                .ok_or_else(|| "fixture interruption".to_owned())
        })
        .expect_err("first migration is interrupted");
    assert_eq!(error, "fixture interruption");
    assert!(!root.join(FORMAT_MARKER_NAME).exists());
    assert!(
        [first.clone(), second.clone()]
            .iter()
            .filter(|path| container::is_current(&fs::read(path).expect("read partial migration")))
            .count()
            == 1
    );
    drop(archive);

    let archive = LockedArchive::acquire_at(&root, &scratch.lock()).expect("recover migration");
    assert_eq!(archive.read(&first).expect("read first"), Some(first_plain));
    assert_eq!(
        archive.read(&second).expect("read second"),
        Some(second_plain)
    );
    assert_eq!(
        fs::metadata(&first)
            .expect("inspect migrated first")
            .modified()
            .expect("read migrated first modification time"),
        first_modified,
    );
    assert_eq!(
        fs::metadata(&second)
            .expect("inspect migrated second")
            .modified()
            .expect("read migrated second modification time"),
        second_modified,
    );
    assert_eq!(
        fs::read(root.join(FORMAT_MARKER_NAME)).expect("read format marker"),
        FORMAT_MARKER,
    );
    archive.ensure_current().expect("migration is idempotent");
    assert!(
        fs::read_dir(&root)
            .expect("list recovered archive")
            .all(|entry| !atomic::is_temporary_path(&entry.expect("archive entry").path()))
    );
}

#[test]
fn current_marker_disables_pre_container_read_paths() {
    let scratch = Scratch::new("current-only");
    let root = scratch.archive();
    fs::write(root.join(FORMAT_MARKER_NAME), FORMAT_MARKER).expect("write current marker");
    let path = write_bundle(&root, 1, &raw_gzip(&plain_bundle("old".to_owned())));
    let archive =
        LockedArchive::acquire_at(&root, &scratch.lock()).expect("acquire current archive");
    assert!(
        archive
            .read(&path)
            .expect_err("raw gzip is unsupported after migration")
            .contains("current container")
    );
}

#[test]
fn count_grooming_preserves_reachable_sources_and_foreign_entries() {
    let scratch = Scratch::new("count");
    let root = scratch.archive();
    let archive = LockedArchive::acquire_at(&root, &scratch.lock()).expect("acquire archive");
    let foreign = root.join("operator-note.txt");
    fs::write(&foreign, b"retain me").expect("write foreign fixture");
    for ordinal in 0..retention::MAX_BUNDLES {
        archive
            .publish_and_groom(
                &source(ordinal),
                &plain_bundle(format!("payload-{ordinal}")),
                &BTreeSet::from([source(ordinal)]),
            )
            .expect("seed retained bundle");
    }
    let oldest = source(0);
    let newest = source(retention::MAX_BUNDLES);
    archive
        .publish_and_groom(
            &newest,
            &plain_bundle("new publication".to_owned()),
            &BTreeSet::from([oldest.clone(), newest.clone()]),
        )
        .expect("groom count-bound archive");

    assert!(root.join(format!("{oldest}.json")).exists());
    assert!(root.join(format!("{newest}.json")).exists());
    assert_eq!(
        fs::read(&foreign).expect("read foreign fixture"),
        b"retain me"
    );
    let bundles = fs::read_dir(&root)
        .expect("list count-bound archive")
        .map(|entry| entry.expect("archive entry").file_name())
        .filter(|name| name.to_str().is_some_and(is_bundle_name))
        .count();
    assert_eq!(bundles, retention::MAX_BUNDLES);
}

#[test]
fn uncertain_bytes_refuse_before_pending_publication() {
    let scratch = Scratch::new("byte-refusal");
    let root = scratch.archive();
    let archive = LockedArchive::acquire_at(&root, &scratch.lock()).expect("acquire archive");
    let foreign = root.join("foreign.bin");
    File::create(&foreign)
        .expect("create foreign fixture")
        .set_len(retention::MAX_STORED_BYTES + 1)
        .expect("size foreign fixture");
    let pending = source(9);
    let error = archive
        .publish_and_groom(
            &pending,
            &plain_bundle("pending".to_owned()),
            &BTreeSet::from([pending.clone()]),
        )
        .expect_err("uncertain bytes prevent a bounded plan");
    assert!(error.contains("protected or uncertain entries"), "{error}");
    assert!(!root.join(format!("{pending}.json")).exists());
    assert!(foreign.exists());
}

#[test]
fn invalid_entry_shape_refuses_the_complete_plan_before_mutation() {
    let scratch = Scratch::new("shape-refusal");
    let root = scratch.archive();
    let archive = LockedArchive::acquire_at(&root, &scratch.lock()).expect("acquire archive");
    fs::create_dir(root.join("uncertain-entry")).expect("create uncertain directory");
    let pending = source(3);
    let error = archive
        .publish_and_groom(
            &pending,
            &plain_bundle("pending".to_owned()),
            &BTreeSet::from([pending.clone()]),
        )
        .expect_err("uncertain entry refuses planning");
    assert!(error.contains("stored bytes are uncertain"), "{error}");
    assert!(!root.join(format!("{pending}.json")).exists());
}

#[test]
fn disappearance_is_a_cache_miss_and_real_io_faults_remain_visible() {
    let scratch = Scratch::new("read-races");
    let root = scratch.archive();
    let archive = LockedArchive::acquire_at(&root, &scratch.lock()).expect("acquire archive");
    let missing = root.join(format!("{}.json", source(1)));
    assert_eq!(
        archive.read(&missing).expect("missing peer is harmless"),
        None
    );
    fs::create_dir(&missing).expect("create unreadable bundle shape");
    assert!(
        archive
            .read(&missing)
            .expect_err("real I/O fault remains a fault")
            .contains("read bundle")
    );
}

#[test]
fn common_lock_serializes_publishers_and_admission() {
    use std::sync::mpsc;

    let scratch = Scratch::new("lock");
    let root = scratch.archive();
    let archive = LockedArchive::acquire_at(&root, &scratch.lock()).expect("first lock");
    let name = sill::LockName::new(ARCHIVE_LOCK_NAME).expect("fixture lock name");
    let probe = sill::LockFile::new(name, scratch.lock());
    assert!(matches!(
        probe.try_acquire().expect("probe contended archive"),
        sill::Acquisition::Contended,
    ));
    let first = source(1);
    let second = source(2);
    let first_path = root.join(format!("{first}.json"));
    let (started_tx, started_rx) = mpsc::sync_channel(0);
    let thread_root = root.clone();
    let thread_lock = scratch.lock();
    let thread_first = first.clone();
    let thread_second = second.clone();
    let peer = std::thread::spawn(move || {
        started_tx.send(()).expect("announce peer acquisition");
        let archive = LockedArchive::acquire_at(&thread_root, &thread_lock)
            .expect("peer acquires released archive");
        archive
            .publish_and_groom(
                &thread_second,
                &plain_bundle("peer publication".to_owned()),
                &BTreeSet::from([thread_first.clone(), thread_second.clone()]),
            )
            .expect("peer publication converges");
        archive
            .read(&thread_root.join(format!("{thread_first}.json")))
            .expect("peer admission read")
    });
    started_rx.recv().expect("peer begins acquisition");
    archive
        .publish_and_groom(
            &first,
            &plain_bundle("first publication".to_owned()),
            &BTreeSet::from([first.clone()]),
        )
        .expect("first publication completes under lock");
    drop(archive);
    assert!(peer.join().expect("join peer archive operation").is_some());
    assert!(first_path.exists());
    assert!(
        fs::read_dir(&root)
            .expect("list converged concurrent archive")
            .all(|entry| !atomic::is_temporary_path(&entry.expect("archive entry").path()))
    );
}

#[test]
fn large_compressible_and_incompressible_documents_are_bounded() {
    let compressible = plain_bundle("a".repeat(2 * 1024 * 1024));
    let compressed = container::encode(&compressible).expect("encode compressible bundle");
    assert!(compressed.len() * 5 < compressible.len());
    assert_eq!(
        container::decode(&compressed).expect("decode compressible"),
        compressible
    );

    let mut value = 0x1234_5678_u64;
    let random = (0..2 * 1024 * 1024)
        .map(|_| {
            value = value
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            char::from(b'!' + u8::try_from(value % 90).expect("bounded byte"))
        })
        .collect::<String>();
    let incompressible = plain_bundle(random);
    let stored = container::encode(&incompressible).expect("encode incompressible bundle");
    assert_eq!(
        container::decode(&stored).expect("decode incompressible"),
        incompressible
    );
    assert!(
        u64::try_from(stored.len()).expect("stored fixture length fits u64")
            <= retention::MAX_STORED_BYTES
    );
}

#[test]
#[ignore = "explicit large-bundle paired performance measurement"]
fn large_bundle_records_paired_publication_and_admission_measurements() {
    const PAYLOAD_REPETITIONS: usize = 1_800_000;
    let plain = plain_bundle("archive-measurement-payload\n".repeat(PAYLOAD_REPETITIONS));
    validate_plain_bundle(&plain).expect("validate measured admitted bundle");
    let predecessor = raw_gzip(&plain);
    let current = container::encode(&plain).expect("encode measured current bundle");
    assert!(current.len() * 5 <= plain.len());

    let mut predecessor_publish = Vec::new();
    let mut current_publish = Vec::new();
    let mut predecessor_admit = Vec::new();
    let mut current_admit = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let raw = raw_gzip(&plain);
        predecessor_publish.push(start.elapsed());
        assert_eq!(
            container::decode_pre_container(&raw).expect("admit predecessor control"),
            plain,
        );

        let start = Instant::now();
        let encoded = container::encode(&plain).expect("publish current sample");
        current_publish.push(start.elapsed());

        let start = Instant::now();
        let expanded =
            container::decode_pre_container(&predecessor).expect("admit predecessor sample");
        validate_plain_bundle(&expanded).expect("validate predecessor sample");
        predecessor_admit.push(start.elapsed());

        let start = Instant::now();
        decode_current_bundle(&encoded).expect("admit current sample");
        current_admit.push(start.elapsed());
    }
    let predecessor_publish = median(predecessor_publish);
    let current_publish = median(current_publish);
    let predecessor_admit = median(predecessor_admit);
    let current_admit = median(current_admit);
    eprintln!(
        concat!(
            "archive_measurement plain_bytes={} current_bytes={} ratio={:.1}x ",
            "publish_predecessor_ms={:.3} publish_current_ms={:.3} ",
            "admit_predecessor_ms={:.3} admit_current_ms={:.3}"
        ),
        plain.len(),
        current.len(),
        plain.len() as f64 / current.len() as f64,
        predecessor_publish.as_secs_f64() * 1000.0,
        current_publish.as_secs_f64() * 1000.0,
        predecessor_admit.as_secs_f64() * 1000.0,
        current_admit.as_secs_f64() * 1000.0,
    );
    assert_no_regression(predecessor_publish, current_publish, "publication");
    assert_no_regression(predecessor_admit, current_admit, "admission");
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

fn assert_no_regression(before: Duration, after: Duration, stage: &str) {
    let tolerance = (before / 10).max(Duration::from_millis(5));
    assert!(
        after <= before + tolerance,
        "{stage} regressed from {before:?} to {after:?}; tolerance={tolerance:?}",
    );
}
