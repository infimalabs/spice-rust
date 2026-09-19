use std::fs;
use std::path::Path;

use gate::{
    ActorCapture, BatteryCapture, BatteryState, ClaimCapture, ConfigInventoryEvidence,
    ConfigKeyEvidence, CoreArmReport, CoreEvidence, CoreFinding, CoreObservation, CoreStanding,
    GateId, HygieneConfiguration, RimCapture, Selector,
};

mod support;

use support::{
    Tree, acquired, acquired_arm, contract_repository, git_commit, git_repository, measured,
    policy_document, workspace_document,
};

#[test]
fn configuration_key_admission_names_the_unknown_active_layer_key() {
    let repository = git_repository("core-acquire-config-key-unknown");
    repository.write(
        "spice-rust.toml",
        &policy_document(concat!(
            "[tasks]\n",
            "stems = ['rim']\n",
            "strms = ['misspelled']\n",
        )),
    );
    repository.write("baseline.toml", &workspace_document("baseline.toml"));
    repository.write("src/app.rs", "pub fn app() {}\n");
    git_commit(&repository, "unknown active configuration key");

    let report = acquired(&repository, ClaimCapture::Idle, ActorCapture::Absent);
    let configuration = acquired_arm(&report, GateId::ConfigKeyValidity);
    let expected = ConfigKeyEvidence::new(vec![
        CoreFinding::new(
            "unknown-key",
            None::<String>,
            concat!(
                "tracked layer key tasks.strms is absent from the packaged ",
                "configuration registry",
            ),
        )
        .expect("unknown-key evidence"),
    ]);
    assert_eq!(
        configuration.observation(),
        &measured(CoreEvidence::ConfigKeyValidity(expected))
    );
    assert_eq!(configuration.standing(), CoreStanding::Failed);

    repository.remove();
}

#[test]
fn configuration_key_admission_names_a_known_key_forbidden_in_the_active_layer() {
    let repository = git_repository("core-acquire-config-key-layer");
    repository.write(
        "spice-rust.toml",
        &policy_document("[say]\ncommand = ['/usr/bin/say']\n"),
    );
    repository.write("baseline.toml", &workspace_document("baseline.toml"));
    repository.write("src/app.rs", "pub fn app() {}\n");
    git_commit(&repository, "forbidden active configuration layer");

    let report = acquired(&repository, ClaimCapture::Idle, ActorCapture::Absent);
    let configuration = acquired_arm(&report, GateId::ConfigKeyValidity);
    let expected = ConfigKeyEvidence::new(vec![
        CoreFinding::new(
            "forbidden-layer",
            None::<String>,
            "tracked layer key say.command is packaged for the local layer, not tracked",
        )
        .expect("layer-policy evidence"),
    ]);
    assert_eq!(
        configuration.observation(),
        &measured(CoreEvidence::ConfigKeyValidity(expected))
    );
    assert_eq!(configuration.standing(), CoreStanding::Failed);

    repository.remove();
}

