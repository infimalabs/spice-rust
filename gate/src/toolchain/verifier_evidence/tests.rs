use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::process::{Command, Stdio};

use super::*;
use sill::ProcessGroup;

const AUTHORITY: &[u8] = b"one-time verifier authority fixture";
const CONTAINMENT_ROOT: &str = "GATE_VERIFIER_CONTAINMENT_ROOT";
const COMPILER_PID: &str = "GATE_VERIFIER_COMPILER_PID";
const DESCENDANT_PID: &str = "GATE_VERIFIER_DESCENDANT_PID";
// Recursive test-harness startup is outside the containment behavior under
// test. Leave it a deliberate margin while the fake compiler remains live
// well beyond the supervisor deadline.
const CONTAINMENT_STARTUP_ALLOWANCE: Duration = Duration::from_secs(5);
const CONTAINMENT_RUNTIME: Duration = Duration::from_secs(2);

fn source(label: &[u8]) -> Digest {
    Digest::compute(Domain::GateTree, label)
}

fn tool(name: &str, program: &str) -> ToolEvidence {
    ToolEvidence::Identified {
        name: format!("fixture {name}"),
        requested_program: name.to_owned(),
        resolved_program: program.to_owned(),
        version: format!("{name} fixture-version"),
        digest: Digest::compute(Domain::GateTool, program.as_bytes()).to_string(),
    }
}

fn contract(gate_id: GateId) -> VerifierCompilerContract {
    let tools = match gate_id {
        GateId::RustTypecheck => vec![tool("cargo", "/usr/bin/true")],
        GateId::CanvasWebTypecheck => vec![
            tool("cargo", "/usr/bin/true"),
            tool("trunk", "/usr/bin/true"),
        ],
        _ => panic!("fixture supports compiler arms only"),
    };
    VerifierCompilerContract::new(
        gate_id,
        source(b"fixture selected tree"),
        PathBuf::from("/fixture/repository"),
        PathBuf::from("/fixture/selection"),
        tools,
        Vec::new(),
    )
    .expect("fixture compiler contract")
}

fn complete_processes(contract: &VerifierCompilerContract) -> Vec<VerifierCompilerProcessEvidence> {
    expected_stages(contract.gate_id)
        .iter()
        .enumerate()
        .map(|(index, stage)| {
            VerifierCompilerProcessEvidence::mint(
                contract,
                *stage,
                compiler_environment(&contract.environment, &contract.repository),
                stage_arguments(*stage, &contract.repository),
                u64::try_from(index + 1).expect("small fixture duration"),
                Some(0),
            )
        })
        .collect()
}

fn admits(candidate: &VerifierCompilerEvidence, contract: &VerifierCompilerContract) -> bool {
    candidate.admits(
        contract.gate_id,
        &contract.source,
        &contract.repository,
        &contract.selection_root,
        &contract.environment,
        &contract.tools,
    )
}

fn first_process_mut(
    candidate: &mut VerifierCompilerEvidence,
) -> &mut VerifierCompilerProcessEvidence {
    candidate
        .processes
        .first_mut()
        .expect("complete evidence has a compiler process")
}

fn complete_evidence(contract: &VerifierCompilerContract) -> VerifierCompilerEvidence {
    contract
        .complete(&complete_processes(contract))
        .expect("complete fixture evidence")
}

#[test]
fn exact_complete_evidence_admits_each_compiler_arm() {
    for gate_id in [GateId::RustTypecheck, GateId::CanvasWebTypecheck] {
        let contract = contract(gate_id);
        let evidence = contract
            .complete(&complete_processes(&contract))
            .expect("complete evidence");

        assert!(evidence.admits(
            gate_id,
            &contract.source,
            &contract.repository,
            &contract.selection_root,
            &contract.environment,
            &contract.tools,
        ));
    }
}

#[test]
fn canvas_contract_uses_the_canonical_isolated_output_pair() {
    let fixture = contract(GateId::CanvasWebTypecheck);
    let canvas = VerifierCompilerContract::new(
        fixture.gate_id,
        fixture.source,
        fixture.repository,
        fixture.selection_root,
        fixture.tools,
        vec![
            (
                OsString::from(CARGO_TARGET_DIR),
                OsString::from("/foreign/shared-target"),
            ),
            (
                OsString::from(CARGO_BUILD_BUILD_DIR),
                OsString::from("/foreign/shared-build"),
            ),
        ],
    )
    .expect("canvas compiler contract");

    assert_eq!(
        environment_path(&canvas.environment, CARGO_TARGET_DIR),
        Some(canvas.repository.join("target/xtask-app"))
    );
    assert_eq!(
        environment_path(&canvas.environment, CARGO_BUILD_BUILD_DIR),
        Some(canvas.repository.join("target/xtask-app-build"))
    );
    let evidence = complete_evidence(&canvas);
    assert!(admits(&evidence, &canvas));
    assert!(evidence.admits(
        canvas.gate_id,
        &canvas.source,
        &canvas.repository,
        &canvas.selection_root,
        &[
            (
                OsString::from(CARGO_TARGET_DIR),
                OsString::from("/another/ambient-target"),
            ),
            (
                OsString::from(CARGO_BUILD_BUILD_DIR),
                OsString::from("/another/ambient-build"),
            ),
        ],
        &canvas.tools,
    ));
}

