use std::fs;
use std::path::{Path, PathBuf};

use gix::bstr::BStr;
use gix::index::entry::{Flags, Mode, Stage, Stat};
use gix::object::tree::EntryKind;

const MAIN_BRANCH: &str = "refs/heads/main";
const FIXTURE_STAMP: &str = "1700000000 +0000";

/// Initialize the on-disk repository shape without launching Git.
pub(crate) fn initialize(worktree: &Path) -> PathBuf {
    let existing = worktree.join(".git");
    let git_dir = if existing.is_dir() {
        existing
    } else {
        gix::init(worktree)
            .expect("initialize fixture repository in process")
            .path()
            .to_path_buf()
    };
    fs::create_dir_all(git_dir.join("info")).expect("create fixture repository info directory");
    let exclude = git_dir.join("info/exclude");
    if !exclude.exists() {
        fs::write(exclude, []).expect("write fixture repository exclusions");
    }
    seed_app_stage(worktree);
    git_dir
}

pub(crate) fn seed_app_stage(worktree: &Path) {
    seed_app_stage_as(worktree, b"install fixture app");
}

#[allow(dead_code, reason = "shared by release repository fixtures")]
pub(crate) fn stage(worktree: &Path, paths: &[&str]) {
    let repository = open(worktree);
    let mut index = repository
        .index_or_load_from_head_or_empty()
        .expect("load fixture index")
        .into_owned();
    for path in paths {
        index.remove_entries(|_, candidate, _| candidate == path.as_bytes());
        let absolute = worktree.join(path);
        let metadata = fs::metadata(&absolute).expect("inspect fixture path");
        let mode = if executable(&metadata) {
            Mode::FILE_EXECUTABLE
        } else {
            Mode::FILE
        };
        let id = repository
            .write_blob(fs::read(&absolute).expect("read fixture path"))
            .expect("write fixture blob")
            .detach();
        index.dangerously_push_entry(
            Stat::default(),
            id,
            Flags::from_stage(Stage::Unconflicted),
            mode,
            BStr::new(path.as_bytes()),
        );
    }
    index.sort_entries();
    index.remove_tree();
    index
        .write(gix::index::write::Options::default())
        .expect("write fixture index");
}

#[allow(dead_code, reason = "shared by release repository fixtures")]
pub(crate) fn commit(worktree: &Path, message: &str) -> String {
    fs::write(worktree.join(".git/HEAD"), format!("ref: {MAIN_BRANCH}\n"))
        .expect("select fixture main branch");
    let repository = open(worktree);
    let tree = write_index_tree(&repository);
    let parent = repository.head_id().ok().map(gix::Id::detach);
    let signature = gix::actor::SignatureRef {
        name: "Spice Fixture".into(),
        email: "spice@example.invalid".into(),
        time: FIXTURE_STAMP,
    };
    repository
        .commit_as(signature, signature, MAIN_BRANCH, message, tree, parent)
        .expect("commit fixture tree")
        .detach()
        .to_string()
}

#[allow(dead_code, reason = "shared by release repository fixtures")]
pub(crate) fn write_tree(worktree: &Path) -> String {
    let repository = open(worktree);
    write_index_tree(&repository).to_string()
}

#[allow(dead_code, reason = "shared only by install refresh coverage")]
pub(crate) fn seed_app_stage_as(worktree: &Path, identity: &[u8]) {
    let generation = wire::Digest::compute(wire::Domain::DistInventory, identity);
    let stage = worktree.join(serve::APP_STAGE);
    let root = stage
        .join(serve::APP_STAGE_GENERATIONS)
        .join(generation.as_str());
    fs::create_dir_all(&root).expect("create fixture app generation");
    for file in wire::manifest::APP_FILES {
        fs::write(
            root.join(file),
            [identity, b" ", file.as_bytes(), b"\n"].concat(),
        )
        .expect("write fixture app asset");
    }
    fs::write(
        stage.join(serve::APP_STAGE_CURRENT),
        format!("{generation}\n"),
    )
    .expect("write fixture app pointer");
}

fn open(worktree: &Path) -> gix::Repository {
    gix::open_opts(worktree, gix::open::Options::isolated()).expect("open fixture repository")
}

fn write_index_tree(repository: &gix::Repository) -> gix::ObjectId {
    let index = repository.index().expect("open fixture index");
    let mut editor = repository.empty_tree().edit().expect("edit fixture tree");
    for entry in index.entries() {
        let kind = match entry.mode {
            Mode::FILE => EntryKind::Blob,
            Mode::FILE_EXECUTABLE => EntryKind::BlobExecutable,
            unsupported => panic!("unsupported fixture index mode {unsupported:?}"),
        };
        editor
            .upsert(entry.path(&index), kind, entry.id)
            .expect("insert fixture tree entry");
    }
    editor.write().expect("write fixture tree").detach()
}

#[cfg(unix)]
fn executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_metadata: &fs::Metadata) -> bool {
    false
}
