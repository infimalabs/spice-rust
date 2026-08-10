use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use wire::manifest::{JOURNAL_BACKUP_VERSION, JournalBackupManifest, JournalBackupMember};
use wire::{Digest, Domain, FaultClass, Outcome, ReplyPosition, RestoreVerification, WireVersion};

const FAULT_EXIT: i32 = 1;
const REFUSAL_EXIT: i32 = 2;
const AUTHORITY: &str = "spicejournal.sqlite3";

#[path = "support/fixture_root.rs"]
mod fixture_root;

struct Fixture {
    root: PathBuf,
    bundle: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        #[cfg(unix)]
        let fixture_base = PathBuf::from("/tmp");
        #[cfg(not(unix))]
        let fixture_base = std::env::temp_dir();
        let root = fixture_root::claim(&fixture_base, &format!("srv-{name}"));
        let bundle = root.join("backup");
        fs::create_dir(root.join(".git")).expect("fixture Git directory");
        fs::create_dir_all(bundle.join("tape")).expect("bundle directories");
        drop(
            journal::Journal::open(bundle.join(AUTHORITY))
                .expect("finalized authority fixture opens"),
        );
        fs::write(bundle.join("tape/lane-a"), b"sealed transcript member")
            .expect("tape fixture writes");
        let fixture = Self { root, bundle };
        fixture.write_manifest(JOURNAL_BACKUP_VERSION, journal::SCHEMA_VERSION);
        fixture
    }

    fn write_manifest(&self, version: u16, journal_schema: u32) {
        self.write_manifest_versions(version, WireVersion::CURRENT, journal_schema);
    }

    fn write_manifest_versions(&self, version: u16, wire: WireVersion, journal_schema: u32) {
        let mut members = [AUTHORITY, "tape/lane-a"]
            .into_iter()
            .map(|path| {
                let bytes = fs::read(self.bundle.join(path)).expect("member reads");
                JournalBackupMember {
                    path: path.to_owned(),
                    mode: member_mode(&self.bundle.join(path)),
                    bytes: u64::try_from(bytes.len()).expect("fixture length crosses"),
                    digest: Digest::compute(Domain::JournalBackupMember, &bytes),
                }
            })
            .collect::<Vec<_>>();
        members.sort_by(|left, right| left.path.cmp(&right.path));
        let manifest = JournalBackupManifest {
            version,
            wire,
            journal_schema,
            authority: AUTHORITY.to_owned(),
            members,
        };
        self.persist_manifest(&manifest);
    }

    fn rewrite_manifest(&self, rewrite: impl FnOnce(&mut JournalBackupManifest)) {
        let bytes = fs::read(self.bundle.join("manifest.json")).expect("manifest reads");
        let mut manifest = wire::decode::<JournalBackupManifest>(&bytes).expect("manifest decodes");
        rewrite(&mut manifest);
        self.persist_manifest(&manifest);
    }

    fn persist_manifest(&self, manifest: &JournalBackupManifest) {
        fs::write(
            self.bundle.join("manifest.json"),
            wire::encode(manifest).expect("manifest encodes"),
        )
        .expect("manifest writes");
    }

    fn invoke(&self, json: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        if json {
            command.arg("--json");
        }
        command
            .args(["restore", "verify"])
            .arg(&self.bundle)
            .current_dir(&self.root)
            .output()
            .expect("spice restore verify launches")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        #[cfg(unix)]
        for directory in [&self.root, &self.bundle, &self.bundle.join("tape")] {
            use std::os::unix::fs::PermissionsExt;

            let _ = fs::set_permissions(directory, fs::Permissions::from_mode(0o755));
        }
        fs::remove_dir_all(&self.root).expect("fixture cleanup");
    }
}

