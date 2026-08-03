use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Output};

use gate::{
    AnalysisEvidence, Contract, GateId, HYGIENE_ARMS, HygieneConfiguration, HygieneExclusion,
    HygieneOutcome, HygieneProvenance, HygieneReport, HygieneSource, Jurisdiction, Selector,
    acquire_hygiene, analyze_hygiene,
};
use serde_json::Value;

mod support;

use support::Tree;

const GIT: &str = "/usr/bin/git";
const PROCESS_FIXTURE: &str = "acquisition_process_fixture";
const REPOSITORY_ENV: &str = "SPICE_HYGIENE_ACQUIRE_REPOSITORY"; // env-policy: allow
const MARKER_ENV: &str = "SPICE_HYGIENE_INTERPRETER_MARKER"; // env-policy: allow

#[test]
fn one_captured_source_reproduces_the_existing_five_arm_analysis() {
    let repository = hygiene_repository("hygiene-acquire-parity", false);
    let tree = Selector::staged()
        .materialize(&repository.root)
        .expect("materialize the captured hygiene source");
    let contract = Contract::load(tree.root()).expect("load the captured contract");
    let configuration = configuration();

    let prior_jurisdiction = Jurisdiction::walk(tree.root(), contract.policy().ignore())
        .expect("walk the existing hygiene fixture jurisdiction");
    let expected = analyze_hygiene(&prior_jurisdiction, &configuration)
        .expect("run the existing source-derived hygiene fixture");
    let source = HygieneSource::capture(&tree, &contract).expect("capture one hygiene source");
    let acquired =
        acquire_hygiene(&source, &configuration).expect("acquire the complete hygiene family");

    assert_eq!(source.root(), tree.root());
    assert_eq!(
        source.digest(),
        &prior_jurisdiction
            .tree_digest()
            .expect("fixture tree identity")
    );
    assert_eq!(without_elapsed(&acquired), without_elapsed(&expected));
    assert_eq!(
        acquired
            .arms
            .iter()
            .map(|measurement| measurement.gate_id)
            .collect::<Vec<_>>(),
        HYGIENE_ARMS
            .iter()
            .map(|declaration| declaration.id())
            .collect::<Vec<_>>()
    );
    assert!(acquired.arms.iter().all(|measurement| {
        measurement.evidence.iter().any(|evidence| {
            matches!(
                evidence,
                AnalysisEvidence::Analyzer(_) | AnalysisEvidence::LanguageApplicability { .. }
            )
        })
    }));

    repository.remove();
}

#[test]
fn compiled_and_tracked_exclusions_are_visible_in_the_acquired_evidence() {
    let repository = hygiene_repository("hygiene-acquire-exclusions", true);
    let tree = Selector::staged()
        .materialize(&repository.root)
        .expect("materialize the captured hygiene source");
    let contract = Contract::load(tree.root()).expect("load the captured contract");
    let source = HygieneSource::capture(&tree, &contract).expect("capture one hygiene source");

    let ignored = source
        .jurisdiction()
        .entries()
        .iter()
        .find(|entry| entry.path() == ".venv/broken.py")
        .expect("compiled exclusion remains in the total inventory");
    assert!(!ignored.admission().admits());

    let provenance = HygieneProvenance {
        release: "current".to_owned(),
        layer: "repository".to_owned(),
        document: "spice.toml".to_owned(),
        field: "policy.exclude[0]".to_owned(),
    };
    let exclusion = HygieneExclusion::new(
        GateId::PrivateInternals,
        provenance.clone(),
        "tracked private-boundary exclusion",
    )
    .expect("admit the tracked exclusion");
    let configured = configuration()
        .with_exclusions([exclusion])
        .expect("one exclusion per arm");
    let report = acquire_hygiene(&source, &configured)
        .expect("ignored malformed source cannot fail acquisition");

    for (measurement, declaration) in report.arms.iter().zip(HYGIENE_ARMS) {
        assert_eq!(measurement.roots, declaration.jurisdiction().roots());
        assert_eq!(measurement.includes, declaration.jurisdiction().includes());
        assert_eq!(measurement.excludes, declaration.jurisdiction().excludes());
        assert_ne!(measurement.outcome, HygieneOutcome::ParseFailure);
    }
    let private = report
        .arms
        .iter()
        .find(|measurement| measurement.gate_id == GateId::PrivateInternals)
        .expect("private-boundary arm");
    assert_eq!(private.outcome, HygieneOutcome::Excluded);
    assert!(private.findings.is_empty());
    assert!(matches!(
        private.evidence.as_slice(),
        [AnalysisEvidence::Exclusion(evidence)]
            if evidence.provenance == provenance
                && evidence.reason == "tracked private-boundary exclusion"
    ));

    repository.remove();
}

