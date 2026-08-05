#![cfg(all(
    unix,
    any(
        all(target_arch = "aarch64", target_vendor = "apple", target_os = "macos"),
        all(target_arch = "x86_64", target_os = "linux", target_env = "musl")
    )
))]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use spice::placeholder::PlaceholderReason;
use wire::manifest::{BuildPins, InventoryEntry, ProofReceipt, ReleaseTarget, TargetPayload};
use wire::{Digest, Domain, Fault, Outcome, ProofEvidence, Refusal, ReplyPosition};

const REFUSAL_EXIT: i32 = 2;
const FAULT_EXIT: i32 = 1;

#[test]
fn both_forms_and_leaves_return_the_same_direct_evidence_without_daemon_state() {
    let fixture = Fixture::new();
    let before_seal = snapshot(fixture.repository());

    let sealed_text = text_reply(&fixture.invoke(&["proof"]));
    let after_text_seal = snapshot(fixture.repository());
    let sealed_json = json_reply(&fixture.invoke(&["proof", "--json"]));
    assert_eq!(sealed_text, sealed_json);
    assert_eq!(sealed_json.target, fixture.target);
    assert_eq!(snapshot(fixture.repository()), after_text_seal);

    let receipt_path = fixture.receipt_path();
    let receipt_bytes = fs::read(&receipt_path).expect("sealed receipt exists");
    let receipt: ProofReceipt = wire::decode(&receipt_bytes).expect("sealed receipt decodes");
    assert_eq!(
        wire::encode(&receipt).expect("sealed receipt re-encodes"),
        receipt_bytes,
        "seal publishes canonical receipt bytes"
    );
    assert_eq!(
        Digest::compute(Domain::ProofReceiptV3, &receipt_bytes),
        sealed_json.receipt
    );
    assert_eq!(receipt.target, fixture.target);

    let after_seal = after_text_seal;
    let added = after_seal
        .keys()
        .filter(|path| !before_seal.contains_key(*path))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(added, [fixture.relative_receipt_path()]);
    assert!(
        after_seal
            .keys()
            .all(|path| !path.to_string_lossy().contains(".spice-atomic-")),
        "atomic seal leaves no staged publication file"
    );

    let replayed_text = text_reply(&fixture.invoke(&["proof", "replay"]));
    let replayed_json = json_reply(&fixture.invoke(&["proof", "replay", "--json"]));
    assert_eq!(replayed_text, replayed_json);
    assert_eq!(replayed_json, sealed_json);
    assert_eq!(snapshot(fixture.repository()), after_seal);
    assert_eq!(
        fixture.battery_invocations(),
        4 * proof::CheckKind::ALL.len()
    );
    fixture.assert_daemonless();
}

#[test]
fn missing_corrupt_and_absent_receipt_inputs_refuse_before_runtime_execution() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.payload_path()).expect("remove payload");
    let missing = snapshot(fixture.repository());
    for command in [&["proof"][..], &["proof", "replay"][..]] {
        let refusal = refusal_pair(&fixture, command, "proof::payload-refused");
        assert!(
            refusal
                .observed
                .as_deref()
                .is_some_and(|observed| observed.contains("is missing"))
        );
    }
    assert_eq!(snapshot(fixture.repository()), missing);

    fs::write(fixture.payload_path(), b"{}\n").expect("write corrupt payload");
    let corrupt = snapshot(fixture.repository());
    for command in [&["proof"][..], &["proof", "replay"][..]] {
        let refusal = refusal_pair(&fixture, command, "proof::payload-refused");
        assert!(
            refusal
                .observed
                .as_deref()
                .is_some_and(|observed| observed.contains("does not decode"))
        );
    }
    assert_eq!(snapshot(fixture.repository()), corrupt);

    let wrong_target = match fixture.target {
        ReleaseTarget::Aarch64AppleDarwin => ReleaseTarget::X86_64UnknownLinuxMusl,
        ReleaseTarget::X86_64UnknownLinuxMusl => ReleaseTarget::Aarch64AppleDarwin,
    };
    fs::write(
        fixture.payload_path(),
        wire::encode(&payload(wrong_target)).expect("wrong-target payload encodes"),
    )
    .expect("write wrong-target payload");
    let wrong_layout = snapshot(fixture.repository());
    for command in [&["proof"][..], &["proof", "replay"][..]] {
        refusal_pair(&fixture, command, "proof::layout-refused");
    }
    assert_eq!(snapshot(fixture.repository()), wrong_layout);

    fixture.write_payload();
    let absent_receipt = snapshot(fixture.repository());
    let refusal = refusal_pair(&fixture, &["proof", "replay"], "proof::receipt-refused");
    assert!(
        refusal
            .observed
            .as_deref()
            .is_some_and(|observed| observed.contains("is missing"))
    );
    assert_eq!(snapshot(fixture.repository()), absent_receipt);

    fs::create_dir_all(
        fixture
            .receipt_path()
            .parent()
            .expect("receipt parent exists"),
    )
    .expect("create receipt directory");
    fs::write(fixture.receipt_path(), b"{}\n").expect("write corrupt receipt");
    let corrupt_receipt = snapshot(fixture.repository());
    refusal_pair(&fixture, &["proof", "replay"], "proof::receipt-refused");
    assert_eq!(snapshot(fixture.repository()), corrupt_receipt);
    assert_eq!(fixture.battery_invocations(), 0);
    fixture.assert_daemonless();
}

