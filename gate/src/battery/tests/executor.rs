//! Running the compiled battery: one capture, one pass, every arm once.

use super::*;
use crate::metrics::{EvidenceState, MetricId, MetricKey, StructuralArmEvidence};
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt as _;

/// What each metric arm reports for this fixture, by state and by evidence.
///
/// Every compiled metric arm reaches its input, so there is no second list of
/// arms blocked before measurement and an arm absent from this one fails rather
/// than passing unexamined. Each row is pinned by state *and* by a substring of
/// the reason, because neither alone separates the family: the two passing arms
/// carry the one sentence every passing structural arm carries, so what says
/// each of them measured its own scope is the row-level assertion -- below for
/// complexity, and in `super::file_shape` for file shape.
const ACQUIRED_METRIC_ARMS: [(GateId, BatteryState, &str); 4] = [
    // The fixture's three routines are each inside every declared complexity
    // bound, so the arm measures and passes. A pass is also what an arm that
    // measured *nothing* would report, so the rows themselves are checked
    // below rather than inferred from this state.
    (
        GateId::Complexity,
        BatteryState::Passed,
        "all declared structural measurements completed within threshold",
    ),
    // Every source file the fixture commits is inside its own elastic bound, so
    // the file-shape arm reports that same passing sentence. What the rows
    // themselves carry -- one path key per selected file, paired across both
    // metrics, each with the limit it was judged against -- is asserted in
    // `super::file_shape`, since a whole-arm state cannot see whether anything
    // was measured at all.
    (
        GateId::FileShape,
        BatteryState::Passed,
        "all declared structural measurements completed within threshold",
    ),
    // The fixture repository commits three `.py` sources, and `.py` is in the
    // workspace policy's magic class. The scan reaches its baseline for every
    // path and measures `src/lib.rs` clean, but those three are a declared class
    // this build's C-grammar parser cannot read exactly, and an unestablished row
    // outranks a measured one.
    (
        GateId::MagicNumbers,
        BatteryState::Unsupported,
        "predecessor AST/tree-sitter parent semantics",
    ),
    // The Markdown arm measured the one case-mismatched inline link the fixture
    // commits. A failing arm's reason is the shared bound sentence rather than
    // the finding, so the state is what distinguishes this row from the one
    // above; an arm that started answering for the other's scope would land on
    // the other's state and fail here.
    (
        GateId::MarkdownLinks,
        BatteryState::Failed,
        "exceeded their declared bound",
    ),
];

#[test]
fn runner_executes_all_27_once_in_order_with_complete_context() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::all_passed(&fixture.source);
    let report = run_battery(&inputs, &mut executor).expect("complete battery");

    assert_eq!(executor.calls, GateId::BATTERY);
    assert_eq!(report.results().len(), GateId::BATTERY.len());
    assert!(report.admits());
    assert_eq!(report.context().source(), &fixture.source);
    assert_eq!(report.context().actor().source(), "active-actor");
    assert_eq!(report.context().actor().value(), "fixture-actor");
    assert_eq!(report.context().scope().name(), "selected-index");
    assert_eq!(
        report.context().scope().provenance(),
        "compiled test selector"
    );
    assert_eq!(report.context().configuration().len(), 1);
    assert_eq!(
        report.context().configuration()[0].key(),
        "policy.test_paths"
    );
    assert_eq!(report.context().configuration()[0].release(), "current");
    assert_eq!(
        report.context().configuration()[0].layer(),
        OriginLayer::Repository
    );
    assert_eq!(
        report.context().configuration()[0].document(),
        "spice-rust.toml"
    );
    assert_eq!(
        report.context().configuration()[0].field(),
        &["policy".to_owned(), "test_paths".to_owned()]
    );
    assert_eq!(report.context().bounds().len(), 1);
    assert_eq!(report.context().bounds()[0].rule(), "file-shape");
    assert_eq!(report.context().bounds()[0].measured(), 101);
    assert_eq!(report.context().bounds()[0].active_limit(), 152);
    assert_eq!(report.context().bounds()[0].latch_transition(), "clear");
    assert_eq!(
        report.context().bounds()[0]
            .actor()
            .expect("bound actor")
            .value(),
        "fixture-actor"
    );
    assert_eq!(
        report.context().bounds()[0].actor_inputs().candidate_path(),
        "src/lib.rs"
    );
    assert_eq!(
        report.context().bounds()[0].actor_inputs().active_actor(),
        Some("fixture-actor")
    );
    assert_eq!(report.context().latches().len(), 1);
    assert_eq!(report.context().latches()[0].kind(), "file-loc");
    assert_eq!(report.context().latches()[0].path(), "src/lib.rs");
    assert_eq!(report.context().latches()[0].function(), None);
    assert_eq!(report.context().tools().len(), 1);
    assert_eq!(report.context().tools()[0].name(), "cargo");
    assert_eq!(report.context().tools()[0].program(), "cargo");
    assert!(
        report
            .context()
            .tools()
            .iter()
            .all(|tool| { !matches!(tool.program(), "python" | "task" | "sh" | "bash" | "zsh") })
    );
    assert_eq!(report.context().tools()[0].version(), Some("cargo fixture"));
    assert_eq!(
        report.context().tools()[0].executable_digest(),
        Some(&Digest::compute(Domain::GateTool, b"fixture cargo"))
    );
    assert_eq!(report.context().policy(), &fixture.policy_digest());
    assert_eq!(report.context().state(), &fixture.state.before().digest());

    for (position, (result, expected)) in report.results().iter().zip(GateId::BATTERY).enumerate() {
        assert_eq!(result.position(), position);
        assert_eq!(
            result.predecessor(),
            expected
                .predecessor_declared()
                .then(|| expected.predecessor_name())
        );
        assert_eq!(result.gate_id(), expected);
        assert_eq!(result.state(), BatteryState::Passed);
        assert_eq!(result.evidence().gate_id(), expected);
        assert_eq!(
            result.evidence().state().expect("typed state"),
            BatteryState::Passed
        );
        assert!(!result.evidence().reason().expect("typed reason").is_empty());
        assert_family_evidence(result.evidence(), &fixture.source);
    }

    let encoded = report.encode().expect("canonical report");
    assert_eq!(BatteryReport::decode(&encoded).expect("decode"), report);
}