/// Only the `[policy]` subtables migration requires are admitted.
///
/// Most subtables the rule-policy migration reads are predecessor spellings it
/// translates once, not settings a current repository declares, and the
/// workspace schema refuses them. Admitting one would tell a repository it may
/// write a key that makes its whole document fail to load, so admission names
/// each instead.
///
/// The file classes are the exception, and the document below carries both
/// kinds at once because that is the only way to see the registry tell them
/// apart. `policy.file_shape` and `policy.languages` are predecessor spellings
/// too, but no repository loads without them, which makes them settings this
/// release reads rather than relics it tolerates. Reporting them absent from
/// the registry would refuse every document that can be gated at all.
#[test]
fn configuration_key_admission_separates_required_file_classes_from_migrated_subtables() {
    let repository = git_repository("core-acquire-config-key-migrated");
    repository.write(
        "spice-rust.toml",
        &policy_document(concat!(
            "[policy.limits]\n",
            "file_loc = 1234\n",
            "[policy.flex]\n",
            "ratio = 2.25\n",
            "jitter_percent = 7\n",
            "[policy.magic]\n",
            "examine_threshold = 33\n",
            "[policy.markdown_depth]\n",
            "base_chars = 100\n",
            "max_bounded_chars = 300\n",
            "[policy.markdown_depth_budget]\n",
            "extensions = ['.rst']\n",
            "[[policy.rules]]\n",
            "routine_length = { max = 55 }\n",
        )),
    );
    repository.write("baseline.toml", &workspace_document("baseline.toml"));
    repository.write("src/app.rs", "pub fn app() {}\n");
    git_commit(&repository, "every migrated policy subtable");

    let report = acquired(&repository, ClaimCapture::Idle, ActorCapture::Absent);
    let configuration = acquired_arm(&report, GateId::ConfigKeyValidity);
    let expected = ConfigKeyEvidence::new(
        [
            "policy.flex",
            "policy.limits",
            "policy.magic",
            "policy.markdown_depth",
            "policy.markdown_depth_budget",
            "policy.rules",
        ]
        .into_iter()
        .map(|key| {
            CoreFinding::new(
                "unknown-key",
                None::<String>,
                format!(
                    "tracked layer key {key} is absent from the packaged configuration registry"
                ),
            )
            .expect("migrated subtable evidence")
        })
        .collect(),
    );
    assert_eq!(
        configuration.observation(),
        &measured(CoreEvidence::ConfigKeyValidity(expected))
    );
    assert_eq!(configuration.standing(), CoreStanding::Failed);

    repository.remove();
}

#[test]
fn false_disable_inventory_fails_when_a_declared_path_has_no_consumer() {
    let repository = false_disable_repository("core-false-disable-missing", false);
    let report = acquired(&repository, ClaimCapture::Idle, ActorCapture::Absent);
    let arm = acquired_arm(&report, GateId::ConfigFalseDisable);
    let evidence = false_disable_evidence(arm);

    assert_eq!(
        evidence.declared(),
        &[config_path(&["vendor", "codex", "fast_mode"])]
    );
    assert!(evidence.observed().is_empty());
    assert!(evidence.violations().is_empty());
    assert_eq!(arm.standing(), CoreStanding::Failed);
    assert!(arm.diagnostic().contains("missing=vendor.codex.fast_mode"));

    repository.remove();
}

#[test]
fn false_disable_inventory_passes_with_canonical_honored_paths() {
    let repository = false_disable_repository("core-false-disable-honored", true);
    let report = acquired(&repository, ClaimCapture::Idle, ActorCapture::Absent);
    let arm = acquired_arm(&report, GateId::ConfigFalseDisable);
    let evidence = false_disable_evidence(arm);
    let expected = [
        config_path(&["alpha", "feature", "enabled"]),
        config_path(&["vendor", "codex", "fast_mode"]),
    ];

    assert_eq!(evidence.declared(), expected);
    assert_eq!(evidence.observed(), expected);
    assert!(evidence.violations().is_empty());
    assert_eq!(arm.standing(), CoreStanding::Passed);

    repository.remove();
}

