use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use wire::{Digest, Domain};

use super::*;
use crate::{
    ChangeArmReach, ChangeReachEdge, ChangeSelectionPolicy, ChangeSelectionPolicyInputs, Selector,
    select_change_gates, test_repository,
};

#[test]
fn current_battery_classification_is_exhaustive_and_exact() {
    let classes = GateId::BATTERY.map(|gate_id| {
        gate_action_reuse_class(gate_id).expect("every battery arm has one reuse class")
    });
    assert_eq!(
        classes
            .iter()
            .filter(|class| **class == ActionReuseClass::LiveOnly)
            .count(),
        2
    );
    assert_eq!(
        classes
            .iter()
            .filter(|class| **class == ActionReuseClass::AncestrySensitive)
            .count(),
        5
    );
    assert_eq!(
        classes
            .iter()
            .filter(|class| **class == ActionReuseClass::Cacheable)
            .count(),
        20
    );
    assert_eq!(gate_action_reuse_class(GateId::SuiteSeam), None);
    assert_eq!(gate_action_reuse_class(GateId::MutationCoverage), None);
    assert_eq!(gate_action_reuse_class(GateId::TestSubsumption), None);
}

#[test]
fn manifest_tracks_content_mode_deletion_and_non_files_but_not_unrelated_paths() {
    let fixture = action_fixture();
    let first = compile_magic(&fixture, b"selection-a", b"baseline-a");
    let first_digest = first.digest().expect("digest first manifest");
    assert_eq!(
        first
            .entries()
            .iter()
            .map(ActionInputManifestEntry::path)
            .collect::<Vec<_>>(),
        ["src/lib.rs", "src/removed.rs", "src/tool.rs"]
    );
    assert!(matches!(
        &first.entries()[0],
        ActionInputManifestEntry::Present {
            mode: GitFileMode::Regular,
            ..
        }
    ));
    assert!(matches!(
        &first.entries()[1],
        ActionInputManifestEntry::Deleted { .. }
    ));
    assert!(matches!(
        &first.entries()[2],
        ActionInputManifestEntry::Present {
            mode: GitFileMode::Executable,
            ..
        }
    ));

    fs::write(
        fixture.path().join("src/removed.rs"),
        "pub const REMOVED: u8 = 3;\n",
    )
    .expect("restore deleted action input");
    test_repository::stage(fixture.path(), &["src/removed.rs"]);
    let restored = compile_magic(&fixture, b"selection-restored", b"baseline-a");
    assert_ne!(
        restored.digest().expect("digest restored input"),
        first_digest,
        "a deleted-to-present transition must move the manifest"
    );
    fs::remove_file(fixture.path().join("src/removed.rs")).expect("delete restored action input");
    test_repository::stage(fixture.path(), &["src/removed.rs"]);

    fs::write(
        fixture.path().join("unrelated.rs"),
        "pub const OTHER: u8 = 9;\n",
    )
    .expect("change unrelated candidate input");
    test_repository::stage(fixture.path(), &["unrelated.rs"]);
    let unrelated = compile_magic(&fixture, b"selection-b", b"baseline-a");
    assert_eq!(
        unrelated.digest().expect("digest unrelated manifest"),
        first_digest,
        "candidate and selection movement outside the action closure must retain the key input"
    );

    fs::write(
        fixture.path().join("src/lib.rs"),
        "pub const VALUE: u8 = 8;\n",
    )
    .expect("change selected content");
    test_repository::stage(fixture.path(), &["src/lib.rs"]);
    let changed_content = compile_magic(&fixture, b"selection-c", b"baseline-a");
    assert_ne!(
        changed_content.digest().expect("digest content mutation"),
        first_digest
    );

    set_executable(&fixture.path().join("src/tool.rs"), false);
    test_repository::stage(fixture.path(), &["src/tool.rs"]);
    let changed_mode = compile_magic(&fixture, b"selection-d", b"baseline-a");
    assert_ne!(
        changed_mode.digest().expect("digest mode mutation"),
        changed_content.digest().expect("digest prior mode")
    );

    let changed_baseline = compile_magic(&fixture, b"selection-e", b"baseline-b");
    assert_ne!(
        changed_baseline.digest().expect("digest baseline mutation"),
        changed_mode.digest().expect("digest prior baseline")
    );
}