#[test]
fn valid_bundle_verifies_in_both_forms_without_mutating_the_repository() {
    let fixture = Fixture::new("valid");
    let before = tree(&fixture.root);

    let json = fixture.invoke(true);
    assert!(
        json.status.success(),
        "JSON verify failed: {:?}",
        json.stderr
    );
    let Outcome::Reply(reply) =
        wire::decode::<Outcome<RestoreVerification>>(&json.stdout).expect("JSON outcome decodes")
    else {
        panic!("valid bundle must reply");
    };
    assert_eq!(reply.position, ReplyPosition::Direct {});
    assert_eq!(reply.body.wire, WireVersion::CURRENT);
    assert_eq!(reply.body.journal_schema, journal::SCHEMA_VERSION);
    assert_eq!(reply.body.members, 2);
    assert_eq!(reply.body.facts, 0);
    assert_eq!(reply.body.receipts, 0);
    assert_eq!(reply.body.heads, 0);
    assert_eq!(reply.body.threads, 0);
    assert_eq!(
        reply.body.authority,
        Digest::compute(
            Domain::JournalBackupMember,
            &fs::read(fixture.bundle.join(AUTHORITY)).expect("authority reads")
        )
    );
    assert_eq!(
        reply.body.bundle,
        Digest::compute(
            Domain::JournalBackupManifest,
            &fs::read(fixture.bundle.join("manifest.json")).expect("manifest reads")
        )
    );

    let text = fixture.invoke(false);
    assert!(
        text.status.success(),
        "text verify failed: {:?}",
        text.stderr
    );
    let text_body = wire::decode::<RestoreVerification>(&text.stdout).expect("text body decodes");
    assert_eq!(text_body, reply.body, "forms report one typed result");
    assert_eq!(
        tree(&fixture.root),
        before,
        "verification mutated its inputs"
    );
}

fn member_mode(path: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::metadata(path)
            .expect("member metadata reads")
            .permissions()
            .mode()
            & 0o7777
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        0
    }
}

#[test]
fn missing_forward_and_incomplete_bundles_refuse_by_class() {
    let missing = Fixture::new("missing");
    let absent = missing.root.join("absent");
    let output = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args(["--json", "restore", "verify"])
        .arg(absent)
        .current_dir(&missing.root)
        .output()
        .expect("missing verify launches");
    assert_refusal(&output, "restore::bundle-incomplete");

    let forward = Fixture::new("forward");
    forward.write_manifest(JOURNAL_BACKUP_VERSION, journal::SCHEMA_VERSION + 1);
    assert_refusal(&forward.invoke(true), "restore::schema-forward");

    let retired_bundle = Fixture::new("retired-bundle");
    retired_bundle.write_manifest(JOURNAL_BACKUP_VERSION + 1, journal::SCHEMA_VERSION);
    assert_refusal(&retired_bundle.invoke(true), "restore::bundle-version");

    let foreign_wire = Fixture::new("foreign-wire");
    foreign_wire.write_manifest_versions(
        JOURNAL_BACKUP_VERSION,
        WireVersion(WireVersion::CURRENT.0 + 1),
        journal::SCHEMA_VERSION,
    );
    assert_refusal(&foreign_wire.invoke(true), "restore::wire-version");

    let hidden_forward = Fixture::new("hidden-forward");
    let connection = rusqlite::Connection::open(hidden_forward.bundle.join(AUTHORITY))
        .expect("authority opens for forward fixture");
    connection
        .execute(
            "UPDATE journal_schema SET version = ?1 WHERE singleton = 1",
            [journal::SCHEMA_VERSION + 1],
        )
        .expect("authority schema advances");
    connection
        .pragma_update(None, "user_version", journal::SCHEMA_VERSION + 1)
        .expect("authority pragma advances");
    drop(connection);
    hidden_forward.write_manifest(JOURNAL_BACKUP_VERSION, journal::SCHEMA_VERSION);
    assert_refusal(&hidden_forward.invoke(true), "restore::schema-forward");

    let incomplete = Fixture::new("incomplete");
    fs::remove_file(incomplete.bundle.join("tape/lane-a")).expect("member removes");
    assert_refusal(&incomplete.invoke(true), "restore::bundle-incomplete");

    let extra = Fixture::new("extra");
    fs::write(extra.bundle.join("unmanifested"), b"not admitted").expect("extra member writes");
    assert_refusal(&extra.invoke(true), "restore::bundle-incomplete");
}

