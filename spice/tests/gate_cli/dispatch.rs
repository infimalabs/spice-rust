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
fn source_override_refuses_its_exact_capability_without_creating_trust() {
    let repository = source_override_repository();
    let discovered = spice::repository::Repository::discover(repository.path())
        .expect("discover source-policy fixture");
    let journal = discovered
        .admit_state()
        .expect("admit source-policy fixture state")
        .journal()
        .to_path_buf();
    assert!(!journal.exists(), "fixture began with trust authority");

    let boundary = run_commit_worker(repository.path());

    let failure = terminal_failure(&boundary);
    assert_eq!(
        failure["reason"],
        "gate::pre-commit-configuration-untrusted"
    );
    let detail = failure["detail"]
        .as_str()
        .expect("trust refusal carries exact capability detail");
    for evidence in [
        "standing=absent",
        "provenance=tracked",
        "spice-rust.toml",
        "Spice source gate",
        "cargo",
        "--package",
        "spice",
        "gate",
        "selection",
        "spice trust grant",
    ] {
        assert!(detail.contains(evidence), "missing {evidence:?}: {detail}");
    }
    assert!(!boundary.progress.contains("test-selection"));
    assert!(!boundary.progress.contains("[change-gate-selection-report]"));
    assert!(
        boundary
            .progress
            .contains("{\"phase\":\"configured\",\"steps\":1}")
    );
    assert!(!journal.exists(), "the refusal created trust authority");
}

#[test]
fn explicitly_trusted_source_override_runs_one_public_spice_leaf_through_the_hook() {
    let repository = source_override_repository();
    let first = run_commit_worker(repository.path());
    let failure = terminal_failure(&first);
    let detail = failure["detail"]
        .as_str()
        .expect("trust refusal carries the exact digest");
    let digest = detail
        .split_once("digest=")
        .and_then(|(_, remainder)| remainder.split_whitespace().next())
        .expect("trust refusal names digest")
        .parse::<Digest>()
        .expect("trust refusal digest is canonical");
    grant_fixture_trust(&repository, digest.clone());
    repository.install_pre_commit();

    let programs = repository.path().join("programs");
    fs::create_dir(&programs).expect("create source-gate executable path");
    let log = repository.path().join("source-gate.log");
    executable(
        &programs.join("cargo"),
        &format!(
            concat!(
                "#!/bin/sh\n",
                "printf '%s|%s|%s\\n' \"$*\" \"$SPICE_RS_PRE_COMMIT_SOURCE\" ",
                "\"$SPICE_RS_PRE_COMMIT_PATHS\" >> '{}'\n",
            ),
            log.display(),
        ),
    );
    let path = std::env::join_paths([programs, PathBuf::from("/usr/bin"), PathBuf::from("/bin")])
        .expect("compose source-gate executable path");

    let output = fixture_command("/usr/bin/git")
        .args(["commit", "-qm", "Exercise the trusted source gate"])
        .current_dir(repository.path())
        .env("PATH", path)
        .output()
        .expect("commit through the installed Rust hook");

    assert!(
        output.status.success(),
        "trusted source hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    let invocation = fs::read_to_string(log).expect("read source-gate invocation");
    let fields = invocation.trim_end().split('|').collect::<Vec<_>>();
    assert_eq!(
        fields.len(),
        3,
        "unexpected source-gate log: {invocation:?}"
    );
    assert_eq!(
        fields[0],
        "run --quiet --locked --package spice --bin spice -- gate selection"
    );
    let staged_source = fields[1]
        .parse::<Digest>()
        .expect("configured source handoff is a canonical digest");
    assert_ne!(
        staged_source, digest,
        "the staged-tree identity must remain distinct from configuration trust",
    );
    assert_eq!(fields[2], "[\"src/lib.rs\"]");
}

#[test]
fn recursion_marker_refuses_public_and_hidden_dispatcher_entrypoints() {
    let scratch = Scratch::new("dispatcher-recursion");
    let source = wire::Digest::compute(wire::Domain::GateTree, b"recursive fixture");
    let variable = wire::SpiceVariable::PreCommitSource.as_str();
    let public = fixture_command(env!("CARGO_BIN_EXE_spice"))
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
    let hidden = fixture_command(env!("CARGO_BIN_EXE_spice"))
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

fn source_override_repository() -> GateRepository {
    let repository = GateRepository::new();
    fs::write(
        repository.path().join("spice-rust.toml"),
        include_str!("../../../spice-rust.toml"),
    )
    .expect("write the exact source-repository policy");
    repository.add("spice-rust.toml");
    git(
        repository.path(),
        &["commit", "-qm", "Adopt the source-repository gate"],
    );
    repository.write_source(2);
    repository.add("src/lib.rs");
    repository
}

fn grant_fixture_trust(repository: &GateRepository, digest: Digest) {
    let discovered = spice::repository::Repository::discover(repository.path())
        .expect("discover trusted source-policy fixture");
    let state = discovered
        .admit_state()
        .expect("admit trusted source-policy fixture state");
    fs::create_dir_all(state.data()).expect("create fixture trust authority parent");
    let mut authority = journal::Journal::open(state.journal()).expect("open fixture authority");
    let mut write = journal::JournalWrite::new(
        wire::Actor::Spice,
        "recovery:source-gate-trust-fixture"
            .parse()
            .expect("fixture origin"),
        "2030-08-14T12:00:00.000Z".parse().expect("fixture instant"),
        wire::Producer::CommandDoor,
    );
    let pended = write
        .fact(
            wire::body::TrustPended {
                surface: "configuration".to_owned(),
                digest: digest.clone(),
                previous: None,
            },
            journal::FactCause::Root,
        )
        .expect("build fixture trust fact");
    write.trust_head(pended, move |_| wire::TrustHead {
        digest,
        state: wire::TrustState::Granted,
    });
    authority.apply(write).expect("commit fixture trust grant");
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
