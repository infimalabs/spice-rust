//! Deterministic count-and-byte retention planning.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{FORMAT_MARKER_NAME, decode_current_bundle, is_bundle_name};

/// Maximum recognized source bundles retained after a completed publication.
pub(super) const MAX_BUNDLES: usize = 12;
/// Maximum bytes in every regular archive entry, including the format marker
/// and retained foreign files. The measured twelve-bundle corpus is about
/// 10 MiB, leaving explicit headroom without restoring unbounded growth.
pub(super) const MAX_STORED_BYTES: u64 = 16 * 1024 * 1024;

pub(super) struct Plan {
    evictions: Vec<PathBuf>,
}

impl Plan {
    pub(super) fn evictions(&self) -> &[PathBuf] {
        &self.evictions
    }
}

#[derive(Clone)]
struct Entry {
    path: PathBuf,
    source: Option<String>,
    bytes: u64,
    modified: Option<SystemTime>,
    certain: bool,
}

impl Entry {
    fn owned_bundle(&self) -> bool {
        self.source.is_some()
    }

    fn protected(&self, protected: &BTreeSet<String>) -> bool {
        !self.certain
            || self
                .source
                .as_ref()
                .is_some_and(|source| protected.contains(source))
    }
}

pub(super) fn plan(
    root: &Path,
    pending_path: &Path,
    pending_bytes: &[u8],
    protected: &BTreeSet<String>,
) -> Result<Plan, String> {
    decode_current_bundle(pending_bytes).map_err(|error| {
        format!(
            "validate pending archive bundle {}: {error}",
            pending_path.display(),
        )
    })?;
    let mut entries = scan(root, Some(pending_path))?;
    let source = pending_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| {
            format!(
                "pending bundle has no UTF-8 source: {}",
                pending_path.display()
            )
        })?;
    entries.push(Entry {
        path: pending_path.to_path_buf(),
        source: Some(source.to_owned()),
        bytes: u64::try_from(pending_bytes.len()).expect("stored bundle length fits u64"),
        modified: Some(SystemTime::now()),
        certain: true,
    });
    build(entries, protected)
}

pub(super) fn verify_bounds(root: &Path) -> Result<(), String> {
    let entries = scan(root, None)?;
    let protected = entries
        .iter()
        .filter_map(|entry| entry.source.clone())
        .collect::<BTreeSet<_>>();
    let plan = build(entries, &protected)?;
    if !plan.evictions.is_empty() {
        return Err("archive remained above its count or byte ceiling after grooming".to_owned());
    }
    Ok(())
}

fn scan(root: &Path, replaced: Option<&Path>) -> Result<Vec<Entry>, String> {
    let listing =
        fs::read_dir(root).map_err(|error| format!("list archive {}: {error}", root.display()))?;
    let mut entries = Vec::new();
    for entry in listing {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!("read archive entry in {}: {error}", root.display(),));
            }
        };
        let path = entry.path();
        if replaced.is_some_and(|replaced| path == replaced) {
            continue;
        }
        let name = entry.file_name();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "archive entry metadata is uncertain for {}: {error}",
                    path.display(),
                ));
            }
        };
        if !metadata.file_type().is_file() {
            return Err(format!(
                "archive entry is not a regular file; stored bytes are uncertain: {}",
                path.display(),
            ));
        }
        let bytes = metadata.len();
        if name.to_str() == Some(FORMAT_MARKER_NAME) {
            entries.push(Entry {
                path,
                source: None,
                bytes,
                modified: None,
                certain: true,
            });
            continue;
        }
        let Some(name) = name.to_str() else {
            entries.push(Entry {
                path,
                source: None,
                bytes,
                modified: None,
                certain: false,
            });
            continue;
        };
        if !is_bundle_name(name) {
            entries.push(Entry {
                path,
                source: None,
                bytes,
                modified: None,
                certain: false,
            });
            continue;
        }
        let stored = match fs::read(&path) {
            Ok(stored) => stored,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!("read archive bundle {}: {error}", path.display()));
            }
        };
        decode_current_bundle(&stored).map_err(|error| {
            format!(
                "archive bundle {} is unknown or corrupt: {error}",
                path.display()
            )
        })?;
        let modified = metadata.modified().ok();
        entries.push(Entry {
            path,
            source: name.strip_suffix(".json").map(str::to_owned),
            bytes,
            modified,
            certain: modified.is_some(),
        });
    }
    Ok(entries)
}

fn build(mut entries: Vec<Entry>, protected: &BTreeSet<String>) -> Result<Plan, String> {
    let mut bundles = entries.iter().filter(|entry| entry.owned_bundle()).count();
    let mut bytes = entries.iter().try_fold(0_u64, |total, entry| {
        total
            .checked_add(entry.bytes)
            .ok_or_else(|| "archive stored-byte total overflowed u64".to_owned())
    })?;
    let mut candidates = entries
        .drain(..)
        .filter(|entry| entry.owned_bundle() && !entry.protected(protected))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.modified
            .cmp(&right.modified)
            .then_with(|| left.path.cmp(&right.path))
    });
    let mut evictions = Vec::new();
    for candidate in candidates {
        if bundles <= MAX_BUNDLES && bytes <= MAX_STORED_BYTES {
            break;
        }
        bundles = bundles.saturating_sub(1);
        bytes = bytes.saturating_sub(candidate.bytes);
        evictions.push(candidate.path);
    }
    if bundles > MAX_BUNDLES || bytes > MAX_STORED_BYTES {
        return Err(format!(
            concat!(
                "full-verifier archive cannot meet its retention ceilings: ",
                "retained bundles={}/{}, stored bytes={}/{}; ",
                "protected or uncertain entries must be resolved or archived manually"
            ),
            bundles, MAX_BUNDLES, bytes, MAX_STORED_BYTES,
        ));
    }
    Ok(Plan { evictions })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_modification_times_evict_by_stable_path_order() {
        let modified = SystemTime::UNIX_EPOCH;
        let entries = ["b", "a"]
            .into_iter()
            .map(|source| Entry {
                path: PathBuf::from(format!("{source}.json")),
                source: Some(source.to_owned()),
                bytes: 1,
                modified: Some(modified),
                certain: true,
            })
            .collect();
        let plan = build(entries, &BTreeSet::new()).expect("build tie plan");
        assert!(plan.evictions.is_empty());

        let mut entries = ["b", "a"]
            .into_iter()
            .map(|source| Entry {
                path: PathBuf::from(format!("{source}.json")),
                source: Some(source.to_owned()),
                bytes: MAX_STORED_BYTES,
                modified: Some(modified),
                certain: true,
            })
            .collect::<Vec<_>>();
        entries.push(Entry {
            path: PathBuf::from("marker"),
            source: None,
            bytes: 1,
            modified: None,
            certain: true,
        });
        let plan = build(entries, &BTreeSet::new()).expect("build over-byte tie plan");
        assert_eq!(
            plan.evictions,
            [PathBuf::from("a.json"), PathBuf::from("b.json")]
        );
    }
}
