#![cfg(unix)]

use std::env;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use gate::{GateId, Selector, VerifierCompilerEvidence, VerifierCompilerProcessEvidence};
use spice::gate::{
    FullVerifierEvidence, SemanticFullVerifierAdmission, admit_full_verifier_evidence,
    admit_semantic_full_verifier_evidence, prepare_strict,
};
use spice::repository::Repository;

#[test]
fn full_verifier_semantic_reuse_crosses_the_complete_evidence_lifecycle() {
    let fixture = Fixture::new();
    fixture.write_repository();
    fixture.write_tools();

    let output = Command::new(env::current_exe().expect("resolve integration-test executable"))
        .args([
            "--ignored",
            "--exact",
            "semantic_lifecycle_fixture_process",
            "--nocapture",
        ])
        .current_dir(&fixture.repository)
        .env("PATH", fixture.search_path())
        .output()
        .expect("launch isolated semantic lifecycle fixture");

    assert!(
        output.status.success(),
        "semantic lifecycle fixture failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore = "subprocess fixture with isolated cwd and compiler environment"]
fn semantic_lifecycle_fixture_process() {
    let root = env::current_dir().expect("fixture current directory");
    let repository = Repository::discover(&root).expect("discover fixture repository");
    let evidence = publish_producer_evidence(&repository);
    assert_pre_commit_reuses_complete_evidence(&root);

    reject_cross_worktree_reuse(&root, &evidence);
    reject_compiler_contract_drift(&repository);

    fs::write(root.join("app/schema.json"), "{\"version\":2}\n")
        .expect("move tracked build-script input");
    git(&root, &["add", "app/schema.json"]);
    git(&root, &["commit", "-qm", "move compiler input"]);
    let app = fs::read_to_string(root.join("app/browser/lifecycle.mjs"))
        .expect("read frontend current-source input");
    fs::write(
        root.join("app/browser/lifecycle.mjs"),
        format!("// a syntax-neutral current-source change\n{app}"),
    )
    .expect("write current-source change");
    git(&root, &["add", "app/browser/lifecycle.mjs"]);

    let admitted = match admit_semantic_full_verifier_evidence(&repository, Selector::staged())
        .expect("compare the later source commit")
    {
        SemanticFullVerifierAdmission::Admitted(admitted) => admitted,
        SemanticFullVerifierAdmission::Unavailable { detail } => {
            panic!("later source did not admit semantic evidence: {detail}")
        }
    };
    assert!(
        admitted
            .invalidated_gate_ids()
            .contains(&GateId::RustTypecheck),
    );
    assert!(
        admitted
            .invalidated_gate_ids()
            .contains(&GateId::CanvasWebTypecheck),
    );
    assert!(admitted.reusable_gate_ids().contains(&GateId::Complexity));
    assert!(!admitted.reusable_gate_ids().is_empty());
    assert!(!admitted.invalidated_gate_ids().is_empty());
    assert!(
        admitted
            .diagnostics()
            .iter()
            .any(|detail| detail.contains("Cargo/rustc-discovered dependency evidence")),
    );

    let cargo_log = fixture_peer(&root, "cargo.log");
    let trunk_log = fixture_peer(&root, "trunk.log");
    fs::write(&cargo_log, "").expect("clear Cargo execution log");
    fs::write(&trunk_log, "").expect("clear Trunk execution log");
    let mut progress = Vec::new();
    let report = spice::change_gate::run_current_source(&mut progress)
        .expect("run the production current-source change gate");
    assert!(report.admits(), "current-source execution must admit");
    let progress = String::from_utf8(progress).expect("progress is UTF-8");
    assert!(
        progress.contains("gate=complexity disposition=reused"),
        "reusable arm did not cross the production plan:\n{progress}",
    );
    assert!(
        progress.contains("gate=canvas-web-typecheck disposition=executed"),
        "invalidated Canvas arm did not execute:\n{progress}",
    );
    let cargo = fs::read_to_string(cargo_log).expect("read Cargo execution log");
    assert_eq!(
        cargo
            .lines()
            .filter(|line| line.contains("check --locked --package app"))
            .count(),
        1,
        "Canvas current-source compiler must execute exactly once:\n{cargo}",
    );
    let trunk = fs::read_to_string(trunk_log).expect("read Trunk execution log");
    assert_eq!(
        trunk
            .lines()
            .filter(|line| line.contains("build index.html --release --locked --filehash=false"))
            .count(),
        1,
        "Canvas current-source build must execute exactly once:\n{trunk}",
    );
}

fn assert_pre_commit_reuses_complete_evidence(root: &Path) {
    let progress = fixture_peer(root, "pre-commit-progress.jsonl");
    let terminal = fixture_peer(root, "pre-commit-terminal.bin");
    fs::write(&progress, "").expect("create pre-commit progress fixture");
    fs::write(&terminal, "").expect("create pre-commit terminal fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args([
            "__gate-pre-commit-worker",
            progress.to_str().expect("progress path is text"),
            terminal.to_str().expect("terminal path is text"),
        ])
        .current_dir(root)
        .output()
        .expect("launch exact pre-commit worker");
    assert!(
        output.status.success(),
        "pre-commit worker failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let terminal = fs::read(&terminal).expect("read pre-commit terminal fixture");
    let Some((kind, document)) = terminal.split_first() else {
        panic!("pre-commit worker published no terminal");
    };
    assert_eq!(*kind, b'R', "pre-commit worker refused complete evidence");
    let report: serde_json::Value =
        serde_json::from_slice(document).expect("decode pre-commit report");
    assert!(
        report["full_verifier"].is_object(),
        "pre-commit worker fell through instead of reusing complete evidence: {report}",
    );
    assert!(
        report.get("packaged").is_none(),
        "complete evidence must replace packaged execution: {report}",
    );
}

fn publish_producer_evidence(repository: &Repository) -> FullVerifierEvidence {
    let mut prepared = prepare_strict(Selector::staged()).expect("prepare producer source");
    let compiler = compiler_evidence(&mut prepared);
    let battery = prepared
        .execute_prepared_compiler_receipt(&compiler, 1)
        .expect("execute producer battery from exact compiler evidence");
    assert!(
        battery.run().admits(),
        "producer battery must be green: {:?}",
        battery
            .run()
            .results()
            .iter()
            .filter(|result| !result.state().admits())
            .map(|result| {
                (
                    result.gate_id(),
                    result.state(),
                    result.reason(),
                    result.findings(),
                )
            })
            .collect::<Vec<_>>(),
    );
    let evidence = FullVerifierEvidence::mint(repository, 1, &battery, &compiler, &prepared)
        .expect("mint complete full-verifier evidence");
    let encoded = evidence.encode().expect("serialize complete evidence");
    assert!(!encoded.is_empty());
    evidence
        .publish(repository)
        .expect("publish producer evidence");
    assert!(
        admit_full_verifier_evidence(repository, Selector::staged())
            .expect("read exact producer evidence")
            .is_some(),
        "the complete bundle must re-admit against its exact source",
    );
    evidence
}

fn compiler_evidence(
    prepared: &mut spice::gate::PreparedBattery<'_>,
) -> Vec<VerifierCompilerEvidence> {
    let contracts = [GateId::RustTypecheck, GateId::CanvasWebTypecheck]
        .into_iter()
        .map(|gate_id| {
            prepared
                .verifier_compiler_contract(gate_id, 1)
                .expect("capture compiler contract")
        })
        .collect::<Vec<_>>();
    let mut processes = Vec::<VerifierCompilerProcessEvidence>::new();
    for contract in &contracts {
        for stage in contract.stages() {
            let run = contract.run(*stage).expect("run compiler evidence stage");
            assert!(
                run.success(),
                "compiler evidence stage failed: {}",
                String::from_utf8_lossy(run.stderr()),
            );
            processes.push(run.into_evidence().expect("successful process evidence"));
        }
    }
    contracts
        .iter()
        .map(|contract| {
            contract
                .complete(&processes)
                .expect("complete compiler evidence")
        })
        .collect()
}

fn reject_cross_worktree_reuse(root: &Path, evidence: &FullVerifierEvidence) {
    let linked = fixture_peer(root, "linked");
    git(
        root,
        &[
            "worktree",
            "add",
            "-q",
            linked.to_str().expect("linked path is text"),
            "HEAD",
        ],
    );
    let repository = Repository::discover(&linked).expect("discover linked worktree");
    evidence
        .publish(&repository)
        .expect("copy exact bundle to linked worktree state");
    env::set_current_dir(&linked).expect("enter linked worktree");
    let admission = admit_semantic_full_verifier_evidence(&repository, Selector::staged())
        .expect("probe linked worktree authority");
    env::set_current_dir(root).expect("return to producer worktree");
    assert!(
        matches!(admission, SemanticFullVerifierAdmission::Unavailable { .. }),
        "compiler evidence must not cross worktree authority",
    );
}

fn reject_compiler_contract_drift(repository: &Repository) {
    let cargo = fixture_peer(repository.worktree_root(), "tools").join("cargo");
    let original = fs::read(&cargo).expect("read fixture Cargo wrapper");
    let mut drifted = original.clone();
    drifted.extend_from_slice(b"\n# compiler contract drift\n");
    fs::write(&cargo, drifted).expect("move compiler executable bytes");
    assert!(
        admit_full_verifier_evidence(repository, Selector::staged())
            .expect("probe drifted compiler contract")
            .is_none(),
        "exact evidence must not cross compiler executable drift",
    );
    fs::write(&cargo, original).expect("restore compiler executable bytes");
    let mut permissions = fs::metadata(&cargo)
        .expect("read restored Cargo wrapper")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&cargo, permissions).expect("restore Cargo executable mode");
}

fn fixture_peer(root: &Path, name: &str) -> PathBuf {
    root.parent()
        .expect("fixture repository has a parent")
        .join(name)
}

struct Fixture {
    base: PathBuf,
    repository: PathBuf,
    tools: PathBuf,
    cargo_log: PathBuf,
    trunk_log: PathBuf,
    real_cargo: PathBuf,
    original_path: OsString,
}

impl Fixture {
    #[expect(
        clippy::disallowed_methods,
        reason = "the integration-test process rim captures PATH once for its isolated child"
    )]
    fn new() -> Self {
        let base = env::temp_dir().join(format!(
            "spice-semantic-full-verifier-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        let repository = base.join("repository");
        let tools = base.join("tools");
        fs::create_dir_all(repository.join("app/src"))
            .expect("create app fixture source directory");
        fs::create_dir_all(&tools).expect("create fixture tool directory");
        let original_path = env::var_os("PATH").expect("test process PATH");
        let real_cargo = find_program("cargo", &original_path).expect("find real Cargo");
        Self {
            cargo_log: base.join("cargo.log"),
            trunk_log: base.join("trunk.log"),
            base,
            repository,
            tools,
            real_cargo,
            original_path,
        }
    }

    fn write_repository(&self) {
        git(&self.repository, &["init", "-q"]);
        git(
            &self.repository,
            &["config", "user.email", "semantic@example.invalid"],
        );
        git(
            &self.repository,
            &["config", "user.name", "Semantic Fixture"],
        );
        git(&self.repository, &["config", "commit.gpgSign", "false"]);
        fs::write(
            self.repository.join("spice-rust.toml"),
            concat!(
                "[policy]\n",
                "env_names = []\n",
                "generated_paths = []\n",
                "package_roots = [\"app\"]\n",
                "test_engines = [\"cargo-workspace\"]\n",
                "test_paths = []\n",
                "\n[policy.repo_truth]\n",
                "docs = [\"AGENTS.md\"]\n",
                "\n[policy.file_shape]\n",
                "source_suffixes = [\".html\", \".json\", \".md\", \".rs\", \".toml\"]\n",
                "\n[policy.languages]\n",
                "complexity = [\".rs\"]\n",
                "magic = [\".rs\"]\n",
            ),
        )
        .expect("write strict tracked policy");
        fs::write(
            self.repository.join("baseline.toml"),
            "version = 1\n\n[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n",
        )
        .expect("write strict tracked baseline");
        fs::write(self.repository.join("AGENTS.md"), "# Fixture\n")
            .expect("write declared repository document");
        fs::write(
            self.repository.join("Cargo.toml"),
            concat!(
                "[workspace]\n",
                "members = [\"app\"]\n",
                "resolver = \"3\"\n",
                "\n[workspace.package]\nedition = \"2024\"\n",
            ),
        )
        .expect("write workspace manifest");
        fs::write(
            self.repository.join("app/Cargo.toml"),
            concat!(
                "[package]\nname = \"app\"\nversion = \"0.0.0\"\nedition.workspace = true\n",
                "build = \"build.rs\"\n",
            ),
        )
        .expect("write app package manifest");
        fs::write(
            self.repository.join("Cargo.lock"),
            concat!(
                "# This file is automatically @generated by Cargo.\n",
                "# It is not intended for manual editing.\n",
                "version = 4\n\n",
                "[[package]]\nname = \"app\"\nversion = \"0.0.0\"\n",
            ),
        )
        .expect("write Cargo lockfile");
        fs::write(
            self.repository.join("app/build.rs"),
            concat!(
                "fn main() {\n",
                "    let _ = std::fs::read_to_string(\"schema.json\").unwrap();\n",
                "    println!(\"cargo:rerun-if-changed=schema.json\");\n",
                "}\n",
            ),
        )
        .expect("write build script");
        fs::write(self.repository.join("app/schema.json"), "{\"version\":1}\n")
            .expect("write build-script input");
        fs::write(
            self.repository.join("app/src/lib.rs"),
            "pub fn app_fixture() {}\n",
        )
        .expect("write app Rust source");
        fs::create_dir_all(self.repository.join("app/browser"))
            .expect("create frontend fixture directory");
        fs::write(
            self.repository.join("app/browser/lifecycle.mjs"),
            "export const lifecycle = \"fixture\";\nvoid lifecycle;\n",
        )
        .expect("write frontend source");
        fs::write(
            self.repository.join("app/index.html"),
            "<!doctype html><title>fixture</title>\n",
        )
        .expect("write Canvas entry point");
        git(&self.repository, &["add", "."]);
        git(&self.repository, &["commit", "-qm", "fixture baseline"]);
    }

    fn write_tools(&self) {
        write_executable(
            &self.tools.join("cargo"),
            &format!(
                concat!(
                    "#!/bin/sh\n",
                    "printf '%s\\n' \"$*\" >> \"{}\"\n",
                    "if [ \"$1\" = \"--version\" ]; then\n",
                    "  printf '%s\\n' 'cargo 1.97.1 (c980f4866 2026-06-30)'\n",
                    "  exit 0\n",
                    "fi\n",
                    "case \"$1\" in\n",
                    "  metadata|test) exec \"{}\" \"$@\" ;;\n",
                    "  *) exit 0 ;;\n",
                    "esac\n",
                ),
                self.cargo_log.display(),
                self.real_cargo.display(),
            ),
        );
        write_executable(
            &self.tools.join("trunk"),
            &format!(
                concat!(
                    "#!/bin/sh\n",
                    "printf '%s\\n' \"$*\" >> \"{}\"\n",
                    "if [ \"$1\" = \"--version\" ]; then\n",
                    "  printf '%s\\n' 'trunk 0.21.14'\n",
                    "fi\n",
                    "exit 0\n",
                ),
                self.trunk_log.display(),
            ),
        );
        fs::write(&self.cargo_log, "").expect("create Cargo log");
        fs::write(&self.trunk_log, "").expect("create Trunk log");
    }

    fn search_path(&self) -> OsString {
        env::join_paths(
            std::iter::once(self.tools.clone()).chain(env::split_paths(&self.original_path)),
        )
        .expect("compose fixture PATH")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.base).expect("remove semantic lifecycle fixture");
    }
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write fixture executable");
    let mut permissions = fs::metadata(path)
        .expect("read fixture executable")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("make fixture executable");
}

fn find_program(name: &str, search_path: &OsString) -> Option<PathBuf> {
    env::split_paths(search_path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("fixture Git launches");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}