#[test]
fn canonical_receipt_drift_is_a_verification_refusal_without_mutation() {
    let fixture = Fixture::new();
    json_reply(&fixture.invoke(&["proof", "--json"]));
    fixture.clear_invocations();
    let mut receipt: ProofReceipt =
        wire::decode(&fs::read(fixture.receipt_path()).expect("read receipt"))
            .expect("receipt decodes");
    receipt.battery = Digest::compute(Domain::ProofBattery, b"drifted battery");
    fs::write(
        fixture.receipt_path(),
        wire::encode(&receipt).expect("drifted receipt remains canonical"),
    )
    .expect("write drifted receipt");
    let before = snapshot(fixture.repository());

    let refusal = refusal_pair(
        &fixture,
        &["proof", "replay"],
        "proof::verification-refused",
    );
    assert!(
        refusal
            .observed
            .as_deref()
            .is_some_and(|observed| observed.contains("does not bind the replay battery"))
    );
    assert_eq!(snapshot(fixture.repository()), before);
    assert_eq!(
        fixture.battery_invocations(),
        2 * proof::CheckKind::ALL.len()
    );
    fixture.assert_daemonless();
}

#[test]
fn refused_batteries_keep_receipt_and_repository_bytes_unchanged_in_both_forms() {
    let fixture = Fixture::new();
    json_reply(&fixture.invoke(&["proof", "--json"]));
    fixture.clear_invocations();
    fs::write(&fixture.refuse_marker, b"refuse").expect("arm refused runner");
    let before = snapshot(fixture.repository());

    for command in [&["proof"][..], &["proof", "replay"][..]] {
        let refusal = refusal_pair(&fixture, command, "proof::battery-refused");
        assert!(refusal.witness.is_some());
        // Both member lists are written out whole rather than joined from
        // `CheckKind::ALL`, because an assertion built from the array the
        // subject builds from shortens together with it: a truncation would
        // leave the two sides equal and only a renaming would ever show.
        // Naming the five also reds these lines when the array gains a sixth
        // member, rather than quietly accepting the shorter set it once had.
        assert_eq!(
            refusal.observed.as_deref(),
            Some(
                "gate=failed(code=19), mutation-corpus=failed(code=19), \
                 dialect-refusals=failed(code=19), wait-and-process=failed(code=19), \
                 supply-rebuild=failed(code=19)"
            )
        );
        // `refusal_pair` has already proved the text form is exactly the reason
        // and the remedy, so a remedy that names the member is the text form
        // naming it. Stated against the remedy because that is the only field
        // of the four the text rendering keeps.
        assert_eq!(
            refusal.remedy,
            "repair gate, mutation-corpus, dialect-refusals, wait-and-process, \
             supply-rebuild and rerun the proof leaf"
        );
    }

    assert_eq!(snapshot(fixture.repository()), before);
    assert_eq!(
        fixture.battery_invocations(),
        4 * proof::CheckKind::ALL.len()
    );
    fixture.assert_daemonless();
}

