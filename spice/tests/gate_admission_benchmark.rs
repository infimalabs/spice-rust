#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde::Serialize;
use spice::install::{apply, installed_binary, installed_git_hooks, plan};
use spice::repository::Repository;

const SAMPLES: usize = 30;
static BENCHMARK: Mutex<()> = Mutex::new(());

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct BenchmarkReport {
    version: u64,
    durations_micros: Vec<u64>,
    median_micros: u64,
    p95_micros: u64,
    maximum_micros: u64,
    budget_met: bool,
    git_version: String,
    filesystem_type: String,
    architecture: &'static str,
    executable_bytes: u64,
    git_subprocesses_per_run: u64,
    evidence_bytes_read_per_run: u64,
    evidence_files_opened_per_run: u64,
}

#[test]
#[ignore = "environmental 30-run measurement of the installed warm pre-commit hook"]
fn gate_warm_admission_benchmark() {
    let _serial = BENCHMARK.lock().expect("serialize admission benchmarks");
    let fixture = Fixture::new();
    fixture.seed_proofs();

    let warmed = fixture.invoke_hook();
    assert!(
        warmed.status.success(),
        "warmup failed: {}",
        streams(&warmed)
    );

    let mut durations = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        let output = fixture.invoke_hook();
        let elapsed = started.elapsed();
        assert!(
            output.status.success(),
            "measured hook failed: {}",
            streams(&output)
        );
        durations.push(u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX));
    }

    let mut ordered = durations.clone();
    ordered.sort_unstable();
    let (evidence_files, evidence_bytes) = fixture.evidence_read_footprint();
    let median = midpoint(ordered[14], ordered[15]);
    let p95 = ordered[28];
    let maximum = *ordered.last().expect("benchmark has measured samples");
    let report = BenchmarkReport {
        version: 1,
        durations_micros: durations,
        median_micros: median,
        p95_micros: p95,
        maximum_micros: maximum,
        budget_met: median <= 75_000 && p95 <= 150_000 && maximum <= 250_000,
        git_version: command_text("/usr/bin/git", &["--version"], fixture.linked()),
        filesystem_type: filesystem_type(fixture.linked()),
        architecture: std::env::consts::ARCH,
        executable_bytes: fs::metadata(&fixture.binary)
            .expect("measure installed benchmark executable")
            .len(),
        git_subprocesses_per_run: 0,
        evidence_bytes_read_per_run: evidence_bytes,
        evidence_files_opened_per_run: evidence_files,
    };
    let report_json = serde_json::to_string(&report).expect("benchmark report encodes");
    println!("{report_json}");
    assert!(
        report.budget_met,
        "installed warm admission exceeded its latency budget: {report_json}"
    );
}

#[test]
fn linked_worktree_uses_shared_fingerprints_inside_the_hard_bound() {
    let _serial = BENCHMARK.lock().expect("serialize admission benchmarks");
    let fixture = Fixture::new();
    fixture.seed_proofs();

    let started = Instant::now();
    let output = fixture.invoke_peer_hook();
    let elapsed = started.elapsed();

    assert!(output.status.success(), "{}", streams(&output));
    assert!(
        elapsed.as_secs_f64() < 2.0,
        "linked-worktree admission took {elapsed:?}: {}",
        streams(&output),
    );
}

#[test]
fn current_manifest_receipt_miss_refuses_inside_the_hard_bound() {
    let _serial = BENCHMARK.lock().expect("serialize admission benchmarks");
    let fixture = Fixture::new();
    fixture.seed_proofs();
    let action = fixture.remove_one_receipt_index();

    let started = Instant::now();
    let output = fixture.invoke_hook();
    let elapsed = started.elapsed();
    let rendered = streams(&output);

    assert!(!output.status.success(), "{rendered}");
    assert!(
        rendered.contains("gate::pre-commit-proof-missing"),
        "{rendered}"
    );
    assert!(
        rendered.contains(&format!("command=`spice gate prove {action}`")),
        "{rendered}",
    );
    assert!(
        elapsed.as_secs_f64() < 2.0,
        "receipt miss took {elapsed:?}: {rendered}",
    );
}