#[test]
fn one_selected_arm_runs_alone_and_matches_its_complete_report_row() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut complete_executor = RecordingExecutor::all_passed(&fixture.source);
    let complete = run_battery(&inputs, &mut complete_executor).expect("complete battery");

    let selected_id = GateId::EnvNameLedger;
    let mut selected_executor = RecordingExecutor::all_passed(&fixture.source);
    let selected = run_battery_arm(&inputs, &mut selected_executor, selected_id)
        .expect("selected battery arm");

    assert_eq!(selected_executor.calls, [selected_id]);
    assert_eq!(
        &selected,
        complete
            .results()
            .iter()
            .find(|result| result.gate_id() == selected_id)
            .expect("complete report contains selected arm")
    );
}

/// One capture answers for the whole battery, including its Git-derived arms.
///
/// The repository is moved after capture in every direction a core arm reads:
/// an unstaged edit, a file left partially staged, and a merge in progress.
/// Each mutation is proved real with the same program the arms run, so the
/// clean observations below are a binding rather than a scanner that never saw
/// anything. An executor that acquired the core family on first request would
/// report the partial staging and the merge, and its report would name a
/// capture that never held either.
///
/// Nothing here supplies an authority. The context the report carries is
/// asserted against the same tracked documents the repository was built from,
/// so a capture that quietly invented a policy, scope, configuration, or tool
/// would disagree with the repository it claims to describe.
#[test]
fn one_capture_reads_every_authority_and_runs_each_family_once() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    seed_one_claim(&repository, "peer-actor");
    let hygiene = crate::HygieneConfiguration::new(["app/main.py"], ["tests"])
        .expect("production hygiene roots");
    let selector = crate::Selector::staged();
    let mut events = Vec::new();
    let mut record = |event| events.push(event);
    let changes = ["app/main.py", "src/lib.rs", "tests/test_main.py"]
        .map(str::to_owned)
        .to_vec();
    let mut capture = crate::BatteryCapture::capture_traced(
        &repository,
        &selector,
        crate::ClaimCapture::Idle,
        acting("battery-actor"),
        crate::RimCapture::new(&repository, None, changes)
            .expect("the fixture repository root is absolute"),
        hygiene,
        &mut record,
    )
    .expect("capture production battery source");
    assert_ne!(capture.root(), repository);

    move_the_repository_after_capture(&repository);

    let report = capture.execute().expect("complete production battery");
    assert_captured_authorities(&report, &capture, &repository);
    drop(capture);

    assert_each_family_ran_once(&events);
    assert_every_family_projected_its_own_evidence(&report);
    assert_the_git_arms_kept_the_capture(&report);

    let encoded = report.encode().expect("encode production report");
    assert_eq!(
        BatteryReport::decode(&encoded).expect("decode production report"),
        report
    );
    let receipt = BatteryReceipt::mint(report, &fixture_baseline("production executor"));
    let receipt_bytes = receipt.encode().expect("encode production receipt");
    assert_eq!(
        BatteryReceipt::verify(&receipt_bytes).expect("verify production receipt"),
        receipt
    );
}

