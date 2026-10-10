use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use wire::manifest::{JOURNAL_BACKUP_MAX_BYTES, JOURNAL_BACKUP_MAX_MEMBERS};
use wire::{Digest, Domain, TaskStamp};

use crate::model::{Artifact, Manifest, ObjectMetadata};

use super::{
    Error, METADATA, PAYLOAD, admitted_directory, admitted_relative, create_private_directories,
    create_replacement_directory, decode_canonical, manifest_path, object_dir, open_regular_file,
    parse_task_key, read_manifest, read_regular, task_directories_with_limit,
    validate_manifest_shape, validate_object,
};

const COPY_BUFFER_BYTES: usize = 64 * 1024;

/// One already-authenticated backup member that can stream its captured bytes.
pub trait BackupSource {
    fn path(&self) -> &str;
    fn mode(&self) -> u32;
    fn bytes(&self) -> u64;
    fn copy_to(&self, target: &mut dyn Write) -> io::Result<u64>;
}

struct SourcePlan<'a> {
    source: &'a dyn BackupSource,
    path: String,
    mode: u32,
    bytes: u64,
}

/// One immutable regular file in a captured artifact-store generation.
#[derive(Debug)]
pub struct BackupMember {
    path: String,
    mode: u32,
    bytes: u64,
    digest: Digest,
    captured: PathBuf,
}

impl BackupMember {
    /// Canonical slash-separated path relative to the artifact-store root.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Exact POSIX permission bits observed while capturing the member.
    #[must_use]
    pub const fn mode(&self) -> u32 {
        self.mode
    }

    /// Exact member length observed while capturing the member.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Content digest used by the deterministic artifact-generation identity.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }

    /// Stream the immutable captured member through one fixed-size buffer.
    pub fn copy_to(&self, target: &mut dyn Write) -> io::Result<u64> {
        let mut source = open_regular_file(&self.captured)?;
        let mut copied = 0_u64;
        let mut buffer = [0_u8; COPY_BUFFER_BYTES];
        loop {
            let read = source.read(&mut buffer)?;
            if read == 0 {
                return Ok(copied);
            }
            target.write_all(&buffer[..read])?;
            copied += read as u64;
        }
    }
}

impl BackupSource for BackupMember {
    fn path(&self) -> &str {
        self.path()
    }

    fn mode(&self) -> u32 {
        self.mode()
    }

    fn bytes(&self) -> u64 {
        self.bytes()
    }

    fn copy_to(&self, target: &mut dyn Write) -> io::Result<u64> {
        self.copy_to(target)
    }
}

impl PartialEq for BackupMember {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
            && self.mode == other.mode
            && self.bytes == other.bytes
            && self.digest == other.digest
    }
}

impl Eq for BackupMember {}

/// A bounded, provenance-validated artifact-store generation backed by private files.
#[derive(Debug)]
pub struct BackupGeneration {
    _generation: CapturedGeneration,
    members: Vec<BackupMember>,
    tasks: Vec<TaskStamp>,
    identity: Digest,
}

impl BackupGeneration {
    /// Every captured member in canonical path order.
    #[must_use]
    pub fn members(&self) -> &[BackupMember] {
        &self.members
    }

    /// Artifact-bearing tasks named by this captured generation.
    #[must_use]
    pub fn tasks(&self) -> &[TaskStamp] {
        &self.tasks
    }

    /// Deterministic identity over canonical member metadata and content digests.
    #[must_use]
    pub const fn identity(&self) -> &Digest {
        &self.identity
    }

