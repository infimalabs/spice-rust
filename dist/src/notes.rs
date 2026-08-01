//! Daemonless release-note derivation from immutable Git objects and one seal.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use wire::manifest::{
    ProofEnvironment, ProofStanding, RELEASE_MANIFEST_VERSION, ReleaseManifest, ReleaseTarget,
};
use wire::{
    Digest, Domain, ReleaseNoteChange, ReleaseNotes, ReleaseNotesRange, ReleaseNotesTarget,
    SchemaMovement,
};

const SCHEMA_PATH: &str = "journal/src/lib.rs";
const SCHEMA_PREFIX: &str = "pub const SCHEMA_VERSION: u32 = ";
const OPERATOR_ACTION: &str = "Release-Operator-Action: ";
const FIXED_REFUSAL: &str = "Release-Fixed-Refusal: ";
const SHA1_HEX_LEN: usize = 40;
const TARGETS: [ReleaseTarget; 2] = [
    ReleaseTarget::Aarch64AppleDarwin,
    ReleaseTarget::X86_64UnknownLinuxMusl,
];

/// Derive one operator-facing release document without a checkout or journal.
///
/// # Errors
///
/// Refuses a non-immutable range, an incomplete or stale manifest, source and
/// Git disagreement, malformed schema evidence, or unresolved authored note
/// placeholders. Filesystem and Git-store acquisition failures remain typed.
pub fn release_notes(
    repository: impl AsRef<Path>,
    range: &str,
    manifest_path: impl AsRef<Path>,
    current_battery: &Digest,
) -> Result<ReleaseNotes, NotesError> {
    let range = GitRange::parse(range)?;
    let repository = open_repository(repository.as_ref())?;
    let commits = first_parent_range(&repository, range.from, range.to)?;
    let from_schema = schema_at(&repository, range.from)?;
    let to_schema = schema_at(&repository, range.to)?;
    let manifest = admit_manifest(manifest_path.as_ref(), current_battery)?;
    let expected_build = source_commit_digest(&range.to_text)?;
    if manifest.value.source != expected_build {
        return Err(NotesError::SourceMismatch {
            manifest: manifest.value.source,
            git: expected_build,
        });
    }
    let authored = authored_notes(commits)?;
    let commit_count = u64::try_from(authored.changes.len())
        .map_err(|error| NotesError::Range(format!("commit count cannot cross as u64: {error}")))?;

    Ok(ReleaseNotes {
        manifest: manifest.digest,
        build: manifest.value.source,
        range: ReleaseNotesRange {
            from: range.from_text,
            to: range.to_text,
            commits: commit_count,
        },
        schema: SchemaMovement {
            from: from_schema,
            to: to_schema,
        },
        targets: manifest
            .value
            .targets
            .into_iter()
            .map(|target| ReleaseNotesTarget {
                target: target.target,
                payload: target.payload,
                receipt: target.receipt,
            })
            .collect(),
        operator_actions: authored.operator_actions,
        fixed_refusals: authored.fixed_refusals,
        changes: authored.changes,
    })
}

/// Identity used by target payloads and release manifests for one Git commit.
///
/// # Errors
///
/// Refuses anything other than one full lower-case SHA-1 object spelling.
pub fn source_commit_digest(commit: &str) -> Result<Digest, NotesError> {
    parse_object_id(commit, "source commit")?;
    Ok(Digest::compute(Domain::DistSourceCommit, commit.as_bytes()))
}

#[derive(Clone, Debug)]
struct GitRange {
    from: gix::ObjectId,
    to: gix::ObjectId,
    from_text: String,
    to_text: String,
}

impl GitRange {
    fn parse(text: &str) -> Result<Self, NotesError> {
        let Some((from, to)) = text.split_once("..") else {
            return Err(NotesError::Range(
                "range must be FULL_FROM_COMMIT..FULL_TO_COMMIT".to_owned(),
            ));
        };
        if to.contains("..") {
            return Err(NotesError::Range(
                "range must contain exactly one two-dot delimiter".to_owned(),
            ));
        }
        Ok(Self {
            from: parse_object_id(from, "range lower commit")?,
            to: parse_object_id(to, "range upper commit")?,
            from_text: from.to_owned(),
            to_text: to.to_owned(),
        })
    }
}

