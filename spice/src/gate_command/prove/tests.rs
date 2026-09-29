use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::*;

#[test]
fn admission_refreshes_a_byte_identical_index_replacement() {
    let fixture = StagedFixture::new("semantic-index-refresh");
    let repository = Repository::discover(&fixture.root).expect("discover staged fixture");
    let staged = fixture.staged(&repository);
    let expected_source = staged.selection().source().clone();
    let expected_selection = staged.selection().clone();
    let expected_tree = staged
        .anchor()
        .staged_authority()
        .expect("staged authority")
        .candidate_tree_identity()
        .to_owned();

    fixture.replace_index();
    let Ok(Some(refreshed)) =
        refresh_staged_source(&staged, &repository, &gate::ClaimCapture::Idle)
    else {
        panic!("semantic index replacement must return a fresh staged selection");
    };

    assert_eq!(refreshed.selection(), &expected_selection);
    assert_eq!(refreshed.selection().source(), &expected_source);
    assert_eq!(
        refreshed
            .anchor()
            .staged_authority()
            .expect("refreshed staged authority")
            .candidate_tree_identity(),
        expected_tree,
    );
    refreshed
        .anchor()
        .verify_source()
        .expect("refreshed staged seal is current");
}

#[test]
fn publication_validation_refuses_merge_control_movement() {
    let fixture = StagedFixture::new("merge-control-publication");
    let repository = Repository::discover(&fixture.root).expect("discover staged fixture");
    let staged = fixture.staged(&repository);
    let candidate_tree =
        spice::gate::capture_candidate_tree(&repository).expect("capture candidate tree");

    fs::write(
        fixture.root.join(".git/MERGE_MSG"),
        "arrived after proof capture\n",
    )
    .expect("create merge control after capture");

    let error = validate_source(&repository, staged.anchor(), &candidate_tree)
        .expect_err("publication validation must refuse moved merge controls");
    assert_eq!(error.reason(), "gate::source-moved");
    assert!(error.detail().contains("merge-control source moved"));
}

#[test]
fn admission_refuses_semantic_index_movement_before_publication() {
    let fixture = StagedFixture::new("semantic-index-movement");
    let repository = Repository::discover(&fixture.root).expect("discover staged fixture");
    let staged = fixture.staged(&repository);

    fixture.stage_value(3);
    let Err(error) = refresh_staged_source(&staged, &repository, &gate::ClaimCapture::Idle) else {
        panic!("changed staged content cannot retain prior proof selection");
    };

    assert_eq!(error.reason(), "gate::source-moved");
    assert!(error.detail().contains("Tree"), "{}", error.detail());
}

#[test]
fn registered_gate_lookup_is_exact() {
    assert_eq!(gate_id("formatters"), Some(GateId::Formatters));
    assert_eq!(gate_id("Formatters"), None);
    assert_eq!(gate_id("formatters "), None);
}

#[test]
fn missing_filters_only_an_action_with_a_current_receipt() {
    assert!(action_needed(false, false));
    assert!(action_needed(false, true));
    assert!(action_needed(true, false));
    assert!(!action_needed(true, true));
}

#[test]
fn complete_fingerprint_returns_inside_the_bound_without_entering_planning() {
    let authority = ActionFingerprintAuthority::new(ActionFingerprintAuthorityFields {
        candidate_tree: "0123456789abcdef0123456789abcdef01234567".to_owned(),
        baseline: None,
        source: Digest::compute(Domain::GateTree, b"source"),
        policy: Digest::compute(Domain::GatePolicy, b"policy"),
        gate_set: Digest::compute(Domain::GateSet, b"gate set"),
        tools: Digest::compute(Domain::GateTool, b"tools"),
        environment: Digest::compute(Domain::GateConfiguration, b"environment"),
        cargo_source: None,
    })
    .expect("valid fingerprint authority");
    let planned = AtomicUsize::new(0);
    let started = Instant::now();

    let Ok(resolution) = fingerprint::resolve(
        Some(&authority),
        |_| Ok(Some(Vec::new())),
        || {
            planned.fetch_add(1, Ordering::Relaxed);
            Ok((Vec::new(), Vec::new(), Vec::new()))
        },
    ) else {
        panic!("complete fingerprint must resolve");
    };

    assert!(
        matches!(resolution, fingerprint::ActionResolution::Fingerprint(actions) if actions.is_empty())
    );
    assert_eq!(planned.load(Ordering::Relaxed), 0);
    assert!(started.elapsed() < Duration::from_millis(100));

    let Ok(resolution) = fingerprint::resolve(
        Some(&authority),
        |_| Ok(None),
        || {
            planned.fetch_add(1, Ordering::Relaxed);
            Ok((Vec::new(), Vec::new(), Vec::new()))
        },
    ) else {
        panic!("missing fingerprint planning failed");
    };
    let fingerprint::ActionResolution::Planned((present, pending, expected)) = resolution else {
        panic!("missing fingerprint did not enter planning");
    };
    assert!(present.is_empty());
    assert!(pending.is_empty());
    assert!(expected.is_empty());
    assert_eq!(planned.load(Ordering::Relaxed), 1);
}