    /// Capture and validate caller-authenticated streaming members.
    pub fn from_sources(sources: &[&dyn BackupSource]) -> Result<Self, Error> {
        if !within_member_bound(sources.len()) {
            return Err(Error::corrupt(
                "artifact backup inventory",
                format!("capture exceeds {JOURNAL_BACKUP_MAX_MEMBERS} members"),
            ));
        }
        let mut aggregate = 0_u64;
        let mut paths = BTreeSet::new();
        let mut plans = Vec::with_capacity(sources.len());
        for source in sources {
            let path = source.path().to_owned();
            let mode = source.mode();
            let bytes = source.bytes();
            admitted_relative(&path)?;
            if mode & !0o7777 != 0 || !paths.insert(path.clone()) {
                return Err(Error::corrupt(
                    path,
                    "backup member has an invalid mode or repeats a path",
                ));
            }
            aggregate = admit_backup_bytes(aggregate, bytes).ok_or_else(|| {
                Error::corrupt(
                    "artifact backup inventory",
                    format!("capture exceeds {JOURNAL_BACKUP_MAX_BYTES} aggregate bytes"),
                )
            })?;
            plans.push(SourcePlan {
                source: *source,
                path,
                mode,
                bytes,
            });
        }
        let generation = CapturedGeneration::create(&std::env::temp_dir())?;
        let mut members = Vec::with_capacity(plans.len());
        let mut observed = 0_u64;
        for (index, plan) in plans.iter().enumerate() {
            members.push(capture_source(plan, &generation, index, &mut observed)?);
        }
        if observed != aggregate {
            return Err(Error::corrupt(
                "artifact backup generation",
                format!("preflight declared {aggregate} bytes, captured {observed}"),
            ));
        }
        members.sort_by(|left, right| left.path.cmp(&right.path));
        generation.seal()?;
        let tasks = validate_captured_generation(&members)?;
        let identity = generation_identity(&members)?;
        Ok(Self {
            _generation: generation,
            members,
            tasks,
            identity,
        })
    }
}

fn capture_source(
    plan: &SourcePlan<'_>,
    generation: &CapturedGeneration,
    index: usize,
    aggregate: &mut u64,
) -> Result<BackupMember, Error> {
    let (captured, mut target) = generation.create_member(index)?;
    let copied = {
        let mut exact = ExactWriter {
            target: &mut target,
            remaining: plan.bytes,
        };
        let copied = plan.source.copy_to(&mut exact).map_err(|error| {
            if error.kind() == io::ErrorKind::InvalidData {
                Error::corrupt(&plan.path, error.to_string())
            } else {
                Error::io("capture streamed artifact member", &captured, error)
            }
        })?;
        if exact.remaining != 0 || copied != plan.bytes {
            return Err(Error::corrupt(
                &plan.path,
                "streamed artifact member length changed during capture",
            ));
        }
        copied
    };
    *aggregate = admit_backup_bytes(*aggregate, copied).ok_or_else(|| {
        Error::corrupt(
            "artifact backup generation",
            format!("capture crossed {JOURNAL_BACKUP_MAX_BYTES} aggregate bytes"),
        )
    })?;
    target
        .sync_all()
        .map_err(|error| Error::io("synchronize streamed artifact member", &captured, error))?;
    let (bytes, digest) = Digest::compute_reader(
        Domain::TaskArtifactPayload,
        open_regular_file(&captured)
            .map_err(|error| Error::io("open streamed artifact member", &captured, error))?,
        copied.max(1),
    )
    .map_err(|error| Error::io("digest streamed artifact member", &captured, error))?;
    if bytes != copied {
        return Err(Error::corrupt(
            &plan.path,
            "streamed artifact member changed while hashing",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        target
            .set_permissions(fs::Permissions::from_mode(0o400))
            .map_err(|error| Error::io("seal streamed artifact member", &captured, error))?;
    }
    Ok(BackupMember {
        path: plan.path.clone(),
        mode: plan.mode,
        bytes,
        digest,
        captured,
    })
}

struct ExactWriter<'a> {
    target: &'a mut File,
    remaining: u64,
}

impl Write for ExactWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "stream exceeded its declared artifact member length",
            ));
        }
        let written = self.target.write(bytes)?;
        self.remaining -= written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.target.flush()
    }
}

impl PartialEq for BackupGeneration {
    fn eq(&self, other: &Self) -> bool {
        self.members == other.members && self.tasks == other.tasks
    }
}

impl Eq for BackupGeneration {}

