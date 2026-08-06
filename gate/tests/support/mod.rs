use gate::{
    ActorCapture, ArmObservation, ArmOutcome, CORE_GATE_IDS, ClaimCapture, Contract, CoreArmInput,
    CoreArmReport, CoreEvidence, CoreInputs, CoreObservation, CoreReport, CoreSource, GateId,
    LatchEntry, Selector, ToolchainDeclaration, acquire_core, evaluate_core,
};

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

pub(crate) mod toolchain;

/// Names tried before a scratch root is declared unobtainable.
const CREATE_ATTEMPTS: usize = 128;

/// The one program every fixture repository is built with.
pub(crate) const GIT: &str = "/usr/bin/git";

static NEXT_TREE: AtomicU32 = AtomicU32::new(0);

/// Docker Desktop's amd64 translation cannot reap concurrent fixture Git
/// children reliably. Each integration binary owns this support module, so one
/// barrier keeps every Git child lifetime in that binary disjoint.
static FIXTURE_GIT_BARRIER: Mutex<()> = Mutex::new(());

/// A source tree the fixture creates, owns, and can prove it removed.
pub(crate) struct Tree {
    pub(crate) root: PathBuf,
    removed: bool,
}

impl Tree {
    pub(crate) fn new(label: &str) -> Self {
        let directory = std::env::temp_dir();
        let process = std::process::id();
        let root = create_scratch_root(std::iter::repeat_with(|| {
            let unique = NEXT_TREE.fetch_add(1, Ordering::Relaxed);
            directory.join(format!("gate-{label}-{process}-{unique}"))
        }));
        Self {
            root,
            removed: false,
        }
    }

    pub(crate) fn write(&self, relative: &str, contents: &str) {
        let path = self.root.join(relative);
        let parent = path.parent().expect("a fixture file sits in a directory");
        fs::create_dir_all(parent).expect("create a fixture directory");
        fs::write(&path, contents).expect("write a fixture file");
    }

    #[allow(dead_code)]
    pub(crate) fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// Remove the owned tree and prove it is gone.
    ///
    /// Each integration binary compiles this module alone; enforcement relies
    /// on `Drop`, while the ownership tests call this observable path.
    #[allow(dead_code)]
    pub(crate) fn remove(mut self) {
        fs::remove_dir_all(&self.root)
            .unwrap_or_else(|error| panic!("remove fixture at {}: {error}", self.root.display()));
        assert!(
            !self.root.exists(),
            "fixture outlived removal at {}",
            self.root.display()
        );
        self.removed = true;
    }
}