#[test]
fn stale_verifier_evidence_runs_both_real_compiler_arms() {
    let fixture = Fixture::new();
    let repository = compiler_fixture_repository(&fixture);
    let (programs, cargo_log, trunk_log) = install_compiler_programs(&fixture);
    let rim = crate::RimCapture::new(
        &repository,
        Some(OsString::from(programs.as_os_str())),
        Vec::new(),
    )
    .expect("absolute compiler rim");
    let mut capture = crate::BatteryCapture::capture(
        &repository,
        &crate::Selector::staged(),
        crate::ClaimCapture::Idle,
        acting("compiler-actor"),
        rim,
        crate::HygieneConfiguration::new(["app/main.py"], ["tests"]).expect("hygiene roots"),
    )
    .expect("capture compiler fixture");
    let (native, mut evidence) = compiler_evidence(&capture, Vec::new());
    for item in &mut evidence {
        item.corrupt_source_for_test();
    }

    fs::write(
        repository.join("src/lib.rs"),
        "pub fn fixture() { let _index_moved_after_capture = true; }\n",
    )
    .expect("move the live worktree after compiler capture");
    fixture_git(&repository, &["add", "src/lib.rs"]);
    let drifted = native
        .run(crate::VerifierCompilerStage::NativeClippy)
        .expect("retained selected compiler still provides verifier feedback");
    assert!(drifted.success());
    assert!(
        drifted.into_evidence().is_some(),
        "a later live index changed compilation of the retained immutable selection"
    );
    fs::remove_file(&cargo_log).expect("clear verifier Cargo trace");

    capture
        .execute_with_verifier_evidence(&evidence, Vec::new())
        .expect("invalid evidence falls through to a complete battery");
    assert_fallback_compiler_trace(&cargo_log, &trunk_log);
}

#[test]
fn rustflags_drift_across_prepared_captures_runs_both_real_compiler_arms() {
    let fixture = Fixture::new();
    let repository = compiler_fixture_repository(&fixture);
    let (programs, cargo_log, trunk_log) = install_compiler_programs(&fixture);
    let capture = |actor: &str| {
        crate::BatteryCapture::capture(
            &repository,
            &crate::Selector::staged(),
            crate::ClaimCapture::Idle,
            acting(actor),
            crate::RimCapture::new(
                &repository,
                Some(OsString::from(programs.as_os_str())),
                Vec::new(),
            )
            .expect("absolute compiler rim"),
            crate::HygieneConfiguration::new(["app/main.py"], ["tests"]).expect("hygiene roots"),
        )
        .expect("capture compiler fixture")
    };
    let producer = capture("compiler-producer");
    let producer_environment = vec![(
        OsString::from("RUSTFLAGS"),
        OsString::from("--cfg=producer"),
    )];
    let (_, evidence) = compiler_evidence(&producer, producer_environment);
    fs::remove_file(&cargo_log).expect("clear producer Cargo trace");
    let mut consumer = capture("compiler-consumer");
    let consumer_environment = vec![(
        OsString::from("RUSTFLAGS"),
        OsString::from("--cfg=consumer"),
    )];

    consumer
        .execute_with_verifier_evidence(&evidence, consumer_environment)
        .expect("environment-mismatched evidence falls through to the battery");

    assert_fallback_compiler_trace(&cargo_log, &trunk_log);
}

fn compiler_fixture_repository(fixture: &Fixture) -> PathBuf {
    let repository = production_repository(&fixture.root);
    fs::write(
        repository.join("Cargo.toml"),
        "[workspace]\nmembers = []\nresolver = \"3\"\n",
    )
    .expect("fixture Cargo manifest");
    fs::write(repository.join("Cargo.lock"), "version = 4\n").expect("fixture Cargo lock");
    fs::write(
        repository.join("app/index.html"),
        "<!doctype html><title>fixture</title>\n",
    )
    .expect("fixture canvas entry point");
    fixture_git(&repository, &["add", "--all"]);
    fixture_git(&repository, &["commit", "-q", "-m", "compiler fixture"]);
    repository
}

fn install_compiler_programs(fixture: &Fixture) -> (PathBuf, PathBuf, PathBuf) {
    let programs = fixture.root.join("compiler-programs");
    fs::create_dir(&programs).expect("compiler program directory");
    let cargo_log = fixture.root.join("compiler-cargo-log");
    let trunk_log = fixture.root.join("compiler-trunk-log");
    let executable = |path: &Path, body: String| {
        fs::write(path, body).expect("write compiler fixture program");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .expect("make compiler fixture executable");
    };
    executable(
        &programs.join("cargo"),
        format!(
            concat!(
                "#!/bin/sh\n",
                "if [ \"$*\" = \"--version\" ]; then\n",
                "  printf '%s\\n' 'cargo 1.97.1 (c980f4866 2026-06-30)'\n",
                "  exit 0\n",
                "fi\n",
                "printf '%s\\n' \"$*\" >> '{}'\n",
                "exit 0\n",
            ),
            cargo_log.display()
        ),
    );
    executable(
        &programs.join("trunk"),
        format!(
            concat!(
                "#!/bin/sh\n",
                "if [ \"$*\" = \"--version\" ]; then\n",
                "  printf '%s\\n' 'trunk 0.21.14'\n",
                "  exit 0\n",
                "fi\n",
                "printf '%s\\n' \"$*\" >> '{}'\n",
                "exit 0\n",
            ),
            trunk_log.display()
        ),
    );
    (programs, cargo_log, trunk_log)
}

