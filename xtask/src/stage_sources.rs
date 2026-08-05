//! What the committed stage was built from, recorded beside the stage itself.
//!
//! Byte equality is a claim only the owning environment can settle, but the
//! identity of the inputs is a claim every host can settle, so it is recorded
//! separately and compared everywhere. This module owns that listing alone: it
//! says which sources a stage was compiled from, never that the bytes beside it
//! are the ones those sources produce. See [`crate::app`] for the bytes.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use wire::{Digest, Domain};

use crate::app::AppBuildError;

/// Everything the staged app bytes are built from, named rather than derived.
///
/// A directory stands for every regular file beneath it. `app` and `wire` are
/// the only workspace crates the browser build compiles, and `Cargo.lock` pins
/// every registry crate they link; a test refuses either manifest declaring a
/// path dependency this list does not already cover, so the closure cannot grow
/// silently. `app/tests` and `app/browser` are absent because no staged byte is
/// built from them.
const STAGE_SOURCE_INPUTS: [&str; 7] = [
    "Cargo.lock",
    "app/Cargo.toml",
    "app/index.html",
    "app/src",
    "app/style.css",
    "wire/Cargo.toml",
    "wire/src",
];

/// Where the identity of those inputs is recorded, beside the stage it explains.
pub(crate) const STAGE_SOURCE_LISTING: &str = "dist/stage-sources.txt";

/// Record beside the stage the identity of the inputs this run compiled.
pub(crate) fn record_stage_sources(root: &Path) -> Result<(), AppBuildError> {
    let listing = stage_source_listing(root)?;
    let path = root.join(STAGE_SOURCE_LISTING);
    fs::write(&path, listing).map_err(|source| AppBuildError::Io { path, source })
}

/// Refuse a stage whose recorded inputs are not the inputs now in the tree.
///
/// This is the whole of what a host that cannot reproduce the staged bytes can
/// still settle about them, and it settles it without compiling anything: the
/// recorded listing either describes the current sources or it does not. An
/// absent listing is a refusal rather than a pass, because a stage that cannot
/// say what built it is the stage this check exists to catch.
pub(crate) fn compare_stage_sources(root: &Path) -> Result<(), AppBuildError> {
    let path = root.join(STAGE_SOURCE_LISTING);
    let recorded =
        fs::read_to_string(&path).map_err(|source| AppBuildError::Io { path, source })?;
    let stale = disagreeing_source_paths(&recorded, &stage_source_listing(root)?);
    if stale.is_empty() {
        return Ok(());
    }
    Err(AppBuildError::StaleStage(stale))
}

/// The content identity of every staged-app build input, one file per line.
///
/// A line is a file digest and the repository-relative path it covers, in path
/// order. The listing is kept whole rather than folded into one digest so that
/// a refusal can name the files that moved, and so a reader can see which
/// inputs are claimed without running anything.
fn stage_source_listing(root: &Path) -> Result<String, AppBuildError> {
    let mut paths = Vec::new();
    for input in STAGE_SOURCE_INPUTS {
        collect_source_paths(root, input, &mut paths)?;
    }
    paths.sort();
    let mut listing = String::new();
    for path in paths {
        let file = root.join(&path);
        let bytes = fs::read(&file).map_err(|source| AppBuildError::Io { path: file, source })?;
        let _ = writeln!(
            listing,
            "{} {path}",
            Digest::compute(Domain::GateFile, &bytes)
        );
    }
    Ok(listing)
}

/// Every regular file at or beneath one named input, repository-relative.
///
/// The relative path is carried down rather than stripped back off afterwards,
/// so the name recorded for a file is built the same way on every host and no
/// step can fail in a way that has to be interpreted. Anything that is neither
/// a regular file nor a directory refuses, because the stage is built from
/// bytes and only those two shapes say which bytes.
fn collect_source_paths(
    root: &Path,
    relative: &str,
    found: &mut Vec<String>,
) -> Result<(), AppBuildError> {
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path).map_err(|source| AppBuildError::Io {
        path: path.clone(),
        source,
    })?;
    if metadata.is_file() {
        found.push(relative.to_owned());
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(AppBuildError::SourceInput(path));
    }
    for entry in fs::read_dir(&path).map_err(|source| AppBuildError::Io {
        path: path.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| AppBuildError::Io {
            path: path.clone(),
            source,
        })?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| AppBuildError::NonUtf8(entry.path()))?;
        collect_source_paths(root, &format!("{relative}/{name}"), found)?;
    }
    Ok(())
}