#[test]
fn ordinary_verifier_compilers_own_their_process_group() {
    assert_eq!(crate::measure::bounded_process_group(), ProcessGroup::New);
}

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "the recursive fixture passes its private root through one exact environment"
)]
fn pre_commit_deadline_contains_verifier_compiler_descendants() {
    if let Some(root) = env::var_os(CONTAINMENT_ROOT) {
        run_contained_verifier_fixture(Path::new(&root));
        return;
    }

    let root = crate::toolchain::exclusive_scratch_root("gate-verifier-containment");
    let compiler = root.join("fake-cargo");
    let compiler_pid = root.join("compiler.pid");
    let descendant_pid = root.join("descendant.pid");
    fs::write(
        &compiler,
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' \"$$\" > \"$GATE_VERIFIER_COMPILER_PID\"\n",
            "/bin/sleep 30 &\n",
            "child=$!\n",
            "printf '%s\\n' \"$child\" > \"$GATE_VERIFIER_DESCENDANT_PID\"\n",
            "wait\n",
        ),
    )
    .expect("write fake verifier compiler");
    let mut permissions = fs::metadata(&compiler)
        .expect("fake compiler metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&compiler, permissions).expect("make fake compiler executable");

    let mut environment = env::vars_os()
        .filter(|(name, _)| name != OsStr::new(CONTAINMENT_ROOT))
        .collect::<Vec<_>>();
    environment.push((
        OsString::from(CONTAINMENT_ROOT),
        root.as_os_str().to_owned(),
    ));
    let spec = Spec {
        argv: Argv::new(
            env::current_exe().expect("resolve gate test executable"),
            [
                OsString::from("pre_commit_deadline_contains_verifier_compiler_descendants"),
                OsString::from("--nocapture"),
                OsString::from("--test-threads=1"),
            ],
        )
        .expect("declare contained verifier fixture"),
        cwd: root.clone(),
        environment: Environment::new(environment).expect("capture fixture environment"),
        group: ProcessGroup::New,
        deadlines: Deadlines::new(
            CONTAINMENT_STARTUP_ALLOWANCE + CONTAINMENT_RUNTIME,
            Duration::from_millis(100),
            Duration::from_millis(100),
        )
        .expect("declare fixture deadlines"),
        wait: WaitPolicy::new(
            "pre-commit verifier containment fixture",
            Cancellation::Never,
            RestartDisposition::Never,
        )
        .expect("declare fixture wait policy"),
        output: Output::new(
            StdinPolicy::Null,
            StreamPolicy::Capture { limit: 1024 * 1024 },
            StreamPolicy::Capture { limit: 1024 * 1024 },
        ),
        redaction: Redaction::None,
        grace: Duration::from_millis(100),
    };
    let outcome = sill::spawn(spec)
        .expect("launch contained verifier fixture")
        .wait()
        .expect("terminate and reap contained verifier fixture");
    assert!(
        matches!(outcome.completion(), Completion::TimedOut(_)),
        "fixture did not reach its outer worker deadline: {:?}",
        outcome.completion()
    );
    assert_process_gone(&compiler_pid, "verifier compiler");
    assert_process_gone(&descendant_pid, "verifier compiler descendant");
    fs::remove_dir_all(root).expect("remove verifier containment fixture");
}

fn run_contained_verifier_fixture(root: &Path) {
    let _containment =
        crate::measure::inherit_process_group().expect("enter commit-gate containment");
    assert_eq!(
        crate::measure::bounded_process_group(),
        ProcessGroup::Inherit
    );
    let contract = VerifierCompilerContract::new(
        GateId::RustTypecheck,
        source(b"contained verifier selected tree"),
        root.to_path_buf(),
        root.to_path_buf(),
        vec![tool(
            "cargo",
            root.join("fake-cargo")
                .to_str()
                .expect("fixture path is UTF-8"),
        )],
        vec![
            (
                OsString::from(COMPILER_PID),
                root.join("compiler.pid").into_os_string(),
            ),
            (
                OsString::from(DESCENDANT_PID),
                root.join("descendant.pid").into_os_string(),
            ),
        ],
    )
    .expect("declare contained verifier contract");
    let run = contract
        .run(VerifierCompilerStage::NativeClippy)
        .expect("run fake verifier compiler");
    panic!(
        "outer worker deadline did not contain fake verifier compiler: completion={:?}",
        run.completion
    );
}

