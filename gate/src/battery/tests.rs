use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use wire::{Digest, Domain};

use super::fixtures;
use super::*;
use crate::RulePolicy;
use crate::core::{CoreEvidence, CoreObservation, MergeIntegrityEvidence, StagingEvidence};
use crate::manifest::{GateId, OriginLayer, PREDECESSOR_GATE_RELEASE};
use crate::state::{
    ActorInputs, ActorResolution, BoundDecision, BoundRule, CurrentState, PredecessorSources,
};
use crate::toolchain::{ArmFinding, ArmOutcome, FindingDetail, LOCAL_PATH_MARKER};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

/// The Rust file carrying a local path marker, selected as changed.
const MARKED: &str = "src/marked.rs";

/// The prose file carrying a discouraged word, selected as changed.
const PROSE: &str = "docs/guide.md";

mod executor;
mod manifest;
mod minting;
mod report;

fn fixture_baseline(floors: &str) -> Digest {
    Digest::compute(Domain::GateBaseline, floors.as_bytes())
}

fn minted(fixture: &Fixture, baseline: &Digest) -> (BatteryReport, BatteryReceipt) {
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::all_passed(&fixture.source);
    let report = run_battery(&inputs, &mut executor).expect("complete battery");
    let receipt = BatteryReceipt::mint(report.clone(), baseline);
    (report, receipt)
}

struct RecordingExecutor {
    calls: Vec<GateId>,
    evidence: Vec<ArmEvidence>,
    refusal: Option<usize>,
    invalid: Option<usize>,
}

impl RecordingExecutor {
    fn all_passed(source: &Digest) -> Self {
        Self::with_evidence(fixtures::passing(source))
    }

    fn nonpassing(source: &Digest) -> Self {
        Self::with_evidence(fixtures::nonpassing(source))
    }

    fn with_evidence(evidence: Vec<ArmEvidence>) -> Self {
        Self {
            calls: vec![],
            evidence,
            refusal: None,
            invalid: None,
        }
    }

    fn faulting(source: &Digest, refusal: usize, invalid: usize) -> Self {
        Self {
            refusal: Some(refusal),
            invalid: Some(invalid),
            ..Self::all_passed(source)
        }
    }
}

impl BatteryExecutor for RecordingExecutor {
    fn execute(
        &mut self,
        arm: &CompiledArm,
        inputs: &BatteryInputs<'_>,
    ) -> Result<ArmExecution, BatteryError> {
        assert_eq!(inputs.source().as_str().len(), 64);
        assert_eq!(inputs.actor().value(), "fixture-actor");
        assert_eq!(inputs.scope().name(), "selected-index");
        assert_eq!(inputs.configuration().len(), 1);
        assert!(!inputs.policy().document().rules().is_empty());
        assert!(
            inputs
                .policy()
                .document()
                .rules()
                .iter()
                .any(|rule| rule.id().as_str().starts_with("legacy-rule:"))
        );
        assert_eq!(inputs.state().latches().len(), 1);
        assert_eq!(inputs.bounds().len(), 1);
        assert_eq!(inputs.tools().len(), 1);

        self.calls.push(arm.gate_id());
        if self.refusal == Some(arm.position()) {
            return Err(BatteryError::new("provider\nlost"));
        }
        let evidence = self.evidence[arm.position()].clone();
        let state = evidence.state()?;
        let provenance = evidence.provenance()?;
        let reason = evidence.reason()?;
        let execution = ArmExecution::new(state, reason, provenance, evidence)?;
        if self.invalid == Some(arm.position()) {
            let mut value = serde_json::to_value(execution).expect("execution JSON");
            value["reason"] = serde_json::json!("forged\nreason");
            return Ok(serde_json::from_value(value).expect("structurally typed execution"));
        }
        Ok(execution)
    }
}

