#![cfg(unix)]

use std::env;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use gate::{
    BatteryReceipt, GateId, Selector, VerifierCompilerEvidence, VerifierCompilerProcessEvidence,
};
use spice::gate::{
    FullVerifierEvidence, FullVerifierEvidenceAdmission, FullVerifierEvidenceStage,
    PreparedBattery, SemanticFullVerifierAdmission, admit_full_verifier_evidence,
    admit_merge_parent_full_verifier_evidence, admit_semantic_full_verifier_evidence,
    inspect_full_verifier_evidence, prepare_strict,
};
use spice::repository::Repository;
use wire::{Digest, Domain};

const ARCHIVE_BUNDLE_NAME_BYTES: usize = 64 + ".json".len();
const ARCHIVE_CONTAINER_MAGIC: &[u8; 8] = b"SPVARC\0\0";
const ARCHIVE_CONTAINER_VERSION: u32 = 1;
const ARCHIVE_CONTAINER_HEADER_BYTES: usize = ARCHIVE_CONTAINER_MAGIC.len() + 4 + 8 + 8 + 32;

#[test]
fn full_verifier_retries_stale_state_and_crosses_the_complete_evidence_lifecycle() {
    let fixture = Fixture::new();
    fixture.write_repository();
    fixture.write_tools();

    let output = Command::new(env::current_exe().expect("resolve integration-test executable"))
        .args([
            "--ignored",
            "--exact",
            "semantic_lifecycle_fixture_process",
            "--nocapture",
        ])
        .current_dir(&fixture.repository)
        .env("PATH", fixture.search_path())
        .env_remove("SPICE_RS_BOOTSTRAP_CARGO_HOME") // env-policy: allow
        .env_remove("SPICE_RS_NATIVE_CARGO") // env-policy: allow
        .output()
        .expect("launch isolated semantic lifecycle fixture");

    assert!(
        output.status.success(),
        "semantic lifecycle fixture failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn admitted_parent_archives_compose_one_normal_two_parent_commit() {
    let fixture = Fixture::new();
    fixture.write_repository();
    fixture.write_tools();

    let output = Command::new(env::current_exe().expect("resolve integration-test executable"))
        .args([
            "--ignored",
            "--exact",
            "merge_parent_fixture_process",
            "--nocapture",
        ])
        .current_dir(&fixture.repository)
        .env("PATH", fixture.search_path())
        .env_remove("SPICE_RS_BOOTSTRAP_CARGO_HOME") // env-policy: allow
        .env_remove("SPICE_RS_NATIVE_CARGO") // env-policy: allow
        .output()
        .expect("launch isolated merge-parent fixture");

    assert!(
        output.status.success(),
        "merge-parent fixture failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn concurrent_linked_worktree_state_receipts_compose_and_remain_source_bound() {
    let fixture = Fixture::new();
    fixture.write_repository();
    fixture.write_tools();

    let output = Command::new(env::current_exe().expect("resolve integration-test executable"))
        .args([
            "--ignored",
            "--exact",
            "concurrent_state_receipt_fixture_process",
            "--nocapture",
        ])
        .current_dir(&fixture.repository)
        .env("PATH", fixture.search_path())
        .env_remove("SPICE_RS_BOOTSTRAP_CARGO_HOME") // env-policy: allow
        .env_remove("SPICE_RS_NATIVE_CARGO") // env-policy: allow
        .output()
        .expect("launch isolated concurrent-state fixture");

    assert!(
        output.status.success(),
        "concurrent-state fixture failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore = "subprocess fixture with isolated cwd and compiler environment"]
fn concurrent_state_receipt_fixture_process() {
    let left = env::current_dir().expect("fixture current directory");
    let right = configure_concurrent_state_fixture(&left);
    bind_operational_generation("left-generation");
    let (left_repository, left_prepared, left_compiler, left_battery) =
        capture_concurrent_verifier(&left);
    bind_operational_generation("right-generation");
    let (right_repository, right_prepared, right_compiler, right_battery) =
        capture_concurrent_verifier(&right);
    assert_ne!(left_prepared.source(), right_prepared.source());

    let left_evidence = publish_concurrent_verifier(
        &left,
        &left_repository,
        &left_prepared,
        &left_compiler,
        &left_battery,
    );
    publish_concurrent_verifier(
        &right,
        &right_repository,
        &right_prepared,
        &right_compiler,
        &right_battery,
    );
    assert_composed_claims(&right);
    assert_retained_evidence_is_source_bound(
        &left,
        &right,
        &left_repository,
        &right_repository,
        &left_evidence,
    );
    env::set_current_dir(&left).expect("return to fixture root for cleanup");
}

fn bind_operational_generation(generation: &str) {
    // SAFETY: the fixture runs as the sole exact test in a private subprocess;
    // no sibling thread reads or writes these process variables.
    unsafe {
        env::set_var("SPICE_COMMAND_PLAN_EXECUTION_DIGEST", generation); // env-policy: allow
        env::set_var("SPICE_RUNTIME_PYTHON", format!("/{generation}/python")); // env-policy: allow
        env::set_var("SPICE_SHELL_HOOK_INSTALLED_APPLIANCE", generation); // env-policy: allow
    }
}

fn configure_concurrent_state_fixture(left: &Path) -> PathBuf {
    let policy_path = left.join("spice-rust.toml");
    let policy = fs::read_to_string(&policy_path).expect("read fixture policy");
    fs::write(
        &policy_path,
        policy.replace(
            "docs = [\"AGENTS.md\"]",
            "docs = [\"LEFT.md\", \"RIGHT.md\"]",
        ),
    )
    .expect("select two independently bounded documents");
    fs::write(left.join("LEFT.md"), "left baseline\n").expect("write left baseline");
    fs::write(left.join("RIGHT.md"), "right baseline\n").expect("write right baseline");
    git(left, &["add", "spice-rust.toml", "LEFT.md", "RIGHT.md"]);
    git(left, &["commit", "-qm", "concurrent state baseline"]);

    let right = fixture_peer(left, "concurrent-right");
    git(
        left,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "concurrent-right",
            right.to_str().expect("right worktree path is text"),
            "HEAD",
        ],
    );
    fs::write(left.join("LEFT.md"), "l".repeat(11_000)).expect("grow left document over base");
    git(left, &["add", "LEFT.md"]);
    fs::write(right.join("RIGHT.md"), "r".repeat(11_000)).expect("grow right document over base");
    git(&right, &["add", "RIGHT.md"]);
    right
}

fn capture_concurrent_verifier(
    worktree: &Path,
) -> (
    Repository,
    PreparedBattery<'static>,
    Vec<VerifierCompilerEvidence>,
    BatteryReceipt,
) {
    env::set_current_dir(worktree).expect("enter concurrent worktree");
    let repository = Repository::discover(worktree).expect("discover concurrent repository");
    let mut prepared = prepare_strict(Selector::staged()).expect("prepare concurrent source");
    let compiler = compiler_evidence(&mut prepared);
    let battery = prepared
        .execute_prepared_compiler_receipt(&compiler, 1)
        .expect("execute concurrent battery");
    assert!(battery.run().admits());
    (repository, prepared, compiler, battery)
}

fn publish_concurrent_verifier(
    worktree: &Path,
    repository: &Repository,
    prepared: &PreparedBattery<'_>,
    compiler: &[VerifierCompilerEvidence],
    battery: &BatteryReceipt,
) -> FullVerifierEvidence {
    env::set_current_dir(worktree).expect("enter publishing worktree");
    let evidence = FullVerifierEvidence::mint(
        repository,
        1,
        prepared.source(),
        battery,
        compiler,
        prepared,
    )
    .expect("publish captured state transaction");
    evidence
        .publish(repository)
        .expect("publish verifier evidence");
    evidence
}

fn assert_composed_claims(right: &Path) {
    let common_git = PathBuf::from(git_output(right, &["rev-parse", "--git-common-dir"]));
    let common_git = if common_git.is_absolute() {
        common_git
    } else {
        right.join(common_git)
    };
    let state = gate::CurrentState::load(gate::current_state_path(&common_git))
        .expect("load composed ledger")
        .before()
        .clone();
    assert_eq!(
        state
            .claims()
            .iter()
            .map(gate::PeerClaim::path)
            .collect::<Vec<_>>(),
        ["LEFT.md", "RIGHT.md"],
    );
}

fn assert_retained_evidence_is_source_bound(
    left: &Path,
    right: &Path,
    left_repository: &Repository,
    right_repository: &Repository,
    left_evidence: &FullVerifierEvidence,
) {
    env::set_current_dir(left).expect("admit retained left evidence");
    assert!(
        admit_full_verifier_evidence(left_repository, Selector::staged())
            .expect("admit left evidence after right publication")
            .is_some(),
    );
    env::set_current_dir(right).expect("admit retained right evidence");
    assert!(
        admit_full_verifier_evidence(right_repository, Selector::staged())
            .expect("admit right evidence after composition")
            .is_some(),
    );

    left_evidence
        .publish(right_repository)
        .expect("copy foreign-source evidence into right worktree");
    let admission = inspect_full_verifier_evidence(right_repository, Selector::staged())
        .expect("probe foreign-source evidence");
    let FullVerifierEvidenceAdmission::Unavailable(miss) = admission else {
        panic!("state composition must not weaken exact staged-source admission");
    };
    assert_eq!(miss.stage(), FullVerifierEvidenceStage::Source);
}

#[test]
#[ignore = "subprocess fixture with isolated cwd and compiler environment"]
fn merge_parent_fixture_process() {
    let root = env::current_dir().expect("fixture current directory");
    make_static_compiler_baseline(&root);
    let base = create_admitted_left_parent(&root);
    let right_head = create_admitted_right_parent(&root, &base);
    let repository = open_parent_merge(&root, &right_head);

    assert_initial_merge_parent_reuse(&repository);
    assert_forward_migration_preserves_merge_admission(&repository);
    assert_malformed_merge_receipt_fails_closed(&repository);
    assert_resolution_input_invalidates_compilers(&root, &repository);
    assert_protected_resolution_executes_fresh(&root, &repository);
    assert_selected_resolution_reuses_and_commits(&root);
}

fn make_static_compiler_baseline(root: &Path) {
    let manifest =
        fs::read_to_string(root.join("app/Cargo.toml")).expect("read merge fixture manifest");
    fs::write(
        root.join("app/Cargo.toml"),
        manifest.replace("build = \"build.rs\"\n", ""),
    )
    .expect("remove dynamic compiler boundary");
    git(root, &["rm", "-q", "app/build.rs", "app/schema.json"]);
    git(root, &["add", "app/Cargo.toml"]);
    git(root, &["commit", "-qm", "static compiler baseline"]);
}

fn create_admitted_left_parent(root: &Path) -> String {
    git(root, &["checkout", "-qb", "left"]);
    fs::write(
        root.join("AGENTS.md"),
        "# Fixture\n\nLeft parent documentation.\n",
    )
    .expect("write left parent source");
    git(root, &["add", "AGENTS.md"]);
    let left_repository = Repository::discover(root).expect("discover left repository");
    publish_producer_evidence(&left_repository);
    git(root, &["commit", "-qm", "left parent"]);
    git_output(root, &["rev-parse", "HEAD~1"])
}

fn create_admitted_right_parent(root: &Path, base: &str) -> String {
    let right = fixture_peer(root, "right");
    git(
        root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "right",
            right.to_str().expect("right worktree path is text"),
            base,
        ],
    );
    env::set_current_dir(&right).expect("enter right worktree");
    let browser = right.join("app/browser/lifecycle.mjs");
    let source = fs::read_to_string(&browser).expect("read right browser source");
    fs::write(&browser, format!("// right parent\n{source}")).expect("write right parent source");
    git(&right, &["add", "app/browser/lifecycle.mjs"]);
    let right_repository = Repository::discover(&right).expect("discover right repository");
    publish_producer_evidence(&right_repository);
    git(&right, &["commit", "-qm", "right parent"]);
    let right_head = git_output(&right, &["rev-parse", "HEAD"]);
    env::set_current_dir(root).expect("return to left worktree");
    right_head
}

fn open_parent_merge(root: &Path, right_head: &str) -> Repository {
    git(root, &["merge", "--no-ff", "--no-commit", right_head]);
    let repository = Repository::discover(root).expect("rediscover merge repository");
    assert!(repository.git_dir().join("MERGE_HEAD").is_file());
    repository
}

fn assert_initial_merge_parent_reuse(repository: &Repository) {
    assert!(
        admit_full_verifier_evidence(repository, Selector::staged())
            .expect("probe ordinary exact merge evidence")
            .is_none(),
        "a merged source must not become exact evidence through parent composition",
    );

    let admitted = admitted_merge(repository);
    assert!(admitted.reusable_gate_ids().contains(&GateId::RepoDocs));
    assert!(
        admitted
            .reusable_gate_ids()
            .contains(&GateId::CanvasWebTypecheck),
        "reusable={:?} invalidated={:?} diagnostics={:?}",
        admitted.reusable_gate_ids(),
        admitted.invalidated_gate_ids(),
        admitted.diagnostics(),
    );
    assert!(
        admitted
            .reusable_gate_ids()
            .contains(&GateId::RustTypecheck),
    );
}

fn assert_forward_migration_preserves_merge_admission(repository: &Repository) {
    let before = admitted_merge(repository);
    let archive_root = repository
        .admit_state()
        .expect("admit migration fixture state")
        .full_verifier_evidence_archive()
        .to_path_buf();
    for entry in fs::read_dir(&archive_root).expect("list migration fixture archive") {
        let path = entry.expect("migration archive entry").path();
        let is_bundle = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.len() == ARCHIVE_BUNDLE_NAME_BYTES && name.ends_with(".json"));
        if !is_bundle {
            continue;
        }
        let stored = fs::read(&path).expect("read current migration fixture bundle");
        fs::write(&path, compress_raw_gzip(&expand_archived_bundle(&stored)))
            .expect("restore recognized raw-gzip predecessor bundle");
    }
    fs::remove_file(archive_root.join("FORMAT")).expect("remove current migration marker");

    let after = admitted_merge(repository);
    assert_eq!(before.reusable_gate_ids(), after.reusable_gate_ids());
    assert_eq!(before.invalidated_gate_ids(), after.invalidated_gate_ids());
    assert_eq!(before.diagnostics(), after.diagnostics());
    assert_eq!(
        fs::read(archive_root.join("FORMAT")).expect("read republished migration marker"),
        b"spice/full-verifier-archive/v1\n",
    );
}

fn assert_malformed_merge_receipt_fails_closed(repository: &Repository) {
    let archive = fs::read_dir(
        repository
            .admit_state()
            .expect("admit merge fixture state")
            .full_verifier_evidence_archive(),
    )
    .expect("read merge parent archive")
    .find(|entry| {
        entry.as_ref().is_ok_and(|entry| {
            entry.file_name().to_str().is_some_and(|name| {
                name.len() == ARCHIVE_BUNDLE_NAME_BYTES && name.ends_with(".json")
            })
        })
    })
    .expect("one parent archive")
    .expect("read parent archive entry")
    .path();
    let stored = fs::read(&archive).expect("read archived parent bundle");
    let original = expand_archived_bundle(&stored);
    let marker = b"\"compiler\":\"{";
    let offset = original
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("locate merge compiler receipt")
        + marker.len()
        - 1;
    let mut malformed = original;
    malformed[offset] = b'x';
    fs::write(&archive, compress_archived_bundle(&malformed))
        .expect("malform merge compiler receipt");
    assert!(matches!(
        admit_merge_parent_full_verifier_evidence(repository)
            .expect("probe malformed merge compiler receipt"),
        SemanticFullVerifierAdmission::Unavailable { .. },
    ));
    fs::write(&archive, stored).expect("restore merge compiler receipt");
}

/// Archived bundles use the version-one integrity-checked gzip container. The
/// malformed-receipt probe must reseal its changed plain bytes so admission
/// exercises the receipt decoder rather than stopping at container integrity.
fn expand_archived_bundle(stored: &[u8]) -> Vec<u8> {
    use std::io::Read as _;
    assert_eq!(
        &stored[..ARCHIVE_CONTAINER_MAGIC.len()],
        ARCHIVE_CONTAINER_MAGIC,
    );
    let mut plain = Vec::new();
    flate2::read::GzDecoder::new(&stored[ARCHIVE_CONTAINER_HEADER_BYTES..])
        .read_to_end(&mut plain)
        .expect("expand archived parent bundle");
    plain
}

fn compress_archived_bundle(plain: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(plain)
        .expect("compress malformed parent bundle");
    let payload = encoder.finish().expect("finish malformed parent bundle");
    let mut stored = Vec::with_capacity(ARCHIVE_CONTAINER_HEADER_BYTES + payload.len());
    stored.extend_from_slice(ARCHIVE_CONTAINER_MAGIC);
    stored.extend_from_slice(&ARCHIVE_CONTAINER_VERSION.to_be_bytes());
    stored.extend_from_slice(
        &u64::try_from(plain.len())
            .expect("plain fixture length fits u64")
            .to_be_bytes(),
    );
    stored.extend_from_slice(
        &u64::try_from(payload.len())
            .expect("stored fixture length fits u64")
            .to_be_bytes(),
    );
    stored.extend_from_slice(blake3::hash(plain).as_bytes());
    stored.extend_from_slice(&payload);
    stored
}

fn compress_raw_gzip(plain: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(plain)
        .expect("compress predecessor parent bundle");
    encoder.finish().expect("finish predecessor parent bundle")
}

fn assert_resolution_input_invalidates_compilers(root: &Path, repository: &Repository) {
    fs::write(
        root.join("app/src/lib.rs"),
        "pub fn app_fixture() {}\npub fn resolution_only() {}\n",
    )
    .expect("write resolution-only compiler input");
    git(root, &["add", "app/src/lib.rs"]);
    let resolved = admitted_merge(repository);
    assert!(
        resolved
            .invalidated_gate_ids()
            .contains(&GateId::RustTypecheck),
    );
    assert!(
        resolved
            .invalidated_gate_ids()
            .contains(&GateId::CanvasWebTypecheck),
    );
    git(root, &["checkout", "HEAD", "--", "app/src/lib.rs"]);
}

fn assert_protected_resolution_executes_fresh(root: &Path, repository: &Repository) {
    let policy =
        fs::read_to_string(root.join("spice-rust.toml")).expect("read protected policy source");
    fs::write(
        root.join("spice-rust.toml"),
        policy.replace(
            "generated_paths = []",
            "generated_paths = [\"generated/**\"]",
        ),
    )
    .expect("write protected resolution source");
    git(root, &["add", "spice-rust.toml"]);
    match admit_merge_parent_full_verifier_evidence(repository)
        .expect("probe protected resolution authority")
    {
        SemanticFullVerifierAdmission::Admitted(admitted) => {
            assert!(admitted.reusable_gate_ids().is_empty());
            assert_eq!(admitted.invalidated_gate_ids(), GateId::BATTERY);
        }
        SemanticFullVerifierAdmission::Unavailable { .. } => {}
    }
    let mut protected_progress = Vec::new();
    let protected = spice::change_gate::run_resolution(&mut protected_progress)
        .expect("run protected merge resolution gate");
    assert!(protected.admits());
    assert_eq!(
        protected.selection().gate_ids(),
        protected.selection().applicable_gate_ids(),
    );
    let protected_progress =
        String::from_utf8(protected_progress).expect("protected progress is UTF-8");
    assert!(protected_progress.contains("disposition=admitted"));
    assert!(!protected_progress.contains("disposition=reused"));
    assert!(!protected_progress.contains("disposition=fresh-exhaustive"));
    git(root, &["checkout", "HEAD", "--", "spice-rust.toml"]);
}

fn assert_selected_resolution_reuses_and_commits(root: &Path) {
    let mut progress = Vec::new();
    let report = spice::change_gate::run_resolution(&mut progress)
        .expect("run selected merge resolution gate");
    assert!(report.admits(), "selected merge resolution must admit");
    let progress = String::from_utf8(progress).expect("merge progress is UTF-8");
    assert!(
        progress.contains("gate=javascript-unused disposition=reused"),
        "right-parent selected evidence was not reused:\n{progress}",
    );

    git(root, &["commit", "-qm", "normal admitted merge"]);
    assert_eq!(
        git_output(root, &["rev-list", "--parents", "-n", "1", "HEAD"])
            .split_whitespace()
            .count(),
        3,
        "the admitted resolution must become one normal two-parent commit",
    );
}

fn admitted_merge(repository: &Repository) -> Box<spice::gate::AdmittedSemanticVerifierEvidence> {
    match admit_merge_parent_full_verifier_evidence(repository)
        .expect("compare both admitted merge parents")
    {
        SemanticFullVerifierAdmission::Admitted(admitted) => admitted,
        SemanticFullVerifierAdmission::Unavailable { detail } => {
            panic!("merge parents did not compose: {detail}")
        }
    }
}

#[test]
#[ignore = "subprocess fixture with isolated cwd and compiler environment"]
fn semantic_lifecycle_fixture_process() {
    let root = env::current_dir().expect("fixture current directory");
    stage_protected_source_change(&root);
    let repository = Repository::discover(&root).expect("discover fixture repository");
    let evidence = publish_producer_evidence_after_state_disappears(&repository);
    assert_source_gate_reuses_complete_evidence(&root);
    assert_pre_commit_reuses_complete_evidence(&root);

    reject_cross_worktree_reuse(&root, &evidence);
    reject_compiler_contract_drift(&repository);

    fs::write(root.join("app/schema.json"), "{\"version\":2}\n")
        .expect("move tracked build-script input");
    git(&root, &["add", "app/schema.json"]);
    git(&root, &["commit", "-qm", "move compiler input"]);
    let app = fs::read_to_string(root.join("app/browser/lifecycle.mjs"))
        .expect("read frontend current-source input");
    fs::write(
        root.join("app/browser/lifecycle.mjs"),
        format!("// a syntax-neutral current-source change\n{app}"),
    )
    .expect("write current-source change");
    git(&root, &["add", "app/browser/lifecycle.mjs"]);

    let admitted = match admit_semantic_full_verifier_evidence(&repository, Selector::staged())
        .expect("compare the later source commit")
    {
        SemanticFullVerifierAdmission::Admitted(admitted) => admitted,
        SemanticFullVerifierAdmission::Unavailable { detail } => {
            panic!("later source did not admit semantic evidence: {detail}")
        }
    };
    assert!(
        admitted
            .invalidated_gate_ids()
            .contains(&GateId::RustTypecheck),
    );
    assert!(
        admitted
            .invalidated_gate_ids()
            .contains(&GateId::CanvasWebTypecheck),
    );
    assert!(admitted.reusable_gate_ids().contains(&GateId::Complexity));
    assert!(!admitted.reusable_gate_ids().is_empty());
    assert!(!admitted.invalidated_gate_ids().is_empty());
    assert!(
        admitted
            .diagnostics()
            .iter()
            .any(|detail| detail.contains("Cargo/rustc-discovered dependency evidence")),
    );

    let cargo_log = fixture_peer(&root, "cargo.log");
    let trunk_log = fixture_peer(&root, "trunk.log");
    fs::write(&cargo_log, "").expect("clear Cargo execution log");
    fs::write(&trunk_log, "").expect("clear Trunk execution log");
    let mut progress = Vec::new();
    let report = spice::change_gate::run_current_source(&mut progress)
        .expect("run the production current-source change gate");
    assert!(report.admits(), "current-source execution must admit");
    let progress = String::from_utf8(progress).expect("progress is UTF-8");
    assert!(
        progress.contains("gate=complexity disposition=reused"),
        "reusable arm did not cross the production plan:\n{progress}",
    );
    assert!(
        progress.contains("gate=canvas-web-typecheck disposition=executed"),
        "invalidated Canvas arm did not execute:\n{progress}",
    );
    let cargo = fs::read_to_string(cargo_log).expect("read Cargo execution log");
    assert_eq!(
        cargo
            .lines()
            .filter(|line| line.contains("check --locked --package app"))
            .count(),
        1,
        "Canvas current-source compiler must execute exactly once:\n{cargo}",
    );
    let trunk = fs::read_to_string(trunk_log).expect("read Trunk execution log");
    assert_eq!(
        trunk
            .lines()
            .filter(|line| line.contains("build index.html --release --locked --filehash=false"))
            .count(),
        1,
        "Canvas current-source build must execute exactly once:\n{trunk}",
    );
}

fn stage_protected_source_change(root: &Path) {
    fs::write(
        root.join("app/src/lib.rs"),
        "pub fn app_fixture() {\n    let _protected_source = true;\n}\n",
    )
    .expect("write protected staged source change");
    git(root, &["add", "app/src/lib.rs"]);
}

fn assert_source_gate_reuses_complete_evidence(root: &Path) {
    let cargo_log = fixture_peer(root, "cargo.log");
    let trunk_log = fixture_peer(root, "trunk.log");
    fs::write(&cargo_log, "").expect("clear Cargo execution log");
    fs::write(&trunk_log, "").expect("clear Trunk execution log");
    let output = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args(["gate", "selection"])
        .current_dir(root)
        .output()
        .expect("launch staged-source selection leaf");
    assert!(
        output.status.success(),
        "staged-source selection refused exact evidence: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    let cargo = fs::read_to_string(cargo_log).expect("read Cargo execution log");
    assert!(
        cargo.lines().all(|line| line == "--version"),
        "staged-source selection fell through to focused Cargo execution:\n{cargo}",
    );
    let trunk = fs::read_to_string(trunk_log).expect("read Trunk execution log");
    assert!(
        trunk.lines().all(|line| line == "--version"),
        "staged-source selection fell through to focused Trunk execution:\n{trunk}",
    );
}

fn assert_pre_commit_reuses_complete_evidence(root: &Path) {
    let progress = fixture_peer(root, "pre-commit-progress.jsonl");
    let terminal = fixture_peer(root, "pre-commit-terminal.bin");
    fs::write(&progress, "").expect("create pre-commit progress fixture");
    fs::write(&terminal, "").expect("create pre-commit terminal fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args([
            "__gate-pre-commit-worker",
            progress.to_str().expect("progress path is text"),
            terminal.to_str().expect("terminal path is text"),
            r#"{"version":1}"#,
        ])
        .current_dir(root)
        .output()
        .expect("launch exact pre-commit worker");
    assert!(
        output.status.success(),
        "pre-commit worker failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let terminal = fs::read(&terminal).expect("read pre-commit terminal fixture");
    let Some((kind, document)) = terminal.split_first() else {
        panic!("pre-commit worker published no terminal");
    };
    assert_eq!(*kind, b'R', "pre-commit worker refused complete evidence");
    let report: serde_json::Value =
        serde_json::from_slice(document).expect("decode pre-commit report");
    assert!(
        report["full_verifier"].is_object(),
        "pre-commit worker fell through instead of reusing complete evidence: {report}",
    );
    assert!(
        report.get("packaged").is_none(),
        "complete evidence must replace packaged execution: {report}",
    );
}

fn publish_producer_evidence(repository: &Repository) -> FullVerifierEvidence {
    publish_producer_evidence_with_state_removal(repository, None)
}

fn publish_producer_evidence_after_state_disappears(
    repository: &Repository,
) -> FullVerifierEvidence {
    let state = gate::current_state_path(repository.common_dir());
    fs::create_dir_all(state.parent().expect("gate-state parent"))
        .expect("create full-verifier gate-state parent");
    fs::write(&state, b"{\"version\":1,\"latches\":[],\"claims\":[]}")
        .expect("seed present full-verifier gate state");
    publish_producer_evidence_with_state_removal(repository, Some(&state))
}

fn publish_producer_evidence_with_state_removal(
    repository: &Repository,
    disappearing_state: Option<&Path>,
) -> FullVerifierEvidence {
    let mut prepared = prepare_strict(Selector::staged()).expect("prepare producer source");
    let compiler = compiler_evidence(&mut prepared);
    let battery = prepared
        .execute_prepared_compiler_receipt(&compiler, 1)
        .expect("execute producer battery from exact compiler evidence");
    assert!(
        battery.run().admits(),
        "producer battery must be green: {:?}",
        battery
            .run()
            .results()
            .iter()
            .filter(|result| !result.state().admits())
            .map(|result| {
                (
                    result.gate_id(),
                    result.state(),
                    result.reason(),
                    result.findings(),
                )
            })
            .collect::<Vec<_>>(),
    );
    let unintended = Digest::compute(Domain::GateReceipt, b"unannounced staged source");
    let mismatch =
        FullVerifierEvidence::mint(repository, 1, &unintended, &battery, &compiler, &prepared)
            .expect_err("an unannounced source cannot mint full-verifier evidence");
    assert!(
        mismatch.contains("differs from admitted battery source"),
        "unexpected announcement mismatch: {mismatch}",
    );
    if let Some(state) = disappearing_state {
        fs::remove_file(state).expect("remove captured full-verifier gate state");
    }
    let evidence = FullVerifierEvidence::mint(
        repository,
        1,
        prepared.source(),
        &battery,
        &compiler,
        &prepared,
    )
    .expect("mint complete full-verifier evidence");
    if let Some(state) = disappearing_state {
        assert!(
            state.is_file(),
            "fresh full-verifier retry anchor did not republish gate state",
        );
    }
    let encoded = evidence.encode().expect("serialize complete evidence");
    assert!(!encoded.is_empty());
    evidence
        .publish(repository)
        .expect("publish producer evidence");
    let exact = admit_full_verifier_evidence(repository, Selector::staged())
        .expect("read exact producer evidence");
    let semantic = admit_semantic_full_verifier_evidence(repository, Selector::staged())
        .expect("diagnose exact producer evidence");
    assert!(
        exact.is_some(),
        "the complete bundle must re-admit against its exact source: {}",
        semantic
            .unavailable_detail()
            .unwrap_or("semantic evidence admitted"),
    );
    evidence
}

fn compiler_evidence(
    prepared: &mut spice::gate::PreparedBattery<'_>,
) -> Vec<VerifierCompilerEvidence> {
    let contracts = [GateId::RustTypecheck, GateId::CanvasWebTypecheck]
        .into_iter()
        .map(|gate_id| {
            prepared
                .verifier_compiler_contract(gate_id, 1)
                .expect("capture compiler contract")
        })
        .collect::<Vec<_>>();
    let mut processes = Vec::<VerifierCompilerProcessEvidence>::new();
    for contract in &contracts {
        for stage in contract.stages() {
            let run = contract.run(*stage).expect("run compiler evidence stage");
            assert!(
                run.success(),
                "compiler evidence stage failed: {}",
                String::from_utf8_lossy(run.stderr()),
            );
            processes.push(run.into_evidence().expect("successful process evidence"));
        }
    }
    contracts
        .iter()
        .map(|contract| {
            contract
                .complete(&processes)
                .expect("complete compiler evidence")
        })
        .collect()
}

fn reject_cross_worktree_reuse(root: &Path, evidence: &FullVerifierEvidence) {
    let linked = fixture_peer(root, "linked");
    git(
        root,
        &[
            "worktree",
            "add",
            "-q",
            linked.to_str().expect("linked path is text"),
            "HEAD",
        ],
    );
    let repository = Repository::discover(&linked).expect("discover linked worktree");
    evidence
        .publish(&repository)
        .expect("copy exact bundle to linked worktree state");
    env::set_current_dir(&linked).expect("enter linked worktree");
    let admission = admit_semantic_full_verifier_evidence(&repository, Selector::staged())
        .expect("probe linked worktree authority");
    env::set_current_dir(root).expect("return to producer worktree");
    assert!(
        matches!(admission, SemanticFullVerifierAdmission::Unavailable { .. }),
        "compiler evidence must not cross worktree authority",
    );
}

fn reject_compiler_contract_drift(repository: &Repository) {
    let cargo = fixture_peer(repository.worktree_root(), "tools").join("cargo");
    let original = fs::read(&cargo).expect("read fixture Cargo wrapper");
    let mut drifted = original.clone();
    drifted.extend_from_slice(b"\n# compiler contract drift\n");
    fs::write(&cargo, drifted).expect("move compiler executable bytes");
    let admission = inspect_full_verifier_evidence(repository, Selector::staged())
        .expect("probe drifted compiler contract");
    let FullVerifierEvidenceAdmission::Unavailable(miss) = admission else {
        panic!("exact evidence must not cross compiler executable drift");
    };
    assert_eq!(miss.stage(), FullVerifierEvidenceStage::Compiler);
    fs::write(&cargo, original).expect("restore compiler executable bytes");
    let mut permissions = fs::metadata(&cargo)
        .expect("read restored Cargo wrapper")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&cargo, permissions).expect("restore Cargo executable mode");
}

fn fixture_peer(root: &Path, name: &str) -> PathBuf {
    root.parent()
        .expect("fixture repository has a parent")
        .join(name)
}

struct Fixture {
    base: PathBuf,
    repository: PathBuf,
    tools: PathBuf,
    cargo_log: PathBuf,
    trunk_log: PathBuf,
    real_cargo: PathBuf,
    original_path: OsString,
}

impl Fixture {
    #[expect(
        clippy::disallowed_methods,
        reason = "the integration-test process rim captures PATH once for its isolated child"
    )]
    fn new() -> Self {
        let base = env::temp_dir().join(format!(
            "spice-semantic-full-verifier-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        let repository = base.join("repository");
        let tools = base.join("tools");
        fs::create_dir_all(repository.join("app/src"))
            .expect("create app fixture source directory");
        fs::create_dir_all(&tools).expect("create fixture tool directory");
        let original_path = env::var_os("PATH").expect("test process PATH");
        let real_cargo = find_program("cargo", &original_path).expect("find real Cargo");
        Self {
            cargo_log: base.join("cargo.log"),
            trunk_log: base.join("trunk.log"),
            base,
            repository,
            tools,
            real_cargo,
            original_path,
        }
    }

    fn write_repository(&self) {
        git(&self.repository, &["init", "-q"]);
        git(
            &self.repository,
            &["config", "user.email", "semantic@example.invalid"],
        );
        git(
            &self.repository,
            &["config", "user.name", "Semantic Fixture"],
        );
        git(&self.repository, &["config", "commit.gpgSign", "false"]);
        fs::write(
            self.repository.join("spice-rust.toml"),
            concat!(
                "[policy]\n",
                "env_names = []\n",
                "generated_paths = []\n",
                "package_roots = [\"app\"]\n",
                "test_engines = [\"cargo-workspace\"]\n",
                "test_paths = []\n",
                "\n[policy.repo_truth]\n",
                "docs = [\"AGENTS.md\"]\n",
                "\n[policy.file_shape]\n",
                "source_suffixes = [\".html\", \".json\", \".md\", \".rs\", \".toml\"]\n",
                "\n[policy.languages]\n",
                "complexity = [\".rs\"]\n",
                "magic = [\".rs\"]\n",
            ),
        )
        .expect("write strict tracked policy");
        fs::write(
            self.repository.join("baseline.toml"),
            "version = 1\n\n[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n",
        )
        .expect("write strict tracked baseline");
        fs::write(self.repository.join("AGENTS.md"), "# Fixture\n")
            .expect("write declared repository document");
        fs::write(
            self.repository.join("Cargo.toml"),
            concat!(
                "[workspace]\n",
                "members = [\"app\"]\n",
                "resolver = \"3\"\n",
                "\n[workspace.package]\nedition = \"2024\"\n",
            ),
        )
        .expect("write workspace manifest");
        fs::write(
            self.repository.join("app/Cargo.toml"),
            concat!(
                "[package]\nname = \"app\"\nversion = \"0.0.0\"\nedition.workspace = true\n",
                "build = \"build.rs\"\n",
            ),
        )
        .expect("write app package manifest");
        fs::write(
            self.repository.join("Cargo.lock"),
            concat!(
                "# This file is automatically @generated by Cargo.\n",
                "# It is not intended for manual editing.\n",
                "version = 4\n\n",
                "[[package]]\nname = \"app\"\nversion = \"0.0.0\"\n",
            ),
        )
        .expect("write Cargo lockfile");
        fs::write(
            self.repository.join("app/build.rs"),
            concat!(
                "fn main() {\n",
                "    let _ = std::fs::read_to_string(\"schema.json\").unwrap();\n",
                "    println!(\"cargo:rerun-if-changed=schema.json\");\n",
                "}\n",
            ),
        )
        .expect("write build script");
        fs::write(self.repository.join("app/schema.json"), "{\"version\":1}\n")
            .expect("write build-script input");
        fs::write(
            self.repository.join("app/src/lib.rs"),
            "pub fn app_fixture() {}\n",
        )
        .expect("write app Rust source");
        fs::create_dir_all(self.repository.join("app/browser"))
            .expect("create frontend fixture directory");
        fs::write(
            self.repository.join("app/browser/lifecycle.mjs"),
            "export const lifecycle = \"fixture\";\nvoid lifecycle;\n",
        )
        .expect("write frontend source");
        fs::write(
            self.repository.join("app/index.html"),
            "<!doctype html><title>fixture</title>\n",
        )
        .expect("write Canvas entry point");
        git(&self.repository, &["add", "."]);
        git(&self.repository, &["commit", "-qm", "fixture baseline"]);
    }

    fn write_tools(&self) {
        write_executable(
            &self.tools.join("cargo"),
            &format!(
                concat!(
                    "#!/bin/sh\n",
                    "printf '%s\\n' \"$*\" >> \"{}\"\n",
                    "if [ \"$1\" = \"--version\" ]; then\n",
                    "  printf '%s\\n' 'cargo 1.97.1 (c980f4866 2026-06-30)'\n",
                    "  exit 0\n",
                    "fi\n",
                    "case \"$1\" in\n",
                    "  metadata|test) exec \"{}\" \"$@\" ;;\n",
                    "  *) exit 0 ;;\n",
                    "esac\n",
                ),
                self.cargo_log.display(),
                self.real_cargo.display(),
            ),
        );
        write_executable(
            &self.tools.join("trunk"),
            &format!(
                concat!(
                    "#!/bin/sh\n",
                    "printf '%s\\n' \"$*\" >> \"{}\"\n",
                    "if [ \"$1\" = \"--version\" ]; then\n",
                    "  printf '%s\\n' 'trunk 0.21.14'\n",
                    "fi\n",
                    "exit 0\n",
                ),
                self.trunk_log.display(),
            ),
        );
        fs::write(&self.cargo_log, "").expect("create Cargo log");
        fs::write(&self.trunk_log, "").expect("create Trunk log");
    }

    fn search_path(&self) -> OsString {
        env::join_paths(
            std::iter::once(self.tools.clone()).chain(env::split_paths(&self.original_path)),
        )
        .expect("compose fixture PATH")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.base).expect("remove semantic lifecycle fixture");
    }
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write fixture executable");
    let mut permissions = fs::metadata(path)
        .expect("read fixture executable")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("make fixture executable");
}

fn find_program(name: &str, search_path: &OsString) -> Option<PathBuf> {
    env::split_paths(search_path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("fixture Git launches");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}

fn git_output(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("fixture Git launches");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout)
        .expect("fixture Git output is UTF-8")
        .trim()
        .to_owned()
}