pub(super) fn capture(root: &Path) -> Result<BackupGeneration, Error> {
    let parent = root
        .parent()
        .ok_or_else(|| Error::refused("artifact root", "artifact root has no parent"))?;
    create_private_directories(parent)?;
    let generation = CapturedGeneration::create(parent)?;
    let mut plans = Vec::new();
    let mut preflight_bytes = 0_u64;
    if admitted_directory(root, true)? {
        for task in task_directories_with_limit(root, Some(JOURNAL_BACKUP_MAX_MEMBERS))? {
            let task_dir = root.join(task.as_str());
            let manifest = read_manifest(&task_dir, &task)?;
            validate_manifest_shape(
                &task,
                &manifest,
                &manifest_path(&task_dir).display().to_string(),
            )?;
            preflight_capture_member(
                root,
                &manifest_path(&task_dir),
                &mut plans,
                &mut preflight_bytes,
            )?;
            let mut digests = BTreeSet::new();
            for artifact in &manifest.artifacts {
                if digests.insert(artifact.digest.clone()) {
                    let object = object_dir(&task_dir, &artifact.digest);
                    preflight_capture_member(
                        root,
                        &object.join(METADATA),
                        &mut plans,
                        &mut preflight_bytes,
                    )?;
                    preflight_capture_member(
                        root,
                        &object.join(PAYLOAD),
                        &mut plans,
                        &mut preflight_bytes,
                    )?;
                }
            }
        }
    }
    let mut observed_bytes = 0_u64;
    let mut members = Vec::with_capacity(plans.len());
    for (index, plan) in plans.into_iter().enumerate() {
        members.push(capture_member(
            &plan,
            &generation,
            index,
            &mut observed_bytes,
        )?);
    }
    if observed_bytes != preflight_bytes {
        return Err(Error::corrupt(
            "artifact backup generation",
            format!("preflight declared {preflight_bytes} bytes, captured {observed_bytes}"),
        ));
    }
    members.sort_by(|left, right| left.path.cmp(&right.path));
    generation.seal()?;
    let tasks = validate_captured_generation(&members)?;
    let identity = generation_identity(&members)?;
    Ok(BackupGeneration {
        _generation: generation,
        members,
        tasks,
        identity,
    })
}

#[derive(Debug)]
pub(super) struct CapturePlan {
    source: PathBuf,
    path: String,
    mode: u32,
    identity: FileIdentity,
}

/// Whether a backup capture of `members` members stays within the shared bound.
pub(super) const fn within_member_bound(members: usize) -> bool {
    members <= JOURNAL_BACKUP_MAX_MEMBERS
}

/// The aggregate after adding one member's bytes, or `None` on overflow or a
/// total past the shared inclusive byte bound.
pub(super) fn admit_backup_bytes(aggregate: u64, bytes: u64) -> Option<u64> {
    aggregate
        .checked_add(bytes)
        .filter(|total| *total <= JOURNAL_BACKUP_MAX_BYTES)
}

fn preflight_capture_member(
    root: &Path,
    path: &Path,
    plans: &mut Vec<CapturePlan>,
    aggregate: &mut u64,
) -> Result<(), Error> {
    if !within_member_bound(plans.len() + 1) {
        return Err(Error::corrupt(
            "artifact backup inventory",
            format!("capture exceeds {JOURNAL_BACKUP_MAX_MEMBERS} members"),
        ));
    }
    let relative = path
        .strip_prefix(root)
        .expect("captured artifact is below root");
    let spelling = super::relative_spelling(relative)?;
    let file = match open_regular_file(path) {
        Ok(file) => file,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Err(Error::corrupt(
                path.display().to_string(),
                "referenced artifact member is missing",
            ));
        }
        Err(source) => return Err(Error::io("open artifact backup member", path, source)),
    };
    let metadata = file
        .metadata()
        .map_err(|source| Error::io("inspect artifact backup member", path, source))?;
    if !metadata.file_type().is_file() {
        return Err(Error::corrupt(
            path.display().to_string(),
            "artifact backup member is not a regular file",
        ));
    }
    let identity = FileIdentity::from_metadata(&metadata);
    *aggregate = admit_backup_bytes(*aggregate, identity.len).ok_or_else(|| {
        Error::corrupt(
            "artifact backup inventory",
            format!("capture exceeds {JOURNAL_BACKUP_MAX_BYTES} aggregate bytes"),
        )
    })?;
    plans.push(CapturePlan {
        source: path.to_path_buf(),
        path: spelling,
        mode: file_mode(path, &metadata)?,
        identity,
    });
    Ok(())
}

