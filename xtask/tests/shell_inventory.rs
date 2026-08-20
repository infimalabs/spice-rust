use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const LIVE_SHELL_GLUE: [&str; 5] = [
    "dist/proof-image/cargo-wrapper.sh",
    "dist/proof-runner/build.sh",
    "steer/hooks/entry.sh",
    "steer/hooks/static/stage.sh",
    "steer/hooks/wrappers/spice",
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
    let tracked = shell_files(&root);
    let expected = LIVE_SHELL_GLUE
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        tracked, expected,
        "the shell inventory must change with the selected source tree"
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

fn shell_files(root: &Path) -> BTreeSet<String> {
    if root.join(".git").exists() {
        git_shell_files(root)
    } else {
        let mut tracked = BTreeSet::new();
        collect_shell_files(root, root, &mut tracked);
        tracked
    }
}

fn git_shell_files(root: &Path) -> BTreeSet<String> {
    let output = Command::new("git")
        .args(["ls-files", "-z", "--", "*.sh", "steer/hooks/wrappers/*"])
        .current_dir(root)
        .output()
        .expect("Git lists tracked shell files");
    assert!(
        output.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8(path.to_vec()).expect("tracked shell path is UTF-8"))
        .filter(|path| root.join(path).is_file())
        .collect()
}

fn collect_shell_files(root: &Path, directory: &Path, tracked: &mut BTreeSet<String>) {
    for entry in fs::read_dir(directory).expect("selected source directory reads") {
        let entry = entry.expect("selected source entry reads");
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name != OsStr::new(".git")
                && name != OsStr::new("node_modules")
                && name != OsStr::new("target")
            {
                collect_shell_files(root, &path, tracked);
            }
        } else if path.extension() == Some(OsStr::new("sh"))
            || path
                .strip_prefix(root)
                .is_ok_and(|relative| relative.starts_with("steer/hooks/wrappers"))
        {
            let relative = path
                .strip_prefix(root)
                .expect("walked path stays below root")
                .to_str()
                .expect("shell path is UTF-8")
                .to_owned();
            tracked.insert(relative);
        }
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
