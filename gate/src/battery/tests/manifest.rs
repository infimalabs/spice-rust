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
