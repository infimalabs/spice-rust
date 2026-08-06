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
use std::process::Command;

use wire::{Digest, Domain};

use crate::app::AppBuildError;

/// The one program the tracked set is read through.
///
/// Named absolutely, the way the gate names its own Git arms, so this reads no
/// ambient search path. The listing it feeds is compared across machines, and
/// two of them resolving two different programs would put a difference in the
/// record that no source ever moved.
const GIT: &str = "/usr/bin/git";

/// Everything the staged app bytes are built from, named rather than derived.
///
/// A directory stands for every tracked file beneath it. `app` and `wire` are
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

/// Where the staging run is written out, including what a lane must do first.
///
/// Naming the image and the command is not yet enough to act on. Every
/// development lane is a linked worktree, and the staging run asks `git` inside
/// the mount, which cannot resolve a `.git` that points outside it — so the
/// documented invocation refuses from the place the refusal is read. The way
/// through is a clone of the same commit, and the refusal is the only moment a
/// lane is looking, so it says where that is written down.
pub(crate) const STAGE_INSTRUCTIONS: &str = "dist/proof-image/README.md";

/// One exact observation of the inputs a staged-app build reads.
///
/// The digest is an internal build-cache namespace, not a second durable
/// record. It binds a Cargo target directory to the complete listing so two
/// lanes whose source bytes differ cannot address the same compiled artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StageSourceSnapshot {
    listing: String,
    identity: Digest,
}

impl StageSourceSnapshot {
    /// The content identity used to isolate one app build from every build
    /// over a different source inventory.
    pub(crate) const fn identity(&self) -> &Digest {
        &self.identity
    }
}

/// Observe the exact inputs an app build about to start will read.
pub(crate) fn capture_stage_sources(root: &Path) -> Result<StageSourceSnapshot, AppBuildError> {
    let listing = stage_source_listing(root)?;
    let identity = Digest::compute(Domain::DistInventory, listing.as_bytes());
    Ok(StageSourceSnapshot { listing, identity })
}

/// Record beside the stage the identity of the inputs this run compiled.
///
/// The inputs are observed again before the record is written. A source that
/// moves during the build therefore fails the staging run instead of pairing
/// its output with a listing observed at a different instant.
pub(crate) fn record_stage_sources(
    root: &Path,
    expected: &StageSourceSnapshot,
) -> Result<(), AppBuildError> {
    let observed = capture_stage_sources(root)?;
    let changed = disagreeing_source_paths(&expected.listing, &observed.listing);
    if !changed.is_empty() {
        return Err(AppBuildError::SourcesChangedDuringBuild(changed));
    }
    let path = root.join(STAGE_SOURCE_LISTING);
    fs::write(&path, &expected.listing).map_err(|source| AppBuildError::Io { path, source })
}