fn compiler_evidence(
    capture: &crate::BatteryCapture<'_>,
    environment: Vec<(OsString, OsString)>,
) -> (
    crate::VerifierCompilerContract,
    Vec<crate::VerifierCompilerEvidence>,
) {
    let native = capture
        .verifier_compiler_contract(GateId::RustTypecheck, environment.clone())
        .expect("native verifier contract");
    let canvas = capture
        .verifier_compiler_contract(GateId::CanvasWebTypecheck, environment)
        .expect("canvas verifier contract");
    let mut processes = Vec::new();
    for (contract, stages) in [
        (
            &native,
            [Some(crate::VerifierCompilerStage::NativeClippy), None],
        ),
        (
            &canvas,
            [
                Some(crate::VerifierCompilerStage::CanvasCheck),
                Some(crate::VerifierCompilerStage::CanvasApp),
            ],
        ),
    ] {
        for stage in stages.into_iter().flatten() {
            let run = contract.run(stage).expect("compiler evidence process");
            assert!(run.success());
            processes.push(run.into_evidence().expect("clean successful process token"));
        }
    }
    let evidence = vec![
        native.complete(&processes).expect("native evidence"),
        canvas.complete(&processes).expect("canvas evidence"),
    ];
    (native, evidence)
}

fn assert_fallback_compiler_trace(cargo_log: &Path, trunk_log: &Path) {
    let cargo = fs::read_to_string(cargo_log).expect("read fallback Cargo trace");
    assert!(cargo.contains("check --locked --workspace --all-targets"));
    assert!(cargo.contains("check --locked --workspace --target wasm32-unknown-unknown"));
    let trunk = fs::read_to_string(trunk_log).expect("read fallback Trunk trace");
    assert_eq!(
        trunk.trim(),
        "build index.html --release --locked --filehash=false"
    );
}

/// A battery run names who ran it, so a capture with no actor is refused.
#[test]
fn a_capture_without_an_acting_actor_is_refused() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    let Err(error) = crate::BatteryCapture::capture(
        &repository,
        &crate::Selector::staged(),
        crate::ClaimCapture::Idle,
        crate::ActorCapture::Absent,
        crate::RimCapture::new(&repository, None, Vec::new())
            .expect("the fixture repository root is absolute"),
        crate::HygieneConfiguration::new(["app/main.py"], ["tests"]).expect("hygiene roots"),
    ) else {
        panic!("a battery with no actor must be refused")
    };

    assert_eq!(
        error.detail(),
        "battery capture requires an acting actor: a battery report names who ran it"
    );
}

/// One capture reads the ledger twice and both readings must be the same read.
///
/// The caller filters expiry under its own run clock because the gate has none,
/// and filtering can only drop claims. A bound claim the ledger does not hold
/// is therefore not a filtered reading of it, and letting it through would
/// publish a state authority saying a peer holds `app/main.py` beside a
/// repository-document measurement resolved from a claim that authority never
/// contained.
#[test]
fn a_bound_claim_the_state_authority_does_not_hold_is_refused() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    seed_one_claim(&repository, "peer-actor");
    let held = live_ledger(&repository).claims().to_vec();
    assert_eq!(held.len(), 1, "the seeded ledger holds exactly one claim");

    let capture = |bound_claims: Vec<crate::PeerClaim>| {
        crate::BatteryCapture::capture(
            &repository,
            &crate::Selector::staged(),
            crate::ClaimCapture::Idle,
            crate::ActorCapture::Acting {
                actor: "battery-actor".to_owned(),
                bound_claims,
            },
            crate::RimCapture::new(&repository, None, Vec::new())
                .expect("the fixture repository root is absolute"),
            crate::HygieneConfiguration::new(["app/main.py"], ["tests"]).expect("hygiene roots"),
        )
        .map(|_| ())
    };

    capture(held.clone()).expect("the ledger's own claims are what the caller read");

    // The same path the ledger holds, under an owner it never recorded: two
    // readings that cannot both have come from this repository.
    let forged: crate::PeerClaim = serde_json::from_str(&format!(
        r#"{{"path":"{}","actor":"other-actor","created_at":{},"expires_at":{}}}"#,
        held[0].path(),
        held[0].created_at(),
        held[0].expires_at()
    ))
    .expect("a claim shape the ledger itself would decode");
    let Err(error) = capture(vec![forged]) else {
        panic!("a bound claim outside the state authority must be refused")
    };

    assert_eq!(
        error.detail(),
        "battery bound claim on \"app/main.py\" held by \"other-actor\" is not one the state \
         authority holds: the run clock only drops claims, so these are two different readings"
    );
}

/// One acting identity with no peer holding any bound claim.
fn acting(actor: &str) -> crate::ActorCapture {
    crate::ActorCapture::Acting {
        actor: actor.to_owned(),
        bound_claims: Vec::new(),
    }
}

/// The durable ledger one repository shares across its worktrees.
fn live_ledger(repository: &Path) -> crate::StateLedger {
    let (_, common_git) = crate::tree::git_directories(repository)
        .expect("the repository names both Git directories");
    CurrentState::load(crate::current_state_path(&common_git))
        .expect("load the shared ledger")
        .before()
        .clone()
}