#[test]
fn cargo_selectors_are_explicit_and_keep_canonical_report_order() {
    assert!(cargo_request(Request {
        action: Some("rust-typecheck"),
        missing: false,
    }));
    assert!(cargo_request(Request {
        action: Some("cargo:test:fixture::lib:fixture"),
        missing: false,
    }));
    assert!(!cargo_request(Request {
        action: Some("formatters"),
        missing: true,
    }));
    assert_eq!(
        proof_action_order("taste", "cargo:typecheck:fixture::lib:fixture"),
        std::cmp::Ordering::Less
    );
    assert_eq!(
        proof_action_order(
            "cargo:typecheck:fixture::lib:fixture",
            "canvas-web-typecheck"
        ),
        std::cmp::Ordering::Less
    );
    assert_eq!(
        proof_action_order("private-internals", "cargo:test:fixture::lib:fixture"),
        std::cmp::Ordering::Less
    );
}

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "the Cargo planning fixture captures its own test environment"
)]
fn non_reusable_cargo_shard_is_refused_before_action_identity_preparation() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-prove-non-reusable-cargo-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    fs::create_dir_all(root.join("src")).expect("create Cargo fixture source");
    fs::write(
        root.join("Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"fixture\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "build = \"build.rs\"\n\n",
            "[package.metadata.spice.cargo-proof]\n",
            "version = 1\n",
            "complete = true\n",
        ),
    )
    .expect("write Cargo fixture manifest");
    fs::write(
        root.join("Cargo.lock"),
        concat!(
            "# This file is automatically @generated by Cargo.\n",
            "version = 4\n\n",
            "[[package]]\n",
            "name = \"fixture\"\n",
            "version = \"0.1.0\"\n",
        ),
    )
    .expect("write Cargo fixture lockfile");
    fs::write(
        root.join("build.rs"),
        "fn main() { println!(\"{}\", \"cargo::rerun-if-changed=../authority.txt\"); }\n",
    )
    .expect("write Cargo fixture build script");
    fs::write(root.join("src/lib.rs"), "pub const VALUE: u8 = 1;\n")
        .expect("write Cargo fixture library");
    let environment =
        Environment::new(std::env::vars_os()).expect("ambient test environment is valid");
    let plan = gate::plan_cargo_gate_shards_at(CargoGateShardPlanning {
        root: &root,
        base: None,
        source: Digest::compute(Domain::GateTree, b"fixture"),
        changed_sources: &["src/lib.rs".to_owned()],
        selected_paths: &[
            "Cargo.lock".to_owned(),
            "Cargo.toml".to_owned(),
            "build.rs".to_owned(),
            "src/lib.rs".to_owned(),
        ],
        environment,
        cargo: OsStr::new("cargo"),
        external_source: None,
        include_tests: false,
    })
    .expect("plan non-reusable Cargo fixture");
    let shard = plan
        .typechecks()
        .first()
        .expect("fixture has a typecheck shard");

    let Err(ProveFailure::Refusal { reason, detail }) = require_reusable_cargo_shard(shard) else {
        panic!("non-reusable shard must be refused before identity preparation");
    };
    assert_eq!(reason, "gate::prove-cargo-action-not-reusable");
    assert!(detail.contains("computed or interpolated compile-time output"));
    fs::remove_dir_all(root).expect("remove Cargo planning fixture");
}

