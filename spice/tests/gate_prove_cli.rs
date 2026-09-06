use std::path::Path;
use std::process::{Command, Output};

#[test]
fn prove_contract_uses_positional_action_and_unconditional_staged_source() {
    let contract = spice::cli::contracts()
        .iter()
        .find(|contract| contract.path == ["gate", "prove"])
        .expect("proof-aware runner contract is registered");
    assert_eq!(contract.syntax, "gate prove [ACTION] [--missing]");
    assert!(!contract.syntax.contains("--staged"));
    assert!(!contract.syntax.contains("--action"));

    for arguments in [
        ["spice", "gate", "prove"].as_slice(),
        ["spice", "gate", "prove", "formatters", "--missing"].as_slice(),
        ["spice", "gate", "prove", "--missing"].as_slice(),
        ["spice", "gate", "prove", "taste"].as_slice(),
        ["spice", "gate", "prove", "rust-typecheck"].as_slice(),
        ["spice", "gate", "prove", "cargo-tests"].as_slice(),
        ["spice", "gate", "prove", "canvas"].as_slice(),
        ["spice", "gate", "prove", "canvas:build"].as_slice(),
        ["spice", "gate", "prove", "canvas:shard:interaction"].as_slice(),
        [
            "spice",
            "gate",
            "prove",
            "cargo:typecheck:fixture::lib:fixture",
        ]
        .as_slice(),
    ] {
        spice::cli::command()
            .try_get_matches_from(arguments)
            .unwrap_or_else(|error| panic!("valid prove shape was refused: {error}"));
    }
}

#[test]
fn prove_help_and_extra_operands_follow_the_generated_contract() {
    let help = invoke(Path::new("."), &["gate", "prove", "--help"]);
    assert!(help.status.success(), "{}", streams(&help));
    assert!(
        String::from_utf8_lossy(&help.stdout)
            .contains("Usage: spice gate prove [ACTION] [--missing]"),
        "{}",
        streams(&help),
    );

    let extra = invoke(Path::new("."), &["gate", "prove", "one", "two"]);
    assert_eq!(extra.status.code(), Some(2), "{}", streams(&extra));
}

#[test]
fn repository_guidance_uses_proof_aware_completion_commands() {
    for (path, guidance) in [
        ("README.md", include_str!("../../README.md")),
        ("DEVELOPMENT.md", include_str!("../../DEVELOPMENT.md")),
        ("spice/agent/SKILL.md", include_str!("../agent/SKILL.md")),
    ] {
        for command in [
            "spice gate prove",
            "spice gate prove ACTION",
            "spice gate prove --missing",
        ] {
            assert!(guidance.contains(command), "{path} omitted {command}");
        }
    }
    let evidence = include_str!("../../docs/product/planes/gate-evidence.md");
    assert!(evidence.contains("Release 0.31.0"));
    assert!(evidence.contains("--spice-coverage-receipt"));
    assert!(evidence.contains("canvas:shard:<name>"));
    assert!(include_str!("../../README.md").contains("verifies its exact receipts"));
}

#[test]
fn prove_refuses_unknown_actions_before_repository_access() {
    let unknown = invoke(
        Path::new("."),
        &["--json", "gate", "prove", "not-a-gate", "--missing"],
    );
    assert_eq!(unknown.status.code(), Some(2), "{}", streams(&unknown));
    assert_reason(&unknown, "gate::prove-action-unregistered");
}

fn assert_reason(output: &Output, expected: &str) {
    let outcome: wire::Outcome<serde_json::Value> =
        wire::decode(&output.stderr).expect("prove refusal is typed");
    let wire::Outcome::Refusal(refusal) = outcome else {
        panic!("prove invocation did not refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason(), expected);
}

fn invoke(root: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spice"))
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("run Spice proof-aware gate CLI")
}

fn streams(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}
