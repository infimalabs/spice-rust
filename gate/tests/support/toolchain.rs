#![allow(dead_code)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use gate::{
    ArmObservation, ArmOutcome, Contract, GateId, PACKAGED_ENV_PROFILE, PACKAGED_TASTE_WORDS,
    RimCapture, Selector, TOOLCHAIN_ARMS, ToolEvidence, ToolchainSource, acquire_toolchain,
};

use super::{Tree, contract_repository, git_commit, set_environment_names};

/// The version every stub reports when it is asked to be wrong.
pub(crate) const DRIFTED_VERSION: &str = "cargo 0.0.0 (0000000000 1970-01-01)";

/// The program the drift fixture makes wrong; every other stub stays exact.
pub(crate) const DRIFTED_PROGRAM: &str = "cargo";

/// The status every stub exits with when it is asked to refuse its probe.
pub(crate) const REFUSING_STATUS: i32 = 41;

/// The arms this build analyzes itself rather than owing an analyzer.
pub(crate) const COMPILED_ARMS: [GateId; 9] = [
    GateId::Formatters,
    GateId::LocalPaths,
    GateId::Taste,
    GateId::JavascriptUnused,
    GateId::CsharpUnusedCandidates,
    GateId::RustTypecheck,
    GateId::CanvasWebTypecheck,
    GateId::EnvPolicy,
    GateId::EnvNameLedger,
];

/// The compiled scanners applicable to every shared source fixture.
///
/// The environment arms are compiled too, and neither a Rust source nor a clean
/// JavaScript module names an environment, so the shared fixture leaves them
/// nothing to find and they are held to that in their own fixtures rather than
/// here.
pub(crate) const SCANNING_ARMS: [GateId; 4] = [
    GateId::Formatters,
    GateId::LocalPaths,
    GateId::Taste,
    GateId::JavascriptUnused,
];

/// The three compiled scanners the generic dirty fixture intentionally violates.
pub(crate) const GENERIC_FINDING_ARMS: [GateId; 3] =
    [GateId::Formatters, GateId::LocalPaths, GateId::Taste];

/// The prose file both scanning fixtures carry, clean in one and dirty in the other.
pub(crate) const PROSE: &str = "docs/guide.md";

/// The Rust file the formatter reads and the marker scanner searches.
pub(crate) const SOURCE: &str = "src/main.rs";

/// The Python file carrying one waived access and three unwaived lines.
pub(crate) const SETTINGS: &str = "app/settings.py";

/// The second Python file reading the same environment name.
pub(crate) const WORKER: &str = "app/worker.py";

/// The environment name only the waived access reads.
///
/// Both fixture names are drawn from the names this repository already declares
/// in `[policy] env_names`. A fixture literal is a real reference as far as this
/// repository's own environment ledger is concerned, so an invented name here
/// would demand a manifest entry for a name nothing reads.
pub(crate) const WAIVED_NAME: &str = "SPICE_HOOKS"; // env-policy: allow

/// The environment name both fixture files read without a waiver.
pub(crate) const SHARED_NAME: &str = "SPICE_LANE"; // env-policy: allow

/// The exact unwaived read both fixture files spell.
pub(crate) const READ: &str = "cache = os.getenv(\"SPICE_LANE\")";

/// One line reading a watchlisted name beside a name nobody wrote down.
///
/// Neither extractor sees this line whole: the watchlist names the first read
/// and the access gate is the only thing that catches the second.
pub(crate) const MIXED: &str = "flag = os.getenv(\"SPICE_LANE\") or os.environ[key]";

/// One line committing two reads under names nobody wrote down.
///
/// Both accesses report under the same family idiom on the same line of the
/// same file, so the column is the only thing that keeps the second from
/// arriving as a duplicate of the first and being refused.
pub(crate) const DYNAMIC: &str = "both = os.environ[first] or os.getenv(second)";

/// The clean JavaScript module that makes the complete-tree analysis applicable.
pub(crate) const JAVASCRIPT: &str = "web/app.js";

