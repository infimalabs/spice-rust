//! Real-browser verifier action and its end-of-run coverage receipt.

use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use super::Status;

const RECEIPT_FLAG: &str = "--spice-coverage-receipt";
const PREBUILT_APP_FLAG: &str = "--spice-prebuilt-app";
pub(super) const RECEIPT: &[u8] = b"BROWSER-1kKSWhZk browser coverage complete\n";
static NEXT_RECEIPT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BrowserRun {
    pub(super) status: Status,
    pub(super) completed: bool,
}

pub(super) fn run() -> io::Result<BrowserRun> {
    let receipt = receipt_path();
    remove_receipt(&receipt)?;
    let process = Command::new("npm")
        .args(["run", "test:canvas", "--", RECEIPT_FLAG])
        .arg(&receipt)
        .arg(PREBUILT_APP_FLAG)
        .status()?;
    let completed = fs::read(&receipt).is_ok_and(|body| body == RECEIPT);
    remove_receipt(&receipt)?;
    Ok(BrowserRun {
        status: Status {
            success: process.success(),
            code: process.code(),
        },
        completed,
    })
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

#[cfg(test)]
mod tests {
    use super::receipt_path;

    #[test]
    fn browser_receipts_are_unique_within_one_verifier_process() {
        assert_ne!(receipt_path(), receipt_path());
    }
}