/// Which build inputs two listings disagree about, in path order.
///
/// A stage goes stale by an input changing, arriving, or leaving, and all three
/// are answered the same way: a path is stale when its recorded digest is not
/// its observed one, with absence on either side counting as a difference. A
/// line that is not a digest and a path is dropped, which leaves the path it
/// should have covered disagreeing rather than quietly agreeing.
fn disagreeing_source_paths(recorded: &str, observed: &str) -> Vec<String> {
    let read = |listing: &str| -> BTreeMap<String, String> {
        listing
            .lines()
            .filter_map(|line| line.split_once(' '))
            .map(|(digest, path)| (path.to_owned(), digest.to_owned()))
            .collect()
    };
    let recorded = read(recorded);
    let observed = read(observed);
    let mut paths = recorded
        .keys()
        .chain(observed.keys())
        .cloned()
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths.retain(|path| recorded.get(path) != observed.get(path));
    paths
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// The repository these tests read the real recorded listing out of.
    fn repository_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the xtask manifest directory sits inside the repository")
            .to_path_buf()
    }

    /// A stage goes stale three ways, and a comparison answering only one of
    /// them would still pass on the other two. Each class is spelled with its
    /// own path, so an answer that collapsed them cannot look complete.
    #[test]
    fn every_way_a_build_input_can_move_is_reported() {
        let recorded = "aaa app/src/lib.rs\nbbb app/style.css\nccc wire/src/lib.rs\n";
        let observed = "aaa app/src/lib.rs\nzzz app/style.css\nddd app/index.html\n";

        assert_eq!(
            disagreeing_source_paths(recorded, observed),
            ["app/index.html", "app/style.css", "wire/src/lib.rs"],
            "an input that arrived, changed, or left went unreported",
        );
        assert!(
            disagreeing_source_paths(recorded, recorded).is_empty(),
            "a listing disagreed with itself",
        );
    }

    /// The input list is written out rather than derived, so it is only
    /// complete while `app` and `wire` are the only workspace crates the
    /// browser build compiles. A third one would be linked into the stage and
    /// digested by nothing, which is a stale stage this check would call fresh.
    #[test]
    fn the_staged_app_closure_declares_no_path_dependency_off_the_list() {
        let mut declared = Vec::new();
        for manifest in ["app/Cargo.toml", "wire/Cargo.toml"] {
            let text = fs::read_to_string(repository_root().join(manifest))
                .unwrap_or_else(|error| panic!("{manifest}: {error}"));
            for line in text.lines() {
                if let Some((_, rest)) = line.split_once("path = \"") {
                    let (value, _) = rest
                        .split_once('"')
                        .unwrap_or_else(|| panic!("{manifest} left a path unterminated: {line}"));
                    declared.push(format!("{manifest} {value}"));
                }
            }
        }

        assert_eq!(
            declared,
            ["app/Cargo.toml ../wire"],
            "the staged app closure grew a workspace crate; add its sources to \
             STAGE_SOURCE_INPUTS before admitting it here",
        );
    }

    /// The listing beside the stage asserts something about bytes compiled in
    /// another environment, and it is worth nothing unless it is true of this
    /// tree. This is the call `cargo xtask check-app` makes first, made here
    /// without the rebuild that follows it.
    #[test]
    fn the_recorded_stage_identity_describes_the_committed_sources() {
        if let Err(error) = compare_stage_sources(&repository_root()) {
            panic!("{error}");
        }
    }

    /// The refusal is all a host that cannot reproduce the bytes ever sees, so
    /// it carries the whole answer: what moved, where the claim it broke is
    /// recorded, and the one command and environment that can renew it.
    #[test]
    fn a_stale_stage_names_what_moved_and_how_to_renew_it() {
        let refusal = AppBuildError::StaleStage(vec!["wire/src/ident.rs".to_owned()]).to_string();

        for expected in [
            "wire/src/ident.rs",
            STAGE_SOURCE_LISTING,
            "cargo xtask stage-app",
            proof::DEFAULT_IMAGE,
        ] {
            assert!(
                refusal.contains(expected),
                "the refusal never mentioned {expected}: {refusal}",
            );
        }
    }
}