/// Persist one claim into the repository's shared ledger before capture.
///
/// A battery's state authority is published as a digest, so an empty ledger
/// cannot tell a capture that read the repository from one that read nothing.
/// This gives the ledger something to say first.
fn seed_one_claim(repository: &Path, actor: &str) {
    let (_, common_git) = crate::tree::git_directories(repository)
        .expect("the repository names both Git directories");
    let mut state =
        CurrentState::load(crate::current_state_path(&common_git)).expect("load the shared ledger");
    state
        .begin_run(&crate::RenameMap::default(), ["app/main.py"], 100.0)
        .expect("begin one seeding run");
    state
        .reconcile([])
        .expect("no latch observation to reconcile");
    state
        .claim(
            ["app/main.py"],
            &ActorInputs::new("app/main.py")
                .expect("seeding actor provenance")
                .with_active_actor(actor),
        )
        .expect("one seeded claim");
    state
        .finish()
        .expect("finish the seeding run")
        .accept()
        .expect("persist the seeded claim");
}

/// Every authority in the published context came from the captured repository.
///
/// Each expectation is derived from the tracked documents the fixture
/// repository was built from rather than restated here, so an authority that
/// drifted from its source fails against the source instead of against a copy
/// of the drift.
fn assert_captured_authorities(
    report: &BatteryReport,
    capture: &crate::BatteryCapture<'_>,
    repository: &Path,
) {
    let context = report.context();
    assert_eq!(context.source(), capture.source());
    assert_eq!(context.actor().source(), "active-actor");
    assert_eq!(context.actor().value(), "battery-actor");

    assert_eq!(context.scope().name(), "staged-index");
    assert_eq!(
        context.scope().provenance(),
        format!("the index of {}", repository.display())
    );

    let tracked: toml::Value = toml::from_str(
        &fs::read_to_string(repository.join("spice-rust.toml")).expect("tracked policy document"),
    )
    .expect("tracked policy parses");
    let expected = tracked
        .get("policy")
        .and_then(|policy| policy.get("env_names"))
        .expect("the tracked document declares policy.env_names");
    let evidence = context
        .configuration()
        .iter()
        .find(|evidence| evidence.key() == "policy.env_names")
        .expect("the resolved configuration names every tracked leaf");
    assert_eq!(evidence.release(), "current");
    assert_eq!(evidence.layer(), OriginLayer::Repository);
    assert_eq!(evidence.document(), "spice-rust.toml");
    assert_eq!(evidence.field(), ["policy", "env_names"]);
    assert_eq!(
        evidence.value_digest(),
        &Digest::compute(
            Domain::GateConfiguration,
            &wire::encode(expected).expect("tracked value encodes")
        )
    );
    assert!(
        context
            .configuration()
            .iter()
            .all(|evidence| evidence.document() == "spice-rust.toml"),
        "every resolved value must name the document it was read from"
    );

    let reloaded = crate::Contract::load(repository).expect("the tracked contract reloads");
    assert_eq!(
        context.policy(),
        &Digest::compute(
            Domain::GatePolicy,
            &wire::encode(reloaded.rule_policy().document()).expect("policy encodes")
        )
    );

    let ledger = live_ledger(repository);
    assert_eq!(
        ledger
            .claims()
            .iter()
            .map(crate::PeerClaim::actor)
            .collect::<Vec<_>>(),
        ["peer-actor"],
        "the repository must still hold the claim the capture was meant to read"
    );
    assert_eq!(context.state(), &ledger.digest());
    // The repository-document arm measured the one selected document this
    // fixture holds, so the report publishes that exact decision alongside
    // any structural decisions. A capture that reported none would describe a
    // run whose own core family was held to a limit the report never names.
    let document_bounds = context
        .bounds()
        .iter()
        .filter(|bound| bound.actor_inputs().candidate_path() == "app/notes.md")
        .collect::<Vec<_>>();
    let [bound] = document_bounds.as_slice() else {
        panic!(
            "one selected document decided one bound: {:?}",
            document_bounds
        );
    };
    assert_eq!(bound.actor_inputs().candidate_path(), "app/notes.md");
    assert_eq!(bound.measured(), 50);
    assert_eq!(bound.latch_transition(), "clear");
    assert_eq!(
        bound
            .actor()
            .expect("a varied bound resolved an actor")
            .value(),
        "battery-actor",
        "the published decision was seeded by someone other than the actor that ran it"
    );
    assert!(
        context.latches().is_empty(),
        "the fixture ledger holds no durable breach for a run to carry"
    );

    let declared = reloaded
        .tools()
        .first()
        .expect("the contract probed its one declared tool");
    let tool = context
        .tools()
        .iter()
        .find(|tool| tool.name() == declared.name())
        .expect("the published context names every probed tool");
    assert_eq!(tool.program(), declared.program().display().to_string());
    assert_eq!(tool.version(), Some(declared.version()));
    assert_eq!(
        tool.executable_digest(),
        Some(&Digest::compute(
            Domain::GateTool,
            &fs::read(declared.program()).expect("the declared executable")
        ))
    );
}