fn parse_object_id(text: &str, subject: &str) -> Result<gix::ObjectId, NotesError> {
    if text.len() != SHA1_HEX_LEN || !text.bytes().all(lower_hex) {
        return Err(NotesError::Range(format!(
            "{subject} is not one full lower-case SHA-1 object identity: {text:?}"
        )));
    }
    match text.parse() {
        Ok(identity) => Ok(identity),
        Err(error) => Err(NotesError::Range(format!(
            "{subject} does not parse: {error}"
        ))),
    }
}

fn lower_hex(byte: u8) -> bool {
    byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
}

fn open_repository(path: &Path) -> Result<gix::Repository, NotesError> {
    let options = gix::open::Options::isolated()
        .strict_config(true)
        .config_overrides(["gitoxide.objects.noReplace=true"]);
    gix::discover_opts(path, Default::default(), options).map_err(|error| NotesError::Repository {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })
}

#[derive(Clone)]
struct CommitRecord {
    id: gix::ObjectId,
    tree: gix::ObjectId,
    parent: Option<gix::ObjectId>,
    message: Vec<u8>,
}

fn first_parent_range(
    repository: &gix::Repository,
    from: gix::ObjectId,
    to: gix::ObjectId,
) -> Result<Vec<CommitRecord>, NotesError> {
    let mut cursor = to;
    let mut seen = BTreeSet::new();
    let mut commits = Vec::new();
    while cursor != from {
        if !seen.insert(cursor) {
            return Err(NotesError::Range(
                "first-parent history contains a repeated commit".to_owned(),
            ));
        }
        let commit = load_commit(repository, cursor)?;
        cursor = commit.parent.ok_or_else(|| {
            NotesError::Range(format!(
                "lower commit {from} is not on upper commit {to}'s first-parent history"
            ))
        })?;
        commits.push(commit);
    }
    load_commit(repository, from)?;
    commits.reverse();
    Ok(commits)
}

fn load_commit(
    repository: &gix::Repository,
    id: gix::ObjectId,
) -> Result<CommitRecord, NotesError> {
    let object = verified_object(repository, id, &id.to_string())?;
    if object.kind != gix::objs::Kind::Commit {
        return Err(NotesError::Range(format!(
            "object {id} is a {}, not a commit",
            object.kind
        )));
    }
    let commit = object
        .try_into_commit()
        .map_err(|error| NotesError::GitObject {
            object: id.to_string(),
            detail: error.to_string(),
        })?;
    let tree = commit
        .tree_id()
        .map(gix::Id::detach)
        .map_err(|error| NotesError::GitObject {
            object: id.to_string(),
            detail: error.to_string(),
        })?;
    let parent = commit.parent_ids().next().map(gix::Id::detach);
    let message = commit
        .message_raw()
        .map_err(|error| NotesError::GitObject {
            object: id.to_string(),
            detail: error.to_string(),
        })?
        .to_vec();
    Ok(CommitRecord {
        id,
        tree,
        parent,
        message,
    })
}

fn schema_at(repository: &gix::Repository, commit: gix::ObjectId) -> Result<u32, NotesError> {
    let commit = load_commit(repository, commit)?;
    let bytes = blob_at(repository, commit.tree, SCHEMA_PATH)?;
    parse_schema(&bytes, commit.id)
}

