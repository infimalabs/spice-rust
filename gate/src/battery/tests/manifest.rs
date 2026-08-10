//! The compiled manifest contract: exactly 27 canonical arms, or nothing.

use super::*;

#[test]
fn canonical_manifest_is_the_exact_27_arm_compiled_contract() {
    let manifest = BatteryManifest::canonical().expect("canonical manifest");
    assert_eq!(manifest.version(), GATE_BATTERY_VERSION);
    assert_eq!(manifest.predecessor_release(), PREDECESSOR_GATE_RELEASE);
    assert_eq!(manifest.arms().len(), 27);

    for (position, (arm, expected)) in manifest.arms().iter().zip(GateId::BATTERY).enumerate() {
        assert_eq!(
            arm.predecessor(),
            expected
                .predecessor_declared()
                .then(|| expected.predecessor_name())
        );
        let ArmDisposition::Compiled {
            gate_id,
            family,
            declaration,
        } = arm.disposition();
        assert_eq!(*gate_id, expected, "canonical arm {position}");
        assert_eq!(
            (*family, declaration.clone()),
            declaration_identity(expected).expect("compiled identity")
        );
    }
}

#[test]
fn manifest_rejects_every_noncanonical_shape_and_runtime_escape() {
    let source = CANONICAL_BATTERY_MANIFEST;
    let last = source.rfind("[[arms]]").expect("last arm marker");
    let missing = source[..last].to_owned();
    let duplicate = source.replacen(
        "predecessor = \"plan-phase\"",
        "predecessor = \"merge-integrity\"",
        1,
    );
    let reordered = source
        .replacen(
            "predecessor = \"merge-integrity\"",
            "predecessor = \"temporary-arm\"",
            1,
        )
        .replacen(
            "predecessor = \"plan-phase\"",
            "predecessor = \"merge-integrity\"",
            1,
        )
        .replacen(
            "predecessor = \"temporary-arm\"",
            "predecessor = \"plan-phase\"",
            1,
        );
    let unknown = source.replacen(
        "gate_id = \"merge-integrity\"",
        "gate_id = \"foreign-gate\"",
        1,
    );
    let shell_backed = source.replacen(
        "disposition = \"compiled\"",
        "disposition = \"shell-backed\"",
        1,
    );
    let indirect = source.replacen(
        "disposition = \"compiled\"",
        "disposition = \"indirect\"",
        1,
    );
    let alternate_runtime = source.replacen(
        "disposition = \"compiled\"",
        "disposition = \"alternate-runtime\"",
        1,
    );
    // A complete, internally valid refusal arm: every field the deleted
    // `migration-refusal` disposition required, including the owning release its
    // own validator demanded. This decoded before that disposition was deleted,
    // so the case fails the moment the variant returns as an untested path.
    let migration_refusal = source.replacen(
        concat!(
            "disposition = \"compiled\"\n",
            "gate_id = \"merge-integrity\"\n",
            "family = \"core\"\n",
            "declaration = \"gate::core::merge-integrity::merge-tree-comparison-v1\""
        ),
        concat!(
            "disposition = \"migration-refusal\"\n",
            "source_field = [\"battery\", \"merge-integrity\"]\n",
            "owning_release = \"0.31.0\"\n",
            "reason = \"predecessor arm has no compiled successor\"\n",
            "recovery = \"run the 0.31.0 battery for this arm\""
        ),
        1,
    );
    let unknown_shell_field = source.replacen(
        "declaration = \"gate::core::merge-integrity::merge-tree-comparison-v1\"",
        concat!(
            "declaration = \"gate::core::merge-integrity::merge-tree-comparison-v1\"\n",
            "shell_command = \"legacy-check\""
        ),
        1,
    );

    for (name, candidate) in [
        ("missing", missing),
        ("duplicate", duplicate),
        ("reordered", reordered),
        ("unknown", unknown),
        ("shell-backed", shell_backed),
        ("indirect", indirect),
        ("alternate-runtime", alternate_runtime),
        ("migration-refusal", migration_refusal),
        ("unknown-shell-field", unknown_shell_field),
    ] {
        assert_ne!(
            candidate, source,
            "{name} candidate must differ from the canonical manifest to test anything"
        );
        assert!(
            BatteryManifest::decode(&candidate).is_err(),
            "{name} manifest must be refused"
        );
    }
}
