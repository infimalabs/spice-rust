use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use super::*;

struct Fixture {
    root: PathBuf,
    bootstrap: PathBuf,
    repository: PathBuf,
    package: PathBuf,
    cargo: PathBuf,
    rustc: PathBuf,
    rustdoc: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-cargo-isolation-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let bootstrap = root.join("cargo-home");
        let repository = root.join("worktree");
        let package = root.join("package");
        fs::create_dir_all(package.join("src")).expect("create package fixture");
        fs::create_dir(&bootstrap).expect("create bootstrap Cargo home");
        fs::create_dir(&repository).expect("create fixture worktree");

        let manifest =
            b"[package]\nname = \"fixture-dependency\"\nversion = \"1.0.0\"\nedition = \"2024\"\n";
        let source = b"pub fn answer() -> u8 { 42 }\n";
        fs::write(package.join("Cargo.toml"), manifest).expect("write package manifest");
        fs::write(package.join("src/lib.rs"), source).expect("write package source");
        let checksum = serde_json::json!({
            "files": {
                "Cargo.toml": sha256(manifest),
                "src/lib.rs": sha256(source),
            },
            "package": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        });
        fs::write(
            package.join(".cargo-checksum.json"),
            serde_json::to_vec(&checksum).expect("serialize checksum fixture"),
        )
        .expect("write checksum fixture");
        fs::write(
            repository.join("Cargo.toml"),
            "[package]\nname = \"fixture-root\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("write root manifest");
        fs::write(repository.join("Cargo.lock"), lockfile("initial"))
            .expect("write exact lockfile");

        let cargo = root.join("native-cargo");
        let rustc = root.join("native-rustc");
        let rustdoc = root.join("native-rustdoc");
        let script = format!(
            concat!(
                "#!/bin/sh\n",
                "set -eu\n",
                "destination=\n",
                "for argument in \"$@\"; do destination=$argument; done\n",
                "cp -R {} \"$destination/fixture-dependency-1.0.0\"\n",
                "printf '[source.crates-io]\\nreplace-with = \"vendored-sources\"\\n\\n",
                "[source.vendored-sources]\\ndirectory = \"%s\"\\n' \"$destination\"\n",
            ),
            shell_word(&package),
        );
        fs::write(&cargo, script).expect("write fake native Cargo");
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o700))
            .expect("make fake native Cargo executable");
        write_executable(&rustc, "#!/bin/sh\nexit 0\n");
        write_executable(&rustdoc, "#!/bin/sh\nexit 0\n");

        Self {
            root,
            bootstrap,
            repository,
            package,
            cargo,
            rustc,
            rustdoc,
        }
    }

    fn host(&self) -> Host {
        Host::new(
            self.bootstrap.clone(),
            self.cargo.clone(),
            self.rustc.clone(),
            self.rustdoc.clone(),
        )
        .expect("admit fixture host")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        make_writable(&self.root);
        let _ = fs::remove_dir_all(&self.root);
    }
}

const TRACKED_CARGO_GUIDANCE: [(&str, &str); 2] = [
    ("AGENTS.md", ::std::include_str!("../../../AGENTS.md")),
    (
        "spice/CARGO_ISOLATION.md",
        ::std::include_str!("../../CARGO_ISOLATION.md"),
    ),
];

#[test]
fn tracked_guidance_routes_rust_cargo_through_ambient_spice() {
    for (path, guidance) in TRACKED_CARGO_GUIDANCE {
        let normalized = guidance.split_whitespace().collect::<Vec<_>>().join(" ");
        for required in [
            "spice dev cargo --",
            "spice dev xtask --",
            "wrapper policy",
            "Git-common",
            "hard stop",
            "shared mutable",
            "harness authority",
        ] {
            assert!(
                normalized.contains(required),
                "{path} must retain the Cargo and harness authority marker {required:?}",
            );
        }
        for forbidden in [
            "/usr/bin/git",
            "--git-common-dir",
            ".spice-install/bin/spice",
            "spice_rs=",
            "$spice_rs",
        ] {
            assert!(
                !normalized.contains(forbidden),
                "{path} must keep binding machinery out of agent-facing commands: {forbidden:?}",
            );
        }
        assert!(
            !normalized.contains("CARGO_HOME="),
            "{path} must not prescribe a direct Cargo-home override",
        );
        assert!(
            guidance
                .lines()
                .any(|line| line.trim_start().starts_with("spice dev cargo --")),
            "{path} must render an executable ambient Cargo command",
        );
    }
}

