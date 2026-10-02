//! Exact file export for complete canonical documents and evidence.
//!
//! A bounded read never streams a whole document to the terminal; the caller
//! names a file instead. The bytes are published atomically beside the
//! destination, so a reader sees the previous file or the complete new one,
//! and the receipt carries the length and digest a later reader verifies
//! against. `-` is refused rather than read as standard output: that spelling
//! would turn the explicit export back into unbounded terminal output.

use std::io::{self, Cursor, Read};
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
    export_reader(
        path,
        Cursor::new(bytes),
        u64::try_from(bytes.len()).expect("an in-memory length fits u64"),
    )
}

/// Stream exactly `expected_bytes` to `path` atomically and return its receipt.
///
/// # Errors
///
/// Refuses the same destinations and publication failures as [`export`], and
/// refuses a reader that ends early or continues past the expected length.
pub fn export_reader(
    path: &Path,
    mut reader: impl Read,
    expected_bytes: u64,
) -> Result<Export, Refusal> {
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
    let mut receipt = None;
    destination
        .publish(|staged| {
            let (bytes, digest) = if expected_bytes == 0 {
                let mut extra = [0_u8; 1];
                if reader.read(&mut extra)? != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "export reader exceeds its expected 0-byte length",
                    ));
                }
                (0, Digest::compute(Domain::CliExport, &[]))
            } else {
                Digest::copy_reader(Domain::CliExport, &mut reader, staged, expected_bytes)?
            };
            if bytes != expected_bytes {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("export reader ended at {bytes} bytes; expected {expected_bytes}"),
                ));
            }
            receipt = Some((bytes, digest));
            Ok(())
        })
        .map_err(|error| {
            refusal(
                "output::export-write",
                destination.path().display().to_string(),
                &error.to_string(),
            )
        })?;
    let (bytes, digest) = receipt.expect("successful publication measured its input");
    Ok(Export {
        path: destination.path().display().to_string(),
        bytes,
        digest,
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

    #[test]
    fn a_streaming_export_requires_the_exact_declared_length() {
        let root = std::env::temp_dir().join(format!("spice-stream-export-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("fixture directory");
        let path = root.join("payload.bin");
        std::fs::write(&path, b"previous").expect("fixture file");

        let payload = b"\0streamed payload\xff";
        let receipt = export_reader(&path, Cursor::new(payload), payload.len() as u64)
            .expect("exact stream publishes");
        assert_eq!(std::fs::read(&receipt.path).expect("read export"), payload);

        for expected in [payload.len() as u64 - 1, payload.len() as u64 + 1] {
            assert!(
                export_reader(&path, Cursor::new(payload), expected).is_err(),
                "length mismatch refuses"
            );
            assert_eq!(
                std::fs::read(&path).expect("read preserved export"),
                payload,
                "a refused stream leaves the destination unchanged"
            );
        }
        std::fs::remove_dir_all(&root).expect("fixture directory removes");
    }
}
