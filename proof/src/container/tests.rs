use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sill::{Deadlines, StdinPolicy, StreamPolicy};

use super::{Attestation, Container, ContainerError, Limits, OciRuntime, RuntimeKind};

const SILENT_RUNTIME: &str = r#"#!/bin/sh
if [ "$1" = image ]; then
    printf '%s amd64 linux\n' "$5"
    exit 0
fi
if [ "$1" = --version ]; then
    printf 'Docker version 1.0.0, build fixture\n'
    exit 0
fi
if [ "$1" = run ]; then
    printf '%064d\n' 0
    exit 0
fi
exit 0
"#;
const FIXED_MARKER_RUNTIME: &str = r#"#!/bin/sh
if [ "$1" = image ]; then
    printf '%s amd64 linux\n' "$5"
    exit 0
fi
if [ "$1" = --version ]; then
    printf 'Docker version 1.0.0, build fixture\n'
    exit 0
fi
if [ "$1" = run ]; then
    printf '%064d\n' 0
    exit 0
fi
if [ "$1" = exec ]; then
    printf 'spice-proof-attestation-v2:docker:fixed-challenge\n' >&2
fi
exit 0
"#;
const PODMAN_RUNTIME: &str = r#"#!/bin/sh
if [ "$1" = image ]; then
    printf '%s amd64 linux\n' "$5"
    exit 0
fi
if [ "$1" = --version ]; then
    printf 'podman version 1.0.0\n'
    exit 0
fi
exit 0
"#;
const LIFECYCLE_RUNTIME: &str = r#"#!/bin/sh
if [ "$1" = image ]; then
    printf '%s amd64 linux\n' "$5"
    exit 0
fi
if [ "$1" = --version ]; then
    printf 'Docker version 1.0.0, build fixture\n'
    exit 0
fi
if [ "$1" = run ]; then
    printf 'run %s %064d\n' "$4" 0 >> "$0.log"
    printf '%064d\n' 0
    exit 0
fi
if [ "$1" = exec ]; then
    printf 'exec %s\n' "$2" >> "$0.log"
    printf 'spice-proof-attestation-v2:%s:%s\n' "$7" "$8" >&2
    exit 0
fi
if [ "$1" = rm ]; then
    printf 'rm %s\n' "$3" >> "$0.log"
    exit 0
fi
exit 1
"#;
const REFUSING_PREFLIGHT_RUNTIME: &str = r#"#!/bin/sh
if [ "$1" = image ]; then
    printf '%s amd64 linux\n' "$5"
    exit 0
fi
if [ "$1" = --version ]; then
    printf 'Docker version 1.0.0, build fixture\n'
    exit 0
fi
if [ "$1" = run ]; then
    printf 'run %s %064d\n' "$4" 0 >> "$0.log"
    printf '%064d\n' 0
    exit 0
fi
if [ "$1" = exec ]; then
    printf 'exec %s\n' "$2" >> "$0.log"
    printf 'spice-proof-attestation-v2:%s:%s\n' "$7" "$8" >&2
    exit 70
fi
if [ "$1" = rm ]; then
    printf 'rm %s\n' "$3" >> "$0.log"
    exit 0
fi
exit 1
"#;
const REFUSING_START_RUNTIME: &str = r#"#!/bin/sh
if [ "$1" = image ]; then
    printf '%s amd64 linux\n' "$5"
    exit 0
fi
if [ "$1" = --version ]; then
    printf 'Docker version 1.0.0, build fixture\n'
    exit 0
fi
if [ "$1" = run ]; then
    printf 'run %s\n' "$4" >> "$0.log"
    exit 125
fi
if [ "$1" = rm ]; then
    printf 'rm %s\n' "$3" >> "$0.log"
    exit 0
fi
exit 1
"#;