#[test]
fn repository_reach_uses_the_complete_candidate_without_caller_inventory() {
    let fixture = action_fixture();
    let tree = Selector::staged()
        .materialize(fixture.path())
        .expect("materialize candidate tree");
    let selection = selection(&fixture, b"repository-expansion");
    let baseline = non_file(ActionNonFileKind::Baseline, "HEAD", b"head");
    let full = ActionInputInventory::complete(Vec::new(), vec![baseline.clone()])
        .expect("compile full repository inventory");
    let full = match compile_action_input(&tree, &selection, GateId::MergeIntegrity, full)
        .expect("compile repository closure")
    {
        ActionInputClosure::Reusable(manifest) => manifest,
        ActionInputClosure::NonCacheable(reason) => {
            panic!("complete repository closure was non-cacheable: {reason:?}")
        }
    };
    assert_eq!(full.entries().len(), tree.paths().len());

    fs::write(
        fixture.path().join("added.rs"),
        "pub const ADDED: u8 = 1;\n",
    )
    .expect("add repository input");
    test_repository::stage(fixture.path(), &["added.rs"]);
    let expanded_tree = Selector::staged()
        .materialize(fixture.path())
        .expect("materialize expanded candidate tree");
    let expanded = ActionInputInventory::complete(Vec::new(), vec![baseline])
        .expect("compile expanded repository inventory");
    let expanded =
        match compile_action_input(&expanded_tree, &selection, GateId::MergeIntegrity, expanded)
            .expect("compile expanded repository closure")
        {
            ActionInputClosure::Reusable(manifest) => manifest,
            ActionInputClosure::NonCacheable(reason) => {
                panic!("expanded repository closure was non-cacheable: {reason:?}")
            }
        };
    assert_ne!(
        full.digest().expect("digest original repository"),
        expanded.digest().expect("digest expanded repository")
    );
}

#[test]
fn live_unknown_and_incomplete_inputs_never_mint_a_manifest() {
    let fixture = action_fixture();
    let tree = Selector::staged()
        .materialize(fixture.path())
        .expect("materialize candidate tree");
    let selection = selection(&fixture, b"non-cacheable");

    assert_eq!(
        compile_action_input(
            &tree,
            &selection,
            GateId::PlanPhase,
            ActionInputInventory::incomplete("live inventory is irrelevant"),
        )
        .expect("classify live action"),
        ActionInputClosure::NonCacheable(ActionNonCacheable::LiveOnly)
    );
    assert_eq!(
        compile_action_input(
            &tree,
            &selection,
            GateId::MergeIntegrity,
            ActionInputInventory::incomplete("Cargo metadata did not resolve"),
        )
        .expect("classify incomplete action"),
        ActionInputClosure::NonCacheable(ActionNonCacheable::IncompleteInventory {
            reason: "Cargo metadata did not resolve".to_owned(),
        })
    );

    let package = ChangeReachObject::Package {
        package: "fixture".to_owned(),
    };
    let selection = with_rust_package_reach(selection, package.clone());
    let inventory = ActionInputInventory::complete(Vec::new(), Vec::new())
        .expect("complete but unexpanded inventory");
    assert_eq!(
        compile_action_input(&tree, &selection, GateId::RustTypecheck, inventory)
            .expect("classify missing package expansion"),
        ActionInputClosure::NonCacheable(ActionNonCacheable::MissingObjectExpansion {
            object: package.clone(),
        })
    );

    let expansion = ActionInputExpansion::new(package, vec!["src/lib.rs".to_owned()])
        .expect("declare Cargo package closure");
    let inventory = ActionInputInventory::complete(vec![expansion], Vec::new())
        .expect("compile Cargo package inventory");
    assert!(matches!(
        compile_action_input(&tree, &selection, GateId::RustTypecheck, inventory)
            .expect("compile expanded package closure"),
        ActionInputClosure::Reusable(_)
    ));
}

