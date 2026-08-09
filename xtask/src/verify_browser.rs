//! Real-browser verifier action and its end-of-run coverage receipt.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use sill::CancellationToken;

use super::{BROWSER_LABEL, CommandRun, RunError};

const RECEIPT_FLAG: &str = "--spice-coverage-receipt";
const PREBUILT_APP_FLAG: &str = "--spice-prebuilt-app";
pub(super) const RECEIPT: &[u8] = b"BROWSER-1kKSWhZk browser coverage complete\n";
static NEXT_RECEIPT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(super) struct BrowserRun {
    pub(super) process: CommandRun,
    pub(super) completed: bool,
}

pub(super) fn run(
    cancellation: &CancellationToken,
    cargo_jobs: usize,
) -> Result<BrowserRun, RunError> {
    let receipt = receipt_path();
    remove_receipt(&receipt).map_err(RunError::Launch)?;
    let process = super::verify_execution::run_process(
        BROWSER_LABEL,
        "npm",
        [
            std::ffi::OsString::from("run"),
            std::ffi::OsString::from("test:canvas"),
            std::ffi::OsString::from("--"),
            std::ffi::OsString::from(RECEIPT_FLAG),
            receipt.as_os_str().to_owned(),
            std::ffi::OsString::from(PREBUILT_APP_FLAG),
        ],
        cancellation,
        cargo_jobs,
        &[],
    )?;
    let completed = receipt_completed(&receipt);
    remove_receipt(&receipt).map_err(|error| RunError::Cleanup(error.to_string()))?;
    Ok(BrowserRun { process, completed })
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
    use std::fs;

    use super::{RECEIPT, receipt_completed, receipt_path, remove_receipt};

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
}
