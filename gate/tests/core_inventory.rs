use std::fs;
use std::path::Path;

use gate::{
    ActorCapture, ClaimCapture, ConfigInventoryEvidence, ConfigKeyEvidence, CoreArmReport,
    CoreEvidence, CoreFinding, CoreObservation, CoreStanding, GateId,
};

mod support;

use support::{
    Tree, acquired, acquired_arm, contract_repository, git_commit, git_repository, measured,
    workspace_document,
};

#[test]
fn configuration_key_admission_names_the_unknown_active_layer_key() {
    let repository = git_repository("core-acquire-config-key-unknown");
    repository.write(
        "spice.toml",
        concat!("[tasks]\n", "stems = ['rim']\n", "strms = ['misspelled']\n",),
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
    repository.write("spice.toml", "[say]\ncommand = ['/usr/bin/say']\n");
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
        concat!(
            "registry! {\n",
            "    fixed \"direct.enabled\" {\n",
            "        kind: Boolean,\n",
            "        builtin: Some(Builtin::Boolean(false)),\n",
            "        layers: TRACKED,\n",
            "        class: Policy,\n",
            "        flag: None,\n",
            "    }\n",
            "    fixed \"renamed.enabled\" {\n",
            "        kind: Boolean,\n",
            "        builtin: Some(Builtin::Boolean(false)),\n",
            "        layers: TRACKED,\n",
            "        class: Policy,\n",
            "        flag: None,\n",
            "    }\n",
            "    fixed \"qualified.enabled\" {\n",
            "        kind: Boolean,\n",
            "        builtin: Some(Builtin::Boolean(false)),\n",
            "        layers: TRACKED,\n",
            "        class: Policy,\n",
            "        flag: None,\n",
            "    }\n",
            "    fixed \"vendor.codex.fast_mode\" {\n",
            "        kind: Boolean,\n",
            "        builtin: Some(Builtin::Boolean(false)),\n",
            "        layers: TRACKED,\n",
            "        class: Policy,\n",
            "        flag: None,\n",
            "    }\n",
            "}\n",
        ),
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
fn current_false_disable_registry_and_consumer_are_one_inventory() {
    let repository = contract_repository("core-false-disable-current");
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    for path in ["spice/src/config.rs", "spice/src/agent_launch.rs"] {
        let source = fs::read_to_string(workspace.join(path))
            .unwrap_or_else(|error| panic!("read current {path}: {error}"));
        repository.write(path, &source);
    }
    git_commit(&repository, "current false-disable sources");

    let report = acquired(&repository, ClaimCapture::Idle, ActorCapture::Absent);
    let arm = acquired_arm(&report, GateId::ConfigFalseDisable);
    let evidence = false_disable_evidence(arm);
    let expected = [config_path(&["vendor", "codex", "fast_mode"])];

    assert_eq!(evidence.declared(), expected);
    assert_eq!(evidence.observed(), expected);
    assert_eq!(arm.standing(), CoreStanding::Passed);

    repository.remove();
}

fn false_disable_repository(label: &str, honored: bool) -> Tree {
    let repository = contract_repository(label);
    let alpha = if honored {
        concat!(
            "    fixed \"alpha.feature.enabled\" {\n",
            "        kind: Boolean,\n",
            "        builtin: Some(Builtin::Boolean(false)),\n",
            "        layers: TRACKED,\n",
            "        class: Policy,\n",
            "        flag: None,\n",
            "    }\n",
        )
    } else {
        ""
    };
    let config = [
        concat!(
            "registry! {\n",
            "    fixed \"vendor.codex.fast_mode\" {\n",
            "        kind: Boolean,\n",
            "        builtin: Some(Builtin::Boolean(false)),\n",
            "        layers: TRACKED,\n",
            "        class: Policy,\n",
            "        flag: None,\n",
            "    }\n",
        ),
        alpha,
        "}\n",
    ]
    .concat();
    repository.write("spice/src/config.rs", &config);
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
    let config = fs::read_to_string(workspace.join("spice/src/config.rs"))
        .expect("read current configuration registry");
    repository.write("spice/src/config.rs", &config);
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
            .contains(&config_path(&["say", "command"]))
    );
    assert!(evidence.violations().is_empty());
    assert_eq!(arm.standing(), CoreStanding::Passed);

    repository.remove();
}

fn untracked_trust_repository(label: &str) -> Tree {
    let repository = contract_repository(label);
    repository.write(
        "spice/src/config.rs",
        concat!(
            "registry! {\n",
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