#[test]
fn runner_refusal_process_fault_and_state_parent_refusal_stay_distinct() {
    let fixture = Fixture::new();
    fs::remove_file(&fixture.selected_runtime).expect("remove selected runtime");
    let missing_runtime = snapshot(fixture.repository());
    refusal_pair(&fixture, &["proof"], "proof::runner-refused");
    assert_eq!(snapshot(fixture.repository()), missing_runtime);

    fs::write(&fixture.selected_runtime, "#!/missing-proof-interpreter\n")
        .expect("write unspawnable runtime");
    make_executable(&fixture.selected_runtime);
    let unspawnable = snapshot(fixture.repository());
    let process = fault_pair(&fixture, &["proof"], "proof::process-failed");
    assert_eq!(process.class, wire::FaultClass::Environment);
    assert_eq!(snapshot(fixture.repository()), unspawnable);

    fixture.write_selected_runtime();
    fixture.clear_invocations();
    fs::create_dir_all(
        fixture
            .proof_root()
            .parent()
            .expect("proof root has Rust state parent"),
    )
    .expect("create Rust state root");
    fs::write(fixture.proof_root(), b"blocks receipt directory")
        .expect("plant publication obstruction");
    let obstructed = snapshot(fixture.repository());
    let state = refusal_pair(&fixture, &["proof"], "repository::state-path-refused");
    assert!(
        state
            .observed
            .as_deref()
            .is_some_and(|observed| observed.contains(&fixture.proof_root().display().to_string()))
    );
    assert_eq!(snapshot(fixture.repository()), obstructed);
    assert_eq!(fixture.battery_invocations(), 0);
    fixture.assert_daemonless();
}

struct Spoken {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

fn text_reply(spoken: &Spoken) -> ProofEvidence {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    assert_eq!(newline_count(&spoken.out), 1);
    assert_not_unlanded(&spoken.out);
    wire::decode(&spoken.out).expect("text proof body decodes")
}

fn json_reply(spoken: &Spoken) -> ProofEvidence {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    assert_eq!(newline_count(&spoken.out), 1);
    assert_not_unlanded(&spoken.out);
    let outcome: Outcome<ProofEvidence> =
        wire::decode(&spoken.out).expect("JSON proof outcome decodes");
    let Outcome::Reply(reply) = outcome else {
        panic!("proof leaf did not return a reply: {outcome:?}")
    };
    assert_eq!(reply.position, ReplyPosition::Direct {});
    reply.body
}

fn refusal_pair(fixture: &Fixture, command: &[&str], reason: &str) -> Refusal {
    let text = fixture.invoke(command);
    let mut json_command = command.to_vec();
    json_command.push("--json");
    let json = fixture.invoke(&json_command);

    for spoken in [&text, &json] {
        assert_eq!(spoken.code, Some(REFUSAL_EXIT));
        assert!(spoken.out.is_empty());
        assert_eq!(newline_count(&spoken.err), 1);
        assert_not_unlanded(&spoken.err);
    }
    let outcome: Outcome<ProofEvidence> =
        wire::decode(&json.err).expect("JSON proof refusal decodes");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("proof input must refuse: {outcome:?}")
    };
    assert_eq!(refusal.reason, reason);
    assert_eq!(
        text.err,
        format!("spice: {}; {}\n", refusal.reason, refusal.remedy).as_bytes()
    );
    refusal
}

fn fault_pair(fixture: &Fixture, command: &[&str], reason: &str) -> Fault {
    let text = fixture.invoke(command);
    let mut json_command = command.to_vec();
    json_command.push("--json");
    let json = fixture.invoke(&json_command);

    for spoken in [&text, &json] {
        assert_eq!(spoken.code, Some(FAULT_EXIT));
        assert!(spoken.out.is_empty());
        assert_eq!(newline_count(&spoken.err), 1);
        assert_not_unlanded(&spoken.err);
    }
    let outcome: Outcome<ProofEvidence> =
        wire::decode(&json.err).expect("JSON proof fault decodes");
    let Outcome::Fault(fault) = outcome else {
        panic!("proof defect must fault: {outcome:?}")
    };
    assert_eq!(fault.reason, reason);
    assert_eq!(
        text.err,
        format!("spice: {}; {}\n", fault.reason, fault.detail).as_bytes()
    );
    fault
}

fn assert_not_unlanded(bytes: &[u8]) {
    let placeholder = PlaceholderReason::NotLanded.as_str().as_bytes();
    assert!(
        !bytes
            .windows(placeholder.len())
            .any(|window| window == placeholder)
    );
}

fn newline_count(bytes: &[u8]) -> usize {
    bytes.iter().filter(|byte| **byte == b'\n').count()
}

struct Fixture {
    root: PathBuf,
    repository: PathBuf,
    tools: PathBuf,
    selected_runtime: PathBuf,
    invocations: PathBuf,
    refuse_marker: PathBuf,
    python_marker: PathBuf,
    target: ReleaseTarget,
    payload: TargetPayload,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-proof-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let repository = root.join("repository");
        let tools = root.join("tools");
        let selected_runtime = tools.join("selected-runtime");
        let unselected_runtime = tools.join("unselected-runtime-must-not-exist");
        let invocations = root.join("battery-invocations");
        let refuse_marker = root.join("refuse-battery");
        let python_marker = root.join("python-started");
        fs::create_dir_all(repository.join(".git/.spice")).expect("create fixture repository");
        fs::create_dir_all(&tools).expect("create fixture tools");

