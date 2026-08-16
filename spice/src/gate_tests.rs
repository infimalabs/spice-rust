use super::*;

struct Scratch {
    root: std::path::PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        std::fs::create_dir(&root).expect("create gate fixture root");
        Self { root }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).expect("remove gate fixture root");
    }
}

fn fixture_git(root: &Path, arguments: &[&str]) {
    let output = Command::new(GIT)
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .expect("launch fixture Git");
    assert!(
        output.status.success(),
        "Git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn staged_renames_retain_both_owners_independent_of_git_rename_configuration() {
    let scratch = Scratch::new("staged-rename-selection");
    for directory in ["wire/src", "app/src", "app/browser"] {
        std::fs::create_dir_all(scratch.root.join(directory))
            .expect("create rename fixture directory");
    }
    std::fs::write(
        scratch.root.join("wire/src/renamed.rs"),
        "pub fn renamed() {}\n",
    )
    .expect("write cross-package Rust source");
    std::fs::write(
        scratch.root.join("README.fixture.md"),
        "# Renamed fixture\n",
    )
    .expect("write cross-language source");
    fixture_git(&scratch.root, &["init", "--quiet"]);
    fixture_git(
        &scratch.root,
        &["config", "user.email", "gate@example.invalid"],
    );
    fixture_git(&scratch.root, &["config", "user.name", "Gate Fixture"]);
    fixture_git(&scratch.root, &["config", "commit.gpgSign", "false"]);
    fixture_git(&scratch.root, &["add", "."]);
    fixture_git(
        &scratch.root,
        &["commit", "--quiet", "-m", "fixture baseline"],
    );

    std::fs::rename(
        scratch.root.join("wire/src/renamed.rs"),
        scratch.root.join("app/src/renamed.rs"),
    )
    .expect("stage a cross-package Rust rename");
    std::fs::rename(
        scratch.root.join("README.fixture.md"),
        scratch.root.join("app/browser/renamed.mjs"),
    )
    .expect("stage a cross-language rename");
    fixture_git(&scratch.root, &["add", "--all"]);

    let expected = [
        "README.fixture.md",
        "app/browser/renamed.mjs",
        "app/src/renamed.rs",
        "wire/src/renamed.rs",
    ];
    for rename_detection in ["true", "false"] {
        fixture_git(&scratch.root, &["config", "diff.renames", rename_detection]);
        let changes = selected_changes(&scratch.root, &Selector::staged())
            .expect("capture both sides of staged renames");
        assert_eq!(changes, expected, "diff.renames={rename_detection}");

        let selection = ::gate::select_current_source_change_gates(
            Digest::compute(wire::Domain::GateTree, b"staged rename fixture"),
            &changes,
        )
        .expect("classify both sides of staged renames");
        assert_eq!(selection.changed_sources(), expected);
        for gate_id in [
            GateId::RepoDocs,
            GateId::Taste,
            GateId::Formatters,
            GateId::CanvasWebTypecheck,
            GateId::JavascriptUnused,
        ] {
            assert!(selection.gate_ids().contains(&gate_id));
        }
    }
}

#[test]
fn verifier_cargo_budget_binds_once_and_refuses_drift() {
    let mut environment = vec![
        (OsString::from("Z_FIXTURE"), OsString::from("last")),
        (OsString::from(CARGO_BUILD_JOBS), OsString::from("99")),
        (OsString::from("A_FIXTURE"), OsString::from("first")),
    ];
    let mut bound = None;

    bind_cargo_jobs(&mut environment, &mut bound, 0).expect("zero normalizes to one job");
    assert_eq!(bound, Some(1));
    assert_eq!(
        environment
            .iter()
            .filter(|(name, _)| name == CARGO_BUILD_JOBS)
            .count(),
        1
    );
    assert_eq!(
        environment
            .iter()
            .find(|(name, _)| name == CARGO_BUILD_JOBS)
            .map(|(_, value)| value),
        Some(&OsString::from("1"))
    );
    assert!(environment.windows(2).all(|pair| pair[0] < pair[1]));

    let exact = environment.clone();
    bind_cargo_jobs(&mut environment, &mut bound, 1).expect("same budget is idempotent");
    assert_eq!(environment, exact);

    let error = bind_cargo_jobs(&mut environment, &mut bound, 2)
        .expect_err("a later budget cannot change the evidence environment");
    assert_eq!(error.reason(), "gate::verifier-budget-refused");
    assert_eq!(environment, exact);
}

#[test]
fn verifier_environment_ignores_shell_scratch_and_retains_arbitrary_build_inputs() {
    let producer = verifier_environment_from([
        (OsString::from("RUSTFLAGS"), OsString::from("--cfg=bound")),
        (
            OsString::from(SHELL_LAST_COMMAND),
            OsString::from("/fixture/producer-xtask"),
        ),
        (OsString::from("SHLVL"), OsString::from("1")),
        (
            OsString::from("ARBITRARY_BUILD_INPUT"),
            OsString::from("retained"),
        ),
    ]);
    let consumer = verifier_environment_from([
        (
            OsString::from(SHELL_LAST_COMMAND),
            OsString::from("/fixture/consumer-spice"),
        ),
        (OsString::from("SHLVL"), OsString::from("2")),
        (
            OsString::from("ARBITRARY_BUILD_INPUT"),
            OsString::from("retained"),
        ),
        (OsString::from("RUSTFLAGS"), OsString::from("--cfg=bound")),
    ]);

    assert_eq!(producer, consumer);
    assert_eq!(
        producer,
        [
            (
                OsString::from("ARBITRARY_BUILD_INPUT"),
                OsString::from("retained"),
            ),
            (OsString::from("RUSTFLAGS"), OsString::from("--cfg=bound")),
        ]
    );

    let drifted = verifier_environment_from([
        (
            OsString::from("ARBITRARY_BUILD_INPUT"),
            OsString::from("changed"),
        ),
        (OsString::from("RUSTFLAGS"), OsString::from("--cfg=bound")),
    ]);
    assert_ne!(producer, drifted);
}

#[test]
fn cargo_launcher_metadata_normalizes_to_the_installed_compiler_environment() {
    let producer = verifier_environment_from([
        (OsString::from("HOME"), OsString::from("/fixture/home")),
        (
            OsString::from("CARGO_HOME"),
            OsString::from("/fixture/home/.cargo"),
        ),
        (
            OsString::from("RUSTUP_HOME"),
            OsString::from("/fixture/home/.rustup"),
        ),
        (OsString::from("CARGO"), OsString::from("/toolchain/cargo")),
        (
            OsString::from("CARGO_MANIFEST_DIR"),
            OsString::from("/fixture/xtask"),
        ),
        (OsString::from("CARGO_PKG_NAME"), OsString::from("xtask")),
        (
            OsString::from("DYLD_FALLBACK_LIBRARY_PATH"),
            OsString::from("/fixture/target/debug/deps"),
        ),
        (
            OsString::from("RUSTUP_TOOLCHAIN"),
            OsString::from("fixture-toolchain"),
        ),
        (OsString::from("RUST_RECURSION_COUNT"), OsString::from("1")),
        (
            OsString::from("ARBITRARY_BUILD_INPUT"),
            OsString::from("retained"),
        ),
    ]);
    let consumer = verifier_environment_from([
        (OsString::from("HOME"), OsString::from("/fixture/home")),
        (
            OsString::from("ARBITRARY_BUILD_INPUT"),
            OsString::from("retained"),
        ),
    ]);

    assert_eq!(producer, consumer);
    assert!(producer.contains(&(
        OsString::from("CARGO_HOME"),
        OsString::from("/fixture/home/.cargo"),
    )));
    assert!(producer.contains(&(
        OsString::from("RUSTUP_HOME"),
        OsString::from("/fixture/home/.rustup"),
    )));

    let custom = verifier_environment_from([
        (OsString::from("HOME"), OsString::from("/fixture/home")),
        (
            OsString::from("CARGO_HOME"),
            OsString::from("/fixture/custom-cargo"),
        ),
        (
            OsString::from("ARBITRARY_BUILD_INPUT"),
            OsString::from("retained"),
        ),
    ]);
    assert_ne!(
        consumer, custom,
        "a custom Cargo home remains evidence-bound"
    );
}

#[test]
fn isolated_exact_evidence_rebinds_the_lane_to_the_manual_cargo_home() {
    let bootstrap = crate::cargo_isolation::BOOTSTRAP_HOME_VARIABLE;
    let native = crate::cargo_isolation::NATIVE_CARGO_VARIABLE;
    let producer = verifier_environment_from([
        (OsString::from("HOME"), OsString::from("/fixture/home")),
        (
            OsString::from("CARGO_HOME"),
            OsString::from("/fixture/manual-cargo"),
        ),
        (
            OsString::from(bootstrap),
            OsString::from("/fixture/native-cargo"),
        ),
        (OsString::from(native), OsString::from("/fixture/bin/cargo")),
        (
            OsString::from("ARBITRARY_BUILD_INPUT"),
            OsString::from("retained"),
        ),
    ]);
    let consumer = verifier_environment_from_with_cargo_home(
        [
            (OsString::from("HOME"), OsString::from("/fixture/home")),
            (
                OsString::from("CARGO_HOME"),
                OsString::from("/fixture/lane-cargo"),
            ),
            (
                OsString::from(bootstrap),
                OsString::from("/fixture/native-cargo"),
            ),
            (OsString::from(native), OsString::from("/fixture/bin/cargo")),
            (
                OsString::from("ARBITRARY_BUILD_INPUT"),
                OsString::from("retained"),
            ),
        ],
        Path::new("/fixture/manual-cargo"),
    );

    assert_eq!(producer, consumer);
}

#[test]
fn git_commit_metadata_normalizes_to_the_verifier_compiler_environment() {
    let producer = verifier_environment_from([
        (OsString::from("HOME"), OsString::from("/fixture/home")),
        (
            OsString::from("ARBITRARY_BUILD_INPUT"),
            OsString::from("retained"),
        ),
    ]);
    let consumer = verifier_environment_from([
        (OsString::from("HOME"), OsString::from("/fixture/home")),
        (
            OsString::from("ARBITRARY_BUILD_INPUT"),
            OsString::from("retained"),
        ),
        (
            OsString::from("GIT_AUTHOR_DATE"),
            OsString::from("@1786792967 -0500"),
        ),
        (
            OsString::from("GIT_AUTHOR_EMAIL"),
            OsString::from("gate-report@example.invalid"),
        ),
        (
            OsString::from("GIT_AUTHOR_NAME"),
            OsString::from("Gate Report Fixture"),
        ),
        (OsString::from("GIT_EDITOR"), OsString::from(":")),
        (
            OsString::from("GIT_INDEX_FILE"),
            OsString::from("/fixture/git/index"),
        ),
    ]);

    assert_eq!(producer, consumer);
}

#[test]
fn verifier_compiler_failure_names_both_bounded_output_streams() {
    let detail = verifier_compiler_failure_detail(
        ::gate::VerifierCompilerStage::NativeClippy,
        "exited",
        Some(101),
        b"compiler stdout witness",
        b"compiler stderr witness",
    );

    assert_eq!(
        detail,
        concat!(
            "stage=NativeClippy; completion=exited; code=Some(101); ",
            "stdout=compiler stdout witness; stderr=compiler stderr witness"
        )
    );
}