fn assert_process_gone(path: &Path, label: &str) {
    let pid = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {label} pid {}: {error}", path.display()))
        .trim()
        .parse::<u32>()
        .unwrap_or_else(|error| panic!("parse {label} pid {}: {error}", path.display()));
    for _ in 0..100 {
        if !process_exists(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // Do not leave the escaped regression fixture behind after a failure.
    let _ = Command::new("/bin/kill")
        .args(["-KILL", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    panic!("outer worker deadline leaked {label} {pid}");
}

fn process_exists(pid: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[test]
fn stable_preparation_mirrors_once_holds_its_lock_and_preserves_fingerprints() {
    let root = crate::toolchain::exclusive_scratch_root("gate-verifier-stable-source");
    let repository = root.join("repository");
    let selection = root.join("selection");
    fs::create_dir_all(selection.join("src")).expect("create selected source");
    fs::create_dir(&repository).expect("create repository");
    fs::write(selection.join("Cargo.toml"), b"[workspace]\n").expect("write manifest");
    fs::write(
        selection.join("src/lib.rs"),
        b"pub fn answer() -> u8 { 42 }\n",
    )
    .expect("write selected body");
    let contract = || {
        VerifierCompilerContract::new(
            GateId::CanvasWebTypecheck,
            source(b"stable compiler selected tree"),
            repository.clone(),
            selection.clone(),
            vec![
                tool("cargo", "/usr/bin/true"),
                tool("trunk", "/usr/bin/true"),
            ],
            Vec::new(),
        )
        .expect("stable compiler contract")
    };

    let mut prepared = contract().prepare_stable().expect("prepare stable source");
    let cached = canvas_source_root(&repository);
    assert_eq!(prepared.contract.selection_root, selection);
    assert_eq!(prepared.contract.execution_root, cached);
    let evidence = prepared
        .complete_retained(&complete_processes(&prepared.contract))
        .expect("complete prepared evidence");
    assert!(admits(&evidence, &contract()));
    assert_eq!(
        fs::read(cached.join("src/lib.rs")).expect("read mirrored body"),
        b"pub fn answer() -> u8 { 42 }\n"
    );
    let modified = fs::metadata(cached.join("src/lib.rs"))
        .expect("read mirrored metadata")
        .modified()
        .expect("mirrored mtime");
    let contention = contract().prepare_stable();
    assert!(
        contention
            .err()
            .is_some_and(|error| error.to_string().contains("in use")),
        "a second preparation acquired the held stable source"
    );
    prepared.release().expect("release stable source");

    let repeated = contract().prepare_stable().expect("repeat stable source");
    assert_eq!(
        fs::metadata(cached.join("src/lib.rs"))
            .expect("read repeated metadata")
            .modified()
            .expect("repeated mtime"),
        modified,
        "unchanged selected bytes lost their Cargo fingerprint mtime"
    );
    drop(repeated);
    fs::remove_dir_all(root).expect("remove stable source fixture");
}

#[test]
fn partial_or_duplicate_stage_sets_cannot_mint_evidence() {
    let contract = contract(GateId::CanvasWebTypecheck);
    let complete = complete_processes(&contract);

    assert!(contract.complete(&complete[..1]).is_err());
    assert!(
        contract
            .complete(&[complete[0].clone(), complete[0].clone()])
            .is_err()
    );
}

#[test]
fn every_bound_field_mutation_invalidates_evidence() {
    let contract = contract(GateId::CanvasWebTypecheck);
    let exact = contract
        .complete(&complete_processes(&contract))
        .expect("complete evidence");

    let mut candidate = exact.clone();
    candidate.gate_id = GateId::RustTypecheck;
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    candidate.source = source(b"stale selected tree");
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    candidate.manifest = Digest::compute(Domain::GateBatteryManifest, b"foreign manifest");
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    candidate.repository = PathBuf::from("/foreign/repository");
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    candidate.selection_root = PathBuf::from("/foreign/selection");
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    candidate.tools[0] = tool("cargo", "/forged/cargo");
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    candidate
        .environment
        .push((OsString::from("FORGED"), OsString::from("forged")));
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    candidate.processes.pop();
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact;
    candidate.seal = Digest::compute(Domain::GateReceipt, b"forged seal");
    assert!(!admits(&candidate, &contract));
}

#[test]
fn every_bound_process_field_mutation_invalidates_evidence() {
    let contract = contract(GateId::CanvasWebTypecheck);
    let exact = contract
        .complete(&complete_processes(&contract))
        .expect("complete evidence");

    let mut candidate = exact.clone();
    first_process_mut(&mut candidate).gate_id = GateId::RustTypecheck;
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    first_process_mut(&mut candidate).stage = VerifierCompilerStage::NativeClippy;
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    first_process_mut(&mut candidate).source = source(b"stale process source");
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    first_process_mut(&mut candidate).manifest =
        Digest::compute(Domain::GateBatteryManifest, b"stale process manifest");
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    first_process_mut(&mut candidate).repository = PathBuf::from("/forged/repository");
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    first_process_mut(&mut candidate).selection_root = PathBuf::from("/forged/selection");
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    first_process_mut(&mut candidate).tools = vec![tool("cargo", "/forged/process-cargo")];
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    first_process_mut(&mut candidate)
        .environment
        .push((OsString::from("FORGED"), OsString::from("forged")));
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    first_process_mut(&mut candidate)
        .targets
        .push("foreign-target".to_owned());
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    first_process_mut(&mut candidate)
        .arguments
        .push(OsString::from("--forged"));
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    first_process_mut(&mut candidate).exit_code = 17;
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    first_process_mut(&mut candidate).elapsed_ms += 1;
    assert!(!admits(&candidate, &contract));

    let mut candidate = exact.clone();
    first_process_mut(&mut candidate).seal =
        Digest::compute(Domain::GateReceipt, b"forged process seal");
    assert!(!admits(&candidate, &contract));
}

#[test]
fn sealed_evidence_cannot_cross_a_consumer_environment_or_capture_root() {
    let mut producer = contract(GateId::RustTypecheck);
    producer.environment.push((
        OsString::from("RUSTFLAGS"),
        OsString::from("--cfg=producer"),
    ));
    producer.environment.sort();
    let evidence = producer
        .complete(&complete_processes(&producer))
        .expect("producer evidence");

    let mut consumer_environment = producer.environment.clone();
    let (_, rustflags) = consumer_environment
        .iter_mut()
        .find(|(name, _)| name == "RUSTFLAGS")
        .expect("fixture RUSTFLAGS");
    *rustflags = OsString::from("--cfg=consumer");
    assert!(!evidence.admits(
        producer.gate_id,
        &producer.source,
        &producer.repository,
        &producer.selection_root,
        &consumer_environment,
        &producer.tools,
    ));
    assert!(!evidence.admits(
        producer.gate_id,
        &producer.source,
        &producer.repository,
        Path::new("/another/capture/root"),
        &producer.environment,
        &producer.tools,
    ));
}

#[test]
fn canonical_receipt_crosses_capture_roots_without_leaking_environment_values() {
    let mut producers = [
        contract(GateId::RustTypecheck),
        contract(GateId::CanvasWebTypecheck),
    ];
    for producer in &mut producers {
        producer.environment.push((
            OsString::from("RECEIPT_SECRET_FIXTURE"),
            OsString::from("receipt-secret-must-not-cross-process"),
        ));
        producer.environment.sort();
    }
    let evidence = producers.iter().map(complete_evidence).collect::<Vec<_>>();
    let receipt = encode_verifier_compiler_receipt(&evidence, AUTHORITY).expect("encode receipt");
    assert!(
        !receipt
            .windows(b"receipt-secret-must-not-cross-process".len())
            .any(|window| window == b"receipt-secret-must-not-cross-process"),
        "receipt leaked an ambient environment value"
    );
    assert!(
        !receipt
            .windows(AUTHORITY.len())
            .any(|window| window == AUTHORITY),
        "receipt leaked its one-time provenance authority"
    );

    let consumers = producers
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, mut consumer)| {
            consumer.selection_root = PathBuf::from(format!("/consumer/selection/{index}"));
            consumer
        })
        .collect::<Vec<_>>();
    let admitted = admit_verifier_compiler_receipt(&receipt, &consumers, AUTHORITY)
        .expect("fresh receipt admits across private capture roots");
    assert_eq!(admitted.len(), consumers.len());
    assert!(
        admitted
            .iter()
            .zip(&consumers)
            .all(|(candidate, consumer)| admits(candidate, consumer))
    );
}

#[test]
fn canonical_receipt_crosses_private_cargo_materializations() {
    let mut producers = [
        contract(GateId::RustTypecheck),
        contract(GateId::CanvasWebTypecheck),
    ];
    for producer in &mut producers {
        producer.environment.push((
            OsString::from(CARGO_HOME),
            OsString::from("/producer/admitted-cargo-home"),
        ));
        producer.environment.extend([
            (
                OsString::from(PATH),
                OsString::from("/producer/admitted-cargo-home/bin:/tool/bin"),
            ),
            (
                OsString::from(SPICE_RS_BOOTSTRAP_CARGO_HOME),
                OsString::from("/bootstrap/cargo-home"),
            ),
            (
                OsString::from(SPICE_RS_NATIVE_CARGO),
                OsString::from("/bootstrap/cargo-home/bin/cargo"),
            ),
        ]);
        if producer.gate_id == GateId::RustTypecheck {
            producer.environment.extend([
                (
                    OsString::from(CARGO_TARGET_DIR),
                    OsString::from("/producer/artifact-cache"),
                ),
                (
                    OsString::from(CARGO_BUILD_BUILD_DIR),
                    OsString::from("/producer/build-script-cache"),
                ),
            ]);
        }
        producer.environment.sort();
    }
    let evidence = producers.iter().map(complete_evidence).collect::<Vec<_>>();
    let receipt = encode_verifier_compiler_receipt(&evidence, AUTHORITY)
        .expect("encode private Cargo-home receipt");

    let consumers = producers
        .iter()
        .cloned()
        .map(|mut consumer| {
            let names: &[&str] = if consumer.gate_id == GateId::RustTypecheck {
                &[CARGO_HOME, CARGO_TARGET_DIR, CARGO_BUILD_BUILD_DIR]
            } else {
                &[CARGO_HOME]
            };
            for name in names {
                let (_, value) = consumer
                    .environment
                    .iter_mut()
                    .find(|(candidate, _)| candidate == name)
                    .expect("fixture Cargo materialization path");
                *value = OsString::from(format!("/consumer/{name}"));
            }
            consumer.environment.retain(|(name, _)| {
                name != SPICE_RS_BOOTSTRAP_CARGO_HOME && name != SPICE_RS_NATIVE_CARGO
            });
            let (_, path) = consumer
                .environment
                .iter_mut()
                .find(|(name, _)| name == PATH)
                .expect("fixture search path");
            *path = OsString::from("/tool/bin");
            consumer
        })
        .collect::<Vec<_>>();
    let admitted = admit_verifier_compiler_receipt(&receipt, &consumers, AUTHORITY)
        .expect("receipt authority ignores private Cargo materialization paths");
    assert!(
        admitted
            .iter()
            .zip(&consumers)
            .all(|(candidate, consumer)| admits(candidate, consumer))
    );
}

#[test]
fn semantic_compiler_authority_crosses_source_commits_but_not_contract_or_worktree_drift() {
    let producers = [
        contract(GateId::RustTypecheck),
        contract(GateId::CanvasWebTypecheck),
    ];
    let evidence = producers.iter().map(complete_evidence).collect::<Vec<_>>();
    let receipt = encode_verifier_compiler_receipt(&evidence, AUTHORITY).expect("encode receipt");
    let retained =
        semantic_verifier_compiler_authorities(&receipt, &producers[0].source, AUTHORITY)
            .expect("inspect semantic compiler authorities");
    assert_eq!(
        retained,
        producers
            .iter()
            .map(|contract| (contract.gate_id(), contract.semantic_authority()))
            .collect::<Vec<_>>(),
    );

    for producer in &producers {
        let mut next_commit = producer.clone();
        next_commit.source = source(b"different selected tree on a later commit");
        next_commit.selection_root = PathBuf::from("/different/private/selection");
        assert_eq!(
            producer.semantic_authority(),
            next_commit.semantic_authority(),
            "source and private materialization roots are not compiler contracts",
        );

        let mut different_worktree = producer.clone();
        different_worktree.repository = PathBuf::from("/different/worktree");
        assert_ne!(
            producer.semantic_authority(),
            different_worktree.semantic_authority(),
        );

        let mut different_tool = producer.clone();
        different_tool.tools[0] = tool("cargo", "/different/cargo");
        assert_ne!(
            producer.semantic_authority(),
            different_tool.semantic_authority(),
        );

        let mut different_environment = producer.clone();
        different_environment.environment.push((
            OsString::from("RUSTFLAGS"),
            OsString::from("--cfg=contract_drift"),
        ));
        different_environment.environment.sort();
        assert_ne!(
            producer.semantic_authority(),
            different_environment.semantic_authority(),
        );

        let mut different_cargo_home = producer.clone();
        different_cargo_home.environment.push((
            OsString::from(CARGO_HOME),
            OsString::from("/another/private-cargo-home"),
        ));
        different_cargo_home.environment.sort();
        assert_eq!(
            producer.semantic_authority(),
            different_cargo_home.semantic_authority(),
            "private Cargo cache materialization is not compiler authority",
        );
    }

    assert!(
        semantic_verifier_compiler_authorities(
            &receipt,
            &source(b"foreign retained source"),
            AUTHORITY,
        )
        .is_err(),
    );
    assert!(
        semantic_verifier_compiler_authorities(
            &receipt,
            &producers[0].source,
            b"foreign worktree authority",
        )
        .is_err(),
    );
}

#[test]
fn merge_compiler_authority_crosses_only_materialization_and_session_scratch() {
    let repository = Digest::compute(Domain::GateReceipt, b"shared repository fixture");
    for gate_id in [GateId::RustTypecheck, GateId::CanvasWebTypecheck] {
        let mut first = contract(gate_id);
        first.environment.extend([
            (
                OsString::from("PWD"),
                first.repository.as_os_str().to_owned(),
            ),
            (
                OsString::from("CODEX_THREAD_ID"), // env-policy: allow
                OsString::from("producer-session"),
            ),
            (OsString::from("CARGO_BUILD_JOBS"), OsString::from("8")),
            (
                OsString::from("CARGO_HOME"),
                OsString::from("/cargo-homes/first-worktree"),
            ),
        ]);
        first.environment.sort();
        let mut linked = first.clone();
        linked.repository = PathBuf::from("/linked/repository");
        linked.environment = linked
            .environment
            .into_iter()
            .map(|(name, value)| match name.to_str() {
                Some("PWD") => (name, linked.repository.as_os_str().to_owned()),
                Some("CODEX_THREAD_ID") => (name, OsString::from("consumer-session")), // env-policy: allow
                Some("CARGO_BUILD_JOBS") => (name, OsString::from("3")),
                Some("CARGO_HOME") => (name, OsString::from("/cargo-homes/linked-worktree")),
                _ => (name, value),
            })
            .collect();
        assert_ne!(first.semantic_authority(), linked.semantic_authority());
        assert_eq!(
            first.merge_semantic_authority(&repository),
            linked.merge_semantic_authority(&repository),
        );

        linked.environment.push((
            OsString::from("RUSTFLAGS"),
            OsString::from("--cfg=merge_contract_drift"),
        ));
        linked.environment.sort();
        assert_ne!(
            first.merge_semantic_authority(&repository),
            linked.merge_semantic_authority(&repository),
        );
        assert_ne!(
            first.merge_semantic_authority(&repository),
            first.merge_semantic_authority(&Digest::compute(
                Domain::GateReceipt,
                b"foreign repository fixture",
            )),
        );
    }
}

#[test]
fn missing_partial_duplicate_and_stale_receipts_fail_closed() {
    let producers = [
        contract(GateId::RustTypecheck),
        contract(GateId::CanvasWebTypecheck),
    ];
    let evidence = producers.iter().map(complete_evidence).collect::<Vec<_>>();
    let exact =
        encode_verifier_compiler_receipt(&evidence, AUTHORITY).expect("encode exact receipt");

    assert!(admit_verifier_compiler_receipt(&[], &producers, AUTHORITY).is_err());
    assert!(admit_verifier_compiler_receipt(&exact, &producers, b"foreign authority").is_err());
    let partial =
        encode_verifier_compiler_receipt(&evidence[..1], AUTHORITY).expect("encode partial");
    assert!(admit_verifier_compiler_receipt(&partial, &producers, AUTHORITY).is_err());
    let duplicate =
        encode_verifier_compiler_receipt(&[evidence[0].clone(), evidence[0].clone()], AUTHORITY)
            .expect("encode duplicate");
    assert!(admit_verifier_compiler_receipt(&duplicate, &producers, AUTHORITY).is_err());

    let mut stale_source = producers.to_vec();
    stale_source[0].source = source(b"changed staged source");
    assert!(admit_verifier_compiler_receipt(&exact, &stale_source, AUTHORITY).is_err());

    let mut stale_environment = producers.to_vec();
    stale_environment[0]
        .environment
        .push((OsString::from("RUSTFLAGS"), OsString::from("--cfg=changed")));
    stale_environment[0].environment.sort();
    assert!(admit_verifier_compiler_receipt(&exact, &stale_environment, AUTHORITY).is_err());

    let mut stale_tool = producers.to_vec();
    stale_tool[0].tools = vec![tool("cargo", "/changed/cargo")];
    assert!(admit_verifier_compiler_receipt(&exact, &stale_tool, AUTHORITY).is_err());
}

#[test]
fn malformed_noncanonical_and_tampered_receipts_fail_closed() {
    let contracts = [
        contract(GateId::RustTypecheck),
        contract(GateId::CanvasWebTypecheck),
    ];
    let evidence = contracts.iter().map(complete_evidence).collect::<Vec<_>>();
    let exact =
        encode_verifier_compiler_receipt(&evidence, AUTHORITY).expect("encode exact receipt");

    let mut noncanonical = exact.clone();
    noncanonical.push(b'\n');
    assert!(admit_verifier_compiler_receipt(&noncanonical, &contracts, AUTHORITY).is_err());

    let mut unsupported: VerifierCompilerReceiptSet =
        wire::decode(&exact).expect("decode receipt fixture");
    unsupported.version += 1;
    unsupported.authority_seal = unsupported.compute_authority_seal(AUTHORITY);
    let unsupported = wire::encode(&unsupported).expect("encode unsupported receipt");
    assert!(admit_verifier_compiler_receipt(&unsupported, &contracts, AUTHORITY).is_err());

    let mut partial_stage: VerifierCompilerReceiptSet =
        wire::decode(&exact).expect("decode receipt fixture");
    partial_stage.evidence[1].processes.pop();
    partial_stage.evidence[1].seal = partial_stage.evidence[1].compute_seal();
    partial_stage.authority_seal = partial_stage.compute_authority_seal(AUTHORITY);
    let partial_stage = wire::encode(&partial_stage).expect("encode partial-stage receipt");
    assert!(admit_verifier_compiler_receipt(&partial_stage, &contracts, AUTHORITY).is_err());

    let mut tampered: VerifierCompilerReceiptSet =
        wire::decode(&exact).expect("decode receipt fixture");
    tampered.evidence[0].processes[0].targets =
        Digest::compute(Domain::GateReceipt, b"foreign targets");
    tampered.evidence[0].processes[0].seal = tampered.evidence[0].processes[0].compute_seal();
    tampered.evidence[0].seal = tampered.evidence[0].compute_seal();
    tampered.authority_seal = tampered.compute_authority_seal(AUTHORITY);
    let tampered = wire::encode(&tampered).expect("encode tampered receipt");
    assert!(admit_verifier_compiler_receipt(&tampered, &contracts, AUTHORITY).is_err());
}

#[test]
fn ignored_live_compiler_inputs_cannot_mint_selected_source_evidence() {
    let root = crate::toolchain::exclusive_scratch_root("gate-verifier-selected-source");
    let repository = root.join("repository");
    let selection = root.join("selection");
    fs::create_dir_all(repository.join(".cargo")).expect("live Cargo configuration root");
    fs::create_dir_all(repository.join("ignored")).expect("live ignored build root");
    fs::create_dir(&selection).expect("immutable selected root");
    let manifest = concat!(
        "[package]\n",
        "name = \"selected-source-fixture\"\n",
        "version = \"0.0.0\"\n",
        "edition = \"2024\"\n",
        "build = \"ignored/build.rs\"\n",
    );
    for root in [&repository, &selection] {
        fs::write(root.join("Cargo.toml"), manifest).expect("tracked fixture manifest");
        fs::write(root.join(".gitignore"), ".cargo/config.toml\nignored/\n")
            .expect("tracked ignored-input policy");
    }
    fs::write(
        repository.join(".cargo/config.toml"),
        "[build]\nrustflags = []\n",
    )
    .expect("ignored live Cargo configuration");
    fs::write(repository.join("ignored/build.rs"), "fn main() {}\n")
        .expect("ignored live build script");
    for (label, live_only_input) in [
        ("ignored-cargo-config", ".cargo/config.toml"),
        ("ignored-build-script", "ignored/build.rs"),
    ] {
        let cargo = root.join(label);
        fs::write(
            &cargo,
            format!(
                concat!(
                    "#!/bin/sh\n",
                    "if [ -f '{live_only_input}' ]; then\n",
                    "  exit 0\n",
                    "fi\n",
                    "exit 19\n",
                ),
                live_only_input = live_only_input,
            ),
        )
        .expect("write compiler-input witness");
        fs::set_permissions(
            &cargo,
            fs::metadata("/usr/bin/true")
                .expect("read fixture executable permissions")
                .permissions(),
        )
        .expect("make compiler-input witness executable");
        let contract = VerifierCompilerContract::new(
            GateId::RustTypecheck,
            source(label.as_bytes()),
            repository.clone(),
            selection.clone(),
            vec![tool("cargo", cargo.to_str().expect("UTF-8 fixture path"))],
            Vec::new(),
        )
        .expect("selected-source compiler contract");

        let run = contract
            .run(VerifierCompilerStage::NativeClippy)
            .expect("compiler-input witness launches");

        assert!(!run.success());
        assert!(
            run.into_evidence().is_none(),
            "{live_only_input} minted evidence from outside the selected root"
        );
        assert!(
            contract.complete(&[]).is_err(),
            "{live_only_input} admitted evidence without a successful process token"
        );
    }
    fs::remove_dir_all(root).expect("remove selected-source fixture");
}

#[test]
fn a_failed_process_returns_no_reusable_token() {
    let mut contract = contract(GateId::RustTypecheck);
    contract.repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("gate crate has a workspace parent")
        .to_path_buf();
    contract.selection_root = contract.repository.clone();
    contract.execution_root = contract.repository.clone();
    contract.tools = vec![tool("cargo", "/usr/bin/false")];

    let run = contract
        .run(VerifierCompilerStage::NativeClippy)
        .expect("false launches");

    assert!(!run.success());
    assert!(run.into_evidence().is_none());
}

#[test]
fn canvas_readiness_requires_and_observes_the_post_compiler_record() {
    let root = crate::toolchain::exclusive_scratch_root("gate-verifier-canvas-readiness");
    let repository = root.join("repository");
    let selection = root.join("selection");
    fs::create_dir(&repository).expect("fixture repository");
    fs::create_dir(&selection).expect("fixture selected root");

    for (name, record, expected) in [
        (
            "ready-cargo",
            format!("{}\n", crate::VERIFIER_APP_COMPILER_READY_RECORD),
            Ok(()),
        ),
        ("silent-cargo", String::new(), Err("omitted")),
    ] {
        let cargo = root.join(name);
        fs::write(&cargo, format!("#!/bin/sh\nprintf '%s' '{record}'\n"))
            .expect("write readiness fixture Cargo executable");
        fs::set_permissions(
            &cargo,
            fs::metadata("/usr/bin/true")
                .expect("read fixture executable permissions")
                .permissions(),
        )
        .expect("make readiness fixture executable");
        let contract = VerifierCompilerContract::new(
            GateId::CanvasWebTypecheck,
            source(name.as_bytes()),
            repository.clone(),
            selection.clone(),
            vec![tool("cargo", cargo.to_str().expect("UTF-8 fixture path"))],
            Vec::new(),
        )
        .expect("readiness compiler contract");
        let observed = Arc::new(AtomicBool::new(false));
        let callback_observed = Arc::clone(&observed);
        let run = contract.run_supervised_with_readiness(
            VerifierCompilerStage::CanvasApp,
            CancellationToken::new(),
            Arc::new(move || callback_observed.store(true, Ordering::Release)),
        );

        match expected {
            Ok(()) => {
                assert!(run.expect("ready compiler runs").success());
                assert!(observed.load(Ordering::Acquire));
            }
            Err(fragment) => {
                let error = match run {
                    Err(error) => error,
                    Ok(_) => panic!("silent successful compiler is refused"),
                };
                assert!(error.to_string().contains(fragment));
                assert!(!observed.load(Ordering::Acquire));
            }
        }
    }
    fs::remove_dir_all(root).expect("remove readiness fixture");
}

#[test]
fn failed_process_feedback_is_captured_completely_without_minting_evidence() {
    let root = crate::toolchain::exclusive_scratch_root("gate-verifier-captured-output");
    let repository = root.join("repository");
    let selection = root.join("selection");
    fs::create_dir(&repository).expect("fixture repository");
    fs::create_dir(&selection).expect("fixture selected root");
    let cargo = root.join("cargo");
    fs::write(
        &cargo,
        "#!/bin/sh\nprintf 'bounded stdout\\n'\nprintf 'bounded stderr\\n' >&2\nexit 23\n",
    )
    .expect("write fixture Cargo executable");
    fs::set_permissions(
        &cargo,
        fs::metadata("/usr/bin/true")
            .expect("read fixture executable permissions")
            .permissions(),
    )
    .expect("make fixture Cargo executable");
    let contract = VerifierCompilerContract::new(
        GateId::RustTypecheck,
        source(b"captured output selected tree"),
        repository,
        selection,
        vec![tool("cargo", cargo.to_str().expect("UTF-8 fixture path"))],
        Vec::new(),
    )
    .expect("captured output contract");

    let run = contract
        .run(VerifierCompilerStage::NativeClippy)
        .expect("fixture Cargo launches");

    assert!(!run.success());
    assert_eq!(run.code(), Some(23));
    assert_eq!(run.stdout(), b"bounded stdout\n");
    assert_eq!(run.stderr(), b"bounded stderr\n");
    assert!(!run.cancelled());
    assert!(!run.timed_out());
    assert!(run.into_evidence().is_none());
    fs::remove_dir_all(root).expect("remove captured output fixture");
}
