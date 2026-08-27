//! Successful and status-preserving manual Cargo process boundaries.

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::time::{Duration, Instant};

use spice::install::{InstallStatus, apply, plan};
use spice::repository::Repository;
use wire::{ConfigSource, Digest, Domain, TrustState};

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

#[test]
fn developer_xtask_expands_arguments_inside_the_private_cargo_home() {
    let fixture = Fixture::new();
    let opaque = OsString::from_vec(vec![b'o', 0x80, b'k']);
    let output = fixture
        .xtask_command(
            &fixture.first,
            [
                OsString::from("argument with spaces"),
                OsString::new(),
                opaque.clone(),
            ],
        )
        .output()
        .expect("run isolated developer xtask");
    assert_success(&output);
    let home = spoken_home(&output);
    assert_eq!(mode(&home), 0o700);
    assert!(!home.join("registry").exists());
    assert!(!home.join("git").exists());
    let arguments = fs::read(&fixture.arguments).expect("read native Cargo arguments");
    let arguments = arguments
        .strip_suffix(&[0])
        .expect("native Cargo argument record has its terminal delimiter");
    assert_eq!(
        arguments.split(|byte| *byte == 0).collect::<Vec<_>>(),
        [
            b"run".as_slice(),
            b"--locked".as_slice(),
            b"--package".as_slice(),
            b"xtask".as_slice(),
            b"--bin".as_slice(),
            b"xtask".as_slice(),
            b"--".as_slice(),
            b"argument with spaces".as_slice(),
            b"".as_slice(),
            opaque.as_bytes(),
        ]
    );
}

#[test]
fn developer_xtask_preserves_the_native_cargo_exit_status() {
    let fixture = Fixture::new();
    let output = fixture
        .xtask_command(&fixture.first, ["exit-23"])
        .output()
        .expect("run failing isolated developer xtask");
    assert_eq!(output.status.code(), Some(23));
}

#[test]
fn cold_plain_commit_routes_the_source_gate_through_the_installed_private_cargo_lifecycle() {
    let fixture = Fixture::new();
    super::cargo_isolation_support::git(&fixture.first, &["config", "--unset", "core.hooksPath"]);
    let repository = Repository::discover(&fixture.first).expect("discover commit fixture");
    install_appliance(&repository);
    let policy = concat!(
        "[wrappers]\n",
        "spice = 'appliance'\n",
        "\n",
        "[policy]\n",
        "pre_commit_builtins = false\n",
        "pre_commit = [\n",
        "  { label = 'Spice source gate', run = ['spice', 'dev', 'cargo', '--', 'run', '--quiet', '--locked', '--package', 'spice', '--bin', 'spice', '--', 'gate', 'selection'] },\n",
        "]\n",
        "\n",
        "[policy.file_shape]\n",
        "source_suffixes = ['.rs']\n",
        "\n",
        "[policy.languages]\n",
        "complexity = ['.rs']\n",
        "magic = ['.rs']\n",
    );
    fs::write(fixture.first.join("spice-rust.toml"), policy)
        .expect("write configured source-gate policy");
    super::cargo_isolation_support::git(&fixture.first, &["add", "spice-rust.toml"]);
    grant_policy(&repository, policy);

    let conventional = fixture.root.join("conventional-home");
    fs::create_dir(&conventional).expect("create blank conventional home");
    let started = Instant::now();
    let output = fixture
        .git_command(
            &fixture.first,
            [
                "commit",
                "-q",
                "-m",
                "Route source gate through private Cargo",
            ],
        )
        .env("HOME", &conventional)
        .output()
        .expect("run plain commit through installed hooks");

    assert_success(&output);
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "cold plain commit exceeded the focused hook proof deadline: {:?}",
        started.elapsed(),
    );
    assert!(
        !fixture.hostile_marker.exists(),
        "configured gate searched PATH: {}",
        fs::read_to_string(&fixture.hostile_marker).unwrap_or_default(),
    );
    assert!(!conventional.join(".cargo/registry").exists());
    assert!(!conventional.join(".cargo/git").exists());
    assert!(!fixture.bootstrap.join("registry").exists());
    assert!(!fixture.bootstrap.join("git").exists());
    assert_eq!(
        recorded_arguments(&fixture.arguments),
        [
            b"run".as_slice(),
            b"--quiet".as_slice(),
            b"--locked".as_slice(),
            b"--package".as_slice(),
            b"spice".as_slice(),
            b"--bin".as_slice(),
            b"spice".as_slice(),
            b"--".as_slice(),
            b"gate".as_slice(),
            b"selection".as_slice(),
        ]
        .map(<[u8]>::to_vec)
        .to_vec(),
    );
    let homes = fixture.bootstrap.join("spice/source-store-v2/homes");
    let private_homes = fs::read_dir(homes)
        .expect("read private Cargo homes")
        .map(|entry| entry.expect("read private Cargo home entry").path())
        .collect::<Vec<_>>();
    assert_eq!(private_homes.len(), 1);
    assert!(!private_homes[0].join("registry").exists());
    assert!(!private_homes[0].join("git").exists());
}