fn blob_at(
    repository: &gix::Repository,
    mut tree_id: gix::ObjectId,
    path: &str,
) -> Result<Vec<u8>, NotesError> {
    let components = path.split('/').collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let object = verified_object(repository, tree_id, path)?;
        if object.kind != gix::objs::Kind::Tree {
            return Err(NotesError::GitObject {
                object: tree_id.to_string(),
                detail: format!("{path} crossed a non-tree {}", object.kind),
            });
        }
        let tree = object
            .try_into_tree()
            .map_err(|error| NotesError::GitObject {
                object: tree_id.to_string(),
                detail: error.to_string(),
            })?;
        let entry = tree
            .iter()
            .map(|entry| {
                entry.map_err(|error| NotesError::GitObject {
                    object: tree_id.to_string(),
                    detail: error.to_string(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .find(|entry| <_ as AsRef<[u8]>>::as_ref(entry.filename()) == component.as_bytes())
            .ok_or_else(|| NotesError::Schema(format!("{path} is absent")))?;
        tree_id = entry.object_id();
        let last = index + 1 == components.len();
        if last && entry.mode().value() != 0o100644 {
            return Err(NotesError::Schema(format!(
                "{path} has unsupported Git mode {:06o}",
                entry.mode().value()
            )));
        }
    }
    let object = verified_object(repository, tree_id, path)?;
    if object.kind != gix::objs::Kind::Blob {
        return Err(NotesError::GitObject {
            object: tree_id.to_string(),
            detail: format!("{path} names a {}, not a blob", object.kind),
        });
    }
    Ok(object.data.clone())
}

fn verified_object<'repo>(
    repository: &'repo gix::Repository,
    id: gix::ObjectId,
    subject: &str,
) -> Result<gix::Object<'repo>, NotesError> {
    let object = repository
        .find_object(id)
        .map_err(|error| NotesError::GitObject {
            object: id.to_string(),
            detail: format!("{subject}: {error}"),
        })?;
    let actual =
        gix::objs::compute_hash(id.kind(), object.kind, &object.data).map_err(|error| {
            NotesError::GitObject {
                object: id.to_string(),
                detail: error.to_string(),
            }
        })?;
    if actual != id {
        return Err(NotesError::GitObject {
            object: id.to_string(),
            detail: format!("authenticated bytes name {actual}"),
        });
    }
    Ok(object)
}

fn parse_schema(bytes: &[u8], commit: gix::ObjectId) -> Result<u32, NotesError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| NotesError::Schema(format!("{SCHEMA_PATH} at {commit} is not UTF-8")))?;
    let values = text
        .lines()
        .filter_map(|line| line.strip_prefix(SCHEMA_PREFIX))
        .filter_map(|tail| tail.strip_suffix(';'))
        .collect::<Vec<_>>();
    let [value] = values.as_slice() else {
        return Err(NotesError::Schema(format!(
            "{SCHEMA_PATH} at {commit} does not declare one exact SCHEMA_VERSION"
        )));
    };
    let parsed = value.parse::<u32>().map_err(|error| {
        NotesError::Schema(format!("schema {value:?} at {commit} is invalid: {error}"))
    })?;
    if parsed.to_string() != *value {
        return Err(NotesError::Schema(format!(
            "schema {value:?} at {commit} is not canonical"
        )));
    }
    Ok(parsed)
}

struct AdmittedManifest {
    value: ReleaseManifest,
    digest: Digest,
}

fn admit_manifest(path: &Path, current_battery: &Digest) -> Result<AdmittedManifest, NotesError> {
    let bytes = read_manifest(path)?;
    let manifest = wire::decode::<ReleaseManifest>(&bytes).map_err(|error| {
        NotesError::ManifestInvalid(format!("manifest does not decode: {error}"))
    })?;
    let canonical = wire::encode(&manifest).map_err(|error| {
        NotesError::ManifestInvalid(format!("manifest does not encode: {error}"))
    })?;
    if canonical != bytes {
        return Err(NotesError::ManifestInvalid(
            "manifest is not canonical current wire JSON".to_owned(),
        ));
    }
    admit_manifest_shape(&manifest, current_battery)?;
    Ok(AdmittedManifest {
        value: manifest,
        digest: Digest::compute(Domain::DistReleaseManifest, &bytes),
    })
}

