use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use wire::manifest::{
    ProofEnvironment, ProofStanding, RELEASE_MANIFEST_VERSION, ReleaseManifest,
    ReleaseManifestTarget, ReleaseTarget,
};
use wire::{Digest, Domain, Outcome, ReleaseNotes, ReplyPosition};

const REFUSAL_EXIT: i32 = 2;
const OCI_IMAGE: [&str; 2] = [
    "ghcr.io/example/spice-proof@sha256:",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
];

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    base: String,
    head: String,
    manifest: PathBuf,
}

impl Fixture {
    fn valid(name: &str) -> Self {
        Self::new(
            name,
            concat!(
                "Ship sealed release notes\n\n",
                "Release-Operator-Action: Rotate the deployment token\n",
                "Release-Fixed-Refusal: Reject stale release manifests"
            ),
        )
    }

    fn new(name: &str, head_message: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-release-notes-{name}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("journal/src")).expect("fixture source directory");
        git(&root, &["init", "-q"]);
        fs::write(
            root.join("journal/src/lib.rs"),
            "pub const SCHEMA_VERSION: u32 = 1;\n",
        )
        .expect("base schema writes");
        git(&root, &["add", "journal/src/lib.rs"]);
        commit(&root, "Establish schema one", "2001-01-01T00:00:00Z");
        let base = revision(&root, "HEAD");

        fs::write(root.join("release-seed"), "sealed inputs\n").expect("seed file writes");
        git(&root, &["add", "release-seed"]);
        commit(
            &root,
            concat!(
                "Prepare release evidence\n\n",
                "Release-Operator-Action: Rotate the deployment token"
            ),
            "2001-01-02T00:00:00Z",
        );

        fs::write(
            root.join("journal/src/lib.rs"),
            "pub const SCHEMA_VERSION: u32 = 2;\n",
        )
        .expect("head schema writes");
        git(&root, &["add", "journal/src/lib.rs"]);
        commit(&root, head_message, "2001-01-03T00:00:00Z");
        let head = revision(&root, "HEAD");
        let manifest = root.join("release-manifest.json");
        write_manifest(&manifest, &head, true);

        Self {
            root,
            base,
            head,
            manifest,
        }
    }

    fn range(&self) -> String {
        format!("{}..{}", self.base, self.head)
    }

    fn invoke(&self, json: bool, range: &str, manifest: &Path) -> Output {
        self.invoke_from(json, range, manifest, &self.root)
    }

    fn invoke_from(&self, json: bool, range: &str, manifest: &Path, current: &Path) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        if json {
            command.arg("--json");
        }
        command
            .args(["release", "notes", range])
            .arg(manifest)
            .current_dir(current)
            .env("PATH", "")
            .output()
            .expect("spice release notes launches")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("fixture cleanup");
    }
}

