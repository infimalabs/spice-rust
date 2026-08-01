#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use wire::manifest::{
    BuildPins, ProofEnvironment, ProofReceipt, ProofStanding, RELEASE_MANIFEST_VERSION,
    ReleaseManifest, ReleaseTarget, SealedTarget, TARGET_PAYLOAD_VERSION, TargetPayload,
};
use wire::{Digest, Domain, Outcome};

const OCI_SHA256_HEX_LEN: usize = 64;

struct Fixture {
    root: PathBuf,
    manifest: PathBuf,
    from: String,
    to: String,
}

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-release-notes-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("release fixture root is exclusively created");
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.email", "release@example.invalid"]);
        git(&root, &["config", "user.name", "Release Fixture"]);
        fs::write(root.join("change.txt"), "base\n").expect("write base change");
        git(&root, &["add", "change.txt"]);
        git(&root, &["commit", "-q", "-m", "Previous release"]);
        let from = git_output(&root, &["rev-parse", "HEAD"]);
        fs::write(root.join("change.txt"), "base\nfirst\n").expect("write first change");
        git(&root, &["add", "change.txt"]);
        git(&root, &["commit", "-q", "-m", "Name checksum refusals"]);
        fs::write(root.join("change.txt"), "base\nfirst\nsecond\n").expect("write second change");
        git(&root, &["add", "change.txt"]);
        git(&root, &["commit", "-q", "-m", "Publish operator actions"]);
        let to = git_output(&root, &["rev-parse", "HEAD"]);
        let root = fs::canonicalize(root).expect("canonicalize release fixture root");
        let manifest = root.join("dist/stage/release-manifest.json");
        fs::create_dir_all(manifest.parent().expect("manifest parent"))
            .expect("create manifest parent");
        fs::write(&manifest, manifest_bytes(&from, &to)).expect("write sealed manifest");
        fs::write(
            root.join("dist/release.toml"),
            concat!(
                "manifest = 'dist/stage/release-manifest.json'\n",
                "summary = 'Release one closes the daemonless utility surface.'\n",
                "operator_actions = ['Regenerate the installed skill with the previewed install plan.']\n",
                "fixed_refusals = ['Restore verification now names checksum and inventory drift.']\n",
            ),
        )
        .expect("write release narrative");
        Self {
            root,
            manifest,
            from,
            to,
        }
    }

    fn invoke(&self, json: bool) -> Spoken {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        if json {
            command.arg("--json");
        }
        let output = command
            .args(["release", "notes"])
            .current_dir(&self.root)
            .env_remove(steer::hook::INBOX)
            .output()
            .expect("spice release notes process launches");
        Spoken {
            code: output.status.code(),
            out: output.stdout,
            err: output.stderr,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Spoken {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

#[test]
fn notes_bind_manifest_schema_targets_receipts_and_the_actual_git_range() {
    let fixture = Fixture::new("valid");
    let json = reply_body(fixture.invoke(true));
    assert_eq!(json["release"], "1.0.0");
    assert_eq!(json["git_from"], fixture.from);
    assert_eq!(json["git_to"], fixture.to);
    assert_eq!(json["journal_schema"], journal::SCHEMA_VERSION);
    assert_eq!(json["targets"].as_array().map(Vec::len), Some(2));
    assert_eq!(json["commits"].as_array().map(Vec::len), Some(2));
    assert_eq!(json["commits"][0]["title"], "Name checksum refusals");
    assert_eq!(json["commits"][1]["title"], "Publish operator actions");
    assert!(json["build"].as_str().is_some());

    let text = fixture.invoke(false);
    assert_eq!(text.code, Some(0));
    assert!(text.err.is_empty());
    let markdown = String::from_utf8(text.out).expect("release notes Markdown is UTF-8");
    for expected in [
        "# Spice 1.0.0",
        "## Build Identity",
        "## Supported Targets",
        "aarch64-apple-darwin",
        "x86_64-unknown-linux-musl",
        "## Operator Actions",
        "## Fixed Refusals",
        "## Git Changes",
        &fixture.from,
        &fixture.to,
    ] {
        assert!(markdown.contains(expected), "Markdown lost {expected:?}");
    }
    assert!(
        !fixture.root.join(".git/.spice/data").exists(),
        "release notes must not open or create journal state"
    );
}

#[test]
fn unresolved_authored_placeholders_and_receipt_drift_refuse() {
    let placeholder = Fixture::new("placeholder");
    fs::write(
        placeholder.root.join("dist/release.toml"),
        concat!(
            "manifest = 'dist/stage/release-manifest.json'\n",
            "summary = 'TODO describe this release'\n",
            "operator_actions = ['Run the install preview.']\n",
            "fixed_refusals = ['Checksum drift is named.']\n",
        ),
    )
    .expect("write placeholder narrative");
    refusal(placeholder.invoke(true), "release::notes-refused");

    let receipt = Fixture::new("receipt");
    let bytes = fs::read(&receipt.manifest).expect("read sealed manifest");
    let mut manifest: ReleaseManifest = wire::decode(&bytes).expect("decode sealed manifest");
    manifest.targets[0].receipt.payload = digest(Domain::DistTargetPayload, "wrong-payload");
    fs::write(
        &receipt.manifest,
        wire::encode(&manifest).expect("encode receipt-drifted manifest"),
    )
    .expect("write receipt-drifted manifest");
    refusal(receipt.invoke(true), "release::notes-refused");
}

#[test]
fn noncanonical_manifests_and_missing_git_objects_refuse_without_journal_access() {
    let noncanonical = Fixture::new("noncanonical");
    let mut bytes = fs::read(&noncanonical.manifest).expect("read sealed manifest");
    bytes.push(b'\n');
    fs::write(&noncanonical.manifest, bytes).expect("write noncanonical manifest");
    refusal(noncanonical.invoke(true), "release::notes-refused");

    let missing = Fixture::new("missing-object");
    let bytes = fs::read(&missing.manifest).expect("read sealed manifest");
    let mut manifest: ReleaseManifest = wire::decode(&bytes).expect("decode sealed manifest");
    manifest.git_to = "0000000000000000000000000000000000000000".to_owned();
    fs::write(
        &missing.manifest,
        wire::encode(&manifest).expect("encode missing-object manifest"),
    )
    .expect("write missing-object manifest");
    refusal(missing.invoke(true), "release::notes-refused");
    assert!(!missing.root.join(".git/.spice/data").exists());
}

fn manifest_bytes(from: &str, to: &str) -> Vec<u8> {
    let source = digest(Domain::GateTree, "source-tree");
    let app = digest(Domain::DistArtifact, "app");
    let embedded = digest(Domain::DistArtifact, "embedded-app");
    let pins = BuildPins {
        rust: "1.97.1".to_owned(),
        wasm_target: "wasm32-unknown-unknown".to_owned(),
        trunk: "0.21.14".to_owned(),
        cargo_mutants: "26.2.0".to_owned(),
        proof_container: "sha256:fixture".to_owned(),
    };
    let pins_digest = Digest::compute(Domain::DistPins, &wire::encode(&pins).expect("encode pins"));
    let targets = ReleaseTarget::ALL
        .into_iter()
        .map(|target| sealed_target(target, &source, &app, &embedded, &pins, &pins_digest))
        .collect();
    wire::encode(&ReleaseManifest {
        version: RELEASE_MANIFEST_VERSION,
        release: "1.0.0".to_owned(),
        git_from: from.to_owned(),
        git_to: to.to_owned(),
        previous_journal_schema: journal::SCHEMA_VERSION,
        journal_schema: journal::SCHEMA_VERSION,
        targets,
    })
    .expect("encode sealed release manifest")
}

fn sealed_target(
    target: ReleaseTarget,
    source: &Digest,
    app: &Digest,
    embedded: &Digest,
    pins: &BuildPins,
    pins_digest: &Digest,
) -> SealedTarget {
    let inventory = Vec::new();
    let payload = TargetPayload {
        version: TARGET_PAYLOAD_VERSION,
        target,
        source: source.clone(),
        app: app.clone(),
        embedded_app: embedded.clone(),
        native: digest(Domain::DistArtifact, target.as_str()),
        pins: pins.clone(),
        pins_digest: pins_digest.clone(),
        inventory_digest: Digest::compute(
            Domain::DistInventory,
            &wire::encode(&inventory).expect("encode empty inventory"),
        ),
        inventory,
    };
    let payload_digest = Digest::compute(
        Domain::DistTargetPayload,
        &wire::encode(&payload).expect("encode target payload"),
    );
    let environment = match target {
        ReleaseTarget::Aarch64AppleDarwin => {
            ProofEnvironment::native(digest(Domain::ProofNativeRunner, "native-runner"))
        }
        ReleaseTarget::X86_64UnknownLinuxMusl => {
            ProofEnvironment::oci(format!("sha256:{}", "a".repeat(OCI_SHA256_HEX_LEN)))
                .expect("canonical fixture OCI identity")
        }
    };
    let receipt = ProofReceipt {
        version: wire::manifest::PROOF_RECEIPT_VERSION,
        target,
        payload: payload_digest.clone(),
        battery: digest(Domain::ProofBattery, "battery"),
        replay: digest(Domain::ProofReplayV2, target.as_str()),
        environment,
        standing: ProofStanding::Clean,
    };
    let receipt_digest = Digest::compute(
        Domain::ProofReceiptV2,
        &wire::encode(&receipt).expect("encode proof receipt"),
    );
    SealedTarget {
        target,
        payload,
        payload_digest,
        receipt,
        receipt_digest,
    }
}

fn digest(domain: Domain, marker: &str) -> Digest {
    Digest::compute(domain, marker.as_bytes())
}

fn git(root: &PathBuf, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("fixture Git command launches");
    assert!(output.status.success(), "git {arguments:?}: {output:?}");
}

fn git_output(root: &PathBuf, arguments: &[&str]) -> String {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("fixture Git query launches");
    assert!(output.status.success(), "git {arguments:?}: {output:?}");
    String::from_utf8(output.stdout)
        .expect("Git query is UTF-8")
        .trim()
        .to_owned()
}

fn reply_body(spoken: Spoken) -> Value {
    assert_eq!(
        spoken.code,
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&spoken.err)
    );
    assert!(spoken.err.is_empty());
    let outcome: Outcome<Value> = wire::decode(&spoken.out).expect("release notes outcome decodes");
    let Outcome::Reply(reply) = outcome else {
        panic!("release notes must reply: {outcome:?}");
    };
    assert_eq!(reply.position, wire::ReplyPosition::Direct {});
    reply.body
}

fn refusal(spoken: Spoken, reason: &str) {
    assert_eq!(spoken.code, Some(2));
    assert!(spoken.out.is_empty());
    let outcome: Outcome<Value> = wire::decode(&spoken.err).expect("release refusal decodes");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("release notes must refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason, reason);
}