impl Drop for Tree {
    /// Best effort during unwinding; explicit teardown remains observable.
    fn drop(&mut self) {
        if self.removed {
            return;
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// One initialized fixture repository with a committed identity.
///
/// Each integration binary compiles this module alone, so the fixture helpers
/// only some binaries reach are still live code for the ones that do.
#[allow(dead_code)]
pub(crate) fn git_repository(label: &str) -> Tree {
    let repository = Tree::new(label);
    git(&repository, &["init", "-q", "-b", "main"]);
    git(&repository, &["config", "user.name", "Gate Fixture"]);
    git(
        &repository,
        &["config", "user.email", "gate@example.invalid"],
    );
    repository
}

/// Stage everything present and commit it under one message.
#[allow(dead_code)]
pub(crate) fn git_commit(repository: &Tree, message: &str) {
    git(repository, &["add", "--all"]);
    git(repository, &["commit", "-q", "-m", message]);
}

/// Run one fixture Git command that is required to succeed.
#[allow(dead_code)]
pub(crate) fn git(repository: &Tree, arguments: &[&str]) -> String {
    let output = git_output(repository, arguments);
    assert!(
        output.status.success(),
        "git {arguments:?} exited {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("Git fixture output is UTF-8")
        .trim()
        .to_owned()
}

/// Run one fixture Git command and return its complete completion.
#[allow(dead_code)]
pub(crate) fn git_output(repository: &Tree, arguments: &[&str]) -> Output {
    let _fixture_git_guard = fixture_git_barrier();
    Command::new(GIT)
        .args(arguments)
        .current_dir(&repository.root)
        .env_clear()
        .env("HOME", &repository.root)
        .output()
        .unwrap_or_else(|error| panic!("run git {arguments:?}: {error}"))
}

/// Hold the integration binary's Git-child barrier across an external fixture event.
#[allow(dead_code)]
pub(crate) fn fixture_git_barrier() -> MutexGuard<'static, ()> {
    FIXTURE_GIT_BARRIER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// One tracked workspace document, read from the real workspace.
#[allow(dead_code)]
pub(crate) fn workspace_document(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {} for the fixture: {error}", path.display()))
}

/// One tracked policy document a fixture repository can be gated against.
///
/// Every document carries the file-class selections migration requires, so a
/// fixture states only what it is actually about and still loads at all.
#[allow(dead_code)]
pub(crate) fn policy_document(body: &str) -> String {
    format!("{}{body}", gate::NARROWEST_FILE_CLASSES)
}

/// One committed fixture repository carrying the tracked contract documents.
#[allow(dead_code)]
pub(crate) fn contract_repository(label: &str) -> Tree {
    let repository = git_repository(label);
    repository.write("spice.toml", &workspace_document("spice.toml"));
    repository.write("baseline.toml", &workspace_document("baseline.toml"));
    repository.write("src/app.rs", "fn main() {}\n");
    git_commit(&repository, "fixture contract");
    repository
}

#[allow(dead_code)]
pub(crate) fn set_environment_names(repository: &Tree, names: &[&str]) {
    let path = repository.root.join("spice.toml");
    let mut document: toml::Value =
        toml::from_str(&fs::read_to_string(&path).expect("read fixture environment policy"))
            .expect("parse fixture environment policy");
    document["policy"]["env_names"] = toml::Value::Array(
        names
            .iter()
            .map(|name| toml::Value::String((*name).to_owned()))
            .collect(),
    );
    repository.write(
        "spice.toml",
        &toml::to_string(&document).expect("encode fixture environment policy"),
    );
}

#[allow(dead_code)]
pub(crate) fn inapplicable_without_tools(
    declaration: ToolchainDeclaration,
    observation: &ArmObservation,
) -> bool {
    if observation.outcome != ArmOutcome::NotApplicable {
        return false;
    }
    assert!(
        observation.tools.is_empty(),
        "{} probed tools before its applicability predicate",
        declaration.id().as_str()
    );
    true
}

#[allow(dead_code)]
pub(crate) fn fixture_column(line: &str, needle: &str) -> u32 {
    let start = line
        .find(needle)
        .expect("the fixture line names the needle");
    u32::try_from(line[..start].chars().count() + 1).expect("a fixture column fits")
}

/// The first candidate this call creates, never one it merely finds.
pub(crate) fn create_scratch_root(candidates: impl Iterator<Item = PathBuf>) -> PathBuf {
    for root in candidates.take(CREATE_ATTEMPTS) {
        match fs::create_dir(&root) {
            Ok(()) => return root,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => panic!("create fixture root at {}: {error}", root.display()),
        }
    }
    panic!("no fixture root was free within {CREATE_ATTEMPTS} attempts");
}

/// One fixture repository acquired and judged as a whole core battery.
///
/// Every acquisition is checked for the two invariants no arm may break: the
/// battery covers each compiled declaration in canonical order, and every arm
/// is labelled with the digest of the tree it was actually taken from.
#[allow(dead_code)]
pub(crate) fn acquired(repository: &Tree, claim: ClaimCapture, actor: ActorCapture) -> CoreReport {
    evaluate_core(&acquired_inputs(repository, claim, actor, &[]))
}

/// One fixture repository acquired against a known durable latch inventory.
///
/// The inputs are returned unjudged because the elastic bounds an acquisition
/// decided live on them: judging consumes the inputs to produce arm standings,
/// and a caller reading the decisions from a report would be reading a
/// projection rather than what the arms were held to.
#[allow(dead_code)]
pub(crate) fn acquired_inputs(
    repository: &Tree,
    claim: ClaimCapture,
    actor: ActorCapture,
    latches: &[LatchEntry],
) -> CoreInputs {
    let tree = Selector::staged()
        .materialize(&repository.root)
        .expect("materialize the captured index");
    let contract = Contract::load(tree.root()).expect("load the captured contract");
    let source = CoreSource::capture(&tree, &contract, claim, actor, latches)
        .expect("capture one immutable core source");
    let inputs = acquire_core(&source, &contract).expect("acquire one complete core battery");

    assert_eq!(
        inputs
            .arms()
            .iter()
            .map(CoreArmInput::id)
            .collect::<Vec<_>>(),
        CORE_GATE_IDS,
        "acquisition covers every compiled declaration in canonical order"
    );
    for arm in inputs.arms() {
        assert_eq!(
            arm.tree(),
            source.digest(),
            "{} was acquired from a different tree than the captured source",
            arm.id()
        );
    }
    inputs
}

/// The one arm of an acquired report a test is speaking about.
#[allow(dead_code)]
pub(crate) fn acquired_arm(report: &CoreReport, id: GateId) -> &CoreArmReport {
    report
        .arms()
        .iter()
        .find(|arm| arm.id() == id)
        .unwrap_or_else(|| panic!("{id} is missing from an acquired report"))
}

/// The measured observation one piece of evidence stands for.
#[allow(dead_code)]
pub(crate) fn measured(evidence: CoreEvidence) -> CoreObservation {
    CoreObservation::Measured { evidence }
}
