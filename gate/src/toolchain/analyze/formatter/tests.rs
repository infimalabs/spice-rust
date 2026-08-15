use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::formatter_inputs;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[test]
fn focused_inputs_name_only_reached_rust_while_exhaustive_inputs_keep_the_tree() {
    let fixture = Fixture::new();
    for path in ["reached.rs", "unrelated.rs", "notes.md", "target/built.rs"] {
        let destination = fixture.root.join(path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).expect("create formatter fixture directory");
        }
        fs::write(destination, "fn fixture() {}\n").expect("write formatter fixture source");
    }

    let reached = [
        "reached.rs".to_owned(),
        "notes.md".to_owned(),
        "missing.rs".to_owned(),
        "target/built.rs".to_owned(),
    ];
    assert_eq!(
        formatter_inputs(&fixture.root, Some(&reached)).expect("select focused formatter inputs"),
        ["reached.rs"]
    );
    assert_eq!(
        formatter_inputs(&fixture.root, None).expect("select exhaustive formatter inputs"),
        ["reached.rs", "unrelated.rs"]
    );
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-formatter-reach-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("create formatter fixture root");
        Self { root }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove formatter fixture root");
    }
}
