use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use wire::manifest::{
    RELEASE_NOTES_VERSION, ReleaseNotesGate, ReleaseNotesInput, ReleaseNotesInputTarget,
    ReleaseNotesProof, ReleaseTarget,
};
use wire::{Digest, Domain, FaultClass, Outcome, ReleaseNotes, ReplyPosition};

const FAULT_EXIT: i32 = 1;
const REFUSAL_EXIT: i32 = 2;
const TARGETS: [ReleaseTarget; 2] = [
    ReleaseTarget::Aarch64AppleDarwin,
    ReleaseTarget::X86_64UnknownLinuxMusl,
];

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    input: PathBuf,
    value: ReleaseNotesInput,
}

impl Fixture {
    fn valid(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-release-notes-{name}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("fixture directory creates");
        let input = root.join("release-input.json");
        let value = input_fixture();
        write_read_only(
            &input,
            &wire::encode(&value).expect("fixture input encodes"),
        );
        Self { root, input, value }
    }

    fn invoke(&self, json: bool, input: &Path) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        if json {
            command.arg("--json");
        }
        command
            .args(["release", "notes"])
            .arg(input)
            .current_dir(&self.root)
            .env("PATH", "")
            .output()
            .expect("spice release notes launches")
    }

    fn write_input(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.root.join(name);
        write_read_only(&path, bytes);
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("fixture cleanup");
    }
}

#[test]
fn canonical_input_renders_byte_stable_text_and_json_without_mutation() {
    let fixture = Fixture::valid("canonical");
    let before = snapshot(&fixture.input);

    let first_text = fixture.invoke(false, &fixture.input);
    let second_text = fixture.invoke(false, &fixture.input);
    assert!(first_text.status.success(), "{:?}", first_text.stderr);
    assert_eq!(first_text.stdout, second_text.stdout);
    let text_body: ReleaseNotes =
        wire::decode(&first_text.stdout).expect("text body is canonical notes");

    let first_json = fixture.invoke(true, &fixture.input);
    let second_json = fixture.invoke(true, &fixture.input);
    assert!(first_json.status.success(), "{:?}", first_json.stderr);
    assert_eq!(first_json.stdout, second_json.stdout);
    let Outcome::Reply(reply) =
        wire::decode::<Outcome<ReleaseNotes>>(&first_json.stdout).expect("JSON outcome decodes")
    else {
        panic!("canonical release evidence must reply");
    };

    assert_eq!(reply.position, ReplyPosition::Direct {});
    assert_eq!(reply.body, text_body, "both forms carry the same body");
    assert_eq!(reply.body.version, RELEASE_NOTES_VERSION);
    assert_eq!(reply.body.source, fixture.value.source);
    assert_eq!(reply.body.targets.len(), TARGETS.len());
    assert_eq!(
        reply.body.identity,
        Digest::compute(
            Domain::DistReleaseNotes,
            &fs::read(&fixture.input).expect("input reads")
        )
    );
    let mut canonical_body = wire::encode(&reply.body).expect("reply body encodes");
    canonical_body.push(b'\n');
    assert_eq!(first_text.stdout, canonical_body);
    assert_eq!(snapshot(&fixture.input), before, "input file changed");
}

#[test]
fn absent_cross_target_duplicate_and_noncanonical_evidence_refuse_unchanged() {
    let fixture = Fixture::valid("refusals");

    let mut absent: serde_json::Value =
        wire::decode(&fs::read(&fixture.input).expect("input reads")).expect("value decodes");
    absent["targets"][0]
        .as_object_mut()
        .expect("target is an object")
        .remove("gate");
    let absent_path = fixture.write_input(
        "absent-gate.json",
        &wire::encode(&absent).expect("missing evidence fixture encodes"),
    );
    assert_refusal_unchanged(&fixture, &absent_path);

    let mut cross_target = fixture.value.clone();
    cross_target.targets[0].proof.target = ReleaseTarget::X86_64UnknownLinuxMusl;
    let cross_target_path = fixture.write_input(
        "cross-target.json",
        &wire::encode(&cross_target).expect("cross-target fixture encodes"),
    );
    assert_refusal_unchanged(&fixture, &cross_target_path);

    let mut duplicate = fixture.value.clone();
    duplicate.targets[1].gate.receipt = duplicate.targets[0].gate.receipt.clone();
    let duplicate_path = fixture.write_input(
        "duplicate.json",
        &wire::encode(&duplicate).expect("duplicate fixture encodes"),
    );
    assert_refusal_unchanged(&fixture, &duplicate_path);

    let mut noncanonical = fs::read(&fixture.input).expect("canonical input reads");
    noncanonical.push(b'\n');
    let noncanonical_path = fixture.write_input("noncanonical.json", &noncanonical);
    assert_refusal_unchanged(&fixture, &noncanonical_path);
}

#[test]
fn an_unreadable_input_is_one_typed_environment_fault() {
    let fixture = Fixture::valid("unreadable");
    let absent = fixture.root.join("absent.json");

    let output = fixture.invoke(true, &absent);
    assert_eq!(output.status.code(), Some(FAULT_EXIT));
    assert!(output.stdout.is_empty(), "fault spoke on stdout");
    let Outcome::<ReleaseNotes>::Fault(fault) =
        wire::decode(&output.stderr).expect("fault decodes")
    else {
        panic!("missing input must fault: {:?}", output.stderr);
    };
    assert_eq!(fault.class, FaultClass::Environment);
    assert_eq!(fault.reason, "release::input-unreadable");
}

fn input_fixture() -> ReleaseNotesInput {
    let source = digest(Domain::DistSourceCommit, "source");
    ReleaseNotesInput {
        version: RELEASE_NOTES_VERSION,
        source: source.clone(),
        targets: TARGETS
            .into_iter()
            .enumerate()
            .map(|(index, target)| {
                let payload = digest(Domain::DistTargetPayload, &format!("payload-{index}"));
                ReleaseNotesInputTarget {
                    target,
                    payload: payload.clone(),
                    proof: ReleaseNotesProof {
                        target,
                        payload,
                        receipt: digest(Domain::ProofReceiptV2, &format!("proof-{index}")),
                    },
                    gate: ReleaseNotesGate {
                        target,
                        source: source.clone(),
                        receipt: digest(Domain::GateReceipt, &format!("gate-{index}")),
                    },
                }
            })
            .collect(),
    }
}

fn assert_refusal_unchanged(fixture: &Fixture, input: &Path) {
    let before = snapshot(input);
    let output = fixture.invoke(true, input);
    assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
    assert!(output.stdout.is_empty(), "refusal spoke on stdout");
    let Outcome::<ReleaseNotes>::Refusal(refusal) =
        wire::decode(&output.stderr).expect("refusal decodes")
    else {
        panic!("expected typed refusal: {:?}", output.stderr);
    };
    assert_eq!(refusal.reason, "release::notes-refused");
    assert_eq!(snapshot(input), before, "refused input changed");
}

fn write_read_only(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("fixture input writes");
    let mut permissions = fs::metadata(path)
        .expect("fixture metadata reads")
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).expect("fixture becomes read-only");
}

fn snapshot(path: &Path) -> (Vec<u8>, bool) {
    let metadata = fs::metadata(path).expect("input metadata reads");
    (
        fs::read(path).expect("input bytes read"),
        metadata.permissions().readonly(),
    )
}

fn digest(domain: Domain, label: &str) -> Digest {
    Digest::compute(domain, label.as_bytes())
}
