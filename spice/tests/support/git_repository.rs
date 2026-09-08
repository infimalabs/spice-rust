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
    seed_canvas_stage(worktree);
    git_dir
}

pub(crate) fn seed_canvas_stage(worktree: &Path) {
    seed_canvas_stage_as(worktree, b"install fixture app");
}

#[allow(dead_code, reason = "shared only by install refresh coverage")]
pub(crate) fn seed_canvas_stage_as(worktree: &Path, identity: &[u8]) {
    let generation = wire::Digest::compute(wire::Domain::DistInventory, identity);
    let stage = worktree.join(canvas::APP_STAGE);
    let root = stage
        .join(canvas::APP_STAGE_GENERATIONS)
        .join(generation.as_str());
    fs::create_dir_all(&root).expect("create fixture Canvas generation");
    for file in wire::manifest::CANVAS_APP_FILES {
        fs::write(
            root.join(file),
            [identity, b" ", file.as_bytes(), b"\n"].concat(),
        )
        .expect("write fixture Canvas asset");
    }
    fs::write(
        stage.join(canvas::APP_STAGE_CURRENT),
        format!("{generation}\n"),
    )
    .expect("write fixture Canvas pointer");
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
