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

/// The Rust file no module path reaches, named only by a macro in [`SOURCE`].
pub(crate) const INCLUDED: &str = "src/included.rs";

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

/// One committed repository whose only unformatted file is reached by a macro.
///
/// [`SOURCE`] is canonical and reaches [`INCLUDED`] through `include!`, which is
/// exactly the shape the command this arm replaced calls clean: rustfmt opens a
/// file by walking `mod` declarations down from the crate root, and a file named
/// by a macro sits on no module path, so `cargo fmt --all` never reads it.
///
/// The included content is the caller's, because the two things this shape can
/// do to the arm -- give it a file to name, and give it a file it cannot read
/// -- are different reports and each is asserted on its own.
pub(crate) fn including_repository(label: &str, included: &str) -> Tree {
    let repository = contract_repository(label);
    set_environment_names(&repository, &[]);
    repository.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n",
    );
    repository.write(SOURCE, "include!(\"included.rs\");\n\nfn main() {}\n");
    repository.write(INCLUDED, included);
    git_commit(&repository, "fixture included sources");
    repository
}

/// An included file that is readable Rust and is not formatted like rustfmt.
///
/// The file opens on `async fn` on purpose: a file `cargo fmt` never opened
/// reaches rustfmt with no package behind it and must be told which edition to
/// parse under. `async fn` is a parse error before Rust 2018, so a pass that
/// named the wrong edition, or none, refuses this file outright rather than
/// quietly reporting a difference that happened not to depend on the edition at
/// all. The spacing that makes it uncanonical is spread across both lines,
/// because the arm reports the file and no locus within it.
pub(crate) const UNFORMATTED_INCLUDED: &str = "async fn included( ) {\n    let  reached=1;\n}\n";

/// An included file no edition can parse, so the formatter names nothing.
///
/// The arm is then left with an empty finding list and a nonzero exit, which is
/// the shape a canonical tree also has minus the exit code. Reporting clean
/// here is the failure mode the formatter gap was made of, so the refusal is
/// what has to survive rather than the count.
pub(crate) const UNPARSABLE_INCLUDED: &str = "fn included( {\n";

/// What `cargo fmt --all -- --check` on its own says about one fixture tree.
///
/// Run for real rather than inferred, because the whole claim under test is
/// that this exact command reports nothing about a file the arm reports. An
/// assertion that read only the arm could not tell a formatter gap from a
/// fixture that was never unformatted in the first place.
///
/// Callers read this to establish an absence, so silence has to mean the
/// command looked and found nothing rather than that it never ran. Only one
/// nonzero exit is legitimate here -- `--check` reports a diff that way, and it
/// prints that diff on stdout first. A nonzero carrying no stdout at all is a
/// pass that did not happen, and handing it back would let a control assert the
/// absence of a report the command was never in a position to make.
pub(crate) fn workspace_formatter_output(root: &Path) -> String {
    let output = Command::new(toolchain_directory().join("cargo"))
        .args(["fmt", "--all", "--", "--check"])
        .current_dir(root)
        .output()
        .expect("the declared workspace formatter pass runs over the fixture");
    let rendered = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success() || !rendered.is_empty(),
        "the workspace formatter pass reported nothing because it did not run \
         over the fixture: {}, stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    rendered
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
                    "if [ \"$1\" = \"clippy\" ]; then printf 'opaque refusal\\n' >&2; exit 17; fi\n",
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
