//! Shared observations and executable construction for Cargo process fixtures.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Output;

use sha2::{Digest as _, Sha256};

pub(super) fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "isolated Cargo failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

pub(super) fn spoken_home(output: &Output) -> PathBuf {
    PathBuf::from(
        String::from_utf8(output.stdout.clone())
            .expect("fake Cargo emits UTF-8")
            .trim(),
    )
}

pub(super) fn configured_source(home: &Path) -> PathBuf {
    let document: toml::Value = toml::from_str(
        &fs::read_to_string(home.join("config.toml")).expect("read private Cargo config"),
    )
    .expect("parse private Cargo config");
    PathBuf::from(
        document["source"]["vendored-sources"]["directory"]
            .as_str()
            .expect("generated source directory"),
    )
}

pub(super) fn normal_directories(path: &Path) -> usize {
    fs::read_dir(path)
        .expect("read owned directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .count()
}

pub(super) fn tree_bytes(path: &Path) -> u64 {
    fs::read_dir(path)
        .expect("read private home")
        .filter_map(Result::ok)
        .map(|entry| {
            let metadata = fs::symlink_metadata(entry.path()).expect("measure private home entry");
            if metadata.is_dir() {
                tree_bytes(&entry.path())
            } else {
                metadata.len()
            }
        })
        .sum()
}

pub(super) fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn shell_word(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

pub(super) fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write executable fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("make fixture executable");
}

pub(super) fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .expect("read fixture mode")
        .permissions()
        .mode()
        & 0o777
}
