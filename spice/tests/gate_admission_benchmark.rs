#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde::Serialize;
use spice::install::{apply, installed_binary, installed_git_hooks, plan};
use spice::repository::Repository;

const SAMPLES: usize = 30;

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

struct Fixture {
    root: PathBuf,
    linked: PathBuf,
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
        fs::write(linked.join("candidate.txt"), "candidate\n").expect("write benchmark candidate");
        git(&linked, &["add", "--all"]);

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
            hook,
            binary,
            common,
        }
    }

    fn linked(&self) -> &Path {
        &self.linked
    }

    fn seed_proofs(&self) {
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
    }

    fn invoke_hook(&self) -> Output {
        Command::new(&self.hook)
            .current_dir(&self.linked)
            .env_remove(wire::SpiceVariable::Lane.as_str())
            .env_remove(wire::SpiceVariable::PreCommitSource.as_str())
            .output()
            .expect("run installed benchmark hook")
    }

    fn evidence_read_footprint(&self) -> (u64, u64) {
        let evidence = self.common.join(".spice-rs/gate-evidence/v1");
        [evidence.join("actions"), evidence.join("objects")]
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