/// A runtime that resolves the sealed digest to some other stored image.
const SUBSTITUTING_RUNTIME: &str = r#"#!/bin/sh
if [ "$1" = image ]; then
    printf 'sha256:0000000000000000000000000000000000000000000000000000000000000000 amd64 linux\n'
    exit 0
fi
if [ "$1" = --version ]; then
    printf 'Docker version 1.0.0, build fixture\n'
    exit 0
fi
exit 0
"#;

/// A runtime holding the sealed digest built for the wrong machine.
const FOREIGN_PLATFORM_RUNTIME: &str = r#"#!/bin/sh
if [ "$1" = image ]; then
    printf '%s arm64 linux\n' "$5"
    exit 0
fi
if [ "$1" = --version ]; then
    printf 'Docker version 1.0.0, build fixture\n'
    exit 0
fi
exit 0
"#;

/// A runtime whose store has never seen the sealed appliance.
const EMPTY_STORE_RUNTIME: &str = r#"#!/bin/sh
if [ "$1" = image ]; then
    printf 'Error: no such image\n' >&2
    exit 1
fi
if [ "$1" = --version ]; then
    printf 'Docker version 1.0.0, build fixture\n'
    exit 0
fi
exit 0
"#;

#[test]
fn one_battery_scope_disables_network_and_mounts_fresh_empty_caches() {
    let fixture = Fixture::new();
    let container = fixture.container();
    let spec = container
        .battery_spec("spice-proof-fixture")
        .expect("sealed battery scope");
    let arguments = spec
        .argv
        .arguments()
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    assert_eq!(
        arguments,
        [
            "run".to_owned(),
            "--detach".to_owned(),
            "--name".to_owned(),
            "spice-proof-fixture".to_owned(),
            // Named here rather than inferred, so a host whose own
            // architecture differs cannot have a runtime warning written
            // across the attestation stream.
            "--platform".to_owned(),
            "linux/amd64".to_owned(),
            "--network=none".to_owned(),
            "--read-only".to_owned(),
            "--cap-drop=ALL".to_owned(),
            "--security-opt=no-new-privileges".to_owned(),
            "--mount".to_owned(),
            format!(
                "type=bind,source={},destination=/workspace,readonly",
                fixture
                    .workspace
                    .canonicalize()
                    .expect("canonical workspace")
                    .display()
            ),
            "--tmpfs".to_owned(),
            "/cargo-home:rw,noexec,mode=0700".to_owned(),
            "--tmpfs".to_owned(),
            "/cargo-target:rw,exec,mode=0700,size=4g".to_owned(),
            "--workdir".to_owned(),
            "/workspace".to_owned(),
            "--env".to_owned(),
            "CARGO_HOME=/cargo-home".to_owned(),
            "--env".to_owned(),
            "CARGO_TARGET_DIR=/cargo-target".to_owned(),
            "--env".to_owned(),
            "CARGO_MUTANTS_OUTPUT=/cargo-target".to_owned(),
            "--env".to_owned(),
            "CARGO_NET_OFFLINE=true".to_owned(),
            "--env".to_owned(),
            "CARGO_PROFILE_TEST_DEBUG=0".to_owned(),
            "--env".to_owned(),
            "CARGO_INCREMENTAL=0".to_owned(),
            "--env".to_owned(),
            "RUST_TEST_THREADS=1".to_owned(),
            format!("registry.example/proof@sha256:{}", "4".repeat(64)),
            "/bin/sh".to_owned(),
            "-ceu".to_owned(),
            super::BATTERY_KEEPALIVE_SCRIPT.to_owned(),
        ]
    );
    assert!(spec.environment.entries().is_empty());
    assert_eq!(spec.output.stdin(), StdinPolicy::Null);
    assert!(matches!(
        spec.output.stdout(),
        StreamPolicy::Capture { limit: 4096 }
    ));
}

