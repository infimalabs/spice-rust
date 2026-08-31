use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use wire::{Digest, Domain};

use super::fixtures;
use super::*;
use crate::RulePolicy;
use crate::core::{CoreEvidence, CoreObservation, MergeIntegrityEvidence, StagingEvidence};
use crate::manifest::{GateId, OriginLayer, PREDECESSOR_GATE_RELEASE};
use crate::state::{
    ActorInputs, ActorResolution, BoundDecision, BoundRule, CurrentState, LatchEntry, LatchKind,
    LatchObservation, RenameMap, StateKey,
};
use crate::toolchain::{ArmFinding, ArmOutcome, FindingDetail, LOCAL_PATH_MARKER};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

/// The Rust file carrying a local path marker, selected as changed.
const MARKED: &str = "src/marked.rs";

/// The prose file carrying a discouraged word, selected as changed.
const PROSE: &str = "docs/guide.md";

/// The Python sources `production_repository` commits, which this build's
/// scanner refuses by field rather than reading with C-grammar semantics.
///
/// Named here rather than beside one fixture so that every fixture needing the
/// magic arm to reach a verdict takes the same class out of the index. Any one
/// of them left committed reports the class refusal from its own row, which is
/// enough for the arm to fold to that refusal whatever else it measured.
const PREDECESSOR_CLASS_SOURCES: [&str; 3] =
    ["app/__init__.py", "app/main.py", "tests/test_main.py"];

mod executor;
mod file_shape;
mod fold;
mod latch_wire;
mod magic;
mod manifest;
mod merge_parents;
mod report;
mod single_arm;

fn fixture_baseline(floors: &str) -> Digest {
    Digest::compute(Domain::GateBaseline, floors.as_bytes())
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
        let actor_inputs = ActorInputs::new("src/lib.rs")
            .expect("actor inputs")
            .with_active_actor("fixture-actor");
        let state_path = root.join("current.json");
        let mut state = CurrentState::load(&state_path).expect("empty current state");
        state
            .begin_run(&RenameMap::default(), ["src/lib.rs"], 100.0)
            .expect("begin state seed");
        state
            .reconcile([LatchObservation::new(
                LatchEntry::new(
                    LatchKind::FileLoc,
                    StateKey::path("src/lib.rs").expect("fixture path key"),
                )
                .expect("fixture latch key"),
                BoundRule::new("file-shape", "file-loc", 100, 150, false, true)
                    .expect("fixture latch bound"),
                1_000,
                actor_inputs.clone(),
                5,
            )
            .expect("fixture latch observation")])
            .expect("seed fixture latch");
        state
            .finish()
            .expect("finish state seed")
            .accept()
            .expect("persist state seed");
        let state = CurrentState::load(state_path).expect("current state");
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
                "spice-rust.toml",
                vec!["policy".to_owned(), "test_paths".to_owned()],
            )
            .expect("configuration provenance"),
        ];
        let policy = crate::migrate_predecessor_policy(
            "spice-rust.toml",
            &format!(
                "{}{}",
                crate::NARROWEST_FILE_CLASSES,
                r#"
                [[policy.rules]]
                scopes = { paths = ["src/**"] }

                [policy.rules.file_loc]
                multiplier = 1.0
            "#
            ),
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

/// The acting actor every real-repository battery below is captured under.
///
/// Named once and shared because it is not decoration: an elastic limit is
/// seeded from the actor a path's headroom varies with, so two fixtures spelling
/// it differently would measure the same file against two different limits.
const FIXTURE_BATTERY_ACTOR: &str = "battery-actor";

/// Run one complete battery over a repository and return its report.
fn battery_report(repository: &Path) -> BatteryReport {
    battery_report_with_changes(repository, &[])
}

/// Run one battery with the exact staged paths the rim attributed to it.
fn battery_report_with_changes(repository: &Path, changes: &[&str]) -> BatteryReport {
    let mut capture = crate::BatteryCapture::capture(
        repository,
        &crate::Selector::staged(),
        crate::ClaimCapture::Idle,
        crate::ActorCapture::Acting {
            actor: FIXTURE_BATTERY_ACTOR.to_owned(),
            bound_claims: Vec::new(),
        },
        crate::RimCapture::new(
            repository,
            None,
            changes.iter().map(|path| (*path).to_owned()),
        )
        .expect("the fixture repository root is absolute"),
        crate::HygieneConfiguration::new(["app/main.py"], ["tests"]).expect("hygiene roots"),
    )
    .expect("capture the fixture battery source");
    capture.execute().expect("complete fixture battery")
}

fn production_repository(root: &Path) -> PathBuf {
    let repository = root.join("repository");
    fs::create_dir(&repository).expect("production repository root");
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    fs::write(
        repository.join("spice-rust.toml"),
        fs::read(workspace.join("spice-rust.toml")).expect("workspace spice-rust.toml"),
    )
    .expect("fixture spice-rust.toml");
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

/// Apply one operation from the fixture's closed in-process repository vocabulary.
///
/// Operations that produce observable repository identities return them as
/// text for the existing assertions. Anything outside this typed fixture
/// vocabulary refuses instead of falling back to a child process.
fn fixture_git(repository: &Path, arguments: &[&str]) -> String {
    match arguments {
        ["init", ..] => crate::test_repository::initialize(repository),
        ["config", ..] => {}
        ["add", "--all"] => crate::test_repository::stage_all(repository),
        ["add", "-u", "--", paths @ ..] => crate::test_repository::stage(repository, paths),
        ["add", paths @ ..] => crate::test_repository::stage(repository, paths),
        ["commit", .., "-m", message] => {
            crate::test_repository::commit(repository, message);
        }
        ["rm", "-q", "--cached", paths @ ..] => {
            crate::test_repository::remove_from_index(repository, paths);
        }
        ["rm", .., path] => crate::test_repository::remove(repository, &[*path]),
        ["mv", source, destination] => {
            crate::test_repository::move_path(repository, source, destination);
        }
        ["checkout", "-q", "-b", branch] => {
            crate::test_repository::checkout(repository, branch, true);
        }
        ["checkout", "-q", branch] => {
            crate::test_repository::checkout(repository, branch, false);
        }
        ["merge", "--no-commit", "--no-ff", branch] => {
            assert_eq!(
                crate::test_repository::merge(repository, branch),
                crate::test_repository::MergeOutcome::Clean,
                "fixture merge must be clean"
            );
        }
        ["rev-parse", "HEAD"] => return crate::test_repository::head(repository),
        ["rev-parse", "HEAD^{tree}"] => return crate::test_repository::head_tree(repository),
        ["rev-parse", "-q", "--verify", "MERGE_HEAD"] => {
            return fs::read_to_string(repository.join(".git/MERGE_HEAD"))
                .expect("read fixture MERGE_HEAD")
                .trim()
                .to_owned();
        }
        ["diff", "--cached", "--name-only"] => {
            return crate::test_repository::staged_changes(repository);
        }
        ["diff", "--name-only"] => return crate::test_repository::worktree_changes(repository),
        unsupported => panic!("unsupported in-process fixture operation {unsupported:?}"),
    }
    String::new()
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove battery fixture");
    }
}