#[test]
fn acquisition_never_invokes_python_or_python3() {
    let repository = hygiene_repository("hygiene-acquire-tripwire", false);
    let tripwire = Tree::new("hygiene-interpreter-tripwire");
    let marker = tripwire.path("invoked");
    for name in ["python", "python3"] {
        let executable = tripwire.path(name);
        fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' \"$0\" >> \"$SPICE_HYGIENE_INTERPRETER_MARKER\"\nexit 97\n",
        )
        .expect("write interpreter tripwire");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
            .expect("make interpreter tripwire executable");
    }

    let output = Command::new(std::env::current_exe().expect("current test executable"))
        .args(["--ignored", "--exact", PROCESS_FIXTURE, "--nocapture"])
        .env_clear()
        .env("PATH", &tripwire.root)
        .env(REPOSITORY_ENV, &repository.root)
        .env(MARKER_ENV, &marker)
        .output()
        .expect("launch isolated acquisition fixture");
    assert!(
        output.status.success(),
        "isolated acquisition failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !marker.exists(),
        "hygiene acquisition invoked {}",
        fs::read_to_string(&marker).unwrap_or_default()
    );

    tripwire.remove();
    repository.remove();
}

#[test]
#[ignore = "subprocess fixture for the interpreter tripwire"]
#[allow(
    clippy::disallowed_methods,
    reason = "the isolated test process receives only explicit fixture paths"
)]
fn acquisition_process_fixture() {
    let repository = std::env::var_os(REPOSITORY_ENV).expect("fixture repository environment");
    let tree = Selector::staged()
        .materialize(Path::new(&repository))
        .expect("materialize the isolated source");
    let contract = Contract::load(tree.root()).expect("load the isolated contract");
    let source = HygieneSource::capture(&tree, &contract).expect("capture the isolated source");
    let report =
        acquire_hygiene(&source, &configuration()).expect("acquire without an interpreter");
    assert_eq!(report.arms.len(), HYGIENE_ARMS.len());
}

fn configuration() -> HygieneConfiguration {
    HygieneConfiguration::new(["app/main.py"], ["tests"])
        .expect("fixture roots")
        .with_assertion_helpers(["expect_error"])
        .expect("fixture assertion helper")
}

fn hygiene_repository(label: &str, excluded_broken_source: bool) -> Tree {
    let repository = git_repository(label);
    repository.write("spice.toml", &workspace_document("spice.toml"));
    repository.write("baseline.toml", &workspace_document("baseline.toml"));
    repository.write("pyproject.toml", "[project]\nname = 'fixture'\n");
    repository.write("app/__init__.py", "from .main import entry\n");
    repository.write(
        "app/main.py",
        "from app.used import live\n\ndef entry():\n    return live()\n",
    );
    repository.write(
        "app/used.py",
        concat!(
            "def live():\n    return 1\n\n",
            "def test_only():\n    return 2\n\n",
            "def orphan():\n    return 3\n",
        ),
    );
    repository.write("app/_internal.py", "def _secret():\n    return 1\n");
    repository.write(
        "tests/test_main.py",
        concat!(
            "import app._internal as hidden\n",
            "from app.used import live, test_only\n\n",
            "def test_live():\n    assert live() == 1\n",
            "    assert test_only() == 2\n",
            "    assert hidden._secret() == 1\n\n",
            "def test_no_assert():\n    live()\n",
        ),
    );
    if excluded_broken_source {
        repository.write(".venv/broken.py", "def broken(:\n");
    }
    git_commit(&repository, "hygiene fixture");
    repository
}

fn without_elapsed(report: &HygieneReport) -> Value {
    let mut value = serde_json::to_value(report).expect("encode fixture report");
    let arms = value
        .get_mut("arms")
        .and_then(Value::as_array_mut)
        .expect("report arms");
    for arm in arms {
        let evidence = arm
            .get_mut("evidence")
            .and_then(Value::as_array_mut)
            .expect("arm evidence");
        for item in evidence {
            if item.get("evidence") == Some(&Value::String("deadline".to_owned())) {
                item.as_object_mut()
                    .expect("deadline evidence object")
                    .insert("elapsed_ms".to_owned(), Value::from(0));
            }
        }
    }
    value
}

fn git_repository(label: &str) -> Tree {
    let repository = Tree::new(label);
    git(&repository, &["init", "-q", "-b", "main"]);
    git(&repository, &["config", "core.excludesFile", "/dev/null"]);
    git(&repository, &["config", "user.name", "Gate Fixture"]);
    git(
        &repository,
        &["config", "user.email", "gate@example.invalid"],
    );
    repository
}

fn git_commit(repository: &Tree, message: &str) {
    git(repository, &["add", "--all"]);
    git(repository, &["commit", "-q", "-m", message]);
}

fn git(repository: &Tree, arguments: &[&str]) -> String {
    let output = git_output(repository, arguments);
    assert!(
        output.status.success(),
        "git {} failed:\nstdout:\n{}\nstderr:\n{}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git output is UTF-8")
}

fn git_output(repository: &Tree, arguments: &[&str]) -> Output {
    Command::new(GIT)
        .args(arguments)
        .current_dir(&repository.root)
        .output()
        .expect("launch git fixture command")
}

fn workspace_document(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {} for the fixture: {error}", path.display()))
}