fn install_appliance(repository: &Repository) {
    let source = Path::new(env!("CARGO_BIN_EXE_spice"));
    let preview = plan(repository, Some(source)).expect("plan fixture appliance install");
    let receipt = apply(
        repository,
        Some(source),
        &preview.digest().expect("digest complete appliance plan"),
    )
    .expect("install fixture appliance");
    assert_eq!(receipt.status(), InstallStatus::Complete);
}

fn grant_policy(repository: &Repository, tracked: &str) {
    let digest = policy_digest(tracked);
    let state = repository.admit_state().expect("admit fixture state paths");
    fs::create_dir_all(state.data()).expect("create fixture authority parent");
    let mut authority = journal::Journal::open(state.journal()).expect("open fixture authority");
    let mut write = journal::JournalWrite::new(
        wire::Actor::Spice,
        "recovery:cargo-isolation-configured-commit"
            .parse()
            .expect("fixture origin"),
        "2030-04-12T18:44:01.271Z".parse().expect("fixture instant"),
        wire::Producer::CommandDoor,
    );
    let pended = write
        .fact(
            wire::body::TrustPended {
                surface: "configuration".to_owned(),
                digest: digest.clone(),
                previous: None,
            },
            journal::FactCause::Root,
        )
        .expect("build fixture trust fact");
    write.trust_head(pended, move |_| wire::TrustHead {
        digest,
        state: TrustState::Granted,
    });
    authority.apply(write).expect("commit fixture trust grant");
}

fn policy_digest(tracked: &str) -> Digest {
    let mut framed = Vec::new();
    frame_policy(&mut framed, ConfigSource::Tracked, Some(tracked));
    frame_policy(&mut framed, ConfigSource::Local, None);
    Digest::compute(Domain::ConfigExecutableSurface, &framed)
}

fn frame_policy(framed: &mut Vec<u8>, source: ConfigSource, text: Option<&str>) {
    framed.extend_from_slice(source.as_str().as_bytes());
    framed.push(0);
    match text {
        None => framed.push(0),
        Some(text) => {
            framed.push(1);
            framed.extend_from_slice(
                &u64::try_from(text.len())
                    .expect("fixture policy length fits u64")
                    .to_be_bytes(),
            );
            framed.extend_from_slice(text.as_bytes());
        }
    }
}

fn recorded_arguments(path: &Path) -> Vec<Vec<u8>> {
    let arguments = fs::read(path).expect("read native Cargo arguments");
    let arguments = arguments
        .strip_suffix(&[0])
        .expect("native Cargo argument record has its terminal delimiter");
    arguments
        .split(|byte| *byte == 0)
        .map(<[u8]>::to_vec)
        .collect()
}