#[test]
fn corrupt_manifest_digest_and_replay_closure_fault_as_store_evidence() {
    let manifest = Fixture::new("manifest-corrupt");
    fs::write(manifest.bundle.join("manifest.json"), b"not a manifest").expect("manifest corrupts");
    assert_store_fault(&manifest.invoke(true));

    let digest = Fixture::new("digest-corrupt");
    fs::write(digest.bundle.join("tape/lane-a"), b"changed after backup").expect("member corrupts");
    assert_store_fault(&digest.invoke(true));

    let replay = Fixture::new("replay-corrupt");
    let connection = rusqlite::Connection::open(replay.bundle.join(AUTHORITY))
        .expect("authority opens for corruption fixture");
    connection
        .execute("DELETE FROM cursor_head", [])
        .expect("cursor singleton corrupts");
    drop(connection);
    replay.write_manifest(JOURNAL_BACKUP_VERSION, journal::SCHEMA_VERSION);
    let before = tree(&replay.root);
    assert_store_fault(&replay.invoke(true));
    assert_eq!(tree(&replay.root), before, "failed replay mutated bundle");
}

#[test]
fn manifest_bound_truncated_authority_reaches_captured_byte_admission_unchanged() {
    let fixture = Fixture::new("truncated-authority");
    let authority = fixture.bundle.join(AUTHORITY);
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&authority)
        .expect("authority opens for truncation");
    file.set_len(64).expect("authority truncates");
    drop(file);
    fixture.write_manifest(JOURNAL_BACKUP_VERSION, journal::SCHEMA_VERSION);
    let authority_bytes = fs::read(&authority).expect("truncated authority reads");
    let manifest: JournalBackupManifest =
        wire::decode(&fs::read(fixture.bundle.join("manifest.json")).expect("manifest reads"))
            .expect("manifest decodes");
    let declared = manifest
        .members
        .iter()
        .find(|member| member.path == AUTHORITY)
        .expect("manifest names authority");
    assert_eq!(declared.bytes, 64);
    assert_eq!(
        declared.digest,
        Digest::compute(Domain::JournalBackupMember, &authority_bytes)
    );
    let before = tree(&fixture.root);

    let output = fixture.invoke(true);

    assert_eq!(output.status.code(), Some(FAULT_EXIT));
    let Outcome::<RestoreVerification>::Fault(fault) =
        wire::decode(&output.stderr).expect("truncation fault decodes")
    else {
        panic!("truncated authority did not fault: {:?}", output.stderr);
    };
    assert_eq!(fault.class, FaultClass::Store);
    assert_eq!(fault.reason, "restore::bundle-invalid");
    assert!(
        fault.detail.contains("backup authority was refused"),
        "member authentication stopped before captured-byte SQLite admission: {fault:?}"
    );
    assert_eq!(
        tree(&fixture.root),
        before,
        "failed admission mutated bundle"
    );
}

#[test]
fn manifest_canonicality_member_metadata_and_schema_drift_are_store_evidence() {
    let noncanonical = Fixture::new("noncanonical");
    let mut bytes = fs::read(noncanonical.bundle.join("manifest.json")).expect("manifest reads");
    bytes.push(b'\n');
    fs::write(noncanonical.bundle.join("manifest.json"), bytes).expect("manifest rewrites");
    assert_store_fault(&noncanonical.invoke(true));

    let mode = Fixture::new("mode");
    set_mode(&mode.bundle.join("tape/lane-a"), 0o600);
    assert_store_fault(&mode.invoke(true));

    let declared_mode = Fixture::new("declared-mode");
    declared_mode.rewrite_manifest(|manifest| manifest.members[0].mode = 0o10_000);
    assert_store_fault(&declared_mode.invoke(true));

    let length = Fixture::new("length");
    length.rewrite_manifest(|manifest| manifest.members[0].bytes += 1);
    assert_store_fault(&length.invoke(true));

    let digest = Fixture::new("declared-digest");
    digest.rewrite_manifest(|manifest| {
        manifest.members[0].digest =
            Digest::compute(Domain::JournalBackupMember, b"another generation");
    });
    assert_store_fault(&digest.invoke(true));

    let stale_schema = Fixture::new("stale-schema");
    stale_schema.write_manifest(JOURNAL_BACKUP_VERSION, journal::SCHEMA_VERSION - 1);
    assert_store_fault(&stale_schema.invoke(true));
}