/// The clean C# unit that makes the complete-tree analysis applicable.
pub(crate) const CSHARP: &str = "dotnet/App.cs";

/// One C# type whose sole member is reached from inside the same unit.
///
/// The declaration alone is one occurrence, so a member the fixture never calls
/// would report as a candidate. Calling it once is what makes this tree clean
/// without exempting anything.
pub(crate) const CSHARP_SOURCE: &str = concat!(
    "public class App\n{\n",
    "    public void Run()\n    {\n        Step();\n    }\n\n",
    "    public void Step()\n    {\n    }\n}\n"
);

/// Acquire one battery over a committed fixture tree and a captured search path.
///
/// Every invariant that holds for all three fixtures is asserted here once, so
/// each test states only what makes it different.
pub(crate) fn acquired(
    repository: &Tree,
    search_path: Option<OsString>,
    changes: &[&str],
) -> Vec<ArmObservation> {
    let tree = Selector::staged()
        .materialize(&repository.root)
        .expect("materialize the captured index");
    let contract = Contract::load(tree.root()).expect("load the captured contract");
    let selected: Vec<String> = changes.iter().map(|path| (*path).to_owned()).collect();
    let source = ToolchainSource::capture(
        &tree,
        RimCapture::new(&repository.root, search_path, selected.clone())
            .expect("the fixture repository root is absolute"),
    );
    assert_eq!(
        source.root(),
        tree.root(),
        "acquisition observes the captured tree and not the live repository"
    );
    assert_eq!(
        source.rim().directory(),
        repository.root,
        "acquisition anchors the search path where the rim captured it"
    );
    assert_eq!(
        source.changes(),
        selected,
        "acquisition scans the changes the rim captured and no others"
    );

    let observations = acquire_toolchain(&source, &contract);
    assert_eq!(
        observations
            .iter()
            .map(|observation| observation.gate_id)
            .collect::<Vec<_>>(),
        TOOLCHAIN_ARMS
            .into_iter()
            .map(|declaration| declaration.id())
            .collect::<Vec<_>>(),
        "one observation per declared arm, in declaration order, none invented"
    );

    for (declaration, observation) in TOOLCHAIN_ARMS.into_iter().zip(&observations) {
        assert_ne!(
            observation.outcome,
            ArmOutcome::Excluded,
            "{} answered an exclusion acquisition never measured",
            declaration.id().as_str()
        );
        if COMPILED_ARMS.contains(&declaration.id()) {
            assert_ne!(
                observation.outcome,
                ArmOutcome::Unsupported,
                "{} has a compiled analysis and still called itself unanalyzed",
                declaration.id().as_str()
            );
        } else {
            assert!(
                matches!(
                    observation.outcome,
                    ArmOutcome::Unsupported
                        | ArmOutcome::ToolUnavailable
                        | ArmOutcome::UnsupportedVersion
                        | ArmOutcome::NotApplicable
                ),
                "{} has no compiled analysis and answered {:?} anyway",
                declaration.id().as_str(),
                observation.outcome
            );
            assert!(
                observation.findings.is_empty(),
                "{} carried findings no compiled analysis produced",
                declaration.id().as_str()
            );
        }
        assert!(
            !observation.reason.is_empty(),
            "{} reported no cause",
            declaration.id().as_str()
        );
        let expected_tools =
            usize::from(!observation.outcome.is_skip()) * declaration.tools().len();
        assert_eq!(
            observation.tools.len(),
            expected_tools,
            "{} dropped or invented an evidence row",
            declaration.id().as_str()
        );
        for (requirement, evidence) in declaration.tools().iter().zip(&observation.tools) {
            let name = match evidence {
                ToolEvidence::Missing { name, .. }
                | ToolEvidence::Unprobed { name, .. }
                | ToolEvidence::Identified { name, .. } => name,
            };
            assert_eq!(
                name,
                requirement.name(),
                "{} carried its evidence out of declaration order",
                declaration.id().as_str()
            );
        }
    }
    observations
}