/// Move the captured repository in every direction a core arm can observe.
///
/// Each mutation is then proved real with the same program the Git-declared
/// arms run, so a caller asserting that the battery ignored all of it is
/// asserting against a repository that genuinely changed underneath it.
fn move_the_repository_after_capture(repository: &Path) {
    fs::write(repository.join("tests/test_main.py"), "def test_broken(:\n")
        .expect("edit an unstaged file after capture");
    fs::write(
        repository.join("app/main.py"),
        "def entry():\n    return 2\n",
    )
    .expect("stage new content after capture");
    fixture_git(repository, &["add", "app/main.py"]);
    fs::write(
        repository.join("app/main.py"),
        "def entry():\n    return 3\n",
    )
    .expect("leave that staged file dirty after capture");
    let head = fixture_git(repository, &["rev-parse", "HEAD"]);
    fs::write(repository.join(".git/MERGE_HEAD"), format!("{head}\n"))
        .expect("start a merge after capture");

    assert_eq!(
        fixture_git(repository, &["diff", "--cached", "--name-only"]),
        "app/main.py",
        "the mutation must really stage the file the staging arm reads"
    );
    assert!(
        fixture_git(repository, &["diff", "--name-only"])
            .lines()
            .any(|path| path == "app/main.py"),
        "the mutation must really leave that same file unstaged as well"
    );
    // The merge is the one mutation written straight into the Git directory
    // rather than made through Git, so it is the one that could stop being a
    // merge without the write ever failing. Asking Git to resolve the ref is
    // what keeps the merge assertion from passing over a repository that is
    // not mid-merge at all.
    assert_eq!(
        fixture_git(repository, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]),
        head,
        "the mutation must really leave a merge Git itself resolves"
    );
}

/// Every canonical arm is invoked once and no family is acquired twice.
fn assert_each_family_ran_once(events: &[RepositoryExecutionEvent]) {
    let arm_events: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            RepositoryExecutionEvent::ArmInvoked { gate_id, .. } => Some(*gate_id),
            RepositoryExecutionEvent::FamilyInvoked { .. }
            | RepositoryExecutionEvent::AnalysisInvoked { .. } => None,
        })
        .collect();
    assert_eq!(arm_events, GateId::BATTERY);

    let family_events: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            RepositoryExecutionEvent::FamilyInvoked { family } => Some(*family),
            RepositoryExecutionEvent::ArmInvoked { .. }
            | RepositoryExecutionEvent::AnalysisInvoked { .. } => None,
        })
        .collect();
    assert_eq!(
        family_events,
        [
            ArmFamily::Core,
            ArmFamily::Toolchain,
            ArmFamily::Metrics,
            ArmFamily::Hygiene,
        ]
    );

    let mut analyses = events
        .iter()
        .filter_map(|event| match event {
            RepositoryExecutionEvent::AnalysisInvoked { gate_id, .. } => Some(*gate_id),
            RepositoryExecutionEvent::ArmInvoked { .. }
            | RepositoryExecutionEvent::FamilyInvoked { .. } => None,
        })
        .collect::<Vec<_>>();
    let mut expected = GateId::BATTERY.to_vec();
    analyses.sort_unstable();
    expected.sort_unstable();
    assert_eq!(analyses, expected);
}

/// Each result carries evidence its own family owns, refusals included.
fn assert_every_family_projected_its_own_evidence(report: &BatteryReport) {
    assert_eq!(report.results().len(), GateId::BATTERY.len());
    for result in report.results() {
        match result.family() {
            ArmFamily::Core => {
                assert!(matches!(result.evidence(), ArmEvidence::Core { .. }));
            }
            ArmFamily::Toolchain => {
                assert!(matches!(result.evidence(), ArmEvidence::Toolchain { .. }));
            }
            ArmFamily::Metrics => assert_metric_arm_acquired(result),
            ArmFamily::Hygiene => {
                assert!(matches!(result.evidence(), ArmEvidence::Hygiene { .. }));
            }
        }
    }
    assert!(
        report.results().iter().all(|result| {
            !matches!(
                result.evidence(),
                ArmEvidence::Hygiene { report }
                    if report.outcome == crate::HygieneOutcome::ParseFailure
            )
        }),
        "an analyzer read the broken file the sealed tree never held"
    );
}