#[test]
fn every_member_enters_the_named_battery_scope() {
    let fixture = Fixture::new();
    let container = fixture.container();
    let attestation = Attestation::fixture("fixture-challenge");
    let spec = container
        .exec_spec(
            "spice-proof-fixture",
            super::MEMBER_SCRIPT,
            "cargo",
            &["test".to_owned(), "--locked".to_owned()],
            &attestation,
            "fixture member",
        )
        .expect("sealed member command");
    let arguments = spec
        .argv
        .arguments()
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    assert_eq!(
        arguments,
        [
            "exec",
            "spice-proof-fixture",
            "/bin/sh",
            "-ceu",
            super::MEMBER_SCRIPT,
            "spice-proof-attestation",
            "docker",
            "fixture-challenge",
            "cargo",
            "test",
            "--locked",
        ]
    );
}

#[test]
fn one_scratch_owner_is_reused_and_removed_for_the_whole_battery() {
    let fixture = Fixture::with_runtime(LIFECYCLE_RUNTIME);
    let container = fixture.container();
    let battery = container.battery().expect("battery scratch starts empty");

    battery
        .execute(
            "cargo",
            &["test".to_owned(), "-p".to_owned(), "gate".to_owned()],
        )
        .expect("first member enters the scratch owner");
    battery
        .execute(
            "cargo",
            &["test".to_owned(), "-p".to_owned(), "dist".to_owned()],
        )
        .expect("later member reuses the scratch owner");
    battery.close().expect("battery scratch is removed");

    let log =
        fs::read_to_string(fixture.runtime.with_extension("log")).expect("runtime lifecycle log");
    let events = log.lines().collect::<Vec<_>>();
    assert_eq!(
        events.len(),
        5,
        "one start, one preflight, two members, one removal"
    );
    let mut start = events[0].split_whitespace();
    assert_eq!(start.next(), Some("run"));
    let name = start.next().expect("the start names the scratch owner");
    let identifier = start.next().expect("the start returns the owner identity");
    assert_eq!(start.next(), None);
    assert!(name.starts_with("spice-proof-"));
    assert_eq!(identifier, "0".repeat(64));
    assert_eq!(
        events,
        [
            format!("run {name} {identifier}"),
            format!("exec {identifier}"),
            format!("exec {identifier}"),
            format!("exec {identifier}"),
            format!("rm {identifier}"),
        ],
        "preflight and every member must enter the one owner that teardown removes"
    );
}

#[test]
fn a_refused_preflight_removes_the_scratch_owner_before_returning() {
    let fixture = Fixture::with_runtime(REFUSING_PREFLIGHT_RUNTIME);
    let container = fixture.container();

    let error = container
        .battery()
        .expect_err("a refused preflight cannot expose its scratch");

    assert!(error.to_string().contains("preflight battery scratch"));
    let log =
        fs::read_to_string(fixture.runtime.with_extension("log")).expect("runtime lifecycle log");
    let events = log.lines().collect::<Vec<_>>();
    let mut start = events[0].split_whitespace();
    assert_eq!(start.next(), Some("run"));
    let name = start
        .next()
        .expect("scratch owner started before preflight");
    let identifier = start.next().expect("the start returns the owner identity");
    assert_eq!(start.next(), None);
    assert_eq!(
        events,
        [
            format!("run {name} {identifier}"),
            format!("exec {identifier}"),
            format!("rm {identifier}"),
        ],
        "the owner guard must remove a scratch whose preflight refuses"
    );
}

#[test]
fn a_refused_start_never_removes_a_container_it_did_not_create() {
    let fixture = Fixture::with_runtime(REFUSING_START_RUNTIME);
    let container = fixture.container();

    let error = container
        .battery()
        .expect_err("a refused start creates no owned scratch");

    assert!(error.to_string().contains("start battery scratch"));
    let log =
        fs::read_to_string(fixture.runtime.with_extension("log")).expect("runtime lifecycle log");
    let events = log.lines().collect::<Vec<_>>();
    assert_eq!(events.len(), 1, "a failed start has nothing safe to remove");
    assert!(events[0].starts_with("run spice-proof-"));
}