fn capture_member(
    plan: &CapturePlan,
    generation: &CapturedGeneration,
    index: usize,
    aggregate: &mut u64,
) -> Result<BackupMember, Error> {
    let mut source = open_regular_file(&plan.source)
        .map_err(|error| Error::io("open artifact backup member", &plan.source, error))?;
    let opened = source
        .metadata()
        .map_err(|error| Error::io("inspect artifact backup member", &plan.source, error))?;
    if FileIdentity::from_metadata(&opened) != plan.identity {
        return Err(changed_capture_source(&plan.source));
    }
    let (captured, mut target) = generation.create_member(index)?;
    let (bytes, digest) = if plan.identity.len == 0 {
        let mut probe = [0_u8; 1];
        if source
            .read(&mut probe)
            .map_err(|error| Error::io("read artifact backup member", &plan.source, error))?
            != 0
        {
            return Err(changed_capture_source(&plan.source));
        }
        (0, Digest::compute(Domain::TaskArtifactPayload, &[]))
    } else {
        Digest::copy_reader(
            Domain::TaskArtifactPayload,
            &mut source,
            &mut target,
            plan.identity.len,
        )
        .map_err(|error| {
            if error.kind() == io::ErrorKind::InvalidData {
                changed_capture_source(&plan.source)
            } else {
                Error::io("capture artifact backup member", &plan.source, error)
            }
        })?
    };
    *aggregate = admit_backup_bytes(*aggregate, bytes).ok_or_else(|| {
        Error::corrupt(
            "artifact backup generation",
            format!("capture crossed {JOURNAL_BACKUP_MAX_BYTES} aggregate bytes"),
        )
    })?;
    require_unchanged_capture_source(&plan.source, &source, &plan.identity, bytes)?;
    target
        .sync_all()
        .map_err(|error| Error::io("synchronize captured artifact member", &captured, error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        target
            .set_permissions(fs::Permissions::from_mode(0o400))
            .map_err(|error| Error::io("seal captured artifact member", &captured, error))?;
    }
    Ok(BackupMember {
        path: plan.path.clone(),
        mode: plan.mode,
        bytes,
        digest,
        captured,
    })
}

fn require_unchanged_capture_source(
    path: &Path,
    source: &File,
    expected: &FileIdentity,
    observed: u64,
) -> Result<(), Error> {
    let handle = source
        .metadata()
        .map_err(|error| Error::io("inspect captured artifact member", path, error))?;
    let reopened = open_regular_file(path)
        .and_then(|file| file.metadata())
        .map_err(|error| Error::io("reopen captured artifact member", path, error))?;
    if FileIdentity::from_metadata(&handle) == *expected
        && FileIdentity::from_metadata(&reopened) == *expected
        && observed == expected.len
    {
        Ok(())
    } else {
        Err(changed_capture_source(path))
    }
}

fn changed_capture_source(path: &Path) -> Error {
    Error::corrupt(
        path.display().to_string(),
        "artifact backup member changed while it was captured",
    )
}

fn validate_captured_generation(members: &[BackupMember]) -> Result<Vec<TaskStamp>, Error> {
    let mut map = BTreeMap::new();
    for member in members {
        admitted_relative(&member.path)?;
        if member.mode & !0o7777 != 0 {
            return Err(Error::corrupt(
                &member.path,
                format!(
                    "mode {:#o} is outside the POSIX permission mask",
                    member.mode
                ),
            ));
        }
        if map.insert(member.path.as_str(), member).is_some() {
            return Err(Error::corrupt(
                &member.path,
                "backup repeats an artifact member",
            ));
        }
    }
    let manifest_paths = map
        .keys()
        .copied()
        .filter(|path| path.ends_with("/manifest.json"))
        .collect::<Vec<_>>();
    let mut expected = BTreeSet::new();
    let mut tasks = Vec::new();
    for path in manifest_paths {
        let task_text = path.strip_suffix("/manifest.json").expect("matched suffix");
        if task_text.contains('/') {
            return Err(Error::corrupt(
                path,
                "task manifest is not one level below the root",
            ));
        }
        let task = parse_task_key(task_text, path)?;
        let manifest_bytes = read_regular(&map[path].captured, "captured artifact manifest")?;
        let manifest = decode_canonical::<Manifest>(&manifest_bytes, path)?;
        validate_manifest_shape(&task, &manifest, path)?;
        tasks.push(task.clone());
        expected.insert(path.to_owned());
        let mut digests = BTreeSet::new();
        for artifact in &manifest.artifacts {
            if !digests.insert(artifact.digest.clone()) {
                continue;
            }
            let base = format!("{task_text}/objects/{}", artifact.digest);
            let metadata_path = format!("{base}/{METADATA}");
            let payload_path = format!("{base}/{PAYLOAD}");
            let metadata_member = map.get(metadata_path.as_str()).ok_or_else(|| {
                Error::corrupt(&metadata_path, "referenced object metadata is missing")
            })?;
            let payload_member = map.get(payload_path.as_str()).ok_or_else(|| {
                Error::corrupt(&payload_path, "referenced artifact payload is missing")
            })?;
            let metadata_bytes =
                read_regular(&metadata_member.captured, "captured object metadata")?;
            let metadata = decode_canonical::<ObjectMetadata>(&metadata_bytes, &metadata_path)?;
            validate_object(&metadata_path, &metadata, artifact)?;
            validate_captured_payload(&payload_path, payload_member, artifact)?;
            expected.insert(metadata_path);
            expected.insert(payload_path);
        }
    }
    let observed = map
        .keys()
        .map(|path| (*path).to_owned())
        .collect::<BTreeSet<_>>();
    if observed != expected {
        return Err(Error::corrupt(
            "artifact backup inventory",
            format!("members {observed:?} do not equal cited inventory {expected:?}"),
        ));
    }
    Ok(tasks)
}

fn validate_captured_payload(
    subject: &str,
    member: &BackupMember,
    artifact: &Artifact,
) -> Result<(), Error> {
    if member.bytes != artifact.bytes || member.digest != artifact.digest {
        return Err(Error::corrupt(
            subject,
            format!(
                concat!(
                    "artifact {} declares {} bytes and digest {}, ",
                    "captured {} bytes and digest {}"
                ),
                artifact.id, artifact.bytes, artifact.digest, member.bytes, member.digest
            ),
        ));
    }
    if artifact.media_type.is_text() {
        validate_utf8(&member.captured, subject, &artifact.id)?;
    }
    Ok(())
}

fn validate_utf8(path: &Path, subject: &str, artifact_id: &str) -> Result<(), Error> {
    let mut file = open_regular_file(path)
        .map_err(|error| Error::io("open captured text artifact", path, error))?;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES + 3];
    let mut retained = 0;
    loop {
        let read = file
            .read(&mut buffer[retained..])
            .map_err(|error| Error::io("read captured text artifact", path, error))?;
        let length = retained + read;
        if length == 0 {
            return Ok(());
        }
        match std::str::from_utf8(&buffer[..length]) {
            Ok(_) => retained = 0,
            Err(error) if error.error_len().is_some() => {
                return Err(Error::corrupt(
                    subject,
                    format!("artifact {artifact_id} text is not UTF-8"),
                ));
            }
            Err(error) => {
                let valid = error.valid_up_to();
                retained = length - valid;
                if retained > 3 || read == 0 {
                    return Err(Error::corrupt(
                        subject,
                        format!("artifact {artifact_id} text is not UTF-8"),
                    ));
                }
                buffer.copy_within(valid..length, 0);
            }
        }
        if read == 0 {
            return Ok(());
        }
    }
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct GenerationIdentityMember<'a> {
    path: &'a str,
    mode: u32,
    bytes: u64,
    digest: &'a Digest,
}

