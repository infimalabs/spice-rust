//! Product-dispatcher ordering and policy-boundary process proofs.

use super::*;

#[test]
fn packaged_built_in_runs_before_configured_trust_admission() {
    let repository = dispatch_repository(None);

    let boundary = run_commit_worker(repository.path());

    assert_eq!(
        terminal_failure_reason(&boundary),
        "gate::pre-commit-configuration-untrusted"
    );
    let selection = boundary
        .progress
        .find("{\"phase\":\"test-selection\"}")
        .expect("enabled packaged battery reaches test selection");
    let configured = boundary
        .progress
        .find("{\"phase\":\"configured\",\"steps\":1}")
        .expect("configured battery reaches its trust boundary");
    assert!(
        selection < configured,
        "packaged work must precede configured work"
    );
    assert_eq!(
        boundary
            .progress
            .matches("[change-gate-selection-report]")
            .count(),
        1
    );
}

#[test]
fn disabled_built_ins_reach_only_the_configured_battery() {
    let repository = dispatch_repository(Some(false));

    let boundary = run_commit_worker(repository.path());

    assert_eq!(
        terminal_failure_reason(&boundary),
        "gate::pre-commit-configuration-untrusted"
    );
    assert!(!boundary.progress.contains("test-selection"));
    assert!(!boundary.progress.contains("[change-gate-selection-report]"));
    assert!(
        boundary
            .progress
            .contains("{\"phase\":\"configured\",\"steps\":1}")
    );
}

#[test]
fn disabled_built_ins_without_configured_entries_refuse_before_work() {
    let repository = GateRepository::new();
    let policy = repository.path().join("spice-rust.toml");
    let text = fs::read_to_string(&policy).expect("read fixture policy");
    fs::write(
        &policy,
        text.replacen("[policy]\n", "[policy]\npre_commit_builtins = false\n", 1),
    )
    .expect("disable the complete fixture battery");
    repository.add("spice-rust.toml");
    git(
        repository.path(),
        &["commit", "-qm", "Disable every pre-commit entry"],
    );
    repository.write_source(2);
    repository.add("src/lib.rs");

    let boundary = run_commit_worker(repository.path());

    assert_eq!(
        terminal_failure_reason(&boundary),
        "gate::pre-commit-policy-refused"
    );
    assert!(!boundary.progress.contains("test-selection"));
    assert!(!boundary.progress.contains("configured"));
}

#[test]
fn recursion_marker_refuses_public_and_hidden_dispatcher_entrypoints() {
    let scratch = Scratch::new("dispatcher-recursion");
    let source = wire::Digest::compute(wire::Domain::GateTree, b"recursive fixture");
    let variable = wire::SpiceVariable::PreCommitSource.as_str();
    let public = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args(["gate", "pre-commit"])
        .current_dir(scratch.path())
        .env(variable, source.to_string())
        .env_remove(steer::hook::INBOX)
        .output()
        .expect("invoke public recursive dispatcher");
    assert_eq!(public.status.code(), Some(REFUSAL_EXIT));
    assert!(public.stdout.is_empty());
    assert!(String::from_utf8_lossy(&public.stderr).contains("gate::pre-commit-recursion"));

    let progress = scratch.path().join("progress.jsonl");
    let terminal = scratch.path().join("terminal.bin");
    fs::write(&progress, []).expect("create recursion progress file");
    fs::write(&terminal, []).expect("create recursion terminal file");
    let hidden = Command::new(env!("CARGO_BIN_EXE_spice"))
        .arg("__gate-pre-commit-worker")
        .arg(&progress)
        .arg(&terminal)
        .current_dir(scratch.path())
        .env(variable, source.to_string())
        .env_remove(steer::hook::INBOX)
        .output()
        .expect("invoke hidden recursive dispatcher");
    assert_eq!(hidden.status.code(), Some(REFUSAL_EXIT));
    assert!(hidden.stdout.is_empty());
    assert!(String::from_utf8_lossy(&hidden.stderr).contains("gate::pre-commit-recursion"));
    assert!(
        fs::read(&terminal)
            .expect("read recursion terminal")
            .is_empty()
    );
}

fn dispatch_repository(builtins: Option<bool>) -> GateRepository {
    let repository = GateRepository::new();
    let policy = repository.path().join("spice-rust.toml");
    let text = fs::read_to_string(&policy).expect("read fixture policy");
    let switch = builtins.map_or_else(String::new, |enabled| {
        format!("pre_commit_builtins = {enabled}\n")
    });
    let insertion = format!(
        concat!(
            "[policy]\n",
            "{switch}",
            "pre_commit = [{{ label = \"configured fixture\", run = [\"/bin/true\"] }}]\n",
        ),
        switch = switch,
    );
    fs::write(&policy, text.replacen("[policy]\n", &insertion, 1))
        .expect("write configured fixture policy");
    repository.add("spice-rust.toml");
    git(
        repository.path(),
        &["commit", "-qm", "Configure fixture pre-commit"],
    );
    repository.write_source(2);
    repository.add("src/lib.rs");
    repository
}

fn terminal_failure_reason(boundary: &WorkerBoundary) -> String {
    terminal_failure(boundary)["reason"]
        .as_str()
        .expect("worker failure carries a reason")
        .to_owned()
}

fn terminal_failure(boundary: &WorkerBoundary) -> serde_json::Value {
    let Some((kind, document)) = boundary.terminal.split_first() else {
        panic!("dispatcher published no terminal: {boundary:?}");
    };
    assert_eq!(
        *kind, b'F',
        "dispatcher unexpectedly admitted: {boundary:?}"
    );
    serde_json::from_slice(document).expect("worker terminal is a typed failure")
}