#[test]
fn false_disable_inventory_authenticates_the_configuration_root_type() {
    let repository = contract_repository("core-false-disable-root-authority");
    repository.write(
        "spice/src/config.rs",
        &false_disable_registry(&[
            "direct.enabled",
            "renamed.enabled",
            "qualified.enabled",
            "vendor.codex.fast_mode",
        ]),
    );
    repository.write(
        "spice/src/direct.rs",
        concat!(
            "use crate::config::Root;\n",
            "fn consume(root: &Root) -> bool {\n",
            "    let direct = root.direct();\n",
            "    direct.enabled()\n",
            "}\n",
        ),
    );
    repository.write(
        "spice/src/renamed.rs",
        concat!(
            "use crate::config::Root as ConfigRoot;\n",
            "fn consume(root: &ConfigRoot) -> bool {\n",
            "    root.renamed().enabled()\n",
            "}\n",
        ),
    );
    repository.write(
        "spice/src/qualified.rs",
        concat!(
            "fn consume(root: &crate::config::Root) -> bool {\n",
            "    root.qualified().enabled()\n",
            "}\n",
        ),
    );
    repository.write(
        "spice/src/unrelated.rs",
        concat!(
            "use crate::config::Root;\n",
            "mod shadowed {\n",
            "    struct Root;\n",
            "    fn consume(root: &Root) -> bool {\n",
            "        root.vendor().codex().fast_mode()\n",
            "    }\n",
            "}\n",
            "fn generic<Root>(root: &Root) -> bool {\n",
            "    root.vendor().codex().fast_mode()\n",
            "}\n",
            "struct Adapter<Root>(Root);\n",
            "impl<Root> Adapter<Root> {\n",
            "    fn consume(&self, root: &Root) -> bool {\n",
            "        root.vendor().codex().fast_mode()\n",
            "    }\n",
            "}\n",
        ),
    );
    git_commit(&repository, "configuration Root authority fixture");

    let report = acquired(&repository, ClaimCapture::Idle, ActorCapture::Absent);
    let arm = acquired_arm(&report, GateId::ConfigFalseDisable);
    let evidence = false_disable_evidence(arm);
    let declared = [
        config_path(&["direct", "enabled"]),
        config_path(&["qualified", "enabled"]),
        config_path(&["renamed", "enabled"]),
        config_path(&["vendor", "codex", "fast_mode"]),
    ];
    let observed = [
        config_path(&["direct", "enabled"]),
        config_path(&["qualified", "enabled"]),
        config_path(&["renamed", "enabled"]),
    ];

    assert_eq!(evidence.declared(), declared);
    assert_eq!(evidence.observed(), observed);
    assert_eq!(arm.standing(), CoreStanding::Failed);
    assert!(arm.diagnostic().contains("missing=vendor.codex.fast_mode"));

    repository.remove();
}

#[test]
fn false_disable_inventory_drops_a_receiver_rebound_by_any_pattern_form() {
    let repository = contract_repository("core-false-disable-shadowed");
    let paths = [
        "annotated.enabled",
        "arm.enabled",
        "closure.enabled",
        "iterated.enabled",
        "optional.enabled",
        "tuple.enabled",
    ];
    repository.write("spice/src/config.rs", &false_disable_registry(&paths));
    repository.write(
        "spice/src/shadowed.rs",
        concat!(
            "use crate::config::Root;\n",
            "fn closure_parameter(root: &Root, flag: &Flag) -> bool {\n",
            "    let read = |root: &Flag| root.closure().enabled();\n",
            "    read(flag)\n",
            "}\n",
            "fn match_arm(root: &Root, flag: Option<Flag>) -> bool {\n",
            "    match flag {\n",
            "        Some(root) => root.arm().enabled(),\n",
            "        None => false,\n",
            "    }\n",
            "}\n",
            "fn for_loop(root: &Root, flags: &[Flag]) -> bool {\n",
            "    for root in flags {\n",
            "        if root.iterated().enabled() {\n",
            "            return true;\n",
            "        }\n",
            "    }\n",
            "    false\n",
            "}\n",
            "fn annotated_let(root: &Root, flag: Flag) -> bool {\n",
            "    let root: Flag = flag;\n",
            "    root.annotated().enabled()\n",
            "}\n",
            "fn tuple_let(root: &Root, pair: (Flag, Flag)) -> bool {\n",
            "    let (root, _spare) = pair;\n",
            "    root.tuple().enabled()\n",
            "}\n",
            "fn optional_let(root: &Root, flag: Option<Flag>) -> bool {\n",
            "    if let Some(root) = flag {\n",
            "        return root.optional().enabled();\n",
            "    }\n",
            "    false\n",
            "}\n",
        ),
    );
    git_commit(&repository, "value-shadowed receiver fixture");

    let report = acquired(&repository, ClaimCapture::Idle, ActorCapture::Absent);
    let arm = acquired_arm(&report, GateId::ConfigFalseDisable);
    let evidence = false_disable_evidence(arm);
    let declared: Vec<_> = paths
        .iter()
        .map(|path| config_path(&path.split('.').collect::<Vec<_>>()))
        .collect();

    assert_eq!(evidence.declared(), declared);
    assert_eq!(evidence.observed(), Vec::<Vec<String>>::new());
    assert_eq!(arm.standing(), CoreStanding::Failed);
    assert!(
        arm.diagnostic()
            .contains(&format!("missing={}", paths.join(", ")))
    );

    repository.remove();
}

