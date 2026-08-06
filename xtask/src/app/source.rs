//! Exact worktree-source identity for one browser-app compilation.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use wire::{Digest, Domain};

use super::AppBuildError;

const SOURCE_FILES: [&str; 7] = [
    "Cargo.lock",
    "Cargo.toml",
    "rust-toolchain.toml",
    "app/Cargo.toml",
    "app/index.html",
    "app/style.css",
    "wire/Cargo.toml",
];
const SOURCE_DIRECTORIES: [&str; 2] = ["app/src", "wire/src"];

/// One observation of every worktree input the browser-app build reads.
pub(super) struct SourceSnapshot {
    inputs: BTreeMap<String, Digest>,
    identity: Digest,
}

impl SourceSnapshot {
    /// Capture the worktree bytes before compilation starts.
    pub(super) fn capture(root: &Path) -> Result<Self, AppBuildError> {
        let mut inputs = BTreeMap::new();
        let mut framed = Vec::new();
        for relative in source_paths(root)? {
            let path = root.join(&relative);
            let bytes = fs::read(&path).map_err(|source| AppBuildError::Io { path, source })?;
            framed.extend_from_slice(&(relative.len() as u64).to_be_bytes());
            framed.extend_from_slice(relative.as_bytes());
            framed.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
            framed.extend_from_slice(&bytes);
            inputs.insert(relative, Digest::compute(Domain::GateFile, &bytes));
        }
        let identity = Digest::compute(Domain::DistInventory, &framed);
        Ok(Self { inputs, identity })
    }

    pub(super) const fn identity(&self) -> &Digest {
        &self.identity
    }

    /// Refuse to pair output with an identity captured before a source moved.
    pub(super) fn verify_unchanged(&self, root: &Path) -> Result<(), AppBuildError> {
        let observed = Self::capture(root)?;
        let changed = disagreeing_paths(&self.inputs, &observed.inputs);
        if changed.is_empty() {
            return Ok(());
        }
        Err(AppBuildError::SourceIdentity(format!(
            "inputs changed during compilation: {}",
            changed.join(", ")
        )))
    }
}

fn source_paths(root: &Path) -> Result<Vec<String>, AppBuildError> {
    let mut paths = SOURCE_FILES
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for relative in &paths {
        admit_regular(&root.join(relative))?;
    }
    for directory in SOURCE_DIRECTORIES {
        collect_directory(root, &root.join(directory), &mut paths)?;
    }
    paths.sort();
    Ok(paths)
}

fn collect_directory(
    root: &Path,
    directory: &Path,
    paths: &mut Vec<String>,
) -> Result<(), AppBuildError> {
    let metadata = fs::symlink_metadata(directory).map_err(|source| AppBuildError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_dir() {
        return Err(source_path_error(directory, "is not a directory"));
    }
    let mut entries = fs::read_dir(directory)
        .map_err(|source| AppBuildError::Io {
            path: directory.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| AppBuildError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| AppBuildError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_dir() {
            collect_directory(root, &path, paths)?;
        } else if metadata.file_type().is_file() {
            paths.push(relative_spelling(root, &path)?);
        } else {
            return Err(source_path_error(
                &path,
                "is not a regular file or directory",
            ));
        }
    }
    Ok(())
}

fn admit_regular(path: &Path) -> Result<(), AppBuildError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| AppBuildError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_file() {
        Ok(())
    } else {
        Err(source_path_error(path, "is not a regular file"))
    }
}

fn relative_spelling(root: &Path, path: &Path) -> Result<String, AppBuildError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        source_path_error(path, "escaped the worktree while sources were collected")
    })?;
    relative
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| AppBuildError::NonUtf8(PathBuf::from(relative)))
}

fn source_path_error(path: &Path, reason: &str) -> AppBuildError {
    AppBuildError::SourceIdentity(format!("{} {reason}", path.display()))
}

fn disagreeing_paths(
    expected: &BTreeMap<String, Digest>,
    observed: &BTreeMap<String, Digest>,
) -> Vec<String> {
    let mut paths = expected
        .keys()
        .chain(observed.keys())
        .cloned()
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths.retain(|path| expected.get(path) != observed.get(path));
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask belongs to the workspace")
            .to_path_buf()
    }

    fn source_tree() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-app-source-snapshot-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale source fixture");
        }
        fs::create_dir_all(&root).expect("create source fixture");
        for relative in SOURCE_FILES {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("source file has a parent"))
                .expect("create source-file parent");
            fs::write(path, relative).expect("write source-file fixture");
        }
        for directory in SOURCE_DIRECTORIES {
            let path = root.join(directory).join("lib.rs");
            fs::create_dir_all(path.parent().expect("source module has a parent"))
                .expect("create source-module parent");
            fs::write(path, directory).expect("write source-module fixture");
        }
        root
    }

    #[test]
    fn current_app_sources_are_closed_and_include_worktree_only_files() {
        let root = repository_root();
        let snapshot = SourceSnapshot::capture(&root).expect("capture current app sources");
        let paths = source_paths(&root).expect("enumerate current app sources");

        assert!(!snapshot.inputs.is_empty());
        assert!(paths.iter().any(|path| path == "app/src/browser.rs"));
        assert!(paths.iter().any(|path| path == "wire/src/lib.rs"));
        for manifest in ["app/Cargo.toml", "wire/Cargo.toml"] {
            let text = fs::read_to_string(root.join(manifest)).expect("read app-closure manifest");
            assert!(
                !text
                    .lines()
                    .any(|line| line.trim_start().starts_with("build =")),
                "{manifest} gained a custom build input outside the source closure"
            );
        }
        let app = fs::read_to_string(root.join("app/Cargo.toml")).expect("read app manifest");
        assert!(
            app.contains("wire = { path = \"../wire\" }"),
            "the app path dependency moved outside the captured wire closure"
        );
    }

    #[test]
    fn untracked_source_arrival_and_movement_change_the_identity() {
        let root = source_tree();
        let before = SourceSnapshot::capture(&root).expect("capture fixture sources");
        let untracked = root.join("app/src/worktree_only.rs");
        fs::write(&untracked, "first worktree-only source").expect("write worktree-only source");

        let error = before
            .verify_unchanged(&root)
            .expect_err("an arriving source must move the identity");
        assert!(
            error.to_string().contains("app/src/worktree_only.rs"),
            "the arriving source was not named: {error}"
        );

        let captured = SourceSnapshot::capture(&root).expect("capture worktree-only source");
        fs::write(untracked, "different worktree-only source").expect("move worktree-only source");
        let error = captured
            .verify_unchanged(&root)
            .expect_err("a changed source must move the identity");
        assert!(
            error.to_string().contains("app/src/worktree_only.rs"),
            "the changed source was not named: {error}"
        );
        fs::remove_dir_all(root).expect("remove source fixture");
    }

    #[test]
    fn every_source_inventory_difference_is_named() {
        let digest = |bytes| Digest::compute(Domain::GateFile, bytes);
        let expected = BTreeMap::from([
            ("app/src/lib.rs".to_owned(), digest(b"one")),
            ("wire/src/lib.rs".to_owned(), digest(b"two")),
        ]);
        let observed = BTreeMap::from([
            ("app/src/lib.rs".to_owned(), digest(b"three")),
            ("app/style.css".to_owned(), digest(b"four")),
        ]);

        assert_eq!(
            disagreeing_paths(&expected, &observed),
            ["app/src/lib.rs", "app/style.css", "wire/src/lib.rs"]
        );
    }
}
