//! Current task and steering identity mint inventory.

use std::fs;
use std::path::{Path, PathBuf};

const CLOCK_SITES: [(&str, usize); 7] = [
    ("spice/src/board_command/client/submission.rs", 3),
    ("spice/src/board_command/client_document.rs", 1),
    ("spice/src/board_command/followup.rs", 1),
    ("spice/src/board_command/mutation/gate_findings.rs", 1),
    ("spice/src/board_command/progress.rs", 1),
    ("spice/src/resident/steering.rs", 1),
    ("spice/src/resident/taste_surface.rs", 1),
];

#[test]
fn production_identity_mints_are_microsecond_only_and_fully_inventoried() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a workspace parent");
    let mut rust = Vec::new();
    collect_rust(root, &mut rust);
    let retired_task = ["TaskStamp::for_", "millis"].concat();
    let retired_key = ["Key::for_", "millis"].concat();
    let clock = ["jiff::Timestamp::now().as_", "microsecond()"].concat();
    for path in &rust {
        let text = fs::read_to_string(path).expect("Rust source is UTF-8");
        assert!(
            !text.contains(&retired_task),
            "{} retains {retired_task}",
            path.display()
        );
        assert!(
            !text.contains(&retired_key),
            "{} retains {retired_key}",
            path.display()
        );
    }

    let observed = rust
        .iter()
        .filter_map(|path| {
            let text = fs::read_to_string(path).expect("Rust source is UTF-8");
            let count = text.matches(&clock).count();
            (count > 0).then(|| {
                (
                    path.strip_prefix(root)
                        .expect("source stays below workspace")
                        .to_string_lossy()
                        .into_owned(),
                    count,
                )
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        CLOCK_SITES.map(|(path, count)| (path.to_owned(), count))
    );
}

fn collect_rust(root: &Path, into: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(root)
        .expect("workspace directory reads")
        .map(|entry| entry.expect("workspace entry reads").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if !matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("target" | ".git")
            ) {
                collect_rust(&path, into);
            }
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            into.push(path);
        }
    }
}