#[test]
fn corrupt_manifest_refuses_inside_the_hard_bound() {
    let _serial = BENCHMARK.lock().expect("serialize admission benchmarks");
    let fixture = Fixture::new();
    fixture.seed_proofs();
    fs::write(fixture.fingerprint_path(), b"not canonical").expect("corrupt fingerprint fixture");

    let started = Instant::now();
    let output = fixture.invoke_hook();
    let elapsed = started.elapsed();
    let rendered = streams(&output);

    assert!(!output.status.success(), "{rendered}");
    assert!(
        rendered.contains("gate::pre-commit-fingerprint-invalid"),
        "{rendered}"
    );
    assert!(
        rendered.contains("command=`spice gate prove --missing`"),
        "{rendered}"
    );
    assert!(
        elapsed.as_secs_f64() < 2.0,
        "corrupt manifest refusal took {elapsed:?}: {rendered}",
    );
    assert_eq!(fixture.admission_count(), 0);
}

#[test]
fn corrupt_receipt_object_cannot_publish_an_admission() {
    let _serial = BENCHMARK.lock().expect("serialize admission benchmarks");
    let fixture = Fixture::new();
    fixture.seed_proofs();
    fixture.corrupt_one_receipt();

    let output = fixture.invoke_hook();
    let rendered = streams(&output);

    assert!(!output.status.success(), "{rendered}");
    assert!(
        rendered.contains("gate::pre-commit-proof-invalid"),
        "{rendered}"
    );
    assert_eq!(fixture.admission_count(), 0);
}

#[test]
fn proof_lookup_is_receipt_only_and_bounded_for_every_evidence_state() {
    let _serial = BENCHMARK.lock().expect("serialize admission benchmarks");
    let fixture = Fixture::new();

    let cold = fixture.invoke_proof(&[]);
    assert_reason(&cold, "gate::prove-fingerprint-missing");
    let started = Instant::now();
    let missing_fingerprint = fixture.invoke_proof(&[]);
    assert_bounded(started, &missing_fingerprint, "missing fingerprint");
    assert_reason(&missing_fingerprint, "gate::prove-fingerprint-missing");
    assert!(
        streams(&missing_fingerprint).contains("command=`spice gate prove --missing`"),
        "{}",
        streams(&missing_fingerprint),
    );
    assert!(
        fixture.fingerprint_paths().is_empty(),
        "bare lookup published a fingerprint through a hidden planning fallback"
    );

    fixture.seed_proofs();
    let started = Instant::now();
    let present = fixture.invoke_proof(&[]);
    assert_bounded(started, &present, "present receipts");
    assert!(present.status.success(), "{}", streams(&present));
    let report: serde_json::Value = wire::decode(&present.stdout).expect("decode proof inspection");
    assert!(
        report["body"]["actions"]
            .as_array()
            .is_some_and(|actions| !actions.is_empty())
    );
    assert!(
        report["body"]["actions"]
            .as_array()
            .expect("inspection actions")
            .iter()
            .all(|action| action["disposition"] == "present")
    );

    let action = fixture.remove_one_receipt_index();
    let started = Instant::now();
    let missing_receipt = fixture.invoke_proof(&[]);
    assert_bounded(started, &missing_receipt, "missing receipt");
    assert_reason(&missing_receipt, "gate::prove-receipt-missing");
    assert!(
        streams(&missing_receipt).contains(&format!("command=`spice gate prove {action}`")),
        "{}",
        streams(&missing_receipt),
    );
    assert!(
        !fixture.receipt_index(&action).exists(),
        "receipt lookup silently regenerated the missing action"
    );

    fixture.seed_proofs();
    fs::write(fixture.linked.join("candidate.txt"), "moved candidate\n")
        .expect("move staged proof source");
    git(&fixture.linked, &["add", "--all"]);
    let started = Instant::now();
    let stale = fixture.invoke_proof(&[]);
    assert_bounded(started, &stale, "stale fingerprint");
    assert_reason(&stale, "gate::prove-fingerprint-missing");
    assert!(
        streams(&stale).contains("command=`spice gate prove --missing`"),
        "{}",
        streams(&stale),
    );
    assert_eq!(
        fixture.fingerprint_paths().len(),
        1,
        "stale lookup published a replacement fingerprint through a hidden fallback"
    );

    let fingerprint_path = fixture.seed_proofs();
    let fingerprint = fs::read(&fingerprint_path).expect("read current fingerprint fixture");
    fs::write(&fingerprint_path, b"not canonical").expect("corrupt fingerprint fixture");
    let started = Instant::now();
    let invalid = fixture.invoke_proof(&[]);
    assert_bounded(started, &invalid, "invalid fingerprint");
    assert_reason(&invalid, "gate::prove-fingerprint-invalid");
    assert!(
        streams(&invalid).contains("command=`spice gate prove --missing`"),
        "{}",
        streams(&invalid),
    );
    fs::write(&fingerprint_path, fingerprint).expect("restore fingerprint fixture");

    let action = fixture.corrupt_one_receipt_index(&fingerprint_path);
    let started = Instant::now();
    let invalid = fixture.invoke_proof(&[]);
    assert_bounded(started, &invalid, "invalid receipt");
    assert_reason(&invalid, "gate::prove-receipt-invalid");
    assert!(
        streams(&invalid).contains("command=`spice gate prove --missing`"),
        "{}: {}",
        action,
        streams(&invalid),
    );
}