fn assert_family_evidence(evidence: &ArmEvidence, source: &Digest) {
    match evidence {
        ArmEvidence::Core { report } => {
            assert_eq!(report.tree(), source);
            assert!(report.evidence().is_some());
        }
        ArmEvidence::Toolchain { report } => {
            assert_eq!(report.outcome, crate::toolchain::ArmOutcome::Passed);
            assert!(!report.configuration_source.is_empty());
            assert!(!report.configuration_field.is_empty());
            assert!(!report.applicability.is_empty());
            let declaration = crate::toolchain::TOOLCHAIN_ARMS
                .into_iter()
                .find(|arm| arm.id() == report.gate_id)
                .expect("toolchain declaration");
            assert_eq!(report.tools.len(), declaration.tools().len());
            for (evidence, required) in report.tools.iter().zip(declaration.tools()) {
                let crate::toolchain::ToolEvidence::Identified {
                    name,
                    requested_program,
                    version,
                    digest,
                    ..
                } = evidence
                else {
                    panic!("passing toolchain evidence must identify every declared tool")
                };
                assert_eq!(name, required.name());
                assert_eq!(requested_program, required.program());
                assert_eq!(version, required.exact_version());
                assert!(!digest.is_empty());
            }
        }
        ArmEvidence::Metrics { report } => {
            assert!(!report.metrics().is_empty());
            for metric in report.metrics() {
                let crate::metrics::EvidenceState::Measured { measurement } = metric.state() else {
                    panic!("passing metric evidence must be measured")
                };
                if matches!(
                    metric.metric(),
                    crate::metrics::MetricId::FileLines
                        | crate::metrics::MetricId::FileBytes
                        | crate::metrics::MetricId::RoutineCcn
                        | crate::metrics::MetricId::RoutineLength
                ) {
                    let rule = measurement.rule().expect("selected metric policy rule");
                    assert_eq!(rule.rule_id(), Some("fixture-source-rule"));
                    let latch = measurement.latch().expect("metric threshold and latch");
                    assert_eq!(latch.transition(), "clear");
                    assert!(!latch.over_limit());
                }
            }
        }
        ArmEvidence::Hygiene { report } => {
            assert_eq!(report.outcome, crate::hygiene::HygieneOutcome::Passed);
            assert!(!report.configuration.is_empty());
            assert!(!report.evidence.is_empty());
            assert!(!report.traversals.is_empty());
            assert!(report.evidence.iter().any(|evidence| matches!(
                evidence,
                crate::hygiene::AnalysisEvidence::PolicyDecision { .. }
            )));
            assert!(report.evidence.iter().any(|evidence| matches!(
                evidence,
                crate::hygiene::AnalysisEvidence::Analyzer(
                    crate::hygiene::AnalyzerEvidence::Identified { digest, .. }
                ) if !digest.is_empty()
            )));
        }
        ArmEvidence::ExecutorFailure { .. } => {
            panic!("passing fixture must retain family-owned evidence")
        }
    }
}

fn mutate(
    value: &serde_json::Value,
    change: impl FnOnce(&mut serde_json::Value),
) -> serde_json::Value {
    let mut changed = value.clone();
    change(&mut changed);
    changed
}

/// Every forged report must be refused by the exact invariant it violates.
///
/// A re-serialized candidate no longer carries canonical field order, so decode
/// would refuse all of them on bytes alone; naming the expected refusal is what
/// proves each mutation is actually caught by the contract it targets.
fn assert_refusals(candidates: Vec<(&str, serde_json::Value)>) {
    let mut unproven = Vec::new();
    for (invariant, candidate) in candidates {
        let bytes = serde_json::to_vec(&candidate).expect("candidate JSON");
        let error = BatteryReport::decode(&bytes).expect_err("forged report must be refused");
        if !error.detail().contains(invariant) {
            unproven.push(format!(
                "expected a refusal naming {invariant:?}, observed {:?}",
                error.detail()
            ));
        }
    }
    assert!(unproven.is_empty(), "{}", unproven.join("\n"));
}

struct Fixture {
    root: PathBuf,
    source: Digest,
    actor: ActorResolution,
    scope: BatteryScope,
    configuration: Vec<ConfigurationEvidence>,
    policy: RulePolicy,
    state: CurrentState,
    bounds: Vec<BoundDecision>,
    tools: Vec<BatteryToolEvidence>,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-gate-battery-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let worktree = root.join("worktree");
        let shared = root.join("shared");
        fs::create_dir_all(&worktree).expect("worktree state root");
        fs::create_dir_all(&shared).expect("shared state root");
        fs::write(
            worktree.join("file-loc-sticky.json"),
            br#"{"version":1,"paths":["src/lib.rs"]}"#,
        )
        .expect("predecessor latch");
        let sources = PredecessorSources::new(&worktree, &shared);
        let state = CurrentState::load(root.join("current.json"), &sources).expect("current state");
        let actor_inputs = ActorInputs::new("src/lib.rs")
            .expect("actor inputs")
            .with_active_actor("fixture-actor");
        let actor = actor_inputs.require_actor().expect("explicit actor");
        let bounds = vec![
            BoundRule::new("file-shape", "file-loc", 100, 150, false, true)
                .expect("bound rule")
                .decide(101, false, &actor_inputs, 5)
                .expect("bound decision"),
        ];
        let source = Digest::compute(Domain::GateTree, b"fixture selected tree");
        let configuration = vec![
            ConfigurationEvidence::new(
                "policy.test_paths",
                Digest::compute(Domain::ConfigExecutableSurface, b"tests"),
                "current",
                OriginLayer::Repository,
                "spice.toml",
                vec!["policy".to_owned(), "test_paths".to_owned()],
            )
            .expect("configuration provenance"),
        ];
        let policy = crate::migrate_predecessor_policy(
            "spice.toml",
            r#"
                [[policy.rules]]
                scopes = { paths = ["src/**"] }

                [policy.rules.file_loc]
                multiplier = 1.0
            "#,
        )
        .expect("compiled fixture policy");
        let tools = vec![
            BatteryToolEvidence::identified(
                "cargo",
                "cargo",
                "cargo fixture",
                Digest::compute(Domain::GateTool, b"fixture cargo"),
            )
            .expect("tool evidence"),
        ];
        Self {
            root,
            source,
            actor,
            scope: BatteryScope::new("selected-index", "compiled test selector").expect("scope"),
            configuration,
            policy,
            state,
            bounds,
            tools,
        }
    }

    fn inputs(&self) -> BatteryInputs<'_> {
        BatteryInputs::new(
            &self.source,
            &self.actor,
            &self.scope,
            &self.configuration,
            &self.policy,
            self.state.before(),
            &self.bounds,
            &self.tools,
        )
        .expect("battery inputs")
    }

    fn policy_digest(&self) -> Digest {
        Digest::compute(
            Domain::GatePolicy,
            &wire::encode(self.policy.document()).expect("policy JSON"),
        )
    }
}