#[test]
fn an_unpinned_image_refuses_instead_of_falling_back_to_a_tag() {
    let fixture = Fixture::new();
    let error = Container::new(
        OciRuntime::attest(&fixture.runtime, limits()).expect("fixture runtime"),
        "registry.example/proof:latest",
        &fixture.workspace,
        limits(),
    )
    .expect_err("tagged images are not sealed");

    assert!(matches!(error, ContainerError::ImageNotDigestPinned(_)));
}

#[test]
fn a_runtime_that_cannot_produce_the_sealed_image_refuses_before_any_work() {
    let sealed = format!("registry.example/proof@sha256:{}", "4".repeat(64));
    for (script, expected) in [
        (SUBSTITUTING_RUNTIME, "resolved a different appliance"),
        (FOREIGN_PLATFORM_RUNTIME, "arm64"),
        (EMPTY_STORE_RUNTIME, "does not hold proof image"),
    ] {
        let fixture = Fixture::with_runtime(script);
        let error = Container::new(
            OciRuntime::attest(&fixture.runtime, limits()).expect("fixture runtime"),
            sealed.clone(),
            &fixture.workspace,
            limits(),
        )
        .expect_err("a runtime that cannot produce the sealed image is not sealed");

        assert!(
            error.to_string().contains(expected),
            "the refusal did not name {expected}: {error}",
        );
    }
}

/// A prune is the usual reason the image is gone, and it takes the image
/// from every lane at once. The refusal therefore has to carry the whole
/// recovery, not just the loader: a lane told only to run the load action
/// still has to guess which archive, and the guess it makes under time
/// pressure is the build action, which is the emulated build this refusal
/// exists to avoid.
#[test]
fn a_pruned_image_names_the_loader_and_the_archive_it_takes() {
    let sealed = format!("registry.example/proof@sha256:{}", "4".repeat(64));
    let fixture = Fixture::with_runtime(EMPTY_STORE_RUNTIME);
    let error = Container::new(
        OciRuntime::attest(&fixture.runtime, limits()).expect("fixture runtime"),
        sealed,
        &fixture.workspace,
        limits(),
    )
    .expect_err("an empty image store is not sealed");

    let refusal = error.to_string();
    for named in [
        "spice dev xtask -- proof-image load",
        "target/proof-image/spice-proof-image.tar",
        "spice dev xtask -- proof-image build",
        "prune",
    ] {
        assert!(
            refusal.contains(named),
            "the refusal did not name {named}: {refusal}",
        );
    }
}

#[test]
fn an_executable_that_ignores_the_oci_contract_cannot_produce_a_replay_verdict() {
    let fixture = Fixture::new();
    let environment = crate::environment::SealedEnvironment::fixture_oci(fixture.container());
    let error = crate::replay::replay(&environment)
        .expect_err("an unattested runtime cannot produce any verdict");

    assert_eq!(error.check(), crate::replay::CheckKind::Gate);
    assert!(
        error
            .evidence()
            .contains("did not prove its OCI isolation contract"),
        "the refusal did not name the missing attestation: {error:?}",
    );
}

#[test]
fn a_replayed_public_marker_cannot_produce_a_replay_verdict() {
    let fixture = Fixture::with_runtime(FIXED_MARKER_RUNTIME);
    let environment = crate::environment::SealedEnvironment::fixture_oci(fixture.container());
    let error = crate::replay::replay(&environment)
        .expect_err("a fixed marker cannot answer a fresh challenge");

    assert_eq!(error.check(), crate::replay::CheckKind::Gate);
    assert!(
        error
            .evidence()
            .contains("did not prove its OCI isolation contract"),
        "the refusal did not name the stale attestation: {error:?}",
    );
}