        let target = proof::compiled_target().expect("fixture runs only on a proof release target");
        let payload = payload(target);
        let fixture = Self {
            root,
            repository,
            tools,
            selected_runtime,
            invocations,
            refuse_marker,
            python_marker,
            target,
            payload,
        };
        fixture.write_selected_runtime();
        fixture.write_python_sentinels();
        fixture.write_config(&unselected_runtime);
        fixture.write_payload();
        fixture
    }

    fn repository(&self) -> &Path {
        &self.repository
    }

    fn payload_path(&self) -> PathBuf {
        self.repository
            .join("dist/stage/targets")
            .join(self.target.as_str())
            .join("payload.json")
    }

    fn proof_root(&self) -> PathBuf {
        self.repository.join(".git/.spice-rust/proof")
    }

    fn receipt_path(&self) -> PathBuf {
        self.proof_root()
            .join(self.payload.source.as_str())
            .join(self.target.as_str())
            .join("receipt.json")
    }

    fn relative_receipt_path(&self) -> PathBuf {
        self.receipt_path()
            .strip_prefix(&self.repository)
            .expect("receipt belongs to repository")
            .to_path_buf()
    }

    fn write_payload(&self) {
        let path = self.payload_path();
        fs::create_dir_all(path.parent().expect("payload parent"))
            .expect("create payload directory");
        fs::write(path, wire::encode(&self.payload).expect("payload encodes"))
            .expect("write payload");
    }

    fn write_config(&self, unselected_runtime: &Path) {
        let (container, runner) = match self.target {
            ReleaseTarget::Aarch64AppleDarwin => {
                (unselected_runtime, self.selected_runtime.as_path())
            }
            ReleaseTarget::X86_64UnknownLinuxMusl => {
                (self.selected_runtime.as_path(), unselected_runtime)
            }
        };
        let config = format!(
            "[proof]\ncontainer = {}\nrunner = {}\n",
            toml::Value::String(container.display().to_string()),
            toml::Value::String(runner.display().to_string())
        );
        fs::write(self.repository.join(".git/.spice/config.toml"), config)
            .expect("write local proof configuration");
    }

    fn write_selected_runtime(&self) {
        fs::write(&self.selected_runtime, selected_runtime(self))
            .expect("write selected proof runtime");
        make_executable(&self.selected_runtime);
    }

    fn write_python_sentinels(&self) {
        for name in ["python", "python3"] {
            let path = self.tools.join(name);
            fs::write(
                &path,
                format!(
                    "#!/bin/sh\nprintf 'started\\n' >> '{}'\nexit 97\n",
                    self.python_marker.display()
                ),
            )
            .expect("write Python sentinel");
            make_executable(&path);
        }
    }

    fn invoke(&self, argv: &[&str]) -> Spoken {
        let output = Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(argv)
            .current_dir(&self.repository)
            .env_clear()
            .env("PATH", &self.tools)
            .output()
            .expect("spice proof process launches");
        Spoken {
            code: output.status.code(),
            out: output.stdout,
            err: output.stderr,
        }
    }

    fn clear_invocations(&self) {
        fs::write(&self.invocations, b"").expect("clear battery invocation log");
    }

    fn battery_invocations(&self) -> usize {
        fs::read_to_string(&self.invocations)
            .unwrap_or_default()
            .lines()
            .count()
    }

    fn assert_daemonless(&self) {
        assert!(!self.python_marker.exists(), "no Python shim was started");
        for path in [
            ".git/.spice-rust/data",
            ".git/.spice-rust/resident",
            ".git/.spice-rust/canvas",
            ".git/.spice-rust/degraded",
            ".git/.spice-rust/tape",
        ] {
            assert!(
                !self.repository.join(path).exists(),
                "proof leaf created resident state at {path}"
            );
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove proof CLI fixture");
    }
}

