#![cfg(unix)]

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant as MonotonicInstant};

use spice::install::{apply, installed_binary, installed_git_hooks, plan};
use spice::repository::Repository;
use wire::{CanonicalUuid, Digest, Domain};

mod routing_authority;

const ROUTING: [wire::SpiceVariable; 9] = [
    wire::SpiceVariable::AgentVendor,
    wire::SpiceVariable::Hooks,
    wire::SpiceVariable::Inbox,
    wire::SpiceVariable::Lane,
    wire::SpiceVariable::NativeShell,
    wire::SpiceVariable::PreCommitPaths,
    wire::SpiceVariable::PreCommitSource,
    wire::SpiceVariable::RtkExecutable,
    wire::SpiceVariable::WrapperSpice,
];
const SEMANTIC_INPUT: &str = "SPICE_PROOF"; // env-policy: allow

#[test]
fn lane_bound_hook_reuses_lane_free_proof_without_alternate_planning() {
    let fixture = Fixture::new();
    let fingerprint = fixture.seed_lane_free_manifest();
    let before = fixture.fingerprint_paths();
    assert_eq!(before.as_slice(), std::slice::from_ref(&fingerprint));

    let started = MonotonicInstant::now();
    let routed_lookup = fixture.inspect_with_routing();
    let elapsed = started.elapsed();
    assert!(
        routed_lookup.status.success(),
        "{}",
        streams(&routed_lookup),
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "nine-variable routed lookup took {elapsed:?}: {}",
        streams(&routed_lookup),
    );

    let started = MonotonicInstant::now();
    let admitted = fixture.invoke_lane_bound_peer_hook();
    let elapsed = started.elapsed();
    assert!(admitted.status.success(), "{}", streams(&admitted));
    assert!(
        elapsed < Duration::from_secs(2),
        "lane-bound installed hook took {elapsed:?}: {}",
        streams(&admitted),
    );
    assert_eq!(
        fixture.fingerprint_paths().as_slice(),
        std::slice::from_ref(&fingerprint),
    );
    fixture.assert_active_claim_admitted();
    assert!(
        !fixture.cargo_metadata.exists(),
        "installed admission launched Cargo metadata",
    );
}

#[test]
fn semantic_tool_and_environment_inputs_move_fingerprint_authority() {
    let fixture = Fixture::new();
    fixture.seed_lane_free_manifest();

    let baseline = fixture.inspect_with_override(None);
    assert!(baseline.status.success(), "{}", streams(&baseline));

    let changed_home = fixture.root.join("bootstrap-b");
    fs::create_dir(&changed_home).expect("create changed bootstrap home");
    for (label, name, value) in [
        (
            "native Cargo",
            wire::SpiceVariable::NativeCargo.as_str(),
            PathBuf::from("/bin/false"),
        ),
        (
            "bootstrap Cargo home",
            wire::SpiceVariable::BootstrapCargoHome.as_str(),
            changed_home,
        ),
        (
            "arbitrary semantic input",
            SEMANTIC_INPUT,
            PathBuf::from("beta"),
        ),
    ] {
        let started = MonotonicInstant::now();
        let output = fixture.inspect_with_override(Some((name, value.as_os_str())));
        let elapsed = started.elapsed();
        assert_reason(&output, "gate::prove-fingerprint-missing");
        assert!(
            elapsed < Duration::from_secs(2),
            "{label} lookup took {elapsed:?}: {}",
            streams(&output),
        );
    }
}

struct Fixture {
    root: PathBuf,
    linked: PathBuf,
    peer: PathBuf,
    cargo_metadata: PathBuf,
    hook: PathBuf,
    binary: PathBuf,
    common: PathBuf,
    lane: CanonicalUuid,
    claim: Digest,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-gate-routing-admission-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        let main = root.join("main");
        let linked = root.join("linked");
        let peer = root.join("peer");
        let cargo_metadata = root.join("cargo-metadata-launched");
        fs::create_dir_all(&main).expect("create routing repository");
        git(&main, &["init", "--initial-branch=main"]);
        git(&main, &["config", "user.name", "Spice Routing"]);
        git(
            &main,
            &["config", "user.email", "spice-routing@example.invalid"],
        );
        fs::write(
            main.join("spice-rust.toml"),
            format!("{}[gate.walk]\nskip = []\n", gate::NARROWEST_FILE_CLASSES),
        )
        .expect("write routing gate policy");
        fs::write(main.join("candidate.txt"), "baseline\n").expect("write routing baseline");
        git(&main, &["add", "--all"]);
        git(&main, &["commit", "-m", "routing baseline"]);
        for (branch, worktree) in [("routing-linked", &linked), ("routing-peer", &peer)] {
            git(
                &main,
                &[
                    "worktree",
                    "add",
                    "-b",
                    branch,
                    worktree.to_str().expect("routing worktree is UTF-8"),
                    "main",
                ],
            );
            fs::write(worktree.join("candidate.txt"), "candidate\n")
                .expect("write routing candidate");
            git(worktree, &["add", "--all"]);
        }