#[test]
fn repository_configuration_has_no_pre_isolation_xtask_shortcut() {
    let configuration = ::std::include_str!("../../../.cargo/config.toml");
    let configuration: toml::Value =
        toml::from_str(configuration).expect("tracked Cargo configuration parses");
    assert!(
        configuration
            .get("alias")
            .and_then(toml::Value::as_table)
            .is_none_or(|aliases| !aliases.contains_key("xtask")),
        "developer xtasks must enter through exact installed Spice before Cargo",
    );
}

#[test]
fn identical_lanes_reuse_one_immutable_generation_and_bounded_homes() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let first = store::prepare(&fixture.repository, None, &host).expect("prepare manual home");
    let lane = "018f82d4-6f59-7d06-92e3-bce9131163c7"
        .parse()
        .expect("canonical fixture lane");
    let second =
        store::prepare(&fixture.repository, Some(lane), &host).expect("prepare resident home");

    assert_eq!(first.generation(), second.generation());
    assert!(!first.reused_generation());
    assert!(second.reused_generation());
    assert_ne!(first.home(), second.home());
    assert_eq!(mode(first.home()), 0o700);
    assert_eq!(mode(second.home()), 0o700);
    for home in [first.home(), second.home()] {
        let config = fs::read_to_string(home.join("config.toml")).expect("read generated config");
        assert!(config.contains("offline = true"));
        assert!(config.contains("auto-clean-frequency = \"never\""));
        assert!(config.contains(first.generation()));
        assert!(!home.join("registry").exists());
        assert!(!home.join("git").exists());
        assert!(!home.join("credentials.toml").exists());
    }
    let inspection = store::inspect(&host).expect("inspect fixture store");
    assert_eq!(inspection.generations(), 1);
    assert_eq!(inspection.packages(), 1);
    assert_eq!(inspection.homes(), 2);
    assert!(inspection.issues().is_empty());
}

#[test]
fn inspection_of_an_absent_store_is_non_mutating() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let store = host.store_root();
    let error = store::inspect(&host).expect_err("an absent schema cannot be inspected");
    assert_eq!(error.reason(), "cargo-isolation::directory-unavailable");
    assert!(!store.exists(), "inspection must not initialize the schema");
}

#[test]
fn exact_lockfile_bytes_advance_the_home_to_a_new_generation() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let first = store::prepare(&fixture.repository, None, &host).expect("prepare first generation");
    fs::write(fixture.repository.join("Cargo.lock"), lockfile("refreshed"))
        .expect("advance lockfile bytes");
    let second =
        store::prepare(&fixture.repository, None, &host).expect("prepare refreshed generation");

    assert_ne!(first.generation(), second.generation());
    assert_eq!(first.home(), second.home());
    assert!(!second.reused_generation());
    let config = fs::read_to_string(second.home().join("config.toml"))
        .expect("read atomically advanced config");
    assert!(config.contains(second.generation()));
    assert!(!config.contains(first.generation()));
    let inspection = store::inspect(&host).expect("inspect refreshed store");
    assert_eq!(inspection.generations(), 2);
    assert_eq!(inspection.packages(), 1);
}

#[test]
fn source_generation_authority_binds_the_exact_home_and_lockfile() {
    let fixture = Fixture::new();
    let prepared = store::prepare(&fixture.repository, None, &fixture.host())
        .expect("prepare source generation");
    let lock_bytes =
        fs::read(fixture.repository.join("Cargo.lock")).expect("read fixture lockfile");

    let authority = source_generation_authority(prepared.home(), &lock_bytes)
        .expect("admit exact source generation");
    assert_eq!(authority.generation(), prepared.generation());
    assert_eq!(authority.manifest().len(), 64);

    let error = source_generation_authority(prepared.home(), lockfile("different").as_bytes())
        .expect_err("different lock bytes must refuse the retained home generation");
    assert_eq!(error.reason(), "cargo-isolation::generation-lock-mismatch");
}