#[cfg(all(target_arch = "aarch64", target_vendor = "apple", target_os = "macos"))]
fn selected_runtime(fixture: &Fixture) -> String {
    format!(
        concat!(
            "#!/bin/sh\n",
            "member=\" $* \"\n",
            "[ \"$1\" = spice-proof-native-runner-v1 ] || exit 71\n",
            "identity=$2\n",
            "challenge=$3\n",
            "printf 'spice-proof-native-attestation-v1:runner=%s:challenge=%s:",
            "repository=read-only:cargo-home=fresh-empty:cargo-target=fresh-empty:",
            "network=loopback-only:host-dependencies=denied\\n' ",
            "\"$identity\" \"$challenge\" >&2\n",
            "printf 'battery\\n' >> '{}'\n",
            "[ ! -f '{}' ] || exit 19\n",
            // Every member reports the scope it ran, in the words its own runner
            // uses, because a member that ran none of one is refused rather than
            // sealed. The test members report a summary and not just an
            // announcement, since an announcement counts the tests it skipped
            // among the ones it ran.
            "case \"$member\" in\n",
            "  *' mutants '*) printf 'Found 36 mutants to test\\n' ;;\n",
            "  *) printf 'running 7 tests\\n",
            "test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; ",
            "0 filtered out; finished in 0.00s\\n' ;;\n",
            "esac\n",
            "exit 0\n"
        ),
        fixture.invocations.display(),
        fixture.refuse_marker.display()
    )
}

#[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "musl"))]
fn selected_runtime(fixture: &Fixture) -> String {
    format!(
        concat!(
            "#!/bin/sh\n",
            "member=\" $* \"\n",
            "if [ \"$1\" = --version ]; then\n",
            "  printf 'Docker version fixture\\n'\n",
            "  exit 0\n",
            "fi\n",
            "while [ \"${{1:-}}\" != spice-proof-attestation ]; do\n",
            "  [ \"$#\" -gt 0 ] || exit 72\n",
            "  shift\n",
            "done\n",
            "shift\n",
            "dialect=$1\n",
            "challenge=$2\n",
            "printf 'spice-proof-attestation-v2:%s:%s\\n' \"$dialect\" \"$challenge\" >&2\n",
            "printf 'battery\\n' >> '{}'\n",
            "[ ! -f '{}' ] || exit 19\n",
            // Every member reports the scope it ran, in the words its own runner
            // uses, because a member that ran none of one is refused rather than
            // sealed. The test members report a summary and not just an
            // announcement, since an announcement counts the tests it skipped
            // among the ones it ran.
            "case \"$member\" in\n",
            "  *' mutants '*) printf 'Found 36 mutants to test\\n' ;;\n",
            "  *) printf 'running 7 tests\\n",
            "test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; ",
            "0 filtered out; finished in 0.00s\\n' ;;\n",
            "esac\n",
            "exit 0\n"
        ),
        fixture.invocations.display(),
        fixture.refuse_marker.display()
    )
}

fn payload(target: ReleaseTarget) -> TargetPayload {
    let pins = BuildPins {
        rust: "rustc 1.97.1".to_owned(),
        wasm_target: "wasm32-unknown-unknown".to_owned(),
        trunk: "trunk 0.21.14".to_owned(),
        cargo_mutants: "cargo-mutants 27.1.0".to_owned(),
        proof_container: proof::DEFAULT_IMAGE.to_owned(),
    };
    let app = Digest::compute(Domain::DistArtifact, b"app");
    let embedded = Digest::compute(Domain::DistArtifact, b"embedded");
    let native = Digest::compute(Domain::DistArtifact, b"native");
    let inventory = vec![
        InventoryEntry {
            path: "app/app.wasm".to_owned(),
            mode: 0o644,
            bytes: 3,
            digest: app.clone(),
        },
        InventoryEntry {
            path: "bin/spice".to_owned(),
            mode: 0o755,
            bytes: 6,
            digest: native.clone(),
        },
        InventoryEntry {
            path: "embed/app.bin".to_owned(),
            mode: 0o644,
            bytes: 8,
            digest: embedded.clone(),
        },
    ];
    TargetPayload {
        version: 1,
        target,
        source: Digest::compute(Domain::DistArtifact, b"source"),
        app,
        embedded_app: embedded,
        native,
        pins_digest: Digest::compute(Domain::DistPins, &wire::encode(&pins).expect("pins encode")),
        inventory_digest: Digest::compute(
            Domain::DistInventory,
            &wire::encode(&inventory).expect("inventory encodes"),
        ),
        pins,
        inventory,
    }
}

fn make_executable(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make fixture executable");
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = BTreeMap::new();
    while let Some(path) = pending.pop() {
        let mut entries = fs::read_dir(&path)
            .expect("snapshot directory")
            .map(|entry| entry.expect("snapshot entry").path())
            .collect::<Vec<_>>();
        entries.sort();
        for entry in entries.into_iter().rev() {
            if entry.is_dir() {
                pending.push(entry);
            } else {
                files.insert(
                    entry
                        .strip_prefix(root)
                        .expect("snapshot entry belongs to root")
                        .to_path_buf(),
                    fs::read(entry).expect("snapshot file"),
                );
            }
        }
    }
    files
}