fn production_repository(root: &Path) -> PathBuf {
    let repository = root.join("repository");
    fs::create_dir(&repository).expect("production repository root");
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    fs::write(
        repository.join("spice.toml"),
        fs::read(workspace.join("spice.toml")).expect("workspace spice.toml"),
    )
    .expect("fixture spice.toml");
    fs::write(
        repository.join("baseline.toml"),
        fs::read(workspace.join("baseline.toml")).expect("workspace baseline.toml"),
    )
    .expect("fixture baseline.toml");
    fs::write(
        repository.join("pyproject.toml"),
        "[project]\nname = 'battery-fixture'\n",
    )
    .expect("fixture pyproject.toml");
    for directory in ["app", "tests", "src"] {
        fs::create_dir(repository.join(directory)).expect("fixture source directory");
    }
    fs::write(
        repository.join("app/__init__.py"),
        "from .main import entry\n",
    )
    .expect("fixture package");
    fs::write(
        repository.join("app/main.py"),
        "def entry():\n    return 1\n",
    )
    .expect("fixture production source");
    fs::write(
        repository.join("tests/test_main.py"),
        "from app.main import entry\n\ndef test_entry():\n    assert entry() == 1\n",
    )
    .expect("fixture test source");
    fs::write(repository.join("src/lib.rs"), "pub fn fixture() {}\n").expect("fixture Rust source");
    // One Markdown source whose inline link differs from the tracked path only
    // by case. The mismatch is what makes the markdown arm's result depend on
    // the tracked-path inventory reaching the scanner: with an empty or wrongly
    // rooted inventory there is nothing to compare against, and the same arm
    // would report a clean measurement over the very same bytes.
    fs::write(
        repository.join("app/notes.md"),
        "# Notes\n\nThe entrypoint lives in [main](Main.py).\n",
    )
    .expect("fixture Markdown source");

    fixture_git(&repository, &["init", "-q", "-b", "main"]);
    fixture_git(&repository, &["config", "core.excludesFile", "/dev/null"]);
    fixture_git(&repository, &["config", "user.name", "Battery Fixture"]);
    fixture_git(
        &repository,
        &["config", "user.email", "battery@example.invalid"],
    );
    fixture_git(&repository, &["add", "--all"]);
    fixture_git(&repository, &["commit", "-q", "-m", "fixture"]);
    repository
}

/// The first compiled discouraged word and what it asks for instead.
///
/// Read off the vocabulary rather than spelled here, so a fixture cannot outlive
/// the entry it was written against.
fn discouraged_word() -> (String, String) {
    let (pattern, replacement) = crate::toolchain::PACKAGED_TASTE_WORDS
        .first()
        .expect("the compiled vocabulary is not empty");
    (
        pattern.trim_end_matches('*').to_owned(),
        (*replacement).to_owned(),
    )
}

/// Run one fixture Git command and return its trimmed standard output.
///
/// The output is returned rather than discarded so a fixture that mutates the
/// repository can prove the mutation with the same program the Git-declared
/// arms run, instead of asserting against its own idea of what Git did.
fn fixture_git(repository: &Path, arguments: &[&str]) -> String {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(repository)
        .env_clear()
        .env("HOME", repository)
        .output()
        .expect("run fixture git");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("fixture git output is text")
        .trim()
        .to_owned()
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove battery fixture");
    }
}