/// One committed fixture repository Cargo can read and the scanners can scan.
///
/// Formatters runs the declared `cargo fmt --all -- --check` for real, so the
/// captured tree has to be a package Cargo will actually open.
pub(crate) fn scanning_repository(label: &str, source: &str, prose: &str) -> Tree {
    let repository = contract_repository(label);
    set_environment_names(&repository, &[]);
    repository.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n",
    );
    repository.write(SOURCE, source);
    repository.write(PROSE, prose);
    repository.write(JAVASCRIPT, "void 0;\n");
    repository.write(CSHARP, CSHARP_SOURCE);
    git_commit(&repository, "fixture sources");
    repository
}

/// One committed fixture repository whose Python names the environment.
///
/// One access carries the waiver and one does not, and one name is read in two
/// files, so each environment arm has exactly one thing to say about it.
pub(crate) fn environment_repository(label: &str) -> Tree {
    let repository = contract_repository(label);
    set_environment_names(&repository, &[WAIVED_NAME, SHARED_NAME]);
    repository.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n",
    );
    repository.write("src/lib.rs", "pub fn fixture() {}\n");
    let marker = PACKAGED_ENV_PROFILE.allow_marker;
    let waived = format!("home = os.getenv(\"{WAIVED_NAME}\")  # {marker}");
    repository.write(
        SETTINGS,
        &format!("import os\n\n{waived}\n{READ}\n{MIXED}\n{DYNAMIC}\n"),
    );
    repository.write(WORKER, &format!("import os\n\n{READ}\n"));
    git_commit(&repository, "fixture environment sources");
    repository
}

/// The stable finding name the profile gives a dynamic Python access.
///
/// Read rather than spelled, so the fixture cannot drift from the compiled
/// profile the arm actually reports under.
pub(crate) fn python_idiom() -> String {
    PACKAGED_ENV_PROFILE
        .finding_names
        .iter()
        .find(|(family, _)| *family == "python")
        .map(|(_, name)| (*name).to_owned())
        .expect("the packaged profile names the python access family")
}

/// The one observation carrying this identity.
pub(crate) fn arm(observations: &[ArmObservation], id: GateId) -> &ArmObservation {
    observations
        .iter()
        .find(|observation| observation.gate_id == id)
        .unwrap_or_else(|| panic!("{} was not observed", id.as_str()))
}

/// The first discouraged word and its replacement, read from the vocabulary.
///
/// Read rather than spelled so the fixture cannot drift from the compiled
/// vocabulary, and so this file never has to contain a discouraged word.
pub(crate) fn discouraged_word() -> (String, String) {
    let (pattern, replacement) = PACKAGED_TASTE_WORDS
        .first()
        .expect("the compiled vocabulary is not empty");
    (
        pattern.trim_end_matches('*').to_owned(),
        (*replacement).to_owned(),
    )
}

/// The directory holding the Cargo that built this test.
pub(crate) fn toolchain_directory() -> PathBuf {
    Path::new(env!("CARGO"))
        .parent()
        .expect("the building Cargo sits in a directory")
        .to_owned()
}

/// One directory supplying every declared program, with exactly one wrong.
pub(crate) fn stub_directory(label: &str) -> Tree {
    let stubs = Tree::new(label);
    let mut declared = BTreeMap::new();
    for declaration in TOOLCHAIN_ARMS {
        for requirement in declaration.tools() {
            declared.insert(requirement.program(), requirement.exact_version());
        }
    }
    assert!(
        declared.contains_key(DRIFTED_PROGRAM),
        "the drift fixture names a program no declaration requires"
    );
    declared.insert(DRIFTED_PROGRAM, DRIFTED_VERSION);

    for (program, version) in declared {
        assert!(
            !version.contains('\''),
            "{program} declares a version this stub cannot quote"
        );
        stubs.write(program, &format!("#!/bin/sh\nprintf '%s\\n' '{version}'\n"));
        let path = stubs.path(program);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("make {} executable: {error}", path.display()));
    }
    stubs
}

