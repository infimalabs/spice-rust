//! Exact file export for complete canonical documents and evidence.
//!
//! A bounded read never streams a whole document to the terminal; the caller
//! names a file instead. The bytes are published atomically beside the
//! destination, so a reader sees the previous file or the complete new one,
//! and the receipt carries the length and digest a later reader verifies
//! against. `-` is refused rather than read as standard output: that spelling
//! would turn the explicit export back into unbounded terminal output.

use std::io::Write as _;
use std::path::Path;

use wire::output::Export;
use wire::{Digest, Domain, Refusal, RefusalDiagnostic};

/// Write `bytes` to `path` atomically and return the receipt.
///
/// # Errors
///
/// Refuses `-`, a destination with no file name or no existing parent, and
/// any write, sync, or rename failure; the destination is unchanged unless
/// only the final directory sync failed.
pub fn export(path: &Path, bytes: &[u8]) -> Result<Export, Refusal> {
    if path == Path::new("-") {
        return Err(refusal(
            "output::export-stdout",
            "-".to_owned(),
            "name a file; a complete export never streams to standard output",
        ));
    }
    let destination = atomic::Destination::new(path).map_err(|error| {
        refusal(
            "output::export-destination",
            path.display().to_string(),
            &error.to_string(),
        )
    })?;
    destination
        .publish(|staged| staged.write_all(bytes))
        .map_err(|error| {
            refusal(
                "output::export-write",
                destination.path().display().to_string(),
                &error.to_string(),
            )
        })?;
    Ok(Export {
        path: destination.path().display().to_string(),
        bytes: u64::try_from(bytes.len()).expect("an in-memory length fits u64"),
        digest: Digest::compute(Domain::CliExport, bytes),
    })
}

fn refusal(reason: &str, observed: String, detail: &str) -> Refusal {
    Refusal::bare(
        RefusalDiagnostic::new(
            reason,
            Some("output-export".to_owned()),
            Some("one complete document written to a named file".to_owned()),
            Some(observed),
        )
        .with_detail(detail),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_export_replaces_the_file_whole_and_its_receipt_verifies_the_bytes() {
        let root = std::env::temp_dir().join(format!("spice-export-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("fixture directory");
        let path = root.join("ledger.md");
        std::fs::write(&path, "a longer previous document").expect("fixture file");

        let document = "# Ledger\nnaïve 🦀\n".as_bytes();
        let receipt = export(&path, document).expect("export publishes");
        let written = std::fs::read(&receipt.path).expect("exported file reads");

        assert_eq!(written, document, "no residue of the previous file");
        assert_eq!(receipt.bytes, document.len() as u64);
        assert_eq!(receipt.digest, Digest::compute(Domain::CliExport, &written));
        assert!(
            export(Path::new("-"), document).is_err(),
            "a complete export never streams to standard output"
        );
        std::fs::remove_dir_all(&root).expect("fixture directory removes");
    }
}
