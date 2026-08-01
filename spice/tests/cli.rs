use std::process::Command;

fn spice() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spice"))
}

#[test]
fn root_help_exposes_the_generated_command_families() {
    let output = spice().arg("--help").output().expect("spice launches");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help is UTF-8");
    for family in ["board", "gate", "journal", "task", "up"] {
        assert!(
            stdout.contains(family),
            "root help omitted {family}: {stdout}"
        );
    }
}

#[test]
fn a_registered_unlanded_leaf_refuses_by_name() {
    let output = spice()
        .args(["task", "next"])
        .output()
        .expect("spice launches");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).expect("refusal is UTF-8"),
        "spice: release::not-landed; finish the board::next battery before Rust release one admits this leaf\n",
    );
}

#[test]
fn a_name_outside_the_registry_is_a_clap_refusal() {
    let output = spice()
        .arg("removed-command")
        .output()
        .expect("spice launches");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).expect("refusal is UTF-8");
    assert!(stderr.contains("unrecognized subcommand 'removed-command'"));
}

#[test]
fn a_required_contract_operand_is_enforced_at_the_process_boundary() {
    let output = spice()
        .args(["task", "add"])
        .output()
        .expect("spice launches");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("refusal is UTF-8");
    assert!(stderr.contains("required arguments were not provided"));
    assert!(stderr.contains("<TITLE>"));
    assert!(!stderr.contains("release::not-landed"));
}

#[test]
fn an_operand_outside_a_leaf_contract_is_rejected_by_clap() {
    let output = spice()
        .args(["doctor", "garbage"])
        .output()
        .expect("spice launches");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("refusal is UTF-8");
    assert!(stderr.contains("unexpected argument 'garbage'"));
    assert!(!stderr.contains("release::not-landed"));
}