#[test]
fn runtime_dialect_is_observed_from_the_executable() {
    let docker = Fixture::new();
    let podman = Fixture::with_runtime(PODMAN_RUNTIME);

    assert_eq!(
        OciRuntime::attest(&docker.runtime, limits())
            .expect("observed Docker")
            .kind,
        RuntimeKind::Docker
    );
    assert_eq!(
        OciRuntime::attest(&podman.runtime, limits())
            .expect("observed Podman")
            .kind,
        RuntimeKind::Podman
    );
}

#[test]
fn attestation_is_required_once_and_removed_from_command_evidence() {
    let fixture = Fixture::new();
    let container = fixture.container();
    let attestation = Attestation::fixture("fresh-challenge");
    let mut stderr = super::Captured {
        bytes: b"spice-proof-attestation-v2:docker:fresh-challenge\ncompiler evidence".to_vec(),
        truncated: false,
        complete: true,
    };

    container
        .verify_attestation(&mut stderr, &attestation)
        .expect("the declared dialect attests");

    assert_eq!(stderr.bytes, b"compiler evidence");
}

#[test]
fn attestation_must_name_the_declared_runtime_dialect() {
    let fixture = Fixture::new();
    let container = fixture.container();
    let attestation = Attestation::fixture("fresh-challenge");
    let mut stderr = super::Captured {
        bytes: b"spice-proof-attestation-v2:podman:fresh-challenge\n".to_vec(),
        truncated: false,
        complete: true,
    };

    let error = container
        .verify_attestation(&mut stderr, &attestation)
        .expect_err("a Podman attestation cannot answer for Docker");

    assert!(matches!(error, ContainerError::RuntimeNotAttested { .. }));
}

#[test]
fn attestation_must_answer_the_current_execution_challenge() {
    let fixture = Fixture::new();
    let container = fixture.container();
    let attestation = Attestation::fixture("fresh-challenge");
    let mut stderr = super::Captured {
        bytes: b"spice-proof-attestation-v2:docker:stale-challenge\n".to_vec(),
        truncated: false,
        complete: true,
    };

    let error = container
        .verify_attestation(&mut stderr, &attestation)
        .expect_err("a marker from another execution cannot be replayed");

    assert!(matches!(error, ContainerError::RuntimeNotAttested { .. }));
}

#[test]
fn the_embedded_preflight_is_valid_posix_shell() {
    let _spawn_guard = crate::test_support::process_spawn_guard();
    let status = std::process::Command::new("/bin/sh")
        .args(["-n", "-c", super::PREFLIGHT_SCRIPT])
        .status()
        .expect("host POSIX shell runs the syntax check");

    assert!(
        status.success(),
        "the sealed preflight must parse before use"
    );
}

/// One fixture directory, removed again when the test that made it ends,
/// whichever way it ends.
struct FixtureDirectory(std::path::PathBuf);