#[test]
fn retained_owned_home_rebinds_only_to_the_current_store_for_the_same_lock() {
    let retained = Fixture::new();
    let retained_home = store::prepare(&retained.repository, None, &retained.host())
        .expect("prepare retained source generation");
    let lock_bytes = fs::read(retained.repository.join("Cargo.lock")).unwrap();
    let current = Fixture::new();
    fs::write(current.repository.join("Cargo.lock"), &lock_bytes).unwrap();
    let current_home = store::prepare(&current.repository, None, &current.host())
        .expect("prepare current source generation");

    let authority = readmit_source_generation(
        retained_home.home(),
        &lock_bytes,
        &retained.repository,
        &current.host(),
    )
    .expect("re-admit retained owned home");
    assert_eq!(authority.generation(), current_home.generation());
    let config = fs::read_to_string(retained_home.home().join("config.toml")).unwrap();
    assert!(config.contains(&current.host().store_root().display().to_string()));
    assert!(!config.contains(&retained.host().store_root().display().to_string()));
    assert_eq!(
        readmit_source_generation(
            retained_home.home(),
            &lock_bytes,
            &retained.repository,
            &current.host(),
        )
        .unwrap(),
        authority,
        "current re-admission is idempotent",
    );

    let error = readmit_source_generation(
        retained_home.home(),
        lockfile("different").as_bytes(),
        &retained.repository,
        &current.host(),
    )
    .expect_err("changed lock bytes cannot rebind a retained home");
    assert_eq!(error.reason(), "cargo-isolation::generation-lock-mismatch");

    let config = fs::read(retained_home.home().join("config.toml")).unwrap();
    for host in [&current.host(), &retained.host()] {
        let error = readmit_source_generation(
            retained_home.home(),
            &lock_bytes,
            &current.repository,
            host,
        )
        .expect_err("foreign worktree cannot adopt retained home");
        assert_eq!(error.reason(), "cargo-isolation::home-owner-mismatch");
        assert_eq!(
            fs::read(retained_home.home().join("config.toml")).unwrap(),
            config,
        );
    }
}

#[test]
fn malformed_existing_home_config_is_retained_instead_of_repaired() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let prepared = store::prepare(&fixture.repository, None, &host).expect("prepare owned home");
    let config = prepared.home().join("config.toml");
    let original = fs::read(&config).expect("read admitted home config");
    fs::set_permissions(&config, fs::Permissions::from_mode(0o644))
        .expect("malform home config permissions");
    fs::write(fixture.repository.join("Cargo.lock"), lockfile("refreshed"))
        .expect("advance lockfile for refused repair");

    let error = store::prepare(&fixture.repository, None, &host)
        .expect_err("malformed existing config must not be repaired");
    assert_eq!(error.reason(), "cargo-isolation::permission-mismatch");
    assert_eq!(mode(&config), 0o644);
    assert_eq!(fs::read(&config).expect("reread retained config"), original);
}

#[test]
fn native_cargo_inside_the_owned_store_is_refused() {
    let fixture = Fixture::new();
    let recursive = fixture
        .bootstrap
        .join(STORE_RELATIVE)
        .join("recursive-toolchain");
    fs::create_dir_all(&recursive).expect("create recursive store path");
    let cargo = recursive.join("native-cargo");
    let rustc = recursive.join("native-rustc");
    let rustdoc = recursive.join("native-rustdoc");
    for (source, destination) in [
        (&fixture.cargo, &cargo),
        (&fixture.rustc, &rustc),
        (&fixture.rustdoc, &rustdoc),
    ] {
        fs::copy(source, destination).expect("copy recursive toolchain fixture");
        fs::set_permissions(destination, fs::Permissions::from_mode(0o700))
            .expect("make recursive native tool executable");
    }

    let error = Host::new(fixture.bootstrap.clone(), cargo, rustc, rustdoc)
        .expect_err("recursive native Cargo must be refused");
    assert_eq!(error.reason(), "cargo-isolation::native-cargo-recursive");
}

#[test]
fn registry_package_with_an_unlisted_file_is_refused_before_publication() {
    let fixture = Fixture::new();
    fs::write(fixture.package.join("unlisted.txt"), "not covered")
        .expect("add unlisted package file");
    let host = fixture.host();

    let error = store::prepare(&fixture.repository, None, &host)
        .expect_err("unlisted registry package file must be refused");
    assert_eq!(error.reason(), "cargo-isolation::package-file-set-mismatch");
    let packages = host.store_root().join("packages");
    assert_eq!(
        fs::read_dir(packages)
            .expect("read refused package store")
            .count(),
        0,
    );
}

