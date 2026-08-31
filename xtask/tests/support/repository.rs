use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use gate::test_support::gix;
use gix::object::tree::EntryKind;

/// In-process Git state for integration fixtures that exercise selected trees.
pub struct RepositoryFixture {
    root: PathBuf,
}

impl RepositoryFixture {
    pub fn initialize(root: &Path) -> Self {
        let repository = gix::ThreadSafeRepository::init_opts(
            root,
            gix::create::Kind::WithWorktree,
            gix::create::Options::default(),
            gix::open::Options::isolated(),
        )
        .expect("initialize in-process fixture repository")
        .to_thread_local();
        fs::write(repository.git_dir().join("HEAD"), "ref: refs/heads/main\n")
            .expect("select deterministic fixture branch");
        Self {
            root: root.to_path_buf(),
        }
    }

    pub fn open(root: &Path) -> Self {
        gix::open_opts(root, gix::open::Options::isolated())
            .expect("open in-process fixture repository");
        Self {
            root: root.to_path_buf(),
        }
    }

    pub fn commit_all(&self) {
        let repository = self.repository();
        let tree = snapshot_tree(&repository, &self.root);
        let identity = gix::actor::SignatureRef {
            name: "Xtask Fixture".into(),
            email: "xtask@example.invalid".into(),
            time: "0 +0000",
        };
        repository
            .commit_as(
                identity,
                identity,
                "refs/heads/main",
                "fixture",
                tree,
                gix::commit::NO_PARENT_IDS,
            )
            .expect("commit in-process fixture tree");
        write_index(&repository, tree);
    }

    pub fn stage_all(&self) {
        let repository = self.repository();
        let tree = snapshot_tree(&repository, &self.root);
        write_index(&repository, tree);
    }

    pub fn set_hooks_path(&self, hooks: &Path) {
        let repository = self.repository();
        let mut config = fs::OpenOptions::new()
            .append(true)
            .open(repository.git_dir().join("config"))
            .expect("open fixture repository config");
        writeln!(config, "[core]\n\thooksPath = {}", hooks.display())
            .expect("write fixture hooks path");
    }

    fn repository(&self) -> gix::Repository {
        gix::open_opts(&self.root, gix::open::Options::isolated())
            .expect("open in-process fixture repository")
    }
}

fn snapshot_tree(repository: &gix::Repository, root: &Path) -> gix::ObjectId {
    write_tree(repository, root, root)
}

fn write_tree(repository: &gix::Repository, root: &Path, directory: &Path) -> gix::ObjectId {
    let mut entries = fs::read_dir(directory)
        .expect("read fixture directory")
        .collect::<Result<Vec<_>, _>>()
        .expect("read fixture entries");
    entries.sort_by_key(fs::DirEntry::file_name);
    let mut tree_entries = entries
        .into_iter()
        .filter(|entry| directory != root || entry.file_name() != ".git")
        .map(|entry| {
            let path = entry.path();
            let (kind, oid) = if entry.file_type().expect("inspect fixture entry").is_dir() {
                (EntryKind::Tree, write_tree(repository, root, &path))
            } else {
                let blob = repository
                    .write_blob(fs::read(&path).expect("read fixture file"))
                    .expect("write fixture blob");
                (EntryKind::Blob, blob.detach())
            };
            gix::objs::tree::Entry {
                mode: kind.into(),
                filename: entry
                    .file_name()
                    .to_str()
                    .expect("fixture paths are UTF-8")
                    .into(),
                oid,
            }
        })
        .collect::<Vec<_>>();
    tree_entries.sort();
    repository
        .write_object(gix::objs::Tree {
            entries: tree_entries,
        })
        .expect("write fixture tree")
        .detach()
}

fn write_index(repository: &gix::Repository, tree: gix::ObjectId) {
    let mut index = repository
        .index_from_tree(&tree)
        .expect("derive fixture index from tree");
    index.set_path(repository.index_path());
    index
        .write(Default::default())
        .expect("write fixture index");
}