#[test]
fn populated_range_renders_both_forms_from_sealed_evidence_without_mutation() {
    let fixture = Fixture::valid("populated");
    let before = tree(&fixture.root);
    let range = fixture.range();

    let json = fixture.invoke(true, &range, &fixture.manifest);
    assert!(
        json.status.success(),
        "JSON notes failed: {:?}",
        json.stderr
    );
    let Outcome::Reply(reply) =
        wire::decode::<Outcome<ReleaseNotes>>(&json.stdout).expect("JSON outcome decodes")
    else {
        panic!("valid release evidence must reply");
    };
    assert_eq!(reply.position, ReplyPosition::Direct {});
    assert_eq!(
        reply.body.manifest,
        Digest::compute(
            Domain::DistReleaseManifest,
            &fs::read(&fixture.manifest).expect("manifest reads")
        )
    );
    assert_eq!(
        reply.body.build,
        dist::source_commit_digest(&fixture.head).unwrap()
    );
    assert_eq!(reply.body.range.from, fixture.base);
    assert_eq!(reply.body.range.to, fixture.head);
    assert_eq!(reply.body.range.commits, 2);
    assert_eq!(reply.body.schema.from, 1);
    assert_eq!(reply.body.schema.to, 2);
    assert_eq!(reply.body.targets.len(), 2);
    assert_eq!(
        reply
            .body
            .targets
            .iter()
            .map(|target| target.target)
            .collect::<Vec<_>>(),
        [
            ReleaseTarget::Aarch64AppleDarwin,
            ReleaseTarget::X86_64UnknownLinuxMusl
        ]
    );
    assert_ne!(reply.body.targets[0].receipt, reply.body.targets[1].receipt);
    assert_eq!(reply.body.operator_actions, ["Rotate the deployment token"]);
    assert_eq!(
        reply.body.fixed_refusals,
        ["Reject stale release manifests"]
    );
    assert_eq!(reply.body.changes.len(), 2);
    assert_eq!(reply.body.changes[0].subject, "Prepare release evidence");
    assert_eq!(reply.body.changes[1].commit, fixture.head);
    assert_eq!(reply.body.changes[1].subject, "Ship sealed release notes");

    let text = fixture.invoke(false, &range, &fixture.manifest);
    assert!(
        text.status.success(),
        "text notes failed: {:?}",
        text.stderr
    );
    let text_body = wire::decode::<ReleaseNotes>(&text.stdout).expect("text body decodes");
    assert_eq!(text_body, reply.body, "forms report one typed result");

    let nested = fixture.root.join("journal/src");
    let nested_output = fixture.invoke_from(true, &range, &fixture.manifest, &nested);
    assert!(
        nested_output.status.success(),
        "nested repository discovery failed: {:?}",
        nested_output.stderr
    );
    let Outcome::Reply(nested_reply) =
        wire::decode::<Outcome<ReleaseNotes>>(&nested_output.stdout).expect("nested reply decodes")
    else {
        panic!("nested invocation must reply");
    };
    assert_eq!(nested_reply.body, reply.body);
    assert_eq!(
        tree(&fixture.root),
        before,
        "release notes mutated its inputs"
    );
}

#[test]
fn an_empty_range_is_a_complete_release_document() {
    let fixture = Fixture::valid("empty");
    let manifest = fixture.root.join("base-manifest.json");
    write_manifest(&manifest, &fixture.base, true);
    let range = format!("{}..{}", fixture.base, fixture.base);
    let before = tree(&fixture.root);

    let output = fixture.invoke(true, &range, &manifest);
    let Outcome::Reply(reply) =
        wire::decode::<Outcome<ReleaseNotes>>(&output.stdout).expect("empty outcome decodes")
    else {
        panic!("empty immutable range must reply");
    };
    assert_eq!(reply.body.range.commits, 0);
    assert_eq!(reply.body.schema.from, 1);
    assert_eq!(reply.body.schema.to, 1);
    assert!(reply.body.operator_actions.is_empty());
    assert!(reply.body.fixed_refusals.is_empty());
    assert!(reply.body.changes.is_empty());
    assert_eq!(tree(&fixture.root), before, "empty notes mutated inputs");
}

#[test]
fn manifest_range_and_authorship_inconsistencies_refuse_without_mutation() {
    let mismatch = Fixture::valid("source-mismatch");
    write_manifest(&mismatch.manifest, &mismatch.base, true);
    assert_refusal_unchanged(&mismatch, &mismatch.range(), &mismatch.manifest);

    let reverse = Fixture::valid("reverse");
    write_manifest(&reverse.manifest, &reverse.base, true);
    let reverse_range = format!("{}..{}", reverse.head, reverse.base);
    assert_refusal_unchanged(&reverse, &reverse_range, &reverse.manifest);

    let noncanonical = Fixture::valid("noncanonical-range");
    let noncanonical_range = format!(
        "{}..{}",
        noncanonical.base,
        noncanonical.head.to_uppercase()
    );
    assert_refusal_unchanged(&noncanonical, &noncanonical_range, &noncanonical.manifest);

    let placeholder = Fixture::new(
        "placeholder",
        "Ship release\n\nRelease-Operator-Action: <TODO>",
    );
    assert_refusal_unchanged(&placeholder, &placeholder.range(), &placeholder.manifest);

    let incomplete = Fixture::valid("incomplete-manifest");
    write_manifest(&incomplete.manifest, &incomplete.head, false);
    assert_refusal_unchanged(&incomplete, &incomplete.range(), &incomplete.manifest);
}