/// One metric arm carried its own acquired evidence rather than a family refusal.
///
/// Each arm lands on the state its own input produces, and names the evidence
/// that state rests on. Accepting any acquired state would pass just as well if
/// the family had gone back to refusing wholesale, which is the exact regression
/// this family's acquisition undoes.
///
/// Every arm is claimed by a row of `ACQUIRED_METRIC_ARMS`, and an arm claimed
/// by none fails here rather than passing unexamined. That list is what says
/// which arms this build measures, so an arm quietly leaving it would be an arm
/// nothing checks at all.
fn assert_metric_arm_acquired(result: &BatteryArmResult) {
    let ArmEvidence::Metrics { report } = result.evidence() else {
        panic!(
            "the metrics family projects its own acquired evidence, but {} carried {:?}",
            result.gate_id(),
            result.evidence()
        )
    };
    assert_eq!(report.gate_id(), result.gate_id());
    let (_, expected, evidence) = ACQUIRED_METRIC_ARMS
        .iter()
        .find(|(gate_id, ..)| *gate_id == result.gate_id())
        .unwrap_or_else(|| {
            panic!(
                "{} is claimed by no metric-arm row, so nothing states what it should report: {}",
                result.gate_id(),
                result.reason()
            )
        });
    assert_eq!(
        result.state(),
        *expected,
        "{} reported the wrong state for this fixture: {}",
        result.gate_id(),
        result.reason()
    );
    assert!(
        result.reason().contains(evidence),
        "{} names the evidence its state rests on, but {evidence:?} is absent from: {}",
        result.gate_id(),
        result.reason()
    );
    if result.gate_id() == GateId::Complexity {
        assert_complexity_measured_the_fixture_routine(report);
    }
}

/// The complexity arm's pass rests on rows it measured, not on an empty scope.
///
/// A pass is what this arm reports when every routine is inside its bound, and
/// it is equally what the arm would report having found no routine at all: an
/// empty scope folds to `NotApplicable`, which no measurement outranks. So the
/// rows are read directly rather than inferred from the state.
///
/// The whole set is pinned, not a sample. The fixture commits one `.rs` and
/// three `.py` sources, so this is also what says both vendored grammars are
/// reached through the arm rather than only the one this workspace is written
/// in: a build that measured Rust alone would still pass, and would fail here.
/// Only two of the `.py` sources declare a routine, `app/__init__.py` being a
/// lone import, so a parsed file holding none contributes no row rather than a
/// zero, and an invented row fails here as surely as a dropped one. The lengths
/// differ across the two classes for the same reason: a parser answering one
/// constant per routine could not produce both `1` and `2`.
fn assert_complexity_measured_the_fixture_routine(report: &StructuralArmEvidence) {
    let measured: Vec<_> = report
        .metrics()
        .iter()
        .filter_map(|evidence| match evidence.state() {
            EvidenceState::Measured { measurement } => Some((
                evidence.metric(),
                evidence.key().clone(),
                measurement.value(),
            )),
            _ => None,
        })
        .collect();
    let key = |path: &str, function: &str| {
        MetricKey::function(path, function).expect("a fixture routine keys")
    };
    assert_eq!(
        measured,
        [
            (MetricId::RoutineCcn, key("app/main.py", "entry"), 1),
            (MetricId::RoutineCcn, key("src/lib.rs", "fixture"), 1),
            (
                MetricId::RoutineCcn,
                key("tests/test_main.py", "test_entry"),
                1
            ),
            (MetricId::RoutineLength, key("app/main.py", "entry"), 2),
            (MetricId::RoutineLength, key("src/lib.rs", "fixture"), 1),
            (
                MetricId::RoutineLength,
                key("tests/test_main.py", "test_entry"),
                2
            ),
        ],
        "the complexity arm passed on rows it measured from the fixture's own sources"
    );
}

/// The two Git-declared arms answer for the repository as it stood at capture.
fn assert_the_git_arms_kept_the_capture(report: &BatteryReport) {
    let observation = |gate_id: GateId| {
        let result = report
            .results()
            .iter()
            .find(|result| result.gate_id() == gate_id)
            .expect("the battery ran every compiled arm");
        let ArmEvidence::Core { report: arm } = result.evidence() else {
            panic!("{} is a core arm", gate_id.as_str())
        };
        arm.observation().clone()
    };
    assert_eq!(
        observation(GateId::Staging),
        CoreObservation::Measured {
            evidence: CoreEvidence::Staging(
                StagingEvidence::new(Vec::<String>::new()).expect("clean staging evidence")
            )
        },
        "staging read a partial staging created after the capture it is labelled with"
    );
    assert_eq!(
        observation(GateId::MergeIntegrity),
        CoreObservation::Measured {
            evidence: CoreEvidence::MergeIntegrity(MergeIntegrityEvidence::NoMerge)
        },
        "merge integrity read a merge started after the capture it is labelled with"
    );
}