#[test]
fn baseline_identity_is_required_only_for_ancestry_sensitive_actions() {
    let fixture = action_fixture();
    let tree = Selector::staged()
        .materialize(fixture.path())
        .expect("materialize candidate tree");
    let selection = selection(&fixture, b"baseline-rules");

    let missing =
        ActionInputInventory::complete(Vec::new(), Vec::new()).expect("empty complete inventory");
    assert_eq!(
        compile_action_input(&tree, &selection, GateId::MagicNumbers, missing)
            .expect("classify missing baseline"),
        ActionInputClosure::NonCacheable(ActionNonCacheable::MissingBaseline)
    );

    let unexpected = ActionInputInventory::complete(
        Vec::new(),
        vec![non_file(
            ActionNonFileKind::Baseline,
            "branch-tip",
            b"branch",
        )],
    )
    .expect("inventory with unrelated baseline");
    assert_eq!(
        compile_action_input(&tree, &selection, GateId::Formatters, unexpected)
            .expect("classify unexpected baseline"),
        ActionInputClosure::NonCacheable(ActionNonCacheable::UnexpectedBaseline)
    );
}

#[test]
fn canonical_manifest_round_trips_and_refuses_reordered_entries() {
    let fixture = action_fixture();
    let manifest = compile_magic(&fixture, b"canonical", b"baseline");
    let encoded = manifest.encode().expect("encode canonical manifest");
    assert_eq!(
        ActionInputManifest::decode(&encoded).expect("decode canonical manifest"),
        manifest
    );

    let mut value: serde_json::Value =
        serde_json::from_slice(&encoded).expect("decode fixture JSON value");
    value["entries"]
        .as_array_mut()
        .expect("manifest entries array")
        .reverse();
    let reordered = serde_json::to_vec(&value).expect("encode reordered fixture");
    assert!(ActionInputManifest::decode(&reordered).is_err());

    let mut duplicate_names = vec![
        non_file(ActionNonFileKind::Baseline, "committed-text", b"first"),
        non_file(ActionNonFileKind::Baseline, "committed-text", b"second"),
    ];
    duplicate_names.sort();
    let duplicate_names = ActionInputManifest {
        version: manifest.version,
        entries: manifest.entries.clone(),
        non_files: duplicate_names,
    }
    .encode()
    .expect("encode duplicate-name manifest");
    assert!(ActionInputManifest::decode(&duplicate_names).is_err());
}

#[test]
fn every_declared_non_file_input_moves_the_manifest_digest() {
    let fixture = action_fixture();
    let kinds = [
        ActionNonFileKind::Baseline,
        ActionNonFileKind::BoundLedger,
        ActionNonFileKind::ClaimState,
        ActionNonFileKind::Declaration,
        ActionNonFileKind::Environment,
        ActionNonFileKind::GateSet,
        ActionNonFileKind::Inventory,
        ActionNonFileKind::Policy,
        ActionNonFileKind::Toolchain,
    ];
    let inputs = kinds
        .iter()
        .enumerate()
        .map(|(index, kind)| non_file(*kind, &format!("input-{index}"), b"original"))
        .collect::<Vec<_>>();
    let original = compile_magic_with_inputs(&fixture, b"all-non-files", inputs.clone());
    let original = original.digest().expect("digest all non-file inputs");

    for (index, kind) in kinds.iter().enumerate() {
        let mut changed = inputs.clone();
        changed[index] = non_file(*kind, &format!("input-{index}"), b"changed");
        let changed = compile_magic_with_inputs(&fixture, b"all-non-files", changed);
        assert_ne!(
            changed.digest().expect("digest changed non-file input"),
            original,
            "changing {kind:?} did not move the manifest digest"
        );
    }
}

fn action_fixture() -> Fixture {
    let fixture = Fixture::new();
    test_repository::initialize(fixture.path());
    fs::create_dir_all(fixture.path().join("src")).expect("create source directory");
    fs::write(
        fixture.path().join("src/lib.rs"),
        "pub const VALUE: u8 = 7;\n",
    )
    .expect("write selected source");
    fs::write(
        fixture.path().join("src/removed.rs"),
        "pub const REMOVED: u8 = 1;\n",
    )
    .expect("write removed source");
    fs::write(
        fixture.path().join("src/tool.rs"),
        "pub const TOOL: u8 = 1;\n",
    )
    .expect("write executable source");
    fs::write(
        fixture.path().join("unrelated.rs"),
        "pub const OTHER: u8 = 1;\n",
    )
    .expect("write unrelated source");
    set_executable(&fixture.path().join("src/tool.rs"), true);
    test_repository::stage_all(fixture.path());
    test_repository::commit(fixture.path(), "seed action inputs");

    fs::write(
        fixture.path().join("src/lib.rs"),
        "pub const VALUE: u8 = 2;\n",
    )
    .expect("change selected source");
    fs::remove_file(fixture.path().join("src/removed.rs")).expect("remove selected source");
    test_repository::stage(fixture.path(), &["src/lib.rs", "src/removed.rs"]);
    fixture
}