fn write_manifest(path: &Path, commit: &str, complete: bool) {
    let source = dist::source_commit_digest(commit).expect("fixture commit is canonical");
    let battery = proof::Battery::current().digest().clone();
    let target = |target, label: &'static [u8], environment| ReleaseManifestTarget {
        target,
        payload: Digest::compute(Domain::DistTargetPayload, label),
        receipt: Digest::compute(Domain::ProofReceiptV2, label),
        battery: battery.clone(),
        replay: Digest::compute(Domain::ProofReplayV2, label),
        environment,
        standing: ProofStanding::Clean,
    };
    let mut targets = vec![
        target(
            ReleaseTarget::Aarch64AppleDarwin,
            b"darwin",
            ProofEnvironment::native(Digest::compute(Domain::ProofNativeRunner, b"native-runner")),
        ),
        target(
            ReleaseTarget::X86_64UnknownLinuxMusl,
            b"linux",
            ProofEnvironment::oci(OCI_IMAGE.concat()).expect("fixture OCI identity is pinned"),
        ),
    ];
    if !complete {
        targets.pop();
    }
    let manifest = ReleaseManifest {
        version: RELEASE_MANIFEST_VERSION,
        source,
        targets,
    };
    fs::write(
        path,
        wire::encode(&manifest).expect("fixture manifest encodes"),
    )
    .expect("fixture manifest writes");
}

fn assert_refusal_unchanged(fixture: &Fixture, range: &str, manifest: &Path) {
    let before = tree(&fixture.root);
    let output = fixture.invoke(true, range, manifest);
    assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
    let Outcome::<ReleaseNotes>::Refusal(refusal) =
        wire::decode(&output.stderr).expect("refusal decodes")
    else {
        panic!("expected typed refusal: {:?}", output.stderr);
    };
    assert_eq!(refusal.reason, "release::notes-refused");
    assert!(output.stdout.is_empty(), "refusal spoke on stdout");
    assert_eq!(tree(&fixture.root), before, "refusal mutated its inputs");
}

fn commit(root: &Path, message: &str, instant: &str) {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=Spice Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-q",
            "-m",
            message,
        ])
        .env("GIT_AUTHOR_DATE", instant)
        .env("GIT_COMMITTER_DATE", instant)
        .current_dir(root)
        .output()
        .expect("fixture commit launches");
    assert!(
        output.status.success(),
        "fixture commit failed: {:?}",
        output.stderr
    );
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("fixture Git command launches");
    assert!(
        output.status.success(),
        "fixture Git failed: {:?}",
        output.stderr
    );
}

fn revision(root: &Path, revision: &str) -> String {
    let output = Command::new("git")
        .args(["rev-parse", revision])
        .current_dir(root)
        .output()
        .expect("fixture revision launches");
    assert!(
        output.status.success(),
        "revision failed: {:?}",
        output.stderr
    );
    String::from_utf8(output.stdout)
        .expect("revision is UTF-8")
        .trim()
        .to_owned()
}

fn tree(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut state = BTreeMap::new();
    walk(root, root, &mut state);
    state
}

fn walk(root: &Path, directory: &Path, state: &mut BTreeMap<String, Option<Vec<u8>>>) {
    let mut entries = fs::read_dir(directory)
        .expect("snapshot directory reads")
        .collect::<Result<Vec<_>, _>>()
        .expect("snapshot entries read");
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .expect("snapshot entry belongs to root")
            .to_string_lossy()
            .into_owned();
        let kind = entry.file_type().expect("snapshot file type");
        if kind.is_dir() {
            state.insert(relative, None);
            walk(root, &path, state);
        } else {
            state.insert(relative, Some(fs::read(path).expect("snapshot file reads")));
        }
    }
}
