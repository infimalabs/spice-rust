use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) struct ReportFixture {
    pub(super) root: PathBuf,
}

impl ReportFixture {
    pub(super) fn new() -> Self {
        let root = unique_root("spice-gate-report");
        fs::create_dir(&root).expect("create report fixture root");
        let fixture = Self { root };
        fs::create_dir(fixture.root.join("src")).expect("create report fixture source directory");
        fs::write(
            fixture.root.join("spice-rust.toml"),
            format!("{}[gate.walk]\nskip = []\n", crate::NARROWEST_FILE_CLASSES),
        )
        .expect("write report fixture policy");
        fs::write(
            fixture.root.join("baseline.toml"),
            concat!(
                "version = 1\n\n",
                "[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n",
            ),
        )
        .expect("write report fixture baseline");
        fs::write(fixture.root.join("src/lib.rs"), "pub fn fixture() {}\n")
            .expect("write report fixture source");
        crate::test_repository::initialize(&fixture.root);
        crate::test_repository::stage_all(&fixture.root);
        crate::test_repository::commit(&fixture.root, "report fixture");
        fixture
    }
}

impl Drop for ReportFixture {
    fn drop(&mut self) {
        remove_tree(&self.root);
    }
}

fn unique_root(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn remove_tree(root: &Path) {
    if let Err(error) = fs::remove_dir_all(root) {
        eprintln!(
            "failed to remove report fixture tree {}: {error}",
            root.display()
        );
    }
}