/// One directory supplying every declared program, each refusing its probe.
///
/// The programs are present and executable, so nothing here is missing. They
/// simply never answer the question their declaration asks them.
pub(crate) fn refusing_stub_directory(label: &str) -> Tree {
    let stubs = Tree::new(label);
    for declaration in TOOLCHAIN_ARMS {
        for requirement in declaration.tools() {
            stubs.write(
                requirement.program(),
                &format!("#!/bin/sh\nexit {REFUSING_STATUS}\n"),
            );
            let path = stubs.path(requirement.program());
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .unwrap_or_else(|error| panic!("make {} executable: {error}", path.display()));
        }
    }
    stubs
}

/// Exact-version stubs whose cargo analysis refuses before trunk can run.
pub(crate) fn canvas_nonzero_directory(label: &str) -> Tree {
    let stubs = Tree::new(label);
    let metadata = serde_json::json!({
        "packages": [{
            "id": "fixture-id",
            "name": "fixture",
            "manifest_path": "$PWD/Cargo.toml",
            "targets": [{
                "name": "fixture",
                "kind": ["bin"],
                "src_path": "$PWD/src/main.rs",
                "test": true,
            }],
        }],
        "workspace_members": ["fixture-id"],
    })
    .to_string()
    .replace('"', "\\\"");
    let mut declared = BTreeMap::new();
    for declaration in TOOLCHAIN_ARMS {
        for requirement in declaration.tools() {
            declared.insert(requirement.program(), requirement.exact_version());
        }
    }

    for (program, version) in declared {
        assert!(
            !version.contains('\''),
            "{program} version must be shell safe"
        );
        let body = match program {
            "cargo" => format!(
                concat!(
                    "#!/bin/sh\n",
                    "if [ \"$1\" = \"--version\" ]; then printf '%s\\n' '{version}'; exit 0; fi\n",
                    "if [ \"$1\" = \"fmt\" ]; then exit 0; fi\n",
                    "if [ \"$1\" = \"metadata\" ]; then printf '%s\\n' \"{metadata}\"; exit 0; fi\n",
                    "if [ \"$1\" = \"clippy\" ]; then\n",
                    "  printf 'compiling fixture\\r\\n'\n",
                    "  printf 'error: lint configuration rejected\\n  invalid lint level\\n' >&2\n",
                    "  exit 17\n",
                    "fi\n",
                    "exit 91\n",
                ),
                version = version,
                metadata = metadata,
            ),
            "trunk" => {
                let marker = stubs.path("trunk-analysis-ran");
                assert!(
                    !marker.to_string_lossy().contains('\''),
                    "trunk marker path must be shell safe"
                );
                format!(
                    concat!(
                        "#!/bin/sh\n",
                        "if [ \"$1\" = \"--version\" ]; then ",
                        "printf '%s\\n' '{version}'; exit 0; fi\n",
                        ": > '{marker}'\n",
                        "exit 99\n",
                    ),
                    version = version,
                    marker = marker.display(),
                )
            }
            _ => format!("#!/bin/sh\nprintf '%s\\n' '{version}'\n"),
        };
        stubs.write(program, &body);
        let path = stubs.path(program);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("make {} executable: {error}", path.display()));
    }
    stubs
}

/// Ask one resolved program for its version without going through the gate.
///
/// Nothing is trimmed, because the point of the comparison this feeds is that
/// acquisition retained the bytes rather than a cleaned-up rendering of them.
pub(crate) fn probe_directly(program: &Path, arguments: &[&str]) -> String {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .unwrap_or_else(|error| panic!("probe {}: {error}", program.display()));
    assert!(
        output.status.success(),
        "{} refused its version probe",
        program.display()
    );
    String::from_utf8(output.stdout).expect("version probes print UTF-8")
}
