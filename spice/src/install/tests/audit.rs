use super::*;

#[test]
fn missing_stale_foreign_and_current_receipts_are_distinct() {
    let scratch = Scratch::new("appliance-audit-receipts");
    let repository = scratch.repository();
    let current = scratch.binary(b"current appliance");

    let missing = audit_appliance(&repository, &current).expect("audit missing receipt");
    assert_eq!(missing.state(), ApplianceState::MissingReceipt);
    let preview = preview(&current);
    assert_eq!(missing.repair(), Some(preview.as_str()));

    fs::create_dir_all(record_path(&repository).unwrap().parent().unwrap()).unwrap();
    fs::write(record_path(&repository).unwrap(), b"{}\n").unwrap();
    let stale = audit_appliance(&repository, &current).expect("audit stale receipt");
    assert_eq!(stale.state(), ApplianceState::StaleReceipt);
    assert!(stale.detail().contains("install record did not decode"));
    assert_eq!(stale.repair(), Some(preview.as_str()));

    fs::remove_file(record_path(&repository).unwrap()).unwrap();
    let other = scratch.executable("build/other-spice", b"other appliance");
    install(&repository, &other);
    let foreign = audit_appliance(&repository, &current).expect("audit another build");
    assert_eq!(foreign.state(), ApplianceState::AnotherBuild);
    assert!(
        foreign
            .detail()
            .contains("differs from current build digest")
    );
    assert!(foreign.receipt().is_some());
    assert_eq!(foreign.repair(), Some(preview.as_str()));

    let refresh = plan(&repository, Some(&current)).expect("plan current refresh");
    apply(&repository, Some(&current), &refresh.digest().unwrap()).expect("apply current refresh");
    let admitted = audit_appliance(&repository, &current).expect("audit current appliance");
    assert_eq!(admitted.state(), ApplianceState::Current);
    assert!(admitted.is_current());
    assert_eq!(admitted.repair(), None);
}

#[test]
fn development_guidance_suppression_does_not_weaken_installed_client_audit() {
    let scratch = Scratch::new("appliance-audit-development-policy");
    fs::write(
        scratch.path().join("spice-rust.toml"),
        "[policy]\nsuppress_install_apply_guidance = true\n",
    )
    .expect("the source repository opts out of development guidance");
    let repository = scratch.repository();
    let current = scratch.binary(b"current client");
    let installed = scratch.executable("build/installed-client", b"installed client");
    install(&repository, &installed);

    let audit = audit_appliance(&repository, &current)
        .expect("the installed-client audit remains available");

    assert_eq!(audit.state(), ApplianceState::AnotherBuild);
    assert_eq!(audit.repair(), Some(preview(&current).as_str()));
}

#[test]
fn missing_tampered_binary_and_tampered_hook_are_distinct() {
    let binary_scratch = Scratch::new("appliance-audit-binary");
    let binary_repository = binary_scratch.repository();
    let binary_current = binary_scratch.binary(b"binary appliance");
    install(&binary_repository, &binary_current);

    fs::write(installed_binary(&binary_repository), b"tampered appliance").unwrap();
    let binary =
        audit_appliance(&binary_repository, &binary_current).expect("audit binary tampering");
    assert_eq!(binary.state(), ApplianceState::BinaryTampered);
    assert_eq!(binary.repair(), Some(preview(&binary_current).as_str()));

    let missing_scratch = Scratch::new("appliance-audit-missing-binary");
    let missing_repository = missing_scratch.repository();
    let missing_current = missing_scratch.binary(b"missing appliance");
    install(&missing_repository, &missing_current);
    fs::remove_file(installed_binary(&missing_repository)).unwrap();
    let missing =
        audit_appliance(&missing_repository, &missing_current).expect("audit missing binary");
    assert_eq!(missing.state(), ApplianceState::MissingBinary);
    assert_eq!(missing.repair(), Some(preview(&missing_current).as_str()));

    let hook_scratch = Scratch::new("appliance-audit-hook");
    let hook_repository = hook_scratch.repository();
    let hook_binary = hook_scratch.binary(b"hook appliance");
    install(&hook_repository, &hook_binary);
    let hook = hook_repository
        .common_dir()
        .join(GIT_REFERENCE_TRANSACTION_PATH);
    fs::write(&hook, b"#!/bin/sh\nexit 0\n").unwrap();
    let hook = audit_appliance(&hook_repository, &hook_binary).expect("audit hook tampering");
    assert_eq!(hook.state(), ApplianceState::HookTampered);
    assert!(hook.detail().contains(GIT_REFERENCE_TRANSACTION_PATH));
    assert_eq!(hook.repair(), Some(preview(&hook_binary).as_str()));
}

fn install(repository: &Repository, binary: &Path) {
    let planned = plan(repository, Some(binary)).expect("plan fixture appliance");
    apply(repository, Some(binary), &planned.digest().unwrap()).expect("install fixture appliance");
}

fn preview(binary: &Path) -> String {
    shell_words::join([binary.to_str().unwrap(), "install"])
}