#[test]
fn git_package_without_an_exact_revision_is_refused_before_publication() {
    let fixture = Fixture::new();
    fs::write(
        fixture.package.join(".cargo-checksum.json"),
        serde_json::to_vec(&serde_json::json!({
            "files": {
                "Cargo.toml": sha256(&fs::read(fixture.package.join("Cargo.toml"))
                    .expect("read package manifest")),
                "src/lib.rs": sha256(&fs::read(fixture.package.join("src/lib.rs"))
                    .expect("read package source")),
            },
            "package": null,
        }))
        .expect("serialize Git checksum fixture"),
    )
    .expect("write Git checksum fixture");
    fs::write(
        fixture.repository.join("Cargo.lock"),
        concat!(
            "version = 4\n\n",
            "[[package]]\n",
            "name = \"fixture-dependency\"\n",
            "version = \"1.0.0\"\n",
            "source = \"git+https://example.invalid/dependency\"\n",
        ),
    )
    .expect("write unpinned Git lockfile");

    let error = store::prepare(&fixture.repository, None, &fixture.host())
        .expect_err("an unpinned Git source must not publish");
    assert_eq!(error.reason(), "cargo-isolation::git-revision-unpinned");
    assert_eq!(
        fs::read_dir(fixture.host().store_root().join("packages"))
            .expect("read refused package store")
            .count(),
        0,
    );
}

#[test]
fn registry_checksum_cannot_escape_the_package_store() {
    let fixture = Fixture::new();
    let checksum = "../../outside-store";
    fs::write(
        fixture.package.join(".cargo-checksum.json"),
        serde_json::to_vec(&serde_json::json!({
            "files": {
                "Cargo.toml": sha256(&fs::read(fixture.package.join("Cargo.toml"))
                    .expect("read package manifest")),
                "src/lib.rs": sha256(&fs::read(fixture.package.join("src/lib.rs"))
                    .expect("read package source")),
            },
            "package": checksum,
        }))
        .expect("serialize malformed registry checksum fixture"),
    )
    .expect("write malformed registry checksum fixture");
    fs::write(
        fixture.repository.join("Cargo.lock"),
        format!(
            concat!(
                "version = 4\n\n",
                "[[package]]\n",
                "name = \"fixture-dependency\"\n",
                "version = \"1.0.0\"\n",
                "source = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
                "checksum = {:?}\n",
            ),
            checksum,
        ),
    )
    .expect("write malformed registry lockfile");

    let error = store::prepare(&fixture.repository, None, &fixture.host())
        .expect_err("a malformed registry checksum must not publish");
    assert_eq!(error.reason(), "cargo-isolation::registry-checksum-invalid");
    assert!(!fixture.bootstrap.join("spice/outside-store").exists());
}

