use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const LIVE_SHELL_GLUE: [&str; 4] = [
    "dist/proof-image/cargo-wrapper.sh",
    "dist/proof-runner/build.sh",
    "steer/hooks/entry.sh",
    "steer/hooks/static/stage.sh",
];
const PORTED_SHELL: [&str; 4] = [
    "dist/proof-image/build.sh",
    "dist/proof-image/load.sh",
    "scripts/install-hooks.sh",
    "scripts/verify.sh",
];

#[test]
fn every_tracked_shell_file_has_one_checked_disposition() {
    let root = workspace_root();
    let output = Command::new("git")
        .args(["ls-files", "-z", "--", "*.sh"])
        .current_dir(&root)
        .output()
        .expect("Git lists tracked shell files");
    assert!(
        output.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tracked = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8(path.to_vec()).expect("tracked shell path is UTF-8"))
        .filter(|path| root.join(path).is_file())
        .collect::<BTreeSet<_>>();
    let expected = LIVE_SHELL_GLUE
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        tracked, expected,
        "the shell inventory must change with the Git index"
    );

    let inventory = fs::read_to_string(root.join("SHELL.md")).expect("read shell inventory");
    for path in LIVE_SHELL_GLUE {
        let entry = inventory_entry(&inventory, path);
        assert!(
            entry.contains("Disposition: Irreducible glue."),
            "live entry: {entry}"
        );
        assert!(
            entry.contains("\nCaller:"),
            "live entry has no caller: {entry}"
        );
        assert!(
            entry.contains("\nReason:"),
            "live entry has no reason: {entry}"
        );
    }
    for path in PORTED_SHELL {
        let entry = inventory_entry(&inventory, path);
        assert!(
            entry.contains("Disposition: Ported to Rust and deleted."),
            "ported entry: {entry}"
        );
        assert!(
            !root.join(path).exists(),
            "ported shell path still exists: {path}"
        );
    }
}

fn inventory_entry<'a>(inventory: &'a str, path: &str) -> &'a str {
    let heading = format!("## `{path}`\n");
    let start = inventory
        .find(&heading)
        .unwrap_or_else(|| panic!("shell inventory omits {path}"));
    let entry = &inventory[start..];
    entry.find("\n## `").map_or(entry, |end| &entry[..end])
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a workspace parent")
        .to_path_buf()
}
