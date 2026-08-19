//! Real-browser verifier action and its end-of-run coverage receipt.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sill::CancellationToken;

use super::{ActionProgress, BROWSER_LABEL, CommandRun, RunError};

const RECEIPT_FLAG: &str = "--spice-coverage-receipt";
const PREBUILT_APP_FLAG: &str = "--spice-prebuilt-app";
const CONCURRENCY_FLAG: &str = "--spice-concurrency";
const CARGO_TARGET_DIR: &str = "CARGO_TARGET_DIR";
const SPICE_CANVAS_BINARY: &str = "SPICE_CANVAS_BINARY"; // env-policy: allow
pub(super) const RECEIPT: &[u8] = b"BROWSER-1kKSWhZk browser coverage complete\n";
static NEXT_RECEIPT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Eq, PartialEq)]
struct BrowserEnvironment {
    binary: PathBuf,
}

#[derive(Debug)]
pub(super) struct BrowserRun {
    pub(super) process: CommandRun,
    pub(super) completed: bool,
}

pub(super) fn run(
    cancellation: &CancellationToken,
    cargo_jobs: usize,
    canvas_workers: usize,
) -> Result<BrowserRun, RunError> {
    run_inner(cancellation, cargo_jobs, canvas_workers, None)
}

pub(super) fn run_with_progress(
    cancellation: &CancellationToken,
    cargo_jobs: usize,
    canvas_workers: usize,
    progress: ActionProgress,
) -> Result<BrowserRun, RunError> {
    run_inner(cancellation, cargo_jobs, canvas_workers, Some(progress))
}

fn run_inner(
    cancellation: &CancellationToken,
    cargo_jobs: usize,
    canvas_workers: usize,
    progress: Option<ActionProgress>,
) -> Result<BrowserRun, RunError> {
    let receipt = receipt_path();
    remove_receipt(&receipt).map_err(RunError::Launch)?;
    let environment = BrowserEnvironment::detect()?;
    let overrides = environment.overrides();
    let arguments = [
        std::ffi::OsString::from("run"),
        std::ffi::OsString::from("test:canvas"),
        std::ffi::OsString::from("--"),
        std::ffi::OsString::from(CONCURRENCY_FLAG),
        std::ffi::OsString::from(canvas_workers.to_string()),
        std::ffi::OsString::from(RECEIPT_FLAG),
        receipt.as_os_str().to_owned(),
        std::ffi::OsString::from(PREBUILT_APP_FLAG),
    ];
    let process = match progress {
        Some(progress) => super::verify_execution::run_process_with_progress(
            BROWSER_LABEL,
            "npm",
            arguments,
            cancellation,
            cargo_jobs,
            &overrides,
            super::verify_execution::ProgressSource::BrowserStdout,
            progress,
        ),
        None => super::verify_execution::run_process(
            BROWSER_LABEL,
            "npm",
            arguments,
            cancellation,
            cargo_jobs,
            &overrides,
        ),
    }?;
    let completed = receipt_completed(&receipt);
    remove_receipt(&receipt).map_err(|error| RunError::Cleanup(error.to_string()))?;
    Ok(BrowserRun { process, completed })
}

impl BrowserEnvironment {
    #[expect(
        clippy::disallowed_methods,
        reason = "the browser process rim resolves the prerequisite Cargo build root once"
    )]
    fn detect() -> Result<Self, RunError> {
        let repository = std::env::current_dir().map_err(RunError::Launch)?;
        let target = std::env::var_os(CARGO_TARGET_DIR);
        Ok(Self::at(&repository, target.as_deref()))
    }

    fn at(repository: &Path, target: Option<&OsStr>) -> Self {
        let target = match target {
            Some(target) if Path::new(target).is_absolute() => PathBuf::from(target),
            Some(target) => repository.join(target),
            None => repository.join("target"),
        };
        let binary = target
            .join("debug")
            .join(format!("spice{}", std::env::consts::EXE_SUFFIX));
        Self { binary }
    }

    fn overrides(&self) -> [(&'static str, &OsStr); 1] {
        [(SPICE_CANVAS_BINARY, self.binary.as_os_str())]
    }
}

fn receipt_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "spice-browser-coverage-{}-{}",
        std::process::id(),
        NEXT_RECEIPT.fetch_add(1, Ordering::Relaxed),
    ))
}

fn remove_receipt(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn receipt_completed(path: &Path) -> bool {
    fs::read(path).is_ok_and(|body| body == RECEIPT)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{
        BrowserEnvironment, RECEIPT, SPICE_CANVAS_BINARY, receipt_completed, receipt_path,
        remove_receipt,
    };

    #[test]
    fn browser_receipts_are_unique_within_one_verifier_process() {
        assert_ne!(receipt_path(), receipt_path());
    }

    #[test]
    fn only_the_exact_complete_receipt_admits_browser_coverage() {
        let receipt = receipt_path();
        fs::write(&receipt, b"incomplete browser receipt\n").expect("write incomplete receipt");
        assert!(!receipt_completed(&receipt));

        fs::write(&receipt, RECEIPT).expect("write complete receipt");
        assert!(receipt_completed(&receipt));

        remove_receipt(&receipt).expect("remove receipt fixture");
        assert!(!receipt_completed(&receipt));
    }

    #[test]
    fn browser_consumes_the_resident_binary_from_the_prerequisite_cargo_root() {
        let repository = Path::new("/fixture/worktree");
        for ambient in [
            None,
            Some(OsStr::new("shared-target")),
            Some(OsStr::new("/shared/cargo-target")),
        ] {
            let environment = BrowserEnvironment::at(repository, ambient);
            let target = match ambient {
                Some(target) if Path::new(target).is_absolute() => PathBuf::from(target),
                Some(target) => repository.join(target),
                None => repository.join("target"),
            };
            assert_eq!(
                environment.binary,
                target
                    .join("debug")
                    .join(format!("spice{}", std::env::consts::EXE_SUFFIX))
            );
            assert_eq!(
                environment.overrides(),
                [(SPICE_CANVAS_BINARY, environment.binary.as_os_str())],
                "npm receives the exact binary built before the concurrency fork"
            );
        }
    }
}
