//! At-rest grooming for the repository-common verifier evidence archive.
//!
//! Archived merge-parent bundles are a cache over re-execution: absence or
//! drift is admitted as a normal cache miss by every reader, so grooming can
//! bound the archive without ever changing an admission verdict. Bundles are
//! stored gzip-compressed and sniffed by magic, never renamed, so one reader
//! path serves both compressed and legacy plain documents.

use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

/// Gzip member magic; the sniff that keeps at-rest encoding self-describing.
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
/// Newest bundles the publication sweep keeps. Eviction degrades a later
/// admission to normal re-execution, so the bound needs only to outlast the
/// merge parents an open resolution can plausibly still name.
const ARCHIVE_RETAIN_BUNDLES: usize = 12;
/// Hex length of a bundle stem: archived names are one source digest each.
const BUNDLE_STEM_HEX: usize = 64;

/// Compress one encoded document for at-rest storage.
pub(super) fn compress_document(document: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(document)?;
    encoder.finish()
}

/// Return the plain document bytes whether stored compressed or legacy plain.
pub(super) fn expand_document(document: Vec<u8>) -> io::Result<Vec<u8>> {
    if !document.starts_with(&GZIP_MAGIC) {
        return Ok(document);
    }
    let mut plain = Vec::new();
    GzDecoder::new(document.as_slice()).read_to_end(&mut plain)?;
    Ok(plain)
}

/// Bound the archive to the newest bundles and compress retained plain ones.
///
/// Only exact bundle names are groomed; unrecognized entries are retained and
/// left untouched. A bundle whose modification time cannot be read sorts as
/// newest so uncertainty always retains.
pub(super) fn groom_archive(root: &Path) -> Result<(), String> {
    let (retained, evicted) = partition_retained(bundle_entries(root)?);
    for path in evicted {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("evict {}: {error}", path.display())),
        }
    }
    for path in retained {
        migrate_plain_bundle(&path)?;
    }
    Ok(())
}

/// Split bundle paths into the retained newest set and the evicted remainder.
fn partition_retained(mut entries: Vec<(PathBuf, SystemTime)>) -> (Vec<PathBuf>, Vec<PathBuf>) {
    entries.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
    let mut paths = entries.into_iter().map(|(path, _)| path);
    let retained = paths.by_ref().take(ARCHIVE_RETAIN_BUNDLES).collect();
    (retained, paths.collect())
}

fn bundle_entries(root: &Path) -> Result<Vec<(PathBuf, SystemTime)>, String> {
    let listing =
        fs::read_dir(root).map_err(|error| format!("list archive {}: {error}", root.display()))?;
    let mut entries = Vec::new();
    for entry in listing {
        let entry =
            entry.map_err(|error| format!("read archive entry in {}: {error}", root.display()))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !is_bundle_name(name) {
            continue;
        }
        let newest_on_uncertainty = SystemTime::now();
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(newest_on_uncertainty);
        entries.push((entry.path(), modified));
    }
    Ok(entries)
}

fn is_bundle_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".json") else {
        return false;
    };
    stem.len() == BUNDLE_STEM_HEX
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// Rewrite one retained legacy plain bundle to the compressed representation.
fn migrate_plain_bundle(path: &Path) -> Result<(), String> {
    let document =
        fs::read(path).map_err(|error| format!("read bundle {}: {error}", path.display()))?;
    if document.starts_with(&GZIP_MAGIC) {
        return Ok(());
    }
    let compressed = compress_document(&document)
        .map_err(|error| format!("compress bundle {}: {error}", path.display()))?;
    atomic::Destination::new(path)
        .map_err(|error| format!("resolve bundle {}: {error}", path.display()))?
        .publish(|staged| staged.write_all(&compressed))
        .map_err(|error| format!("migrate bundle {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn round_trip_restores_exact_document_bytes() {
        let document = br#"{"version":1,"battery":"payload"}"#.to_vec();
        let compressed = compress_document(&document).expect("compress document");
        assert!(compressed.starts_with(&GZIP_MAGIC));
        assert_eq!(
            expand_document(compressed).expect("expand compressed"),
            document,
        );
    }

    #[test]
    fn plain_documents_pass_through_expansion_unchanged() {
        let document = br#"{"version":1}"#.to_vec();
        assert_eq!(
            expand_document(document.clone()).expect("expand plain"),
            document,
        );
    }

    #[test]
    fn bundle_names_admit_only_exact_source_digests() {
        let digest = "a".repeat(BUNDLE_STEM_HEX);
        assert!(is_bundle_name(&format!("{digest}.json")));
        assert!(!is_bundle_name(&format!("{digest}.json.tmp")));
        assert!(!is_bundle_name(&format!("{}.json", "a".repeat(63))));
        assert!(!is_bundle_name(&format!("{}Z.json", "a".repeat(63))));
        assert!(!is_bundle_name("full-verifier-evidence.json"));
    }

    #[test]
    fn partition_keeps_the_newest_bound_and_evicts_the_rest() {
        let base = SystemTime::UNIX_EPOCH;
        let entries = (0..ARCHIVE_RETAIN_BUNDLES + 3)
            .map(|age| {
                let path = PathBuf::from(format!("{age}.json"));
                (path, base + Duration::from_secs(1000 - age as u64))
            })
            .collect::<Vec<_>>();
        let (retained, evicted) = partition_retained(entries);
        assert_eq!(retained.len(), ARCHIVE_RETAIN_BUNDLES);
        assert_eq!(
            evicted,
            [
                PathBuf::from(format!("{}.json", ARCHIVE_RETAIN_BUNDLES)),
                PathBuf::from(format!("{}.json", ARCHIVE_RETAIN_BUNDLES + 1)),
                PathBuf::from(format!("{}.json", ARCHIVE_RETAIN_BUNDLES + 2)),
            ],
        );
    }

    #[test]
    fn groom_bounds_migrates_and_retains_foreign_entries() {
        let root = std::env::temp_dir().join(format!(
            "spice-evidence-groom-{}-{}",
            std::process::id(),
            line!(),
        ));
        fs::create_dir_all(&root).expect("create groom fixture root");
        let width = BUNDLE_STEM_HEX;
        for ordinal in 0..ARCHIVE_RETAIN_BUNDLES + 2 {
            let stem = format!("{ordinal:0width$x}");
            fs::write(root.join(format!("{stem}.json")), b"{\"plain\":true}")
                .expect("write bundle fixture");
        }
        let foreign = root.join("not-a-bundle.txt");
        fs::write(&foreign, b"foreign").expect("write foreign fixture");

        groom_archive(&root).expect("groom fixture archive");

        let bundles = fs::read_dir(&root)
            .expect("list groomed root")
            .map(|entry| entry.expect("groomed entry").path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect::<Vec<_>>();
        assert_eq!(bundles.len(), ARCHIVE_RETAIN_BUNDLES);
        for bundle in &bundles {
            let stored = fs::read(bundle).expect("read groomed bundle");
            assert!(stored.starts_with(&GZIP_MAGIC));
            assert_eq!(
                expand_document(stored).expect("expand groomed bundle"),
                b"{\"plain\":true}".to_vec(),
            );
        }
        assert_eq!(
            fs::read(&foreign).expect("read foreign entry"),
            b"foreign".to_vec(),
        );
        fs::remove_dir_all(&root).expect("remove groom fixture root");
    }
}
