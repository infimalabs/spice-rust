use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// Names tried before a scratch root is declared unobtainable.
const CREATE_ATTEMPTS: usize = 128;

static NEXT_TREE: AtomicU32 = AtomicU32::new(0);

/// A source tree the fixture creates, owns, and can prove it removed.
pub(crate) struct Tree {
    pub(crate) root: PathBuf,
    removed: bool,
}

impl Tree {
    pub(crate) fn new(label: &str) -> Self {
        let directory = std::env::temp_dir();
        let process = std::process::id();
        let root = create_scratch_root(std::iter::repeat_with(|| {
            let unique = NEXT_TREE.fetch_add(1, Ordering::Relaxed);
            directory.join(format!("gate-{label}-{process}-{unique}"))
        }));
        Self {
            root,
            removed: false,
        }
    }

    pub(crate) fn write(&self, relative: &str, contents: &str) {
        let path = self.root.join(relative);
        let parent = path.parent().expect("a fixture file sits in a directory");
        fs::create_dir_all(parent).expect("create a fixture directory");
        fs::write(&path, contents).expect("write a fixture file");
    }

    pub(crate) fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// Remove the owned tree and prove it is gone.
    ///
    /// Each integration binary compiles this module alone; enforcement relies
    /// on `Drop`, while the ownership tests call this observable path.
    #[allow(dead_code)]
    pub(crate) fn remove(mut self) {
        fs::remove_dir_all(&self.root)
            .unwrap_or_else(|error| panic!("remove fixture at {}: {error}", self.root.display()));
        assert!(
            !self.root.exists(),
            "fixture outlived removal at {}",
            self.root.display()
        );
        self.removed = true;
    }
}

impl Drop for Tree {
    /// Best effort during unwinding; explicit teardown remains observable.
    fn drop(&mut self) {
        if self.removed {
            return;
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The first candidate this call creates, never one it merely finds.
pub(crate) fn create_scratch_root(candidates: impl Iterator<Item = PathBuf>) -> PathBuf {
    for root in candidates.take(CREATE_ATTEMPTS) {
        match fs::create_dir(&root) {
            Ok(()) => return root,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => panic!("create fixture root at {}: {error}", root.display()),
        }
    }
    panic!("no fixture root was free within {CREATE_ATTEMPTS} attempts");
}