#[test]
fn host_debug_output_redacts_the_seed_environment() {
    let fixture = Fixture::new();
    let host = Host::from_environment(
        &fixture.repository,
        [
            (
                OsString::from(BOOTSTRAP_HOME_VARIABLE),
                fixture.bootstrap.clone().into_os_string(),
            ),
            (
                OsString::from(NATIVE_CARGO_VARIABLE),
                fixture.cargo.clone().into_os_string(),
            ),
            (
                OsString::from(NATIVE_RUSTC_VARIABLE),
                fixture.rustc.clone().into_os_string(),
            ),
            (
                OsString::from(NATIVE_RUSTDOC_VARIABLE),
                fixture.rustdoc.clone().into_os_string(),
            ),
            (
                OsString::from("CARGO_REGISTRY_TOKEN"),
                OsString::from("secret-fixture-token"),
            ),
        ],
    )
    .expect("admit explicit secret-bearing seed environment");

    let debug = format!("{host:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("secret-fixture-token"));
}

#[test]
fn compiler_environment_preserves_the_private_home_and_binds_native_tools() {
    let fixture = Fixture::new();
    let private_home = fixture.root.join("private-cargo-home");
    fs::create_dir(&private_home).expect("create private Cargo home");
    let host = Host::from_environment(
        &fixture.repository,
        [
            (
                OsString::from(BOOTSTRAP_HOME_VARIABLE),
                fixture.bootstrap.clone().into_os_string(),
            ),
            (
                OsString::from(CARGO_HOME_VARIABLE),
                private_home.clone().into_os_string(),
            ),
            (
                OsString::from(NATIVE_CARGO_VARIABLE),
                fixture.cargo.clone().into_os_string(),
            ),
            (
                OsString::from(NATIVE_RUSTC_VARIABLE),
                fixture.rustc.clone().into_os_string(),
            ),
            (
                OsString::from(NATIVE_RUSTDOC_VARIABLE),
                fixture.rustdoc.clone().into_os_string(),
            ),
            (OsString::from("PATH"), OsString::new()),
            (OsString::from(RUSTC_VARIABLE), OsString::from("rustc")),
        ],
    )
    .expect("admit explicit compiler authority");

    let environment = host
        .compiler_environment()
        .expect("bind admitted compiler authority");
    let entries = environment
        .entries()
        .iter()
        .cloned()
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(entries[OsStr::new(CARGO_HOME_VARIABLE)], private_home);
    assert_eq!(entries[OsStr::new("PATH")], OsString::new());
    assert_eq!(
        entries[OsStr::new(NATIVE_CARGO_VARIABLE)],
        fs::canonicalize(&fixture.cargo).expect("canonicalize fixture Cargo")
    );
    assert_eq!(
        entries[OsStr::new(RUSTC_VARIABLE)],
        fs::canonicalize(&fixture.rustc).expect("canonicalize fixture rustc")
    );
    assert_eq!(
        entries[OsStr::new(RUSTDOC_VARIABLE)],
        fs::canonicalize(&fixture.rustdoc).expect("canonicalize fixture rustdoc")
    );
}

#[test]
fn partial_native_toolchain_authority_is_refused() {
    let fixture = Fixture::new();
    let error = Host::from_environment(
        &fixture.repository,
        [
            (
                OsString::from(BOOTSTRAP_HOME_VARIABLE),
                fixture.bootstrap.clone().into_os_string(),
            ),
            (
                OsString::from(NATIVE_CARGO_VARIABLE),
                fixture.cargo.clone().into_os_string(),
            ),
        ],
    )
    .expect_err("a partial native toolchain cannot launch Cargo");
    assert_eq!(
        error.reason(),
        "cargo-isolation::native-toolchain-incomplete"
    );
}

#[test]
fn mixed_native_toolchain_directories_are_refused() {
    let fixture = Fixture::new();
    let other = fixture.root.join("other");
    fs::create_dir(&other).expect("create mixed toolchain directory");
    let rustdoc = other.join("native-rustdoc");
    write_executable(&rustdoc, "#!/bin/sh\nexit 0\n");

    let error = Host::new(
        fixture.bootstrap.clone(),
        fixture.cargo.clone(),
        fixture.rustc.clone(),
        rustdoc,
    )
    .expect_err("mixed native toolchain directories cannot launch Cargo");
    assert_eq!(error.reason(), "cargo-isolation::native-toolchain-mixed");
}

#[test]
fn tracked_toolchain_resolves_the_complete_trio_through_exact_rustup() {
    let fixture = Fixture::new();
    fs::write(
        fixture.repository.join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"fixture-1.0\"\n",
    )
    .expect("write tracked toolchain fixture");
    let bin = fixture.bootstrap.join("bin");
    fs::create_dir(&bin).expect("create bootstrap bin");
    let rustup = bin.join("rustup");
    write_executable(
        &rustup,
        &format!(
            concat!(
                "#!/bin/sh\n",
                "set -eu\n",
                "test \"$1\" = which\n",
                "test \"$2\" = --toolchain\n",
                "test \"$3\" = fixture-1.0\n",
                "case \"$4\" in\n",
                "  cargo) printf '%s\\n' {} ;;\n",
                "  rustc) printf '%s\\n' {} ;;\n",
                "  rustdoc) printf '%s\\n' {} ;;\n",
                "  *) exit 44 ;;\n",
                "esac\n",
            ),
            shell_word(&fixture.cargo),
            shell_word(&fixture.rustc),
            shell_word(&fixture.rustdoc),
        ),
    );

    let host = Host::from_environment(
        &fixture.repository,
        [
            (
                OsString::from(BOOTSTRAP_HOME_VARIABLE),
                fixture.bootstrap.clone().into_os_string(),
            ),
            (
                OsString::from("HOME"),
                fixture.root.clone().into_os_string(),
            ),
        ],
    )
    .expect("tracked toolchain resolves one native trio");
    assert_eq!(host.cargo(), fs::canonicalize(&fixture.cargo).unwrap());
    assert_eq!(host.rustc(), fs::canonicalize(&fixture.rustc).unwrap());
    assert_eq!(host.rustdoc(), fs::canonicalize(&fixture.rustdoc).unwrap());
}

#[test]
fn malformed_links_and_uncertain_home_liveness_are_reported_and_retained() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let prepared = store::prepare(&fixture.repository, None, &host).expect("prepare generation");
    let generation = prepared
        .store()
        .join("generations")
        .join(prepared.generation());
    let vendor = generation.join("vendor");
    let link = fs::read_dir(&vendor)
        .expect("read generation view")
        .next()
        .expect("one package link")
        .expect("read package link")
        .path();
    fs::set_permissions(&generation, fs::Permissions::from_mode(0o700))
        .expect("open generation for corruption fixture");
    fs::set_permissions(&vendor, fs::Permissions::from_mode(0o700))
        .expect("open vendor view for corruption fixture");
    fs::remove_file(&link).expect("remove admitted package link");
    std::os::unix::fs::symlink(&fixture.root, &link).expect("install escaping package link");
    fs::set_permissions(&vendor, fs::Permissions::from_mode(0o500))
        .expect("restore vendor permissions");
    fs::set_permissions(&generation, fs::Permissions::from_mode(0o500))
        .expect("restore generation permissions");
    fs::write(prepared.home().join("owner.json"), b"{}").expect("malform owner record");

    let inspection = store::inspect(&host).expect("inspect malformed owned state");
    assert!(
        inspection
            .issues()
            .iter()
            .any(|issue| issue.contains("generation-link-invalid")),
        "malformed link is surfaced: {:?}",
        inspection.issues(),
    );
    assert!(
        inspection
            .issues()
            .iter()
            .any(|issue| issue.contains("home retained")),
        "malformed owner is surfaced: {:?}",
        inspection.issues(),
    );
    let cleanup = store::cleanup_with_grace(&host, std::time::Duration::ZERO)
        .expect("conservative cleanup succeeds");
    assert!(generation.exists(), "uncertain generation is retained");
    assert_eq!(cleanup.removed_generations(), 0);
    assert!(!cleanup.retained_uncertain().is_empty());
}