#[test]
fn cargo_environment_omits_worktree_outputs_and_matches_forced_wrappers() {
    let normalized = normalized_action_environment(&[
        (OsString::from("PWD"), OsString::from("/worktree-a")),
        (
            OsString::from("CARGO_TARGET_DIR"),
            OsString::from("/worktree-a/target"),
        ),
        (
            OsString::from("CARGO_BUILD_BUILD_DIR"),
            OsString::from("/worktree-a/build"),
        ),
        (OsString::from("RUSTC_WRAPPER"), OsString::from("sccache")),
        (
            OsString::from(wire::SpiceVariable::Lane.as_str()),
            OsString::from("lane-a"),
        ),
        (
            OsString::from(wire::SpiceVariable::PreCommitSource.as_str()),
            OsString::from("staged-source-a"),
        ),
        (
            OsString::from(wire::SpiceVariable::WrapperSpice.as_str()),
            OsString::from("/worktree-a/.spice-install/bin/spice"),
        ),
        (
            OsString::from("CARGO_HOME"),
            OsString::from("/worktree-a/private-cargo"),
        ),
        (OsString::from("RUSTFLAGS"), OsString::from("--cfg=proof")),
        (
            OsString::from(wire::SpiceVariable::NativeCargo.as_str()),
            OsString::from("/toolchain/cargo"),
        ),
    ]);
    assert_eq!(
        normalized,
        [
            (OsString::from("RUSTC_WORKSPACE_WRAPPER"), OsString::new()),
            (OsString::from("RUSTC_WRAPPER"), OsString::new()),
            (OsString::from("RUSTFLAGS"), OsString::from("--cfg=proof")),
            (
                OsString::from(wire::SpiceVariable::NativeCargo.as_str()),
                OsString::from("/toolchain/cargo"),
            ),
        ]
    );
    assert_eq!(
        normalized,
        normalized_action_environment(&[
            (
                OsString::from("CARGO_HOME"),
                OsString::from("/worktree-b/another-private-cargo"),
            ),
            (
                OsString::from(wire::SpiceVariable::Lane.as_str()),
                OsString::from("lane-b"),
            ),
            (
                OsString::from(wire::SpiceVariable::Hooks.as_str()),
                OsString::from("/worktree-b/.spice-install/hooks"),
            ),
            (
                OsString::from(wire::SpiceVariable::PreCommitPaths.as_str()),
                OsString::from("[\"different.rs\"]"),
            ),
            (OsString::from("RUSTFLAGS"), OsString::from("--cfg=proof")),
            (
                OsString::from(wire::SpiceVariable::NativeCargo.as_str()),
                OsString::from("/toolchain/cargo"),
            ),
        ]),
        "private locations and Spice routing must not partition shared receipt identity",
    );

    let Ok((_, first)) = environment(&normalized) else {
        panic!("digest first Cargo environment");
    };
    let drifted = normalized_action_environment(&[
        (
            OsString::from("FIXTURE_BUILD_INPUT"),
            OsString::from("changed"),
        ),
        (OsString::from("RUSTFLAGS"), OsString::from("--cfg=proof")),
    ]);
    let Ok((_, second)) = environment(&drifted) else {
        panic!("digest drifted Cargo environment");
    };
    assert_ne!(first, second);
}

#[test]
fn a_repeated_search_path_entry_does_not_partition_environment_identity() {
    let normalized =
        |path| normalized_action_environment(&[(OsString::from("PATH"), search_path(path))]);
    let once = normalized(&["/install/bin", "/toolchain/bin", "/usr/bin"]);
    let twice = normalized(&["/install/bin", "/toolchain/bin", "/install/bin", "/usr/bin"]);

    assert_eq!(once, twice);
    assert_eq!(
        once[0].1,
        search_path(&["/install/bin", "/toolchain/bin", "/usr/bin"])
    );
    assert_ne!(
        once,
        normalized(&["/toolchain/bin", "/install/bin", "/usr/bin"])
    );
}

fn search_path(paths: &[&str]) -> OsString {
    std::env::join_paths(paths.iter().map(Path::new)).expect("fixture PATH composes")
}