/// Refuse a stage whose recorded inputs are not the inputs now in the tree.
///
/// This is the whole of what a host that cannot reproduce the staged bytes can
/// still settle about them, and it settles it without compiling anything: the
/// recorded listing either describes the current sources or it does not. An
/// absent listing is a refusal rather than a pass, because a stage that cannot
/// say what built it is the stage this check exists to catch.
pub(crate) fn compare_stage_sources(root: &Path) -> Result<StageSourceSnapshot, AppBuildError> {
    let path = root.join(STAGE_SOURCE_LISTING);
    let recorded =
        fs::read_to_string(&path).map_err(|source| AppBuildError::Io { path, source })?;
    let observed = capture_stage_sources(root)?;
    let stale = disagreeing_source_paths(&recorded, &observed.listing);
    if stale.is_empty() {
        return Ok(observed);
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
    let mut listing = String::new();
    for path in tracked_source_paths(root)? {
        let file = root.join(&path);
        let metadata = fs::symlink_metadata(&file).map_err(|source| AppBuildError::Io {
            path: file.clone(),
            source,
        })?;
        if !metadata.is_file() {
            return Err(AppBuildError::SourceInput(file));
        }
        let bytes = fs::read(&file).map_err(|source| AppBuildError::Io { path: file, source })?;
        let _ = writeln!(
            listing,
            "{} {path}",
            Digest::compute(Domain::GateFile, &bytes)
        );
    }
    Ok(listing)
}

/// Every tracked file at or beneath the named inputs, repository-relative.
///
/// The tracked set is asked for rather than the directories walked, because a
/// stray beside these sources compiles nothing and so is not something the
/// stage was built from. A walk records it anyway, and then every host that
/// does not carry that stray reads the stage as stale: one `.DS_Store` under
/// `app/src` is enough to red `cargo xtask check-app` on a lane whose `git
/// status` and `git log` are both clean, because an operator's global excludes
/// hide such a file from every report but this one, which never asked git.
///
/// Each declared input has to name at least one tracked file. An input that
/// names none was renamed, deleted, or never added, and a listing that simply
/// grew shorter would state a closure the stage was not built from -- so this
/// refuses instead, which is also what answers a tree that is no repository at
/// all rather than recording every input as absent.
fn tracked_source_paths(root: &Path) -> Result<Vec<String>, AppBuildError> {
    let output = Command::new(GIT)
        .args(["ls-files", "-z", "--cached", "--"])
        .args(STAGE_SOURCE_INPUTS)
        .current_dir(root)
        .output()
        .map_err(|error| {
            AppBuildError::UnreadableSourceIndex(format!("{GIT} did not run: {error}"))
        })?;
    if !output.status.success() {
        return Err(AppBuildError::UnreadableSourceIndex(format!(
            "{GIT} reported {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let listed = String::from_utf8(output.stdout).map_err(|_| {
        AppBuildError::UnreadableSourceIndex(format!("{GIT} named a path that is not UTF-8"))
    })?;
    let mut paths = listed
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    paths.sort();
    for input in STAGE_SOURCE_INPUTS {
        if !paths.iter().any(|path| covers(input, path)) {
            return Err(AppBuildError::UntrackedSourceInput(input.to_owned()));
        }
    }
    Ok(paths)
}

/// Whether one declared input is this path, or the directory holding it.
///
/// The separator is required rather than left to the prefix, so an input never
/// answers for a sibling whose name merely starts with its own: `wire/src`
/// covers `wire/src/lib.rs` and not `wire/src-old/lib.rs`, and an input the
/// repository stopped tracking is then reported as untracked instead of being
/// covered by whatever sits beside it.
fn covers(input: &str, path: &str) -> bool {
    path == input
        || path
            .strip_prefix(input)
            .is_some_and(|rest| rest.starts_with('/'))
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

    /// One tracked file for every declared input, which is what a fixture has
    /// to carry to be a repository this listing can be made from.
    ///
    /// Written out rather than derived from [`STAGE_SOURCE_INPUTS`], because a
    /// fixture that derived its own contents would agree with any list at all.
    /// It cannot drift silently either: an input this misses names no tracked
    /// file, and every test below then refuses before it asserts anything.
    const FIXTURE_FILES: [&str; 7] = [
        "Cargo.lock",
        "app/Cargo.toml",
        "app/index.html",
        "app/src/lib.rs",
        "app/style.css",
        "wire/Cargo.toml",
        "wire/src/lib.rs",
    ];

    /// A repository carrying every declared build input, each one tracked.
    ///
    /// A real repository rather than a directory, because the whole question
    /// is what git says is in it, and an index is not something a filesystem
    /// fixture can imitate.
    fn tracked_repository(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-stage-sources-{name}-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale fixture");
        }
        fs::create_dir_all(&root).expect("create the fixture repository");
        git(&root, &["init", "-q"]);
        for file in FIXTURE_FILES {
            let path = root.join(file);
            let parent = path
                .parent()
                .expect("every fixture file sits in a directory");
            fs::create_dir_all(parent).expect("create the fixture source directory");
            fs::write(&path, file).expect("write a fixture build input");
        }
        let mut add = vec!["add", "-f", "--"];
        add.extend(FIXTURE_FILES);
        git(&root, &add);
        root
    }

    /// Rebuild the selected source tree as a repository when the test runner
    /// deliberately materializes tracked files without their Git metadata.
    ///
    /// Gate does exactly that for its immutable mutation baseline. Walking the
    /// declared inputs is sound in that private tree because every file Gate
    /// admitted came from the selected index; unlike the caller's worktree it
    /// cannot contain an ignored editor stray. Copying into another temporary
    /// tree also leaves Gate's sealed snapshot untouched.
    fn repository_from_selected_tree(source: &Path) -> PathBuf {
        fn copy_selected(source: &Path, destination: &Path) {
            let metadata = fs::symlink_metadata(source)
                .unwrap_or_else(|error| panic!("inspect {}: {error}", source.display()));
            if metadata.is_dir() {
                fs::create_dir_all(destination).unwrap_or_else(|error| {
                    panic!(
                        "create selected directory {}: {error}",
                        destination.display()
                    )
                });
                let mut entries = fs::read_dir(source)
                    .unwrap_or_else(|error| panic!("read {}: {error}", source.display()))
                    .map(|entry| entry.expect("read selected directory entry"))
                    .collect::<Vec<_>>();
                entries.sort_by_key(std::fs::DirEntry::file_name);
                for entry in entries {
                    copy_selected(&entry.path(), &destination.join(entry.file_name()));
                }
            } else {
                let parent = destination
                    .parent()
                    .expect("a selected file has a destination directory");
                fs::create_dir_all(parent).unwrap_or_else(|error| {
                    panic!("create selected parent {}: {error}", parent.display())
                });
                fs::copy(source, destination).unwrap_or_else(|error| {
                    panic!(
                        "copy selected file {} to {}: {error}",
                        source.display(),
                        destination.display()
                    )
                });
            }
        }

        let root = std::env::temp_dir().join(format!(
            "spice-xtask-stage-sources-selected-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale selected-tree fixture");
        }
        fs::create_dir_all(&root).expect("create the selected-tree fixture");
        git(&root, &["init", "-q"]);
        for input in STAGE_SOURCE_INPUTS {
            copy_selected(&source.join(input), &root.join(input));
        }
        copy_selected(
            &source.join(STAGE_SOURCE_LISTING),
            &root.join(STAGE_SOURCE_LISTING),
        );
        let mut add = vec!["add", "-f", "--"];
        add.extend(STAGE_SOURCE_INPUTS);
        git(&root, &add);
        root
    }

    /// One git command against the fixture, refusing rather than reporting.
    ///
    /// The caller passes `-f` when adding so the fixture carries what it
    /// declares whatever an operator's global excludes say, which is the one
    /// ambient input that could otherwise decide what these tests observe.
    fn git(root: &Path, arguments: &[&str]) {
        let status = Command::new(GIT)
            .args(arguments)
            .current_dir(root)
            .status()
            .unwrap_or_else(|error| panic!("{GIT} {arguments:?}: {error}"));
        assert!(status.success(), "{GIT} {arguments:?} answered {status}");
    }

    /// An untracked file beside the sources is not something the stage was
    /// built from, and a listing that recorded it would call every host that
    /// does not carry it stale. The stray is added after the listing is
    /// recorded, which is the order an operator meets this in: stage, then
    /// open an editor, then watch a clean tree go red.
    #[test]
    fn an_untracked_stray_beside_the_sources_leaves_the_stage_fresh() {
        let root = tracked_repository("stray");
        let listing = stage_source_listing(&root).expect("a tracked fixture has a listing");
        assert_eq!(
            listing.lines().count(),
            FIXTURE_FILES.len(),
            "the listing does not cover the tracked inputs, so agreeing with it proves \
             nothing: {listing}",
        );
        fs::create_dir_all(root.join("dist")).expect("create the fixture stage directory");
        fs::write(root.join(STAGE_SOURCE_LISTING), &listing).expect("record the fixture listing");

        fs::write(root.join("app/src/.DS_Store"), "stray").expect("write an ignored stray");
        fs::write(root.join("wire/src/scratch.rs"), "stray").expect("write an untracked source");

        if let Err(error) = compare_stage_sources(&root) {
            panic!("an untracked file beside the sources made the stage stale: {error}");
        }
        fs::remove_dir_all(root).expect("remove fixture");
    }

    /// Ignoring what git does not carry must not start ignoring what it does.
    /// A tracked input can change or leave the worktree, and both still have to
    /// reach the operator naming the path, because those are the two ways the
    /// stage actually goes stale.
    #[test]
    fn a_tracked_input_that_changed_or_left_is_still_named() {
        let root = tracked_repository("moved");
        let listing = stage_source_listing(&root).expect("a tracked fixture has a listing");
        fs::create_dir_all(root.join("dist")).expect("create the fixture stage directory");
        fs::write(root.join(STAGE_SOURCE_LISTING), &listing).expect("record the fixture listing");
        compare_stage_sources(&root).expect("a tree matching its own record is not stale");

        fs::write(root.join("wire/src/lib.rs"), "moved").expect("change a tracked build input");
        let error = compare_stage_sources(&root).expect_err("a changed input leaves a stale stage");
        assert!(
            matches!(&error, AppBuildError::StaleStage(paths) if *paths == ["wire/src/lib.rs"]),
            "a changed tracked input was not reported as the stale one: {error}",
        );

        fs::remove_file(root.join("app/style.css")).expect("remove a tracked build input");
        let error = compare_stage_sources(&root).expect_err("a removed input cannot be digested");
        assert!(
            error.to_string().contains("app/style.css"),
            "a tracked input that left the worktree went unnamed: {error}",
        );
        fs::remove_dir_all(root).expect("remove fixture");
    }

    /// Staging observes its namespace before compilation and records it after.
    /// A source move between those moments must not let the generated bytes be
    /// paired with a fresh listing that was never the cache namespace the
    /// build used.
    #[test]
    fn a_source_that_moves_during_staging_is_not_recorded_as_the_build_input() {
        let root = tracked_repository("moved-during-build");
        let started = capture_stage_sources(&root).expect("capture build-start sources");
        fs::create_dir_all(root.join("dist")).expect("create fixture dist directory");
        fs::write(root.join("app/src/lib.rs"), "moved while compiling")
            .expect("move a staged source");

        let error = record_stage_sources(&root, &started)
            .expect_err("a later source observation cannot describe an earlier build");

        assert!(
            matches!(
                &error,
                AppBuildError::SourcesChangedDuringBuild(paths)
                    if paths == &["app/src/lib.rs"]
            ),
            "a mid-build source move answered with something else: {error}",
        );
        assert!(
            !root.join(STAGE_SOURCE_LISTING).exists(),
            "the changed source inventory was recorded beside an earlier build"
        );
        fs::remove_dir_all(root).expect("remove fixture");
    }

    /// An input covers what sits beneath it and nothing that merely starts the
    /// same way. Today's list holds no such pair, and git's own pathspec would
    /// keep a sibling out of the answer if it did, so the question is asked of
    /// values here: leave the separator out and the day two inputs share a
    /// prefix, one of them is covered by the other's sources and stops being
    /// held to naming any of its own.
    #[test]
    fn an_input_covers_only_what_sits_beneath_it() {
        assert!(covers("wire/src", "wire/src/lib.rs"));
        assert!(covers("Cargo.lock", "Cargo.lock"));

        for (input, path) in [
            ("wire/src", "wire/src-old/lib.rs"),
            ("wire/src", "wire/srclib.rs"),
            ("wire/src/lib.rs", "wire/src"),
        ] {
            assert!(
                !covers(input, path),
                "{input} covered {path}, which is not a file beneath it",
            );
        }
    }

    /// A tracked symlink names bytes that live somewhere else, possibly outside
    /// the repository entirely, so digesting what it points at would record one
    /// file's content under another file's name. The stage is built from bytes
    /// and only a regular file says which ones.
    #[cfg(unix)]
    #[test]
    fn a_tracked_input_that_is_not_a_regular_file_refuses() {
        let root = tracked_repository("symlink");
        std::os::unix::fs::symlink("lib.rs", root.join("app/src/alias.rs"))
            .expect("write a symlinked build input");
        git(&root, &["add", "-f", "--", "app/src/alias.rs"]);

        let error = stage_source_listing(&root).expect_err("a symlink is not a build input");

        assert!(
            matches!(&error, AppBuildError::SourceInput(path) if path.ends_with("alias.rs")),
            "a tracked symlink was digested as a build input instead of refused: {error}",
        );
        fs::remove_dir_all(root).expect("remove fixture");
    }

    /// A tree git cannot answer for is refused by name rather than read as a
    /// repository holding nothing. Both answers stop the run, but only this one
    /// says the index was never reached; the other reports the first declared
    /// input as untracked and sends an operator looking for a source that never
    /// moved. The fixture is a directory under the temporary root, which is
    /// inside no repository on any machine this runs on.
    #[test]
    fn a_tree_that_is_no_repository_refuses_by_name() {
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-stage-sources-unrepository-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale fixture");
        }
        fs::create_dir_all(&root).expect("create the fixture directory");

        let error = stage_source_listing(&root).expect_err("no index answers for this tree");

        assert!(
            matches!(&error, AppBuildError::UnreadableSourceIndex(reason) if reason.contains(GIT)),
            "a tree with no index answered with something other than an unread one: {error}",
        );
        // This is the refusal a lane actually reaches: every lane is a linked
        // worktree, so mounting one and running the documented command lands
        // here rather than on the stale-stage refusal that names the procedure.
        // Git's own words explain nothing about mounts, so the way out is said
        // here too, or the reader is stranded at the one door they came through.
        assert!(
            error.to_string().contains(STAGE_INSTRUCTIONS),
            "the refusal a mounted worktree reaches never named {STAGE_INSTRUCTIONS}: {error}",
        );
        fs::remove_dir_all(root).expect("remove fixture");
    }

    /// A declared input the repository stopped tracking is the one way asking
    /// git can lose sources a walk would have found. The listing would simply
    /// come back shorter and agree with itself on every host, so the input is
    /// held to naming something rather than left to shrink quietly.
    #[test]
    fn a_declared_input_naming_no_tracked_file_refuses() {
        let root = tracked_repository("untracked-input");
        git(&root, &["rm", "-q", "--cached", "wire/src/lib.rs"]);

        let error = stage_source_listing(&root).expect_err("an untracked input names no sources");

        assert!(
            matches!(&error, AppBuildError::UntrackedSourceInput(input) if input == "wire/src"),
            "an input covering no tracked file answered with something else: {error}",
        );
        fs::remove_dir_all(root).expect("remove fixture");
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
        let source = repository_root();
        let repository = Command::new(GIT)
            .args(["rev-parse", "--is-inside-work-tree"])
            .current_dir(&source)
            .output()
            .is_ok_and(|output| output.status.success());
        let selected = (!repository).then(|| repository_from_selected_tree(&source));
        let root = selected.as_deref().unwrap_or(&source);

        if let Err(error) = compare_stage_sources(root) {
            panic!("{error}");
        }
        if let Some(root) = selected {
            fs::remove_dir_all(root).expect("remove selected-tree fixture");
        }
    }

    /// The refusal is all a host that cannot reproduce the bytes ever sees, so
    /// it carries the whole answer: what moved, where the claim it broke is
    /// recorded, the one command and environment that can renew it, and where
    /// the rest of the procedure is written down.
    ///
    /// That last one is load-bearing rather than a courtesy. The command and the
    /// image alone read as sufficient and are not: every lane is a linked
    /// worktree, and the staging run refuses one, so a reader who acts on the
    /// first three fields is turned away with a message about `git` that names
    /// nothing in this one. The inventory is written out here rather than
    /// derived from the refusal, so deleting a field from the message fails
    /// here instead of quietly shrinking what a stranded host is told.
    #[test]
    fn a_stale_stage_names_what_moved_and_how_to_renew_it() {
        let refusal = AppBuildError::StaleStage(vec!["wire/src/ident.rs".to_owned()]).to_string();

        for expected in [
            "wire/src/ident.rs",
            STAGE_SOURCE_LISTING,
            "cargo xtask stage-app",
            proof::DEFAULT_IMAGE,
            STAGE_INSTRUCTIONS,
        ] {
            assert!(
                refusal.contains(expected),
                "the refusal never mentioned {expected}: {refusal}",
            );
        }
    }
}