#[test]
fn package_files_preserve_cargo_copy_semantics_and_permission_drift_is_retained() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let prepared = store::prepare(&fixture.repository, None, &host).expect("prepare generation");
    let package = fs::read_dir(prepared.store().join("packages"))
        .expect("read package store")
        .next()
        .expect("one package")
        .expect("read package entry")
        .path();
    let manifest = package.join("Cargo.toml");
    assert_eq!(mode(&package), 0o500, "package entries are sealed");
    assert_eq!(mode(&manifest), 0o600, "Cargo may copy source modes");

    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o400))
        .expect("introduce package permission drift");
    let inspection = store::inspect(&host).expect("inspect package permission drift");
    assert!(
        inspection
            .issues()
            .iter()
            .any(|issue| issue.contains("permission-mismatch")),
        "permission drift is surfaced: {:?}",
        inspection.issues(),
    );
    let cleanup = store::cleanup_with_grace(&host, std::time::Duration::ZERO)
        .expect("conservative cleanup succeeds");
    assert!(package.exists(), "drifted package is retained");
    assert_eq!(cleanup.removed_packages(), 0);
}

#[test]
fn recognized_interrupted_temporaries_are_reclaimed_but_live_generation_is_not() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let prepared = store::prepare(&fixture.repository, None, &host).expect("prepare generation");
    let temporary = prepared
        .store()
        .join("packages/.spice-tmp-package-interrupted");
    fs::create_dir(&temporary).expect("create interrupted publication fixture");
    let home_temporary = prepared.store().join("homes/.spice-tmp-home-interrupted");
    fs::create_dir(&home_temporary).expect("create interrupted home fixture");

    let cleanup = store::cleanup_with_grace(&host, std::time::Duration::ZERO)
        .expect("cleanup recognized temporary");
    assert_eq!(cleanup.removed_temporaries(), 2);
    assert!(!temporary.exists());
    assert!(!home_temporary.exists());
    assert_eq!(cleanup.removed_generations(), 0);
    assert!(
        prepared
            .store()
            .join("generations")
            .join(prepared.generation())
            .exists(),
        "a current home protects its generation while a reader may use it",
    );
}

