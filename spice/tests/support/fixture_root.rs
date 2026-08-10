use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

/// Atomically claim one process-local fixture root without reopening residue.
pub(crate) fn claim(base: &Path, stem: &str) -> PathBuf {
    loop {
        let root = base.join(format!(
            "spice-{stem}-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&root) {
            Ok(()) => return root,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => panic!("fixture root {} cannot be claimed: {error}", root.display()),
        }
    }
}