#[test]
fn false_disable_inventory_seeds_an_inherent_method_parameter() {
    let repository = contract_repository("core-false-disable-inherent");
    repository.write(
        "spice/src/config.rs",
        &false_disable_registry(&["vendor.codex.fast_mode"]),
    );
    repository.write(
        "spice/src/launcher.rs",
        concat!(
            "use crate::config::Root;\n",
            "struct Launcher;\n",
            "impl Launcher {\n",
            "    fn compose(&self, root: &Root) -> bool {\n",
            "        let configured = root.vendor().codex();\n",
            "        configured.fast_mode()\n",
            "    }\n",
            "}\n",
        ),
    );
    git_commit(&repository, "inherent method consumer fixture");

    let report = acquired(&repository, ClaimCapture::Idle, ActorCapture::Absent);
    let arm = acquired_arm(&report, GateId::ConfigFalseDisable);
    let evidence = false_disable_evidence(arm);
    let expected = [config_path(&["vendor", "codex", "fast_mode"])];

    assert_eq!(evidence.declared(), expected);
    assert_eq!(evidence.observed(), expected);
    assert!(evidence.violations().is_empty());
    assert_eq!(arm.standing(), CoreStanding::Passed);

    repository.remove();
}

#[test]
fn current_false_disable_registry_and_consumer_are_one_inventory() {
    let repository = contract_repository("core-false-disable-current");
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    for path in [
        "spice/src/config.rs",
        "spice/src/config/registry.rs",
        "spice/src/config/pre_commit.rs",
        "spice/src/extension.rs",
    ] {
        let source = fs::read_to_string(workspace.join(path))
            .unwrap_or_else(|error| panic!("read current {path}: {error}"));
        repository.write(path, &source);
    }
    write_registry_decoy(&repository);
    git_commit(&repository, "current false-disable sources");

    let report = acquired(&repository, ClaimCapture::Idle, ActorCapture::Absent);
    let arm = acquired_arm(&report, GateId::ConfigFalseDisable);
    let evidence = false_disable_evidence(arm);
    let expected = [
        config_path(&["vendor", "codex", "fast_mode"]),
        config_path(&["vendor", "omp", "fast_mode"]),
        config_path(&["vendor", "omp", "lsp"]),
    ];

    assert_eq!(evidence.declared(), expected);
    assert_eq!(evidence.observed(), expected);
    assert_eq!(arm.standing(), CoreStanding::Passed);

    repository.remove();
}

fn false_disable_repository(label: &str, honored: bool) -> Tree {
    let repository = contract_repository(label);
    let mut paths = vec!["vendor.codex.fast_mode"];
    if honored {
        paths.push("alpha.feature.enabled");
    }
    repository.write("spice/src/config.rs", &false_disable_registry(&paths));
    if honored {
        repository.write(
            "spice/src/consumer.rs",
            concat!(
                "use crate::config::Root;\n",
                "fn consume(root: &Root) -> bool {\n",
                "    let configured = root.vendor().codex();\n",
                "    let fast = configured.fast_mode();\n",
                "    let feature = root.alpha().feature();\n",
                "    fast || feature.enabled()\n",
                "}\n",
            ),
        );
    }
    git_commit(&repository, "false-disable inventory fixture");
    repository
}

/// One registry declaring every named path with a tracked false builtin.
fn false_disable_registry(paths: &[&str]) -> String {
    let rows: String = paths
        .iter()
        .map(|path| {
            format!(
                concat!(
                    "    fixed \"{path}\" {{\n",
                    "        kind: Boolean,\n",
                    "        builtin: Some(Builtin::Boolean(false)),\n",
                    "        layers: TRACKED,\n",
                    "        class: Policy,\n",
                    "        flag: None,\n",
                    "    }}\n",
                ),
                path = path,
            )
        })
        .collect();
    ["configuration_registry! {\n", &rows, "}\n"].concat()
}

