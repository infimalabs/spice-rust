//! Process refusals at the exact native Rust toolchain authority boundary.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::process::Output;

use super::cargo_isolation_fixture::Fixture;
use super::cargo_isolation_support::write_executable;

#[test]
fn partial_native_toolchain_is_refused_before_cargo_starts() {
    let fixture = Fixture::new();
    let output = fixture
        .command(&fixture.first, "validate")
        .env_remove(spice::cargo_isolation::NATIVE_RUSTDOC_VARIABLE)
        .output()
        .expect("run partial native toolchain fixture");
    assert_refused_before_cargo(
        &fixture,
        &output,
        "cargo-isolation::native-toolchain-incomplete",
    );
}

#[test]
fn non_executable_native_toolchain_is_refused_before_cargo_starts() {
    let fixture = Fixture::new();
    fs::set_permissions(&fixture.rustc, fs::Permissions::from_mode(0o600))
        .expect("make native rustc non-executable");
    let output = fixture
        .command(&fixture.first, "validate")
        .output()
        .expect("run malformed native toolchain fixture");
    assert_refused_before_cargo(
        &fixture,
        &output,
        "cargo-isolation::native-toolchain-invalid",
    );
}

#[test]
fn mixed_generation_native_toolchain_is_refused_before_cargo_starts() {
    let fixture = Fixture::new();
    let other = fixture.root.join("other-toolchain/bin");
    fs::create_dir_all(&other).expect("create second toolchain directory");
    let rustdoc = other.join("rustdoc");
    write_executable(&rustdoc, "#!/bin/sh\nprintf 'other-rustdoc\\n'\n");
    let output = fixture
        .command(&fixture.first, "validate")
        .env(spice::cargo_isolation::NATIVE_RUSTDOC_VARIABLE, rustdoc)
        .output()
        .expect("run mixed native toolchain fixture");
    assert_refused_before_cargo(&fixture, &output, "cargo-isolation::native-toolchain-mixed");
}

#[test]
fn path_only_compiler_authority_is_refused_before_cargo_starts() {
    let fixture = Fixture::new();
    let output = fixture
        .command(&fixture.first, "validate")
        .env_remove(spice::cargo_isolation::NATIVE_CARGO_VARIABLE)
        .env_remove(spice::cargo_isolation::NATIVE_RUSTC_VARIABLE)
        .env_remove(spice::cargo_isolation::NATIVE_RUSTDOC_VARIABLE)
        .output()
        .expect("run PATH-only compiler authority fixture");
    assert_refused_before_cargo(
        &fixture,
        &output,
        "cargo-isolation::tracked-toolchain-unavailable",
    );
    assert!(
        !fixture.hostile_marker.exists(),
        "tracked resolver must not execute a competing PATH tool"
    );
}

#[test]
fn malformed_tracked_toolchain_is_refused_before_cargo_starts() {
    let fixture = Fixture::new();
    fs::write(
        fixture.first.join("rust-toolchain.toml"),
        "[toolchain\nchannel = 1\n",
    )
    .expect("write malformed tracked toolchain");
    let output = fixture
        .command(&fixture.first, "validate")
        .env_remove(spice::cargo_isolation::NATIVE_CARGO_VARIABLE)
        .env_remove(spice::cargo_isolation::NATIVE_RUSTC_VARIABLE)
        .env_remove(spice::cargo_isolation::NATIVE_RUSTDOC_VARIABLE)
        .output()
        .expect("run malformed tracked toolchain fixture");
    assert_refused_before_cargo(
        &fixture,
        &output,
        "cargo-isolation::tracked-toolchain-invalid",
    );
}

fn assert_refused_before_cargo(fixture: &Fixture, output: &Output, reason: &str) {
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(reason),
        "expected {reason} diagnostic: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        !fixture.counter.exists(),
        "Cargo vendor cannot start under refused toolchain authority"
    );
}