        let repository = Repository::discover(&linked).expect("discover routing worktree");
        let state = repository.admit_state().expect("admit routing state");
        fs::create_dir_all(state.data()).expect("create routing journal parent");
        let lane = routing_authority::uuid(1);
        let task = routing_authority::seed_todo_claim(state.journal(), lane);
        let claim = gate::ClaimCapture::Active {
            task,
            phase: "todo".parse().expect("routing claim phase"),
        };
        let claim = Digest::compute(
            Domain::GateRepositoryAuthority,
            &wire::encode(&claim).expect("encode routing claim"),
        );
        let source = Path::new(env!("CARGO_BIN_EXE_spice"));
        let install = plan(&repository, Some(source)).expect("plan routing installation");
        apply(
            &repository,
            Some(source),
            &install.digest().expect("digest routing installation"),
        )
        .expect("install routing hook");
        let hook = installed_git_hooks(&repository).join("pre-commit");
        let binary = installed_binary(&repository);
        let common = repository.common_dir().to_path_buf();
        fs::create_dir(root.join("bootstrap-a")).expect("create baseline bootstrap home");
        Self {
            root,
            linked,
            peer,
            cargo_metadata,
            hook,
            binary,
            common,
            lane,
            claim,
        }
    }

    fn seed_lane_free_manifest(&self) -> PathBuf {
        let before = self.fingerprint_paths();
        let output = self.invoke(
            &self.binary,
            &self.linked,
            &["gate", "prove", "--missing"],
            false,
            false,
            None,
        );
        assert!(output.status.success(), "{}", streams(&output));
        let after = self.fingerprint_paths();
        let created = after
            .iter()
            .filter(|path| !before.contains(path))
            .collect::<Vec<_>>();
        let [fingerprint] = created.as_slice() else {
            panic!("lane-free proof did not create one fingerprint: {after:?}");
        };
        (*fingerprint).clone()
    }

    fn invoke_lane_bound_peer_hook(&self) -> Output {
        self.invoke(&self.hook, &self.peer, &[], true, false, None)
    }

    fn inspect_with_routing(&self) -> Output {
        self.invoke(
            &self.binary,
            &self.linked,
            &["--json", "gate", "prove"],
            true,
            true,
            None,
        )
    }

    fn inspect_with_override(&self, override_value: Option<(&str, &OsStr)>) -> Output {
        self.invoke(
            &self.binary,
            &self.linked,
            &["--json", "gate", "prove"],
            false,
            false,
            override_value,
        )
    }

    fn invoke(
        &self,
        program: &Path,
        worktree: &Path,
        arguments: &[&str],
        routing: bool,
        pre_commit_source: bool,
        override_value: Option<(&str, &OsStr)>,
    ) -> Output {
        let mut command = Command::new(program);
        command
            .args(arguments)
            .current_dir(worktree)
            .env(
                wire::SpiceVariable::BootstrapCargoHome.as_str(),
                self.root.join("bootstrap-a"),
            )
            .env(wire::SpiceVariable::NativeCargo.as_str(), "/bin/true")
            .env(wire::SpiceVariable::NativeShell.as_str(), "/bin/sh")
            .env(wire::SpiceVariable::RtkExecutable.as_str(), "/bin/true")
            .env(SEMANTIC_INPUT, "alpha")
            .env(gate::TEST_CARGO_METADATA_MARKER, &self.cargo_metadata)
            .env_remove(spice::install::INSTALLED_PRE_COMMIT_ENVIRONMENT);
        for variable in ROUTING {
            command.env_remove(variable.as_str());
        }
        if routing {
            command
                .env(wire::SpiceVariable::Hooks.as_str(), self.root.join("hooks"))
                .env(wire::SpiceVariable::Inbox.as_str(), self.root.join("inbox"))
                .env(wire::SpiceVariable::Lane.as_str(), self.lane.to_string())
                .env(
                    wire::SpiceVariable::PreCommitPaths.as_str(),
                    "[\"candidate.txt\"]",
                )
                .env(wire::SpiceVariable::WrapperSpice.as_str(), &self.binary);
        }
        if pre_commit_source {
            command.env(
                wire::SpiceVariable::PreCommitSource.as_str(),
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            );
        }
        if let Some((name, value)) = override_value {
            command.env(name, value);
        }
        command.output().expect("run routing process")
    }

    fn fingerprint_paths(&self) -> Vec<PathBuf> {
        let directory = self.common.join(".spice-rs/gate-evidence/v1/fingerprints");
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(error) => panic!("read {}: {error}", directory.display()),
        };
        let mut paths = entries
            .map(|entry| entry.expect("read fingerprint entry").path())
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }

    fn assert_active_claim_admitted(&self) {
        let admissions = self.common.join(".spice-rs/gate-evidence/v1/admissions");
        let trees = fs::read_dir(&admissions)
            .expect("read admission trees")
            .collect::<Result<Vec<_>, _>>()
            .expect("read admission tree entries");
        let [tree] = trees.as_slice() else {
            panic!("installed hook must publish one candidate tree: {trees:?}");
        };
        let bundles = fs::read_dir(tree.path())
            .expect("read candidate admissions")
            .collect::<Result<Vec<_>, _>>()
            .expect("read candidate admission entries");
        let [bundle] = bundles.as_slice() else {
            panic!("installed hook must publish one admission bundle: {bundles:?}");
        };
        let document: serde_json::Value = serde_json::from_slice(
            &fs::read(bundle.path()).expect("read active-claim admission bundle"),
        )
        .expect("decode active-claim admission bundle");
        assert_eq!(document["claim"], self.claim.as_str());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove routing fixture");
    }
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("run routing Git command");
    assert!(output.status.success(), "Git failed: {}", streams(&output));
}

fn assert_reason(output: &Output, expected: &str) {
    assert!(!output.status.success(), "{}", streams(output));
    let outcome: wire::Outcome<serde_json::Value> =
        wire::decode(&output.stderr).expect("decode routing refusal");
    let wire::Outcome::Refusal(refusal) = outcome else {
        panic!("routing lookup did not refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason(), expected);
}

fn streams(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}