fn compile_magic(
    fixture: &Fixture,
    selection_seed: &[u8],
    baseline_seed: &[u8],
) -> ActionInputManifest {
    compile_magic_with_inputs(
        fixture,
        selection_seed,
        vec![non_file(
            ActionNonFileKind::Baseline,
            "committed-text",
            baseline_seed,
        )],
    )
}

fn compile_magic_with_inputs(
    fixture: &Fixture,
    selection_seed: &[u8],
    non_files: Vec<ActionNonFileInput>,
) -> ActionInputManifest {
    let tree = Selector::staged()
        .materialize(fixture.path())
        .expect("materialize action candidate");
    let selection = selection(fixture, selection_seed);
    let inventory = ActionInputInventory::complete(Vec::new(), non_files)
        .expect("compile complete magic inventory");
    match compile_action_input(&tree, &selection, GateId::MagicNumbers, inventory)
        .expect("compile magic closure")
    {
        ActionInputClosure::Reusable(manifest) => manifest,
        ActionInputClosure::NonCacheable(reason) => {
            panic!("complete magic closure was non-cacheable: {reason:?}")
        }
    }
}

fn selection(_fixture: &Fixture, seed: &[u8]) -> ChangeSelectionReport {
    let changed = vec![
        "src/lib.rs".to_owned(),
        "src/removed.rs".to_owned(),
        "src/tool.rs".to_owned(),
    ];
    let suffixes = vec![".rs".to_owned()];
    let tracked = vec![
        "Cargo.toml".to_owned(),
        "spice-rust.toml".to_owned(),
        "src/lib.rs".to_owned(),
        "src/tool.rs".to_owned(),
        "unrelated.rs".to_owned(),
    ];
    let policy = ChangeSelectionPolicy::new(ChangeSelectionPolicyInputs {
        tracked_sources: &tracked,
        policy_authorities: &["spice-rust.toml".to_owned()],
        configured: true,
        repository_documents: Some(&[]),
        file_shape_suffixes: Some(&suffixes),
        complexity_suffixes: Some(&suffixes),
        magic_suffixes: Some(&suffixes),
        package_roots: &[],
        generated_paths: &[],
        test_paths: &["tests".to_owned()],
    })
    .expect("compile action fixture selection policy");
    select_change_gates(Digest::compute(Domain::GateTree, seed), &changed, &policy)
        .expect("select action fixture gates")
}

fn with_rust_package_reach(
    selection: ChangeSelectionReport,
    package: ChangeReachObject,
) -> ChangeSelectionReport {
    let mut reach = selection.reach().to_vec();
    let index = reach
        .binary_search_by_key(&GateId::RustTypecheck, ChangeArmReach::gate_id)
        .expect("fixture selects Rust typecheck");
    let provenance = selection
        .edges()
        .iter()
        .filter(|edge| edge.gate_id() == GateId::RustTypecheck)
        .map(|edge| ChangeReachEdge::changed(edge.source(), package.clone()))
        .collect();
    reach[index] = ChangeArmReach::focused(GateId::RustTypecheck, vec![package], provenance);
    selection
        .with_reach(reach)
        .expect("replace Rust reach with one package")
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gate-action-input-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("create action-input fixture");
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove action-input fixture");
    }
}

fn non_file(kind: ActionNonFileKind, name: &str, seed: &[u8]) -> ActionNonFileInput {
    ActionNonFileInput::new(kind, name, Digest::compute(Domain::GateInventory, seed))
        .expect("construct non-file input")
}

#[cfg(target_family = "unix")]
fn set_executable(path: &std::path::Path, executable: bool) {
    use std::os::unix::fs::PermissionsExt as _;

    let mode = if executable { 0o755 } else { 0o644 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .expect("set action fixture permissions");
}

#[cfg(not(target_family = "unix"))]
fn set_executable(_path: &std::path::Path, _executable: bool) {}