/// The scanning arms read the change inventory the executor captured.
///
/// `LocalPaths` and `Taste` scan the selected changes and nothing else, so the
/// list handed in at capture is the whole of what they open. The unselected
/// file is the control: it is committed, dirty for both arms, and named by
/// nobody. An executor that recomputed the change set from the repository would
/// report it, and one that substituted an empty list would report neither file
/// and call a marked tree clean.
#[test]
fn the_scanning_arms_report_exact_findings_over_the_captured_changes() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    let (word, replacement) = discouraged_word();
    let marked = format!("// {LOCAL_PATH_MARKER}somebody/devel\n");
    let prose = format!("This paragraph is {word} and nothing else.\n");
    let column = u32::try_from(prose.find(&word).expect("the fixture names the word") + 1)
        .expect("a fixture column fits");
    let marker_column = u32::try_from(
        marked
            .find(LOCAL_PATH_MARKER)
            .expect("the fixture names the marker")
            + 1,
    )
    .expect("a fixture column fits");
    fs::write(repository.join(MARKED), &marked).expect("fixture marked source");
    fs::create_dir(repository.join("docs")).expect("fixture prose directory");
    fs::write(repository.join(PROSE), &prose).expect("fixture discouraged prose");
    fs::write(
        repository.join("docs/unselected.md"),
        format!("{marked}This paragraph is {word} as well.\n"),
    )
    .expect("fixture change nobody selected");
    fixture_git(&repository, &["add", "--all"]);
    fixture_git(&repository, &["commit", "-q", "-m", "scanned sources"]);

    let hygiene = crate::HygieneConfiguration::new(["app/main.py"], ["tests"])
        .expect("production hygiene roots");
    let mut capture = crate::BatteryCapture::capture(
        &repository,
        &crate::Selector::staged(),
        crate::ClaimCapture::Idle,
        acting("scanning-actor"),
        crate::RimCapture::new(
            &repository,
            None,
            [MARKED, PROSE].map(str::to_owned).to_vec(),
        )
        .expect("the fixture repository root is absolute"),
        hygiene,
    )
    .expect("capture production battery source");
    let report = capture.execute().expect("complete production battery");

    let scanned = |gate_id: GateId| {
        let result = report
            .results()
            .iter()
            .find(|result| result.gate_id() == gate_id)
            .expect("the battery ran every compiled arm");
        let ArmEvidence::Toolchain { report } = result.evidence() else {
            panic!("{} is a toolchain arm", gate_id.as_str())
        };
        assert_eq!(
            report.outcome,
            ArmOutcome::Findings,
            "{} admitted a selected change it should have reported: {}",
            gate_id.as_str(),
            report.reason
        );
        assert_eq!(result.state(), BatteryState::Failed);
        report.findings.clone()
    };

    assert_eq!(
        scanned(GateId::LocalPaths),
        [ArmFinding {
            path: Some(MARKED.to_owned()),
            line: Some(1),
            column: Some(marker_column),
            detail: FindingDetail::LocalPath,
        }],
        "the marker scanner named something other than the one selected marked line"
    );
    assert_eq!(
        scanned(GateId::Taste),
        [ArmFinding {
            path: Some(PROSE.to_owned()),
            line: Some(1),
            column: Some(column),
            detail: FindingDetail::Taste { word, replacement },
        }],
        "the prose scanner named something other than the one selected discouraged word"
    );
}

#[test]
fn every_nonpass_state_refuses_without_erasing_later_results() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::nonpassing(&fixture.source);
    let report = run_battery(&inputs, &mut executor).expect("total nonpass report");

    assert_eq!(executor.calls.len(), 27);
    assert_eq!(executor.calls.last(), Some(&GateId::PrivateInternals));
    assert_eq!(report.results().len(), 27);
    assert!(!report.admits());
    for state in [
        BatteryState::Failed,
        BatteryState::Unavailable,
        BatteryState::Unsupported,
        BatteryState::Unmeasured,
        BatteryState::TimedOut,
        BatteryState::ExplicitSkip,
        BatteryState::NotApplicable,
        BatteryState::Excluded,
    ] {
        assert!(
            report
                .results()
                .iter()
                .any(|result| result.state() == state)
        );
    }
    for result in report.results().iter().filter(|result| {
        matches!(
            result.state(),
            BatteryState::NotApplicable | BatteryState::Excluded
        )
    }) {
        let provenance = result.provenance().expect("admitted skip provenance");
        let expected = result
            .evidence()
            .provenance()
            .expect("typed provenance")
            .expect("skip provenance");
        assert_eq!(provenance, &expected);
    }
}

#[test]
fn executor_refusals_and_invalid_evidence_stay_visible_and_total() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::faulting(&fixture.source, 3, 4);
    let report = run_battery(&inputs, &mut executor).expect("total executor-failure report");

    assert_eq!(executor.calls, GateId::BATTERY);
    assert_eq!(report.results().len(), 27);
    assert_eq!(report.results()[3].state(), BatteryState::Failed);
    assert_eq!(
        report.results()[3].reason(),
        "executor refused arm: provider\\nlost"
    );
    assert_eq!(report.results()[4].state(), BatteryState::Failed);
    assert!(
        report.results()[4]
            .reason()
            .starts_with("executor returned invalid evidence:")
    );
    assert_eq!(
        report.results().last().expect("last result").gate_id(),
        GateId::PrivateInternals
    );

    let value: serde_json::Value =
        serde_json::from_slice(&report.encode().expect("encode")).expect("report JSON");
    assert_refusals(vec![(
        "executor failure names a different compiled declaration",
        mutate(&value, |candidate| {
            candidate["results"][3]["evidence"]["declaration"] =
                serde_json::json!("gate::core::staging::forged-v1")
        }),
    )]);
}