#[test]
fn interrupted_execution_retains_prior_publication_and_cleans_up() {
    let events = RefCell::new(Vec::new());
    let result: Result<Vec<u8>, &str> = complete_actions(
        vec![1_u8, 2, 3],
        |action| {
            events.borrow_mut().push(format!("execute-{action}"));
            if action == 2 {
                Err("interrupted")
            } else {
                Ok(action)
            }
        },
        || {
            events.borrow_mut().push("validate".to_owned());
            Ok(())
        },
        || {
            events.borrow_mut().push("cleanup".to_owned());
            Ok(())
        },
        |receipt| {
            events.borrow_mut().push(format!("publish-{receipt}"));
            Ok(receipt)
        },
    );

    assert_eq!(result, Err("interrupted"));
    assert_eq!(
        events.into_inner(),
        ["execute-1", "validate", "publish-1", "execute-2", "cleanup",]
    );
}

#[test]
fn source_validation_precedes_each_opportunistic_publication() {
    let events = RefCell::new(Vec::new());
    let result = complete_actions(
        vec![1_u8, 2],
        |action| {
            events.borrow_mut().push(format!("execute-{action}"));
            Ok::<_, &str>(action)
        },
        || {
            events.borrow_mut().push("validate".to_owned());
            Ok(())
        },
        || {
            events.borrow_mut().push("cleanup".to_owned());
            Ok(())
        },
        |receipt| {
            events.borrow_mut().push(format!("publish-{receipt}"));
            Ok(receipt)
        },
    );

    assert_eq!(result, Ok(vec![1, 2]));
    assert_eq!(
        events.into_inner(),
        [
            "execute-1",
            "validate",
            "publish-1",
            "execute-2",
            "validate",
            "publish-2",
            "cleanup",
        ]
    );
}

#[test]
fn later_publication_failure_retains_the_prior_receipt() {
    let published = RefCell::new(Vec::new());
    let result: Result<Vec<u8>, &str> = complete_actions(
        vec![1_u8, 2],
        Ok::<_, &str>,
        || Ok(()),
        || Ok(()),
        |receipt| {
            if receipt == 2 {
                return Err("later publication failed");
            }
            published.borrow_mut().push(receipt);
            Ok(receipt)
        },
    );

    assert_eq!(result, Err("later publication failed"));
    assert_eq!(published.into_inner(), [1]);
}

struct StagedFixture {
    root: PathBuf,
}

impl StagedFixture {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-prove-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(root.join("src")).expect("create staged fixture");
        git(&root, &["init", "--initial-branch=main"]);
        git(&root, &["config", "user.name", "Spice Test"]);
        git(&root, &["config", "user.email", "spice@example.invalid"]);
        fs::write(
            root.join("spice-rust.toml"),
            format!("{}[gate.walk]\nskip = []\n", gate::NARROWEST_FILE_CLASSES),
        )
        .expect("write staged gate policy");
        fs::write(root.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n")
            .expect("write staged baseline");
        git(&root, &["add", "--all"]);
        git(&root, &["commit", "-m", "baseline"]);
        let fixture = Self { root };
        fixture.stage_value(2);
        fixture
    }

    fn staged(&self, repository: &Repository) -> StagedActionSelection {
        let anchor = spice::gate::prepare_arm_at_with_claim(
            repository.worktree_root(),
            gate::Selector::staged(),
            GateId::Staging,
            gate::ClaimCapture::Idle,
        )
        .expect("capture staged fixture");
        spice::change_gate::prepare_staged_actions_from_anchor(anchor)
            .expect("select staged fixture actions")
    }

    fn stage_value(&self, value: u8) {
        fs::write(
            self.root.join("src/lib.rs"),
            format!("pub fn value() -> u8 {{ {value} }}\n"),
        )
        .expect("write staged value");
        git(&self.root, &["add", "--all"]);
    }

    fn replace_index(&self) {
        let index = self.root.join(".git/index");
        let replacement = self.root.join(".git/index.replacement");
        fs::copy(&index, &replacement).expect("copy staged index");
        fs::rename(replacement, index).expect("replace staged index");
    }
}

impl Drop for StagedFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove staged fixture");
    }
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("run fixture Git command");
    assert!(
        output.status.success(),
        "Git failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