impl std::ops::Deref for FixtureDirectory {
    type Target = std::path::Path;

    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A `/sys/class/net` directory holding the named interfaces and their
/// flags, alongside the `bonding_masters` control file the kernel puts
/// there, which is a file rather than an interface directory.
fn network_directory(interfaces: &[(&str, u32)]) -> FixtureDirectory {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = FixtureDirectory(std::env::temp_dir().join(format!(
        "spice-proof-net-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    fs::create_dir_all(&*root).expect("fixture network directory");
    fs::write(root.join("bonding_masters"), "\n").expect("fixture control file");
    for (name, flags) in interfaces {
        let interface = root.join(name);
        fs::create_dir_all(&interface).expect("fixture interface");
        fs::write(interface.join("flags"), format!("{flags:#x}\n")).expect("fixture flags");
    }
    root
}

/// Whether the shipped rule calls that directory isolated.
///
/// Captures the shell's streams rather than inheriting them, because a
/// directory the rule cannot classify makes it report the unreadable file.
fn network_isolated(directory: &std::path::Path) -> bool {
    let _spawn_guard = crate::test_support::process_spawn_guard();
    std::process::Command::new("/bin/sh")
        .args([
            std::ffi::OsStr::new("-c"),
            std::ffi::OsStr::new(concat!(network_rule!(), "\nnetwork_isolated \"$1\"\n")),
            std::ffi::OsStr::new("sh"),
            directory.as_os_str(),
        ])
        .output()
        .expect("host POSIX shell runs the network rule")
        .status
        .success()
}

/// The mountinfo line the boundary produces for one destination.
fn mountinfo_line(destination: &str, options: &str, filesystem: &str) -> String {
    format!("153 144 0:77 / {destination} {options},relatime - {filesystem} {filesystem} rw")
}

/// Which bound the shipped rule reports unmet, empty when all are met.
fn mount_violation(lines: &[String]) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let _spawn_guard = crate::test_support::process_spawn_guard();
    let path = std::env::temp_dir().join(format!(
        "spice-proof-mountinfo-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&path, lines.join("\n") + "\n").expect("fixture mountinfo");
    let output = std::process::Command::new("/bin/sh")
        .args([
            std::ffi::OsStr::new("-c"),
            std::ffi::OsStr::new(concat!(mount_rule!(), "\nmount_violation \"$1\"\n")),
            std::ffi::OsStr::new("sh"),
            path.as_os_str(),
        ])
        .output()
        .expect("host POSIX shell runs the mount rule");
    let _ = fs::remove_file(&path);
    String::from_utf8(output.stdout)
        .expect("refusal names are ASCII")
        .trim()
        .to_owned()
}

/// Which scratch bound the shipped cache rule reports unmet.
fn cache_violation(caches: &[&std::path::Path]) -> String {
    let _spawn_guard = crate::test_support::process_spawn_guard();
    let output = std::process::Command::new("/bin/sh")
        .args([
            std::ffi::OsStr::new("-c"),
            std::ffi::OsStr::new(concat!(cache_rule!(), "\ncache_violation \"$@\"\n")),
            std::ffi::OsStr::new("sh"),
        ])
        .args(caches)
        .output()
        .expect("host POSIX shell runs the cache rule");
    String::from_utf8(output.stdout)
        .expect("refusal names are ASCII")
        .trim()
        .to_owned()
}

/// The mounts this boundary actually grants, as the runtime reports them.
fn granted_mounts() -> Vec<String> {
    vec![
        mountinfo_line("/workspace", "ro,relatime", "ext4"),
        mountinfo_line("/cargo-home", "rw,nosuid,nodev,noexec", "tmpfs"),
        mountinfo_line("/cargo-target", "rw,nosuid,nodev", "tmpfs"),
    ]
}

#[test]
fn the_mounts_this_boundary_grants_meet_every_bound_it_asserts() {
    assert_eq!(
        mount_violation(&granted_mounts()),
        "",
        "the executable target mount must still satisfy the preflight it passes today"
    );
}

#[test]
fn each_unmet_mount_bound_is_refused_by_its_own_name() {
    let writable_workspace = vec![
        mountinfo_line("/workspace", "rw,relatime", "ext4"),
        mountinfo_line("/cargo-home", "rw,nosuid,nodev,noexec", "tmpfs"),
        mountinfo_line("/cargo-target", "rw,nosuid,nodev", "tmpfs"),
    ];
    let disk_backed_home = vec![
        mountinfo_line("/workspace", "ro,relatime", "ext4"),
        mountinfo_line("/cargo-home", "rw,nosuid,nodev,noexec", "ext4"),
        mountinfo_line("/cargo-target", "rw,nosuid,nodev", "tmpfs"),
    ];
    let absent_target = vec![
        mountinfo_line("/workspace", "ro,relatime", "ext4"),
        mountinfo_line("/cargo-home", "rw,nosuid,nodev,noexec", "tmpfs"),
    ];

    assert_eq!(
        mount_violation(&writable_workspace),
        "workspace-not-read-only"
    );
    assert_eq!(mount_violation(&disk_backed_home), "cargo-home-not-tmpfs");
    assert_eq!(mount_violation(&absent_target), "cargo-target-not-tmpfs");
}

#[test]
fn battery_preflight_accepts_only_two_initially_empty_caches() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-proof-cache-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let home = root.join("home");
    let target = root.join("target");
    fs::create_dir_all(&home).expect("fixture Cargo home");
    fs::create_dir_all(&target).expect("fixture Cargo target");

    assert_eq!(cache_violation(&[&home, &target]), "");
    fs::write(target.join("artifact"), b"compiled").expect("visible cached artifact");
    assert_eq!(cache_violation(&[&home, &target]), "cache-not-empty");
    fs::remove_file(target.join("artifact")).expect("remove visible artifact");
    fs::write(home.join(".registry"), b"cached").expect("hidden cached artifact");
    assert_eq!(cache_violation(&[&home, &target]), "cache-not-empty");
    fs::remove_file(home.join(".registry")).expect("remove hidden artifact");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("missing", target.join("broken"))
            .expect("broken cached symlink");
        assert_eq!(cache_violation(&[&home, &target]), "cache-not-empty");
    }

    fs::remove_dir_all(root).expect("remove cache fixture");
}

#[test]
fn a_fresh_namespace_of_down_tunnel_stubs_is_isolated() {
    let directory = network_directory(&[
        ("lo", 0x9),
        ("erspan0", 0x1002),
        ("gre0", 0x80),
        ("gretap0", 0x1002),
        ("ip6tnl0", 0x80),
        ("sit0", 0x80),
        ("tunl0", 0x80),
    ]);

    assert!(
        network_isolated(&directory),
        "a --network=none namespace carries no traffic through interfaces that are all down"
    );
}

#[test]
fn an_interface_that_can_carry_traffic_is_refused_whatever_it_is_named() {
    let named_lo = network_directory(&[("lo", 0x1003)]);
    let named_eth0 = network_directory(&[("lo", 0x9), ("eth0", 0x1003)]);

    assert!(
        !network_isolated(&named_lo),
        "an up adapter without IFF_LOOPBACK reaches the network under any name"
    );
    assert!(
        !network_isolated(&named_eth0),
        "an up adapter beside real loopback reaches the network"
    );
}

#[test]
fn an_interface_whose_flags_cannot_be_read_is_refused_rather_than_skipped() {
    let directory = network_directory(&[("lo", 0x9)]);
    fs::create_dir_all(directory.join("eth0")).expect("fixture interface without flags");

    assert!(
        !network_isolated(&directory),
        "an interface this rule cannot classify is not an interface it may pass over"
    );
}

struct Fixture {
    root: std::path::PathBuf,
    runtime: std::path::PathBuf,
    workspace: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        Self::with_runtime(SILENT_RUNTIME)
    }

    fn with_runtime(script: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-proof-container-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let workspace = root.join("workspace");
        let runtime = root.join("docker");
        fs::create_dir_all(&workspace).expect("fixture workspace");
        {
            let _write_guard = crate::test_support::executable_write_guard();
            fs::write(&runtime, script).expect("fixture runtime");
            make_executable(&runtime);
        }
        Self {
            root,
            runtime,
            workspace,
        }
    }

    fn container(&self) -> Container {
        let limits = limits();
        Container::new(
            OciRuntime::attest(&self.runtime, limits).expect("fixture runtime"),
            format!("registry.example/proof@sha256:{}", "4".repeat(64)),
            &self.workspace,
            limits,
        )
        .expect("sealed fixture")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove fixture");
    }
}

fn limits() -> Limits {
    Limits::new(
        Deadlines::new(
            Duration::from_secs(30),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect("deadlines"),
        Duration::from_secs(1),
        4096,
    )
    .expect("limits")
}

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt as _;

    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make runtime executable");
}

#[cfg(not(unix))]
fn make_executable(_path: &std::path::Path) {}
