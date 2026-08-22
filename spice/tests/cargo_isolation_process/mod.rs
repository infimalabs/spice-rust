//! Successful and status-preserving manual Cargo process boundaries.

use std::fs;
use std::os::unix::fs::MetadataExt as _;

use super::cargo_isolation_fixture::Fixture;
use super::cargo_isolation_support::{
    assert_success, configured_source, mode, normal_directories, spoken_home, tree_bytes,
};

#[test]
fn two_linked_worktrees_converge_cold_then_validate_in_private_homes() {
    let fixture = Fixture::new();
    let first = fixture
        .command(&fixture.first, "validate")
        .spawn()
        .expect("launch first isolated Cargo");
    let second = fixture
        .command(&fixture.second, "validate")
        .spawn()
        .expect("launch second isolated Cargo");
    let first = first.wait_with_output().expect("wait for first Cargo");
    let second = second.wait_with_output().expect("wait for second Cargo");

    assert_success(&first);
    assert_success(&second);
    let first_home = spoken_home(&first);
    let second_home = spoken_home(&second);
    assert_ne!(first_home, second_home);
    assert_eq!(mode(&first_home), 0o700);
    assert_eq!(mode(&second_home), 0o700);
    assert_ne!(
        fs::metadata(first_home.join(".global-cache"))
            .expect("first private database")
            .ino(),
        fs::metadata(second_home.join(".global-cache"))
            .expect("second private database")
            .ino(),
    );
    assert!(tree_bytes(&first_home) <= 1024 * 1024);
    assert!(tree_bytes(&second_home) <= 1024 * 1024);
    assert_eq!(
        fs::read_to_string(&fixture.counter)
            .expect("cold seed counter")
            .lines()
            .count(),
        1,
    );
    for output in [&first, &second] {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("Blocking waiting for file lock"));
        assert!(!stderr.contains("package cache lock"));
    }
    assert!(
        !fixture.hostile_marker.exists(),
        "isolated Cargo must not execute a competing PATH tool"
    );

    let first_source = configured_source(&first_home);
    let second_source = configured_source(&second_home);
    assert_eq!(first_source, second_source);
    let packages = fixture.bootstrap.join("spice/source-store-v2/packages");
    assert_eq!(normal_directories(&packages), 1);
    let linked = fs::read_dir(first_source)
        .expect("read generation view")
        .next()
        .expect("one generation package")
        .expect("read generation package entry")
        .path();
    assert!(
        fs::symlink_metadata(linked)
            .expect("linked package metadata")
            .file_type()
            .is_symlink()
    );
}

#[test]
fn native_cargo_exit_status_is_preserved() {
    let fixture = Fixture::new();
    let output = fixture
        .command(&fixture.first, "exit-23")
        .output()
        .expect("run status fixture");
    assert_eq!(output.status.code(), Some(23));
}