#[test]
fn manual_reader_pins_coexist_and_unlocked_crash_residue_is_reclaimed() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let first = store::prepare(&fixture.repository, None, &host).expect("prepare first reader");
    let second = store::prepare(&fixture.repository, None, &host).expect("prepare second reader");
    let pins = first.store().join("pins");
    let active = fs::read_dir(&pins)
        .expect("read active reader pins")
        .map(|entry| entry.expect("read active reader pin").path())
        .collect::<Vec<_>>();
    assert_eq!(active.len(), 2, "each manual reader owns one pin");

    let abandoned = pins.join("reader-00000000000040008000000000000000.lock");
    fs::copy(&active[0], &abandoned).expect("copy a valid crashed-reader record");
    fs::set_permissions(&abandoned, fs::Permissions::from_mode(0o600))
        .expect("seal crashed-reader record");
    let cleanup = store::cleanup_with_grace(&host, std::time::Duration::ZERO)
        .expect("cleanup probes reader pins");
    assert_eq!(cleanup.removed_pins(), 1);
    assert!(
        first
            .store()
            .join("generations")
            .join(first.generation())
            .exists(),
        "active readers retain their generation",
    );

    drop(first);
    drop(second);
    assert_eq!(
        fs::read_dir(pins)
            .expect("read released reader pins")
            .count(),
        0,
        "normal reader exit removes its owned pin",
    );
}

#[test]
fn uncertain_reader_pin_is_not_followed_and_retains_every_generation() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let prepared = store::prepare(&fixture.repository, None, &host).expect("prepare generation");
    let generation = prepared
        .store()
        .join("generations")
        .join(prepared.generation());
    let home = prepared.home().to_path_buf();
    let pins = prepared.store().join("pins");
    drop(prepared);
    make_writable(&home);
    fs::remove_dir_all(&home).expect("remove the generation's home reference");

    let outside = fixture.root.join("unowned-pin-target");
    fs::write(&outside, b"outside stays untouched").expect("write unowned pin target");
    let uncertain = pins.join("reader-00000000000040008000000000000000.lock");
    std::os::unix::fs::symlink(&outside, &uncertain).expect("install uncertain reader pin");

    let cleanup = store::cleanup_with_grace(&host, std::time::Duration::ZERO)
        .expect("cleanup retains generations under uncertain pin liveness");
    assert!(generation.exists(), "uncertain liveness retains generation");
    assert_eq!(cleanup.removed_generations(), 0);
    assert!(!cleanup.retained_uncertain().is_empty());
    assert_eq!(
        fs::read(&outside).expect("read unowned pin target"),
        b"outside stays untouched",
        "cleanup never follows the unowned pin path",
    );
    assert!(uncertain.is_symlink(), "uncertain pin itself is retained");
}

#[test]
fn non_utf8_worktree_identity_is_encoded_without_aliasing() {
    let mut name = b"worktree-".to_vec();
    name.push(0xff);
    let non_utf8 = PathBuf::from(OsString::from_vec(name));
    let encoded = store::encoded_path(&non_utf8);
    assert!(encoded.ends_with("ff"));
    assert_ne!(encoded, store::encoded_path(Path::new("worktree-�")));
}

fn lockfile(comment: &str) -> String {
    format!(
        concat!(
            "# {}\n",
            "version = 4\n\n",
            "[[package]]\n",
            "name = \"fixture-dependency\"\n",
            "version = \"1.0.0\"\n",
            "source = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
            "checksum = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n",
        ),
        comment,
    )
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn shell_word(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write executable fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("make fixture executable");
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .expect("read fixture mode")
        .permissions()
        .mode()
        & 0o777
}

fn make_writable(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    if metadata.is_dir() {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                make_writable(&entry.path());
            }
        }
    } else {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
}