#[test]
fn noncanonical_paths_duplicate_order_and_extra_file_kinds_are_rejected() {
    let traversal = Fixture::new("traversal");
    traversal.rewrite_manifest(|manifest| manifest.authority = "../authority".to_owned());
    assert_store_fault(&traversal.invoke(true));

    let duplicate = Fixture::new("duplicate");
    duplicate.rewrite_manifest(|manifest| manifest.members.push(manifest.members[0].clone()));
    assert_refusal(&duplicate.invoke(true), "restore::bundle-incomplete");

    let unordered = Fixture::new("unordered");
    unordered.rewrite_manifest(|manifest| manifest.members.reverse());
    assert_refusal(&unordered.invoke(true), "restore::bundle-incomplete");

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;

        let linked = Fixture::new("symlink");
        fs::remove_file(linked.bundle.join("tape/lane-a")).expect("member removes");
        symlink("../spicejournal.sqlite3", linked.bundle.join("tape/lane-a"))
            .expect("symlink creates");
        assert_store_fault(&linked.invoke(true));

        let special = Fixture::new("special");
        let _socket = UnixListener::bind(special.bundle.join("special.sock"))
            .expect("special socket creates");
        assert_store_fault(&special.invoke(true));
    }
}

#[test]
#[cfg(unix)]
fn a_genuinely_read_only_bundle_verifies_without_changing_bytes_or_modes() {
    let fixture = Fixture::new("read-only");
    set_mode(&fixture.bundle.join(AUTHORITY), 0o444);
    set_mode(&fixture.bundle.join("tape/lane-a"), 0o444);
    fixture.write_manifest(JOURNAL_BACKUP_VERSION, journal::SCHEMA_VERSION);
    set_mode(&fixture.bundle.join("manifest.json"), 0o444);
    set_mode(&fixture.bundle.join("tape"), 0o555);
    set_mode(&fixture.bundle, 0o555);
    let before = tree(&fixture.root);

    let output = fixture.invoke(true);
    assert!(
        output.status.success(),
        "read-only verification failed: {:?}",
        output.stderr
    );
    assert_eq!(tree(&fixture.root), before, "read-only inputs changed");

    set_mode(&fixture.bundle, 0o755);
    set_mode(&fixture.bundle.join("tape"), 0o755);
    set_mode(&fixture.bundle.join(AUTHORITY), 0o644);
    set_mode(&fixture.bundle.join("tape/lane-a"), 0o644);
    set_mode(&fixture.bundle.join("manifest.json"), 0o644);
}

fn assert_refusal(output: &Output, reason: &str) {
    assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
    let Outcome::<RestoreVerification>::Refusal(refusal) =
        wire::decode(&output.stderr).expect("refusal decodes")
    else {
        panic!("expected typed refusal: {:?}", output.stderr);
    };
    assert_eq!(refusal.reason(), reason);
    assert!(output.stdout.is_empty(), "refusal spoke on stdout");
}

fn assert_store_fault(output: &Output) {
    assert_eq!(output.status.code(), Some(FAULT_EXIT));
    let Outcome::<RestoreVerification>::Fault(fault) =
        wire::decode(&output.stderr).expect("fault decodes")
    else {
        panic!("expected typed fault: {:?}", output.stderr);
    };
    assert_eq!(fault.class, FaultClass::Store);
    assert_eq!(fault.reason, "restore::bundle-invalid");
    assert!(output.stdout.is_empty(), "fault spoke on stdout");
}

fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("fixture mode changes");
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
}

fn tree(root: &Path) -> BTreeMap<String, (u32, Option<Vec<u8>>)> {
    let mut state = BTreeMap::new();
    walk(root, root, &mut state);
    state
}

fn walk(root: &Path, directory: &Path, state: &mut BTreeMap<String, (u32, Option<Vec<u8>>)>) {
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
        let mode = member_mode(&path);
        if kind.is_dir() {
            state.insert(relative, (mode, None));
            walk(root, &path, state);
        } else {
            state.insert(
                relative,
                (mode, Some(fs::read(path).expect("snapshot file reads"))),
            );
        }
    }
}