struct Fixture {
    root: PathBuf,
    linked: PathBuf,
    peer: PathBuf,
    hook: PathBuf,
    binary: PathBuf,
    common: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-gate-admission-benchmark-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        let main = root.join("main");
        let linked = root.join("linked");
        let peer = root.join("peer");
        fs::create_dir_all(&main).expect("create benchmark repository");
        git(&main, &["init", "--initial-branch=main"]);
        git(&main, &["config", "user.name", "Spice Benchmark"]);
        git(
            &main,
            &["config", "user.email", "spice-benchmark@example.invalid"],
        );
        fs::write(main.join("candidate.txt"), "baseline\n").expect("write benchmark baseline");
        git(&main, &["add", "--all"]);
        git(&main, &["commit", "-m", "benchmark baseline"]);
        git(
            &main,
            &[
                "worktree",
                "add",
                "-b",
                "benchmark-linked",
                linked.to_str().expect("linked path is UTF-8"),
                "main",
            ],
        );
        git(
            &main,
            &[
                "worktree",
                "add",
                "-b",
                "benchmark-peer",
                peer.to_str().expect("peer path is UTF-8"),
                "main",
            ],
        );
        for worktree in [&linked, &peer] {
            fs::write(worktree.join("candidate.txt"), "candidate\n")
                .expect("write benchmark candidate");
            git(worktree, &["add", "--all"]);
        }

