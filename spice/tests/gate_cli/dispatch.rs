//! Product-dispatcher ordering and policy-boundary process proofs.

use super::*;

struct PeerCargo(Vec<std::process::Child>);

impl PeerCargo {
    fn start(program: &Path, count: usize) -> Self {
        let children = (0..count)
            .map(|_| {
                fixture_command(program)
                    .arg("__peer-load")
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .expect("launch continuous peer Cargo fixture")
            })
            .collect();
        Self(children)
    }
}

impl Drop for PeerCargo {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn normal_git_commit_with_the_installed_gate_creates_exactly_one_commit_inside_180_seconds() {
    let repository = GateRepository::new();
    repository.write_source(2);
    repository.add("src/lib.rs");
    repository.install_pre_commit();
    let before = git_stdout(repository.path(), &["rev-list", "--count", "HEAD"]);
    let started = Instant::now();

    let output = fixture_command("/usr/bin/git")
        .args(["commit", "-qm", "Exercise bounded commit gate"])
        .current_dir(repository.path())
        .output()
        .expect("launch normal fixture commit");

    assert!(
        output.status.success(),
        "normal commit failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        started.elapsed() < Duration::from_secs(180),
        "normal commit crossed the fixed 180-second gate deadline"
    );
    let after = git_stdout(repository.path(), &["rev-list", "--count", "HEAD"]);
    assert_eq!(
        String::from_utf8(before)
            .unwrap()
            .trim()
            .parse::<u64>()
            .unwrap()
            + 1,
        String::from_utf8(after)
            .unwrap()
            .trim()
            .parse::<u64>()
            .unwrap(),
        "one successful gate permits exactly one commit",
    );
    assert_eq!(
        String::from_utf8(git_stdout(
            repository.path(),
            &["show", "-s", "--format=%s", "HEAD"],
        ))
        .unwrap()
        .trim(),
        "Exercise bounded commit gate",
    );
    assert!(
        git_stdout(repository.path(), &["diff", "--cached", "--name-only"]).is_empty(),
        "the successful commit left staged bytes behind"
    );
    let state = gate::CurrentState::load(gate::current_state_path(&repository.path().join(".git")))
        .expect("accepted ordinary gate published current state");
    assert_eq!(state.before().version(), gate::state::STATE_LEDGER_VERSION);
    assert!(
        repository
            .path()
            .join(".git/.spice-rs/data/gate-state.json")
            .is_file(),
        "ordinary change-gate acceptance is the publication boundary"
    );
}

#[test]
fn resolution_worker_exhausts_real_authority_change_while_ordinary_selection_refuses_it() {
    let repository = GateRepository::new();
    let authority = repository.path().join("spice-rust.toml");
    let mut policy = fs::read_to_string(&authority).expect("read fixture policy authority");
    policy.push_str("\n# allocator-owned resolution fixture\n");
    fs::write(&authority, policy).expect("move fixture policy authority");
    repository.add("spice-rust.toml");

    let ordinary = run_commit_worker(repository.path());
    let Some((ordinary_kind, ordinary_document)) = ordinary.terminal.split_first() else {
        panic!("ordinary authority selection published no terminal");
    };
    assert_eq!(*ordinary_kind, b'F');
    let ordinary: serde_json::Value =
        serde_json::from_slice(ordinary_document).expect("decode ordinary authority refusal");
    assert_eq!(ordinary["reason"], "gate::change-selection-refused");

    let resolution = run_resolution_worker(repository.path());
    let Some((resolution_kind, resolution_document)) = resolution.terminal.split_first() else {
        panic!("resolution authority evidence published no terminal");
    };
    assert_eq!(
        *resolution_kind,
        b'R',
        "resolution worker did not admit exhaustive authority evidence: {}",
        String::from_utf8_lossy(resolution_document),
    );
    let report: serde_json::Value =
        serde_json::from_slice(resolution_document).expect("decode resolution report");
    assert_eq!(report["version"], 2);
    assert!(report.get("packaged").is_some());
    assert!(
        resolution.progress.contains("[rust-test-selection]"),
        "exhaustive authority evidence omitted configured workspace tests: {}",
        resolution.progress,
    );

    let configured = fixture_command(env!("CARGO_BIN_EXE_spice"))
        .arg("__gate-resolution-selection-worker")
        .current_dir(repository.path())
        .env(
            "SPICE_RS_PRE_COMMIT_SOURCE", // env-policy: allow
            report["source"]
                .as_str()
                .expect("resolution report carries string source"),
        )
        .env(
            "SPICE_RS_PRE_COMMIT_PATHS", // env-policy: allow
            serde_json::to_string(&["spice-rust.toml"]).expect("encode configured authority paths"),
        )
        .output()
        .expect("launch private configured resolution selector");
    assert!(
        configured.status.success(),
        "private configured resolution selector failed: stdout={} stderr={}",
        String::from_utf8_lossy(&configured.stdout),
        String::from_utf8_lossy(&configured.stderr),
    );
}

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
fn disabled_built_ins_probe_evidence_then_reach_only_the_configured_battery() {
    let repository = dispatch_repository(Some(false));

    let boundary = run_commit_worker(repository.path());

    assert_eq!(
        terminal_failure_reason(&boundary),
        "gate::pre-commit-configuration-untrusted"
    );
    let selection = boundary
        .progress
        .find("{\"phase\":\"test-selection\"}")
        .expect("configured-only dispatch probes full-verifier evidence");
    assert!(!boundary.progress.contains("[change-gate-selection-report]"));
    let configured = boundary
        .progress
        .find("{\"phase\":\"configured\",\"steps\":1}")
        .expect("configured battery reaches its trust boundary");
    assert!(selection < configured, "evidence lookup precedes fallback");
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
    assert!(boundary.progress.contains("test-selection"));
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
        .env_remove(PRE_COMMIT_PATHS)
        .env_remove(PRE_COMMIT_SOURCE)
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
#[allow(
    clippy::disallowed_methods,
    reason = "the configured process-boundary fixture must preserve the invoking test PATH"
)]
fn configured_source_gate_composes_a_concurrent_state_writer_under_continuous_peer_cargo() {
    let repository = configured_source_gate_repository();
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
    grant_fixture_trust(&repository, digest);

    let scratch = Scratch::new("configured-state-composition");
    let marker = scratch.path().join("selected-tests.log");
    repository.write_selected_test_with_state_writer(&marker);
    repository.add("src/lib.rs");
    repository.install_pre_commit();

    let real_cargo =
        std::env::split_paths(&std::env::var_os("PATH").expect("test process carries PATH"))
            .map(|directory| directory.join("cargo"))
            .find(|candidate| candidate.is_file())
            .expect("locate real Cargo on PATH");
    assert!(!real_cargo.to_string_lossy().contains('\''));
    let programs = scratch.path().join("programs");
    fs::create_dir(&programs).expect("create configured source-gate program directory");
    let cargo = programs.join("cargo");
    executable(
        &cargo,
        &format!(
            concat!(
                "#!/bin/sh\n",
                "if [ \"${{1-}}\" = \"__peer-load\" ]; then read _; exit 0; fi\n",
                "if [ \"$*\" = \"run --quiet --locked --package spice --bin spice -- gate selection\" ]; then\n",
                "  exec '{}' gate selection\n",
                "fi\n",
                "exec '{}' \"$@\"\n",
            ),
            env!("CARGO_BIN_EXE_spice"),
            real_cargo.display(),
        ),
    );
    let mut path_entries = vec![programs];
    path_entries.extend(std::env::split_paths(
        &std::env::var_os("PATH").expect("test process carries PATH"),
    ));
    let path = std::env::join_paths(path_entries).expect("compose configured source-gate path");
    let peers = PeerCargo::start(&cargo, 2);
    let started = Instant::now();

    let output = fixture_command("/usr/bin/git")
        .args(["commit", "-qm", "Compose concurrent gate-state publication"])
        .current_dir(repository.path())
        .env("PATH", path)
        .env_remove(PRE_COMMIT_PATHS)
        .env_remove(PRE_COMMIT_SOURCE)
        .output()
        .expect("commit through configured source gate");

    assert!(
        output.status.success(),
        "configured source gate failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        started.elapsed() < Duration::from_secs(180),
        "configured source gate crossed its parent deadline",
    );
    assert!(
        peers.0.iter().all(|peer| process_exists(peer.id())),
        "peer Cargo fixtures did not remain live through pre-commit completion",
    );
    assert_eq!(
        fs::read_to_string(marker).expect("read selected-test executions"),
        "canonical selected target executed\n",
        "conflict-aware state publication reran an already-complete selected test",
    );
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

fn configured_source_gate_repository() -> GateRepository {
    let repository = GateRepository::new();
    let policy = repository.path().join("spice-rust.toml");
    let text = fs::read_to_string(&policy).expect("read fixture policy");
    let configured = concat!(
        "[policy]\n",
        "pre_commit_builtins = false\n",
        "pre_commit = [{ label = \"Spice source gate\", run = [\"cargo\", \"run\", \"--quiet\", \"--locked\", \"--package\", \"spice\", \"--bin\", \"spice\", \"--\", \"gate\", \"selection\"] }]\n",
    );
    fs::write(&policy, text.replacen("[policy]\n", configured, 1))
        .expect("write configured source-gate policy");
    repository.add("spice-rust.toml");
    git(
        repository.path(),
        &["commit", "-qm", "Configure source gate"],
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