fn read_manifest(path: &Path) -> Result<Vec<u8>, NotesError> {
    let mut file = File::open(path).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            NotesError::ManifestMissing(path.to_path_buf())
        } else {
            NotesError::ManifestIo {
                path: path.to_path_buf(),
                source,
            }
        }
    })?;
    let metadata = file.metadata().map_err(|source| NotesError::ManifestIo {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(NotesError::ManifestInvalid(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| NotesError::ManifestIo {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(bytes)
}

fn admit_manifest_shape(
    manifest: &ReleaseManifest,
    current_battery: &Digest,
) -> Result<(), NotesError> {
    if manifest.version != RELEASE_MANIFEST_VERSION {
        return Err(NotesError::ManifestInvalid(format!(
            "manifest version {} is not current version {RELEASE_MANIFEST_VERSION}",
            manifest.version
        )));
    }
    if manifest.targets.len() != TARGETS.len() {
        return Err(NotesError::ManifestInvalid(format!(
            "manifest has {} targets, expected {}",
            manifest.targets.len(),
            TARGETS.len()
        )));
    }
    let mut payloads = BTreeSet::new();
    let mut receipts = BTreeSet::new();
    for (entry, expected) in manifest.targets.iter().zip(TARGETS) {
        admit_manifest_target(entry, expected, current_battery)?;
        if !payloads.insert(entry.payload.clone()) || !receipts.insert(entry.receipt.clone()) {
            return Err(NotesError::ManifestInvalid(
                "manifest target payloads and receipts must be distinct".to_owned(),
            ));
        }
    }
    Ok(())
}

fn admit_manifest_target(
    entry: &wire::manifest::ReleaseManifestTarget,
    expected: ReleaseTarget,
    current_battery: &Digest,
) -> Result<(), NotesError> {
    if entry.target != expected {
        return Err(NotesError::ManifestInvalid(format!(
            "manifest target {} is {}, expected {expected}",
            expected.as_str(),
            entry.target
        )));
    }
    if entry.battery != *current_battery {
        return Err(NotesError::ManifestInvalid(format!(
            "manifest target {expected} does not bind the current proof battery"
        )));
    }
    if entry.standing != ProofStanding::Clean {
        return Err(NotesError::ManifestInvalid(format!(
            "manifest target {expected} is not clean"
        )));
    }
    let environment_matches = matches!(
        (&entry.environment, expected),
        (
            ProofEnvironment::Native { .. },
            ReleaseTarget::Aarch64AppleDarwin
        ) | (
            ProofEnvironment::Oci { .. },
            ReleaseTarget::X86_64UnknownLinuxMusl
        )
    );
    if !environment_matches {
        return Err(NotesError::ManifestInvalid(format!(
            "manifest target {expected} carries the wrong proof environment"
        )));
    }
    Ok(())
}

struct AuthoredNotes {
    operator_actions: Vec<String>,
    fixed_refusals: Vec<String>,
    changes: Vec<ReleaseNoteChange>,
}

fn authored_notes(commits: Vec<CommitRecord>) -> Result<AuthoredNotes, NotesError> {
    let mut operator_actions = Vec::new();
    let mut fixed_refusals = Vec::new();
    let mut seen_actions = BTreeSet::new();
    let mut seen_refusals = BTreeSet::new();
    let mut changes = Vec::with_capacity(commits.len());
    for commit in commits {
        let message = std::str::from_utf8(&commit.message).map_err(|_| {
            NotesError::Authored(format!("commit {} message is not UTF-8", commit.id))
        })?;
        let subject = authored_subject(message, commit.id)?;
        for line in message.lines().skip(1) {
            if let Some(value) = line.strip_prefix(OPERATOR_ACTION) {
                let value = authored_value(value, OPERATOR_ACTION, commit.id)?;
                if seen_actions.insert(value.clone()) {
                    operator_actions.push(value);
                }
            } else if let Some(value) = line.strip_prefix(FIXED_REFUSAL) {
                let value = authored_value(value, FIXED_REFUSAL, commit.id)?;
                if seen_refusals.insert(value.clone()) {
                    fixed_refusals.push(value);
                }
            }
        }
        changes.push(ReleaseNoteChange {
            commit: commit.id.to_string(),
            subject,
        });
    }
    Ok(AuthoredNotes {
        operator_actions,
        fixed_refusals,
        changes,
    })
}

fn authored_subject(message: &str, commit: gix::ObjectId) -> Result<String, NotesError> {
    let subject = message.lines().next().unwrap_or_default();
    if subject.is_empty() || subject.trim() != subject || subject.chars().any(char::is_control) {
        return Err(NotesError::Authored(format!(
            "commit {commit} has no canonical one-line subject"
        )));
    }
    refuse_placeholder(subject, commit)?;
    Ok(subject.to_owned())
}

fn authored_value(value: &str, field: &str, commit: gix::ObjectId) -> Result<String, NotesError> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() || normalized != value {
        return Err(NotesError::Authored(format!(
            "commit {commit} has noncanonical {field}text"
        )));
    }
    refuse_placeholder(value, commit)?;
    Ok(value.to_owned())
}

fn refuse_placeholder(text: &str, commit: gix::ObjectId) -> Result<(), NotesError> {
    let lower = text.to_ascii_lowercase();
    let trimmed = lower.trim_matches(|character: char| !character.is_ascii_alphanumeric());
    let unresolved = matches!(trimmed, "todo" | "tbd" | "fixme" | "replace me")
        || lower
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|word| matches!(word, "todo" | "tbd" | "fixme"))
        || lower.contains("replace-me")
        || lower.contains("replace me")
        || lower.contains("<todo>")
        || lower.contains("<tbd>")
        || lower.contains("{{")
        || lower.contains("}}")
        || lower.contains("[todo]")
        || lower.contains("[tbd]");
    if unresolved {
        Err(NotesError::Authored(format!(
            "commit {commit} retains an unresolved release-note placeholder"
        )))
    } else {
        Ok(())
    }
}

/// Why release notes could not be derived from the explicit public evidence.
#[derive(Debug)]
pub enum NotesError {
    ManifestMissing(PathBuf),
    ManifestIo { path: PathBuf, source: io::Error },
    ManifestInvalid(String),
    Repository { path: PathBuf, detail: String },
    Range(String),
    GitObject { object: String, detail: String },
    Schema(String),
    SourceMismatch { manifest: Digest, git: Digest },
    Authored(String),
}

impl fmt::Display for NotesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ManifestMissing(path) => {
                write!(formatter, "release manifest is missing: {}", path.display())
            }
            Self::ManifestIo { path, source } => write!(
                formatter,
                "release manifest access failed at {}: {source}",
                path.display()
            ),
            Self::ManifestInvalid(detail) => {
                write!(formatter, "release manifest is invalid: {detail}")
            }
            Self::Repository { path, detail } => write!(
                formatter,
                "Git repository at {} is unavailable: {detail}",
                path.display()
            ),
            Self::Range(detail) => write!(formatter, "release Git range is invalid: {detail}"),
            Self::GitObject { object, detail } => {
                write!(formatter, "Git object {object} is unavailable: {detail}")
            }
            Self::Schema(detail) => {
                write!(formatter, "release schema evidence is invalid: {detail}")
            }
            Self::SourceMismatch { manifest, git } => write!(
                formatter,
                "release manifest build {manifest} disagrees with Git build {git}"
            ),
            Self::Authored(detail) => {
                write!(formatter, "release note authorship is unresolved: {detail}")
            }
        }
    }
}

impl Error for NotesError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ManifestIo { source, .. } => Some(source),
            Self::ManifestMissing(_)
            | Self::ManifestInvalid(_)
            | Self::Repository { .. }
            | Self::Range(_)
            | Self::GitObject { .. }
            | Self::Schema(_)
            | Self::SourceMismatch { .. }
            | Self::Authored(_) => None,
        }
    }
}