fn generation_identity(members: &[BackupMember]) -> Result<Digest, Error> {
    let projection = members
        .iter()
        .map(|member| GenerationIdentityMember {
            path: &member.path,
            mode: member.mode,
            bytes: member.bytes,
            digest: &member.digest,
        })
        .collect::<Vec<_>>();
    let bytes = wire::encode(&projection).map_err(|error| {
        Error::corrupt(
            "artifact backup generation",
            format!("identity does not encode: {error}"),
        )
    })?;
    Ok(Digest::compute(Domain::TaskArtifactStore, &bytes))
}

#[derive(Debug)]
struct CapturedGeneration {
    root: PathBuf,
}

impl CapturedGeneration {
    fn create(parent: &Path) -> Result<Self, Error> {
        create_replacement_directory(parent, "capture").map(|root| Self { root })
    }

    fn create_member(&self, index: usize) -> Result<(PathBuf, File), Error> {
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt;

        let path = self.root.join(index.to_string());
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(&path)
            .map_err(|error| Error::io("create captured artifact member", &path, error))?;
        Ok((path, file))
    }

    fn seal(&self) -> Result<(), Error> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.root, fs::Permissions::from_mode(0o500))
                .map_err(|error| Error::io("seal artifact capture", &self.root, error))?;
        }
        Ok(())
    }
}

impl Drop for CapturedGeneration {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700));
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    len: u64,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    mode: u32,
    #[cfg(unix)]
    modified_seconds: i64,
    #[cfg(unix)]
    modified_nanoseconds: i64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
}

impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        Self {
            len: metadata.len(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            mode: metadata.mode(),
            #[cfg(unix)]
            modified_seconds: metadata.mtime(),
            #[cfg(unix)]
            modified_nanoseconds: metadata.mtime_nsec(),
            #[cfg(unix)]
            changed_seconds: metadata.ctime(),
            #[cfg(unix)]
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }
}

fn file_mode(_path: &Path, metadata: &Metadata) -> Result<u32, Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(metadata.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Err(Error::environment(
            "read artifact member mode",
            _path.display().to_string(),
        ))
    }
}
