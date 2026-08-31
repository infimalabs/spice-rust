use std::fs;
use std::path::{Path, PathBuf};

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
    git_dir
}

/// Add one synthetic linked worktree sharing `main`'s common Git directory.
#[allow(
    dead_code,
    reason = "shared by only the linked-worktree integration targets"
)]
pub(crate) fn link(main: &Path, linked: &Path, name: &str) -> PathBuf {
    let common = initialize(main);
    fs::create_dir_all(linked).expect("create linked fixture worktree");
    let git_dir = common.join("worktrees").join(name);
    fs::create_dir_all(&git_dir).expect("create linked fixture Git directory");
    fs::write(
        linked.join(".git"),
        format!("gitdir: {}\n", git_dir.display()),
    )
    .expect("write linked fixture Git pointer");
    fs::write(git_dir.join("commondir"), "../..\n")
        .expect("write linked fixture common-directory pointer");
    fs::write(
        git_dir.join("gitdir"),
        format!("{}\n", linked.join(".git").display()),
    )
    .expect("write linked fixture reverse pointer");
    fs::write(git_dir.join("HEAD"), format!("ref: refs/heads/{name}\n"))
        .expect("write linked fixture head");
    git_dir
}