fn false_disable_evidence(arm: &CoreArmReport) -> &ConfigInventoryEvidence {
    let CoreObservation::Measured {
        evidence: CoreEvidence::ConfigFalseDisable(evidence),
    } = arm.observation()
    else {
        panic!("false-disable acquisition must produce inventory evidence")
    };
    evidence
}

fn config_path(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

#[test]
fn tracked_trust_inventory_fails_with_one_untracked_policy_source() {
    let repository = untracked_trust_repository("core-tracked-trust-untracked");
    let report = acquired(&repository, ClaimCapture::Idle, ActorCapture::Absent);
    let arm = acquired_arm(&report, GateId::ConfigTrackedTrust);
    let evidence = tracked_trust_evidence(arm);
    let expected = [
        config_path(&["lane", "vendor"]),
        config_path(&["vendor", "codex", "binary"]),
    ];

    assert_eq!(evidence.declared(), expected);
    assert_eq!(evidence.observed(), &expected[..1]);
    let [violation] = evidence.violations() else {
        panic!("one untracked policy source must yield exactly one violation")
    };
    assert_eq!(violation.code(), "config-tracked-trust::untracked-source");
    assert!(!violation.detail().trim().is_empty());
    assert!(violation.detail().contains("vendor.codex.binary"));
    assert_eq!(arm.standing(), CoreStanding::Failed);

    repository.remove();
}

#[test]
fn tracked_trust_inventory_passes_when_every_policy_source_is_tracked() {
    let repository = contract_repository("core-tracked-trust-current");
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    for path in ["spice/src/config.rs", "spice/src/config/registry.rs"] {
        let config = fs::read_to_string(workspace.join(path))
            .unwrap_or_else(|error| panic!("read current configuration registry {path}: {error}"));
        repository.write(path, &config);
    }
    write_registry_decoy(&repository);
    git_commit(&repository, "current tracked-trust registry");
    let report = acquired(&repository, ClaimCapture::Idle, ActorCapture::Absent);
    let arm = acquired_arm(&report, GateId::ConfigTrackedTrust);
    let evidence = tracked_trust_evidence(arm);

    assert!(!evidence.declared().is_empty());
    assert_eq!(evidence.observed(), evidence.declared());
    assert!(
        evidence
            .declared()
            .contains(&config_path(&["vendor", "codex", "binary"]))
    );
    assert!(
        !evidence
            .declared()
            .contains(&config_path(&["vendor", "codex", "model"]))
    );
    assert!(
        !evidence
            .declared()
            .contains(&config_path(&["say", "command"]))
    );
    assert!(evidence.violations().is_empty());
    assert_eq!(arm.standing(), CoreStanding::Passed);

    repository.remove();
}

#[test]
fn focused_inventories_refuse_a_missing_authoritative_registry_despite_a_decoy() {
    let repository = contract_repository("core-configuration-registry-missing");
    write_registry_decoy(&repository);
    git_commit(&repository, "missing authoritative configuration registry");

    for gate_id in [GateId::ConfigFalseDisable, GateId::ConfigTrackedTrust] {
        let result = focused_inventory_result(&repository, gate_id);
        assert!(
            !result.state().admits(),
            "{gate_id} admitted the absent authoritative registry"
        );
        assert!(
            result
                .reason()
                .contains("authoritative root spice/src/config.rs is absent"),
            "{gate_id} did not name the absent registry: {}",
            result.reason()
        );
    }

    repository.remove();
}

#[test]
fn focused_inventories_use_the_declared_split_registry_and_ignore_a_decoy() {
    let repository = contract_repository("core-configuration-registry-split");
    repository.write("spice/src/config.rs", "mod registry;\npub struct Root;\n");
    repository.write(
        "spice/src/config/registry.rs",
        &false_disable_registry(&["vendor.codex.fast_mode"]),
    );
    repository.write(
        "spice/src/consumer.rs",
        concat!(
            "use crate::config::Root;\n",
            "fn consume(root: &Root) -> bool {\n",
            "    root.vendor().codex().fast_mode()\n",
            "}\n",
        ),
    );
    write_registry_decoy(&repository);
    git_commit(&repository, "split authoritative registry with decoy");

    for gate_id in [GateId::ConfigFalseDisable, GateId::ConfigTrackedTrust] {
        let result = focused_inventory_result(&repository, gate_id);
        assert_eq!(
            result.state(),
            BatteryState::Passed,
            "{gate_id} did not use only the declared split registry: {}",
            result.reason()
        );
    }

    repository.remove();
}

#[test]
fn focused_inventories_refuse_a_declared_but_missing_split_registry_module() {
    let repository = contract_repository("core-configuration-registry-split-missing");
    repository.write("spice/src/config.rs", "mod registry;\npub struct Root;\n");
    git_commit(&repository, "missing declared split registry module");

    for gate_id in [GateId::ConfigFalseDisable, GateId::ConfigTrackedTrust] {
        let result = focused_inventory_result(&repository, gate_id);
        assert!(!result.state().admits());
        assert!(
            result
                .reason()
                .contains("authoritative split module spice/src/config/registry.rs is absent"),
            "{gate_id} did not name the missing split registry: {}",
            result.reason()
        );
    }

    repository.remove();
}

fn focused_inventory_result(repository: &Tree, gate_id: GateId) -> gate::BatteryArmResult {
    let mut capture = BatteryCapture::capture_arm(
        &repository.root,
        &Selector::staged(),
        ClaimCapture::Idle,
        ActorCapture::Acting {
            actor: "configuration-inventory-fixture".to_owned(),
            bound_claims: Vec::new(),
        },
        RimCapture::new(&repository.root, None, Vec::new())
            .expect("fixture repository root is absolute"),
        HygieneConfiguration::current(),
        gate_id,
    )
    .unwrap_or_else(|error| panic!("capture focused {gate_id}: {error}"));
    capture
        .execute_arm(gate_id)
        .unwrap_or_else(|error| panic!("execute focused {gate_id}: {error}"))
}

fn write_registry_decoy(repository: &Tree) {
    repository.write(
        "spice/src/registry_decoy.rs",
        concat!(
            "macro_rules! configuration_registry { ($($tokens:tt)*) => {}; }\n",
            "configuration_registry! {\n",
            "    fixed \"vendor.codex.fast_mode\" {\n",
            "        kind: Boolean,\n",
            "        builtin: Some(Builtin::Boolean(false)),\n",
            "        layers: LOCAL,\n",
            "        class: Policy,\n",
            "        flag: None,\n",
            "    }\n",
            "    fixed \"decoy.enabled\" {\n",
            "        kind: Boolean,\n",
            "        builtin: Some(Builtin::Boolean(false)),\n",
            "        layers: TRACKED,\n",
            "        class: Policy,\n",
            "        flag: None,\n",
            "    }\n",
            "}\n",
        ),
    );
}

fn untracked_trust_repository(label: &str) -> Tree {
    let repository = contract_repository(label);
    repository.write(
        "spice/src/config.rs",
        concat!(
            "configuration_registry! {\n",
            "    fixed \"lane.vendor\" {\n",
            "        kind: String,\n",
            "        builtin: Some(Builtin::String(\"codex\")),\n",
            "        layers: TRACKED_FLAG,\n",
            "        class: Policy,\n",
            "        flag: None,\n",
            "    }\n",
            "    fixed \"vendor.codex.binary\" {\n",
            "        kind: String,\n",
            "        builtin: Some(Builtin::String(\"codex\")),\n",
            "        layers: LOCAL,\n",
            "        class: Policy,\n",
            "        flag: None,\n",
            "    }\n",
            "    fixed \"say.command\" {\n",
            "        kind: StringList,\n",
            "        builtin: Some(Builtin::Strings(&[])),\n",
            "        layers: LOCAL,\n",
            "        class: MachineLocal,\n",
            "        flag: None,\n",
            "    }\n",
            "}\n",
        ),
    );
    git_commit(&repository, "tracked-trust inventory fixture");
    repository
}

fn tracked_trust_evidence(arm: &CoreArmReport) -> &ConfigInventoryEvidence {
    let CoreObservation::Measured {
        evidence: CoreEvidence::ConfigTrackedTrust(evidence),
    } = arm.observation()
    else {
        panic!("tracked-trust acquisition must produce inventory evidence")
    };
    evidence
}