        let source = Path::new(env!("CARGO_BIN_EXE_spice"));
        let repository = Repository::discover(&linked).expect("discover benchmark worktree");
        let install = plan(&repository, Some(source)).expect("plan benchmark installation");
        apply(
            &repository,
            Some(source),
            &install.digest().expect("digest benchmark installation"),
        )
        .expect("install benchmark hook");
        let hook = installed_git_hooks(&repository).join("pre-commit");
        let binary = installed_binary(&repository);
        let common = repository.common_dir().to_path_buf();
        Self {
            root,
            linked,
            peer,
            hook,
            binary,
            common,
        }
    }

    fn linked(&self) -> &Path {
        &self.linked
    }

    fn seed_proofs(&self) -> PathBuf {
        let before = self.fingerprint_paths();
        let output = Command::new(&self.binary)
            .args(["gate", "prove", "--missing"])
            .current_dir(&self.linked)
            .env_remove(wire::SpiceVariable::Lane.as_str())
            .env_remove(wire::SpiceVariable::PreCommitSource.as_str())
            .env_remove(spice::install::INSTALLED_PRE_COMMIT_ENVIRONMENT)
            .output()
            .expect("seed benchmark proof receipts");
        assert!(
            output.status.success(),
            "proof seed failed: {}",
            streams(&output)
        );
        let after = self.fingerprint_paths();
        let created = after
            .iter()
            .filter(|path| !before.contains(path))
            .collect::<Vec<_>>();
        match created.as_slice() {
            [path] => (*path).clone(),
            [] if after.len() == 1 => after[0].clone(),
            _ => panic!(
                "proof seed did not select one fingerprint: before={before:?} after={after:?}"
            ),
        }
    }

    fn invoke_hook(&self) -> Output {
        self.invoke_hook_at(&self.linked)
    }

    fn invoke_peer_hook(&self) -> Output {
        self.invoke_hook_at(&self.peer)
    }

    fn invoke_hook_at(&self, worktree: &Path) -> Output {
        Command::new(&self.hook)
            .current_dir(worktree)
            .env_remove(wire::SpiceVariable::Lane.as_str())
            .env_remove(wire::SpiceVariable::PreCommitSource.as_str())
            .output()
            .expect("run installed benchmark hook")
    }

    fn invoke_proof(&self, arguments: &[&str]) -> Output {
        Command::new(&self.binary)
            .arg("--json")
            .args(["gate", "prove"])
            .args(arguments)
            .current_dir(&self.linked)
            .env_remove(wire::SpiceVariable::Lane.as_str())
            .env_remove(wire::SpiceVariable::PreCommitSource.as_str())
            .env_remove(spice::install::INSTALLED_PRE_COMMIT_ENVIRONMENT)
            .output()
            .expect("inspect benchmark proof receipts")
    }

    fn fingerprint_path(&self) -> PathBuf {
        let entries = self.fingerprint_paths();
        let [fingerprint] = entries.as_slice() else {
            panic!("fixture must publish exactly one fingerprint: {entries:?}");
        };
        fingerprint.clone()
    }

    fn fingerprint_paths(&self) -> Vec<PathBuf> {
        let fingerprints = self.common.join(".spice-rs/gate-evidence/v1/fingerprints");
        let entries = match fs::read_dir(&fingerprints) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(error) => panic!(
                "read fingerprint directory {}: {error}",
                fingerprints.display()
            ),
        };
        let mut entries = entries
            .map(|entry| entry.expect("read fingerprint entry").path())
            .collect::<Vec<_>>();
        entries.sort();
        entries
    }

    fn remove_one_receipt_index(&self) -> String {
        let document: serde_json::Value = serde_json::from_slice(
            &fs::read(self.fingerprint_path()).expect("read fingerprint manifest"),
        )
        .expect("decode fingerprint manifest");
        let action = &document["actions"][0];
        let name = action["action"]
            .as_str()
            .expect("fingerprint action has a name")
            .to_owned();
        let key = action["action_key"]
            .as_str()
            .expect("fingerprint action has a key");
        fs::remove_file(
            self.common
                .join(".spice-rs/gate-evidence/v1/actions")
                .join(key),
        )
        .expect("remove one receipt index");
        name
    }

    fn corrupt_one_receipt(&self) {
        let document: serde_json::Value = serde_json::from_slice(
            &fs::read(self.fingerprint_path()).expect("read fingerprint manifest"),
        )
        .expect("decode fingerprint manifest");
        let key = document["actions"][0]["action_key"]
            .as_str()
            .expect("fingerprint action has a key");
        let index: serde_json::Value = serde_json::from_slice(
            &fs::read(
                self.common
                    .join(".spice-rs/gate-evidence/v1/actions")
                    .join(key),
            )
            .expect("read receipt index"),
        )
        .expect("decode receipt index");
        let receipt = index["receipt"]
            .as_str()
            .expect("receipt index names an object");
        fs::write(
            self.common
                .join(".spice-rs/gate-evidence/v1/objects")
                .join(receipt),
            b"corrupt receipt",
        )
        .expect("corrupt receipt object");
    }

    fn admission_count(&self) -> usize {
        fs::read_dir(self.common.join(".spice-rs/gate-evidence/v1/admissions"))
            .expect("read admission directory")
            .count()
    }

    fn receipt_index(&self, action: &str) -> PathBuf {
        let document: serde_json::Value = serde_json::from_slice(
            &fs::read(self.fingerprint_path()).expect("read fingerprint manifest"),
        )
        .expect("decode fingerprint manifest");
        let key = document["actions"]
            .as_array()
            .expect("fingerprint actions")
            .iter()
            .find(|entry| entry["action"] == action)
            .and_then(|entry| entry["action_key"].as_str())
            .expect("fingerprint contains selected action");
        self.common
            .join(".spice-rs/gate-evidence/v1/actions")
            .join(key)
    }

    fn corrupt_one_receipt_index(&self, fingerprint: &Path) -> String {
        let document: serde_json::Value =
            serde_json::from_slice(&fs::read(fingerprint).expect("read fingerprint manifest"))
                .expect("decode fingerprint manifest");
        let action = &document["actions"][0];
        let name = action["action"]
            .as_str()
            .expect("fingerprint action has a name")
            .to_owned();
        let key = action["action_key"]
            .as_str()
            .expect("fingerprint action has a key");
        fs::write(
            self.common
                .join(".spice-rs/gate-evidence/v1/actions")
                .join(key),
            b"not canonical",
        )
        .expect("corrupt one receipt index");
        name
    }

    fn evidence_read_footprint(&self) -> (u64, u64) {
        let evidence = self.common.join(".spice-rs/gate-evidence/v1");
        [
            evidence.join("actions"),
            evidence.join("fingerprints"),
            evidence.join("objects"),
        ]
        .into_iter()
        .flat_map(|directory| {
            fs::read_dir(directory)
                .expect("read benchmark evidence directory")
                .map(|entry| entry.expect("read benchmark evidence entry").path())
        })
        .fold((0_u64, 0_u64), |(files, bytes), path| {
            let metadata = fs::metadata(path).expect("measure benchmark evidence file");
            (files + 1, bytes.saturating_add(metadata.len()))
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove benchmark fixture");
    }
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("run benchmark Git command");
    assert!(output.status.success(), "Git failed: {}", streams(&output));
}

fn command_text(program: &str, arguments: &[&str], root: &Path) -> String {
    let output = Command::new(program)
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("run benchmark metadata command");
    assert!(
        output.status.success(),
        "metadata command failed: {}",
        streams(&output)
    );
    String::from_utf8(output.stdout)
        .expect("benchmark metadata is UTF-8")
        .trim()
        .to_owned()
}

fn midpoint(left: u64, right: u64) -> u64 {
    left / 2 + right / 2 + (left % 2 + right % 2) / 2
}

fn assert_bounded(started: Instant, output: &Output, label: &str) {
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_secs_f64() < 2.0,
        "{label} lookup took {elapsed:?}: {}",
        streams(output),
    );
}

fn assert_reason(output: &Output, expected: &str) {
    assert!(!output.status.success(), "{}", streams(output));
    let outcome: wire::Outcome<serde_json::Value> =
        wire::decode(&output.stderr).expect("decode proof lookup refusal");
    let wire::Outcome::Refusal(refusal) = outcome else {
        panic!("proof lookup did not refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason(), expected);
}

fn filesystem_type(root: &Path) -> String {
    #[cfg(target_os = "macos")]
    {
        let mount = Command::new("/bin/df")
            .arg("-P")
            .arg(root)
            .output()
            .expect("resolve benchmark filesystem device");
        assert!(mount.status.success(), "df failed: {}", streams(&mount));
        let rendered = String::from_utf8(mount.stdout).expect("df output is UTF-8");
        let device = rendered
            .lines()
            .last()
            .and_then(|line| line.split_whitespace().next())
            .expect("df reports a filesystem device");
        let output = Command::new("/usr/sbin/diskutil")
            .arg("info")
            .arg(device)
            .output()
            .expect("measure benchmark filesystem type");
        assert!(
            output.status.success(),
            "diskutil failed: {}",
            streams(&output)
        );
        let rendered = String::from_utf8(output.stdout).expect("diskutil output is UTF-8");
        rendered
            .lines()
            .find_map(|line| line.trim().strip_prefix("File System Personality:"))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .expect("diskutil reports a filesystem personality")
            .to_owned()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let output = Command::new("/usr/bin/stat")
            .args(["-f", "-c", "%T"])
            .arg(root)
            .output()
            .expect("measure benchmark filesystem type");
        assert!(output.status.success(), "stat failed: {}", streams(&output));
        String::from_utf8(output.stdout)
            .expect("stat output is UTF-8")
            .trim()
            .to_owned()
    }
}

fn streams(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}
