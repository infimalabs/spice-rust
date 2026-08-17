//! Lossless local quarantine for an explicitly recovered dirty handoff.
//!
//! This module owns only the Git/filesystem half of recovery. It snapshots one
//! exact dirty generation, makes that generation clean by touching only named
//! paths, advances through [`super::prepare_claim_observed`], and can either
//! restore the pre-claim generation or replay it on the observed synchronized
//! HEAD. Board possession is deliberately left to the command transaction that
//! drives these operations.

use super::*;

use std::collections::BTreeSet;
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt as _;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};

const RECOVERY_DIRECTORY: &str = "spice-task-recovery";
const MANIFEST_FILE: &str = "manifest.json";
const OBJECT_DIRECTORY: &str = "objects";
const RECOVERY_VERSION: u32 = 1;

mod storage;
use storage::*;

/// Opaque identity of one durable dirty-handoff quarantine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recovery {
    transaction: String,
}

impl Recovery {
    /// Content address of the sealed original generation.
    #[must_use]
    pub fn transaction(&self) -> &str {
        &self.transaction
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum RecoveryState {
    Captured,
    Quarantined,
    Syncing {
        boundary: GitBoundary,
    },
    Synchronized {
        boundary: GitBoundary,
    },
    Replaying {
        boundary: GitBoundary,
    },
    Conflict {
        boundary: GitBoundary,
        detail: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    transaction: String,
    state: RecoveryState,
    payload: Payload,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    repository: RepositorySeal,
    reference: String,
    head: String,
    configuration: String,
    index: StoredBlob,
    index_listing: String,
    staged_patch: StoredBlob,
    unstaged_patch: StoredBlob,
    staged_paths: Vec<String>,
    unstaged_paths: Vec<String>,
    tracked: Vec<PathState>,
    untracked: Vec<PathState>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RepositorySeal {
    worktree: String,
    git: String,
    common: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredBlob {
    digest: String,
    length: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PathState {
    path: String,
    entry: Option<StoredEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredEntry {
    kind: EntryKind,
    mode: u32,
    content: StoredBlob,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum EntryKind {
    Regular,
    Symlink,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ObservedEntry {
    kind: EntryKind,
    mode: u32,
    digest: String,
    length: u64,
}

/// Return the current private recovery, if this worktree owns one.
pub fn pending_recovery(repository: &Repository) -> Result<Option<Recovery>, Error> {
    let root = recovery_root(repository);
    match fs::symlink_metadata(&root) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            let manifest = read_manifest(&root)?;
            verify_manifest_identity(repository, &manifest)?;
            Ok(Some(recovery(&manifest)))
        }
        Ok(_) => Err(recovery_refusal(
            "git::recovery-state-unsafe",
            format!("{} is not a private directory", root.display()),
            "move the unexpected recovery path aside and retry",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(recovery_filesystem("inspect recovery state", &root, error)),
    }
}

/// Seal and remove exactly one dirty generation from the visible lane.
pub fn quarantine_for_recovery(repository: &Repository) -> Result<Recovery, Error> {
    if let Some(recovery) = pending_recovery(repository)? {
        return resume_quarantine(repository, recovery);
    }
    let root = create_recovery_root(repository)?;
    let payload = match capture_payload(repository, &root) {
        Ok(payload) => payload,
        Err(error) => {
            discard_incomplete(&root);
            return Err(error);
        }
    };
    let transaction = digest_json(&payload)?;
    let mut manifest = Manifest {
        version: RECOVERY_VERSION,
        transaction,
        state: RecoveryState::Captured,
        payload,
    };
    write_manifest(&root, &manifest)?;
    verify_original(repository, &root, &manifest)?;
    clean_manifest_paths(repository.worktree_root(), &manifest.payload)?;
    manifest.state = RecoveryState::Quarantined;
    write_manifest(&root, &manifest)?;
    Ok(recovery(&manifest))
}

#[cfg(test)]
pub(super) fn capture_recovery_for_test(repository: &Repository) -> Result<Recovery, Error> {
    let root = create_recovery_root(repository)?;
    let payload = capture_payload(repository, &root)?;
    let transaction = digest_json(&payload)?;
    let manifest = Manifest {
        version: RECOVERY_VERSION,
        transaction,
        state: RecoveryState::Captured,
        payload,
    };
    write_manifest(&root, &manifest)?;
    Ok(recovery(&manifest))
}

#[cfg(test)]
pub(super) fn finish_recovery_capture_for_test(
    repository: &Repository,
    recovery: &Recovery,
) -> Result<(), Error> {
    let root = recovery_root(repository);
    let mut manifest = read_expected(&root, recovery)?;
    verify_original(repository, &root, &manifest)?;
    clean_manifest_paths(repository.worktree_root(), &manifest.payload)?;
    manifest.state = RecoveryState::Quarantined;
    write_manifest(&root, &manifest)
}

#[cfg(test)]
pub(super) fn journal_syncing_for_test(
    repository: &Repository,
    recovery: &Recovery,
    boundary: GitBoundary,
) -> Result<(), Error> {
    let root = recovery_root(repository);
    let mut manifest = read_expected(&root, recovery)?;
    manifest.state = RecoveryState::Syncing { boundary };
    write_manifest(&root, &manifest)
}

#[cfg(test)]
pub(super) fn journal_replaying_for_test(
    repository: &Repository,
    recovery: &Recovery,
) -> Result<(), Error> {
    let root = recovery_root(repository);
    let mut manifest = read_expected(&root, recovery)?;
    let boundary = replay_boundary(&manifest)?;
    manifest.state = RecoveryState::Replaying { boundary };
    write_manifest(&root, &manifest)
}

fn resume_quarantine(repository: &Repository, recovery: Recovery) -> Result<Recovery, Error> {
    let root = recovery_root(repository);
    let manifest = read_expected(&root, &recovery)?;
    match manifest.state {
        RecoveryState::Captured => Err(recovery_refusal(
            "git::recovery-capture-interrupted",
            format!("recovery {} stopped while cleaning", recovery.transaction),
            "retry explicit recovery to restore the sealed original state first",
        )),
        RecoveryState::Quarantined
        | RecoveryState::Syncing { .. }
        | RecoveryState::Synchronized { .. }
        | RecoveryState::Replaying { .. }
        | RecoveryState::Conflict { .. } => Ok(recovery),
    }
}

/// Advance the quarantined clean lane through the ordinary claim boundary.
pub fn synchronize_recovery(
    repository: &Repository,
    recovery: &Recovery,
) -> Result<GitBoundary, Error> {
    let root = recovery_root(repository);
    let manifest = read_expected(&root, recovery)?;
    verify_synchronization_identity(repository, &manifest)?;
    match manifest.state.clone() {
        RecoveryState::Synchronized { boundary }
        | RecoveryState::Replaying { boundary }
        | RecoveryState::Conflict { boundary, .. } => return Ok(boundary),
        RecoveryState::Syncing { boundary } => {
            return resume_synchronization(repository, &root, manifest, boundary);
        }
        RecoveryState::Captured => {
            return Err(interrupted_capture(recovery));
        }
        RecoveryState::Quarantined => {}
    }
    verify_text(
        "pre-synchronization HEAD",
        &manifest.payload.head,
        &current_head(repository.worktree_root())?,
    )?;
    require_clean(repository.worktree_root(), "task recovery synchronization")?;
    let sealed = manifest.clone();
    let mut journaled = manifest;
    let boundary = prepare_claim_observed(
        repository,
        || verify_synchronization_identity(repository, &sealed),
        |boundary| {
            journaled.state = RecoveryState::Syncing {
                boundary: boundary.clone(),
            };
            write_manifest(&root, &journaled)
        },
    )?;
    journaled.state = RecoveryState::Synchronized {
        boundary: boundary.clone(),
    };
    write_manifest(&root, &journaled)?;
    Ok(boundary)
}

fn verify_synchronization_identity(
    repository: &Repository,
    manifest: &Manifest,
) -> Result<(), Error> {
    verify_manifest_identity(repository, manifest)?;
    verify_text(
        "symbolic ref",
        &manifest.payload.reference,
        &required_read(
            repository.worktree_root(),
            &["symbolic-ref", "HEAD"],
            "verify synchronization symbolic HEAD",
        )?,
    )?;
    verify_text(
        "configuration",
        &manifest.payload.configuration,
        &configuration(repository.worktree_root())?,
    )
}

fn resume_synchronization(
    repository: &Repository,
    root: &Path,
    mut manifest: Manifest,
    boundary: GitBoundary,
) -> Result<GitBoundary, Error> {
    recover_pending(repository.worktree_root())?;
    let head = current_head(repository.worktree_root())?;
    if head != manifest.payload.head && head != boundary.head {
        return Err(recovery_refusal(
            "git::recovery-sync-ambiguous",
            format!(
                "recovery expected HEAD {} or {}, but observed {head}",
                manifest.payload.head, boundary.head
            ),
            "inspect the retained quarantine and moved branch before retrying",
        ));
    }
    if head == manifest.payload.head {
        let sealed = manifest.clone();
        let resumed = prepare_claim_observed(
            repository,
            || verify_synchronization_identity(repository, &sealed),
            |_| Ok(()),
        )?;
        if resumed != boundary {
            return Err(recovery_refusal(
                "git::recovery-baseline-moved",
                format!("sealed boundary {boundary:?}; resumed boundary {resumed:?}"),
                "restore the quarantine and retry against one stable baseline",
            ));
        }
    }
    manifest.state = RecoveryState::Synchronized {
        boundary: boundary.clone(),
    };
    write_manifest(root, &manifest)?;
    Ok(boundary)
}

/// Restore the exact pre-claim dirty generation and retire its quarantine.
pub fn restore_recovery(repository: &Repository, recovery: &Recovery) -> Result<(), Error> {
    let root = recovery_root(repository);
    let manifest = read_expected(&root, recovery)?;
    refuse_post_replay_restore(&manifest)?;
    recover_pending(repository.worktree_root())?;
    verify_rollback_surface(repository, &manifest)?;
    clean_manifest_paths(repository.worktree_root(), &manifest.payload)?;
    restore_original_head(repository.worktree_root(), &manifest)?;
    restore_index(repository.worktree_root(), &root, &manifest.payload.index)?;
    restore_paths(repository.worktree_root(), &root, &manifest.payload.tracked)?;
    restore_paths(
        repository.worktree_root(),
        &root,
        &manifest.payload.untracked,
    )?;
    verify_original(repository, &root, &manifest)?;
    retire_recovery(&root)
}

fn refuse_post_replay_restore(manifest: &Manifest) -> Result<(), Error> {
    match &manifest.state {
        RecoveryState::Replaying { .. } | RecoveryState::Conflict { .. } => Err(recovery_refusal(
            "git::recovery-already-claimed",
            "replay has begun and may contain claimed conflict state",
            "resume replay under the exact active claim instead of rolling back",
        )),
        RecoveryState::Captured
        | RecoveryState::Quarantined
        | RecoveryState::Syncing { .. }
        | RecoveryState::Synchronized { .. } => Ok(()),
    }
}

/// Replay a synchronized quarantine, retaining all data on any conflict.
pub fn replay_recovery(repository: &Repository, recovery: &Recovery) -> Result<(), Error> {
    let root = recovery_root(repository);
    let mut manifest = read_expected(&root, recovery)?;
    let resuming = matches!(manifest.state, RecoveryState::Replaying { .. });
    let boundary = replay_boundary(&manifest)?;
    if resuming {
        resume_replay_surface(repository, &manifest, &boundary)?;
    } else {
        verify_replay_surface(repository, &manifest, &boundary)?;
    }
    refuse_untracked_collisions(repository.worktree_root(), &manifest.payload.untracked)?;
    manifest.state = RecoveryState::Replaying {
        boundary: boundary.clone(),
    };
    write_manifest(&root, &manifest)?;
    apply_replay_patches(repository.worktree_root(), &root, &mut manifest, &boundary)?;
    restore_paths(
        repository.worktree_root(),
        &root,
        &manifest.payload.untracked,
    )?;
    verify_replayed(repository, &root, &manifest, &boundary)?;
    retire_recovery(&root)
}

fn replay_boundary(manifest: &Manifest) -> Result<GitBoundary, Error> {
    match &manifest.state {
        RecoveryState::Synchronized { boundary } | RecoveryState::Replaying { boundary } => {
            Ok(boundary.clone())
        }
        RecoveryState::Conflict {
            boundary: _,
            detail,
        } => Err(recovery_refusal(
            "git::recovery-conflict-retained",
            detail.clone(),
            "inspect the conflict and retained quarantine before continuing",
        )),
        RecoveryState::Captured => Err(recovery_refusal(
            "git::recovery-capture-interrupted",
            "the dirty snapshot did not reach a clean quarantine",
            "restore the sealed original generation before retrying",
        )),
        RecoveryState::Quarantined | RecoveryState::Syncing { .. } => Err(recovery_refusal(
            "git::recovery-not-synchronized",
            "the clean lane has no completed synchronized boundary",
            "synchronize this recovery before replaying it",
        )),
    }
}

fn apply_replay_patches(
    worktree: &Path,
    root: &Path,
    manifest: &mut Manifest,
    boundary: &GitBoundary,
) -> Result<(), Error> {
    let staged = read_blob(root, &manifest.payload.staged_patch)?;
    apply_patch_layer(
        worktree,
        &["apply", "--index", "--3way", "--whitespace=nowarn"],
        &staged,
        root,
        manifest,
        boundary,
        "staged",
    )?;
    let unstaged = read_blob(root, &manifest.payload.unstaged_patch)?;
    apply_patch_layer(
        worktree,
        &["apply", "--whitespace=nowarn"],
        &unstaged,
        root,
        manifest,
        boundary,
        "unstaged",
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "one patch failure journals its complete replay boundary"
)]
fn apply_patch_layer(
    worktree: &Path,
    args: &[&str],
    patch: &[u8],
    root: &Path,
    manifest: &mut Manifest,
    boundary: &GitBoundary,
    layer: &str,
) -> Result<(), Error> {
    if patch.is_empty() {
        return Ok(());
    }
    let applied = run_with_input(worktree, args, patch)?;
    if applied.success() {
        return Ok(());
    }
    let detail = applied.detail(&format!("replay {layer} recovery layer"));
    manifest.state = RecoveryState::Conflict {
        boundary: boundary.clone(),
        detail: detail.clone(),
    };
    write_manifest(root, manifest)?;
    Err(recovery_refusal(
        "git::recovery-replay-conflict",
        detail,
        "resolve the inspectable index/worktree under the active claim; the quarantine is retained",
    ))
}

fn capture_payload(repository: &Repository, root: &Path) -> Result<Payload, Error> {
    let worktree = repository.worktree_root();
    refuse_unsupported_tree_entries(worktree)?;
    let staged_paths = changed_paths(worktree, &["diff", "--name-only", "-z", "--cached"])?;
    let unstaged_paths = changed_paths(worktree, &["diff", "--name-only", "-z"])?;
    let tracked_paths = union_paths(&staged_paths, &unstaged_paths);
    let untracked_paths = changed_paths(
        worktree,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    if tracked_paths.is_empty() && untracked_paths.is_empty() {
        return Err(recovery_refusal(
            "git::recovery-clean",
            "explicit dirty-handoff recovery found no dirty paths",
            "use ordinary task claim on this clean lane",
        ));
    }
    let index_path = git_state_path(worktree, "index")?;
    let index_bytes = read_required(&index_path, "capture exact Git index")?;
    Ok(Payload {
        repository: repository_seal(repository)?,
        reference: required_read(worktree, &["symbolic-ref", "HEAD"], "read symbolic HEAD")?,
        head: current_head(worktree)?,
        configuration: configuration(worktree)?,
        index: store_blob(root, &index_bytes)?,
        index_listing: index_listing(worktree)?,
        staged_patch: store_git_output(
            root,
            worktree,
            &[
                "diff",
                "--binary",
                "--full-index",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "--cached",
                "HEAD",
            ],
            "capture staged patch",
        )?,
        unstaged_patch: store_git_output(
            root,
            worktree,
            &[
                "diff",
                "--binary",
                "--full-index",
                "--src-prefix=a/",
                "--dst-prefix=b/",
            ],
            "capture unstaged patch",
        )?,
        staged_paths,
        unstaged_paths,
        tracked: capture_paths(worktree, root, &tracked_paths, true)?,
        untracked: capture_paths(worktree, root, &untracked_paths, false)?,
    })
}

fn refuse_unsupported_tree_entries(worktree: &Path) -> Result<(), Error> {
    let mut pending = vec![worktree.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|error| {
            recovery_filesystem("enumerate recovery worktree", &directory, error)
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                recovery_filesystem("read recovery worktree entry", &directory, error)
            })?;
            inspect_tree_entry(worktree, &entry.path(), &mut pending)?;
        }
    }
    Ok(())
}

fn inspect_tree_entry(
    worktree: &Path,
    path: &Path,
    pending: &mut Vec<PathBuf>,
) -> Result<(), Error> {
    let relative = path.strip_prefix(worktree).map_err(|error| {
        recovery_refusal(
            "git::recovery-path-escape",
            error.to_string(),
            "inspect the worktree root before retrying",
        )
    })?;
    if relative == Path::new(".git") {
        return Ok(());
    }
    let written = path_text(relative)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| recovery_filesystem("inspect recovery worktree entry", path, error))?;
    let kind = metadata.file_type();
    if kind.is_file() || kind.is_symlink() {
        return Ok(());
    }
    if ignored_path(worktree, &written)? {
        return Ok(());
    }
    if kind.is_dir() {
        if fs::symlink_metadata(path.join(".git")).is_ok() {
            return Err(recovery_refusal(
                "git::recovery-nested-repository",
                format!("unignored directory {written:?} contains nested Git authority"),
                "move the nested repository out of the handoff before retrying",
            ));
        }
        pending.push(path.to_path_buf());
        return Ok(());
    }
    Err(recovery_refusal(
        "git::recovery-file-kind",
        format!("unignored path {written:?} is a socket, device, or FIFO"),
        "replace unsupported filesystem objects with regular files before retrying",
    ))
}

fn ignored_path(worktree: &Path, path: &str) -> Result<bool, Error> {
    let output = run(worktree, &["check-ignore", "--quiet", "--", path])?;
    match output.status {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(Error::fault(
            "git::recovery-inspection-failed",
            output.detail(&format!("inspect ignore policy for {path}")),
        )),
    }
}

fn capture_paths(
    worktree: &Path,
    root: &Path,
    paths: &[String],
    absence_allowed: bool,
) -> Result<Vec<PathState>, Error> {
    paths
        .iter()
        .map(|path| {
            let entry = capture_entry(worktree, root, path)?;
            if entry.is_none() && !absence_allowed {
                return Err(recovery_refusal(
                    "git::recovery-untracked-moved",
                    format!("untracked path {path:?} disappeared during capture"),
                    "retry after the dirty footprint stops moving",
                ));
            }
            Ok(PathState {
                path: path.clone(),
                entry,
            })
        })
        .collect()
}

fn capture_entry(
    worktree: &Path,
    root: &Path,
    written: &str,
) -> Result<Option<StoredEntry>, Error> {
    let path = admitted_path(worktree, written)?;
    let Some((kind, mode, bytes)) = read_entry_bytes(&path, written)? else {
        return Ok(None);
    };
    Ok(Some(StoredEntry {
        kind,
        mode,
        content: store_blob(root, &bytes)?,
    }))
}

fn read_entry_bytes(
    path: &Path,
    written: &str,
) -> Result<Option<(EntryKind, u32, Vec<u8>)>, Error> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(recovery_filesystem("inspect dirty path", path, error)),
    };
    let kind = metadata.file_type();
    if kind.is_dir() {
        let nested = path.join(".git");
        let reason = if fs::symlink_metadata(&nested).is_ok() {
            "git::recovery-nested-repository"
        } else {
            "git::recovery-file-kind"
        };
        return Err(recovery_refusal(
            reason,
            format!("dirty path {written:?} is a directory"),
            "move unsupported directory content out of the handoff and retry",
        ));
    }
    #[cfg(unix)]
    {
        if kind.is_file() {
            return Ok(Some((
                EntryKind::Regular,
                metadata.mode(),
                read_required(path, "capture dirty file")?,
            )));
        }
        if kind.is_symlink() {
            let target = fs::read_link(path)
                .map_err(|error| recovery_filesystem("read dirty symlink", path, error))?;
            return Ok(Some((
                EntryKind::Symlink,
                metadata.mode(),
                target.into_os_string().into_vec(),
            )));
        }
    }
    Err(recovery_refusal(
        "git::recovery-file-kind",
        format!("dirty path {written:?} has an unsupported file kind"),
        "replace sockets, devices, or FIFOs with regular files before retrying",
    ))
}

fn changed_paths(worktree: &Path, args: &[&str]) -> Result<Vec<String>, Error> {
    let output = run(worktree, args)?;
    if !output.success() {
        return Err(Error::fault(
            "git::recovery-inspection-failed",
            output.detail("enumerate dirty paths"),
        ));
    }
    parse_nul_paths(&output.stdout)
}

fn parse_nul_paths(output: &str) -> Result<Vec<String>, Error> {
    if !output.is_empty() && !output.ends_with('\0') {
        return Err(Error::fault(
            "git::recovery-path-list-invalid",
            "Git returned an unterminated dirty path list",
        ));
    }
    let mut paths = output
        .split_terminator('\0')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for path in &paths {
        validate_relative(path)?;
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn validate_relative(written: &str) -> Result<(), Error> {
    let path = Path::new(written);
    let admitted = !written.is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if admitted {
        Ok(())
    } else {
        Err(recovery_refusal(
            "git::recovery-path-escape",
            format!("dirty path {written:?} is not a normalized repository-relative path"),
            "rename the path inside the repository before retrying",
        ))
    }
}

fn admitted_path(worktree: &Path, written: &str) -> Result<PathBuf, Error> {
    validate_relative(written)?;
    let relative = Path::new(written);
    let mut cursor = worktree.to_path_buf();
    let components = relative.components().collect::<Vec<_>>();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        let Component::Normal(name) = component else {
            unreachable!("validate_relative admitted only normal components");
        };
        cursor.push(name);
        let metadata = fs::symlink_metadata(&cursor)
            .map_err(|error| recovery_filesystem("inspect dirty path parent", &cursor, error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(recovery_refusal(
                "git::recovery-symlink-traversal",
                format!(
                    "dirty path {written:?} crosses unsafe parent {}",
                    cursor.display()
                ),
                "replace the parent with a repository-local directory before retrying",
            ));
        }
    }
    Ok(worktree.join(relative))
}

fn union_paths(left: &[String], right: &[String]) -> Vec<String> {
    left.iter()
        .chain(right)
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn verify_original(repository: &Repository, root: &Path, manifest: &Manifest) -> Result<(), Error> {
    refuse_unsupported_tree_entries(repository.worktree_root())?;
    verify_manifest_identity(repository, manifest)?;
    let payload = &manifest.payload;
    verify_text(
        "symbolic ref",
        &payload.reference,
        &required_read(
            repository.worktree_root(),
            &["symbolic-ref", "HEAD"],
            "verify symbolic HEAD",
        )?,
    )?;
    verify_text(
        "HEAD",
        &payload.head,
        &current_head(repository.worktree_root())?,
    )?;
    verify_text(
        "configuration",
        &payload.configuration,
        &configuration(repository.worktree_root())?,
    )?;
    verify_index(repository.worktree_root(), root, payload)?;
    verify_path_sets(repository.worktree_root(), payload)?;
    verify_paths(repository.worktree_root(), root, &payload.tracked)?;
    verify_paths(repository.worktree_root(), root, &payload.untracked)
}

fn verify_manifest_identity(repository: &Repository, manifest: &Manifest) -> Result<(), Error> {
    if manifest.version != RECOVERY_VERSION {
        return Err(recovery_refusal(
            "git::recovery-version",
            format!("unsupported recovery manifest version {}", manifest.version),
            "use the Spice build that created this retained quarantine",
        ));
    }
    let expected = repository_seal(repository)?;
    if manifest.payload.repository != expected {
        return Err(recovery_refusal(
            "git::recovery-repository-moved",
            format!(
                "sealed {:?}; observed {expected:?}",
                manifest.payload.repository
            ),
            "run recovery only from the worktree that created the quarantine",
        ));
    }
    let computed = digest_json(&manifest.payload)?;
    verify_text("transaction digest", &manifest.transaction, &computed)
}

fn verify_index(worktree: &Path, root: &Path, payload: &Payload) -> Result<(), Error> {
    let index_path = git_state_path(worktree, "index")?;
    let bytes = read_required(&index_path, "verify exact Git index")?;
    verify_blob_bytes(root, &payload.index, &bytes)?;
    verify_text(
        "index listing",
        &payload.index_listing,
        &index_listing(worktree)?,
    )
}

fn verify_path_sets(worktree: &Path, payload: &Payload) -> Result<(), Error> {
    let staged = changed_paths(worktree, &["diff", "--name-only", "-z", "--cached"])?;
    let unstaged = changed_paths(worktree, &["diff", "--name-only", "-z"])?;
    let untracked = changed_paths(
        worktree,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    verify_list("staged paths", &payload.staged_paths, &staged)?;
    verify_list("unstaged paths", &payload.unstaged_paths, &unstaged)?;
    verify_list(
        "untracked paths",
        &path_names(&payload.untracked),
        &untracked,
    )
}

fn verify_paths(worktree: &Path, root: &Path, paths: &[PathState]) -> Result<(), Error> {
    for state in paths {
        let observed = observe_entry(worktree, &state.path)?;
        let expected = state.entry.as_ref().map(observation);
        if observed != expected {
            return Err(snapshot_moved(&state.path, &expected, &observed));
        }
        if let Some(entry) = &state.entry {
            let _ = read_blob(root, &entry.content)?;
        }
    }
    Ok(())
}

fn observe_entry(worktree: &Path, written: &str) -> Result<Option<ObservedEntry>, Error> {
    let path = admitted_path(worktree, written)?;
    let Some((kind, mode, bytes)) = read_entry_bytes(&path, written)? else {
        return Ok(None);
    };
    Ok(Some(ObservedEntry {
        kind,
        mode,
        digest: digest_bytes(&bytes),
        length: bytes.len() as u64,
    }))
}

fn observation(entry: &StoredEntry) -> ObservedEntry {
    ObservedEntry {
        kind: entry.kind,
        mode: entry.mode,
        digest: entry.content.digest.clone(),
        length: entry.content.length,
    }
}

fn clean_manifest_paths(worktree: &Path, payload: &Payload) -> Result<(), Error> {
    restore_tracked_to_head(worktree, &payload.tracked)?;
    for state in &payload.untracked {
        if !path_is_tracked(worktree, &state.path)? {
            remove_named_path(worktree, &state.path)?;
        }
    }
    require_clean(worktree, "task recovery quarantine")
}

fn path_is_tracked(worktree: &Path, path: &str) -> Result<bool, Error> {
    let output = run(worktree, &["ls-files", "--error-unmatch", "--", path])?;
    match output.status {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(Error::fault(
            "git::recovery-inspection-failed",
            output.detail(&format!("inspect index ownership of {path}")),
        )),
    }
}

fn restore_tracked_to_head(worktree: &Path, paths: &[PathState]) -> Result<(), Error> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut owned = vec![
        "restore".to_owned(),
        "--source=HEAD".to_owned(),
        "--staged".to_owned(),
        "--worktree".to_owned(),
        "--".to_owned(),
    ];
    owned.extend(paths.iter().map(|state| state.path.clone()));
    let args = owned.iter().map(String::as_str).collect::<Vec<_>>();
    let restored = run(worktree, &args)?;
    if restored.success() {
        Ok(())
    } else {
        Err(Error::fault(
            "git::recovery-clean-failed",
            restored.detail("restore manifest paths to HEAD"),
        ))
    }
}

fn remove_named_path(worktree: &Path, written: &str) -> Result<(), Error> {
    let path = admitted_path(worktree, written)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
            fs::remove_file(&path)
                .map_err(|error| recovery_filesystem("quarantine named path", &path, error))
        }
        Ok(_) => Err(recovery_refusal(
            "git::recovery-file-kind",
            format!("named quarantine path {written:?} became a directory or special file"),
            "restore the sealed generation before retrying",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(recovery_filesystem(
            "inspect named quarantine path",
            &path,
            error,
        )),
    }
}

fn verify_rollback_surface(repository: &Repository, manifest: &Manifest) -> Result<(), Error> {
    refuse_unsupported_tree_entries(repository.worktree_root())?;
    verify_manifest_identity(repository, manifest)?;
    verify_text(
        "symbolic ref",
        &manifest.payload.reference,
        &required_read(
            repository.worktree_root(),
            &["symbolic-ref", "HEAD"],
            "verify symbolic HEAD",
        )?,
    )?;
    verify_text(
        "configuration",
        &manifest.payload.configuration,
        &configuration(repository.worktree_root())?,
    )?;
    refuse_unsealed_dirty_paths(
        repository.worktree_root(),
        &manifest.payload,
        "git::recovery-rollback-drift",
        "preserve the retained quarantine and inspect concurrent lane edits",
    )
}

fn refuse_unsealed_dirty_paths(
    worktree: &Path,
    payload: &Payload,
    reason: &'static str,
    remedy: &'static str,
) -> Result<(), Error> {
    let current_dirty = current_dirty_paths(worktree)?;
    let admitted = union_paths(
        &path_names(&payload.tracked),
        &path_names(&payload.untracked),
    )
    .into_iter()
    .collect::<BTreeSet<_>>();
    let unexpected = current_dirty
        .difference(&admitted)
        .cloned()
        .collect::<Vec<_>>();
    if unexpected.is_empty() {
        Ok(())
    } else {
        Err(recovery_refusal(
            reason,
            format!("unsealed dirty paths appeared: {}", unexpected.join(", ")),
            remedy,
        ))
    }
}

fn current_dirty_paths(worktree: &Path) -> Result<BTreeSet<String>, Error> {
    let staged = changed_paths(worktree, &["diff", "--name-only", "-z", "--cached"])?;
    let unstaged = changed_paths(worktree, &["diff", "--name-only", "-z"])?;
    let untracked = changed_paths(
        worktree,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    Ok(staged
        .into_iter()
        .chain(unstaged)
        .chain(untracked)
        .collect())
}

fn restore_original_head(worktree: &Path, manifest: &Manifest) -> Result<(), Error> {
    let current = current_head(worktree)?;
    if current == manifest.payload.head {
        return Ok(());
    }
    let allowed = match &manifest.state {
        RecoveryState::Syncing { boundary } | RecoveryState::Synchronized { boundary } => {
            current == boundary.head
        }
        RecoveryState::Captured | RecoveryState::Quarantined => false,
        RecoveryState::Replaying { .. } | RecoveryState::Conflict { .. } => false,
    };
    if !allowed {
        return Err(recovery_refusal(
            "git::recovery-head-moved",
            format!("sealed HEAD {}; observed {current}", manifest.payload.head),
            "inspect the retained quarantine and moved branch before retrying",
        ));
    }
    materialize_and_update(
        worktree,
        &manifest.payload.head,
        &current,
        "restore pre-claim recovery HEAD",
    )
}

fn restore_index(worktree: &Path, root: &Path, blob: &StoredBlob) -> Result<(), Error> {
    let bytes = read_blob(root, blob)?;
    let path = git_state_path(worktree, "index")?;
    atomic_bytes(&path, &bytes, "restore exact Git index")
}

fn restore_paths(worktree: &Path, root: &Path, paths: &[PathState]) -> Result<(), Error> {
    for state in paths {
        remove_named_path(worktree, &state.path)?;
        if let Some(entry) = &state.entry {
            write_entry(worktree, root, &state.path, entry)?;
        }
    }
    Ok(())
}

fn write_entry(
    worktree: &Path,
    root: &Path,
    written: &str,
    entry: &StoredEntry,
) -> Result<(), Error> {
    ensure_parents(worktree, written)?;
    let path = worktree.join(written);
    let bytes = read_blob(root, &entry.content)?;
    match entry.kind {
        EntryKind::Regular => write_regular(&path, &bytes, entry.mode),
        EntryKind::Symlink => write_symlink(&path, &bytes),
    }
}

fn ensure_parents(worktree: &Path, written: &str) -> Result<(), Error> {
    validate_relative(written)?;
    let mut cursor = worktree.to_path_buf();
    let components = Path::new(written).components().collect::<Vec<_>>();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        let Component::Normal(name) = component else {
            unreachable!("validated path contains only normal components");
        };
        cursor.push(name);
        match fs::symlink_metadata(&cursor) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(recovery_refusal(
                    "git::recovery-symlink-traversal",
                    format!("replay parent {} is not a real directory", cursor.display()),
                    "inspect the collision and retained quarantine",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&cursor)
                    .map_err(|error| recovery_filesystem("create replay parent", &cursor, error))?
            }
            Err(error) => return Err(recovery_filesystem("inspect replay parent", &cursor, error)),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn write_regular(path: &Path, bytes: &[u8], mode: u32) -> Result<(), Error> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(mode & 0o7777);
    let mut file = options
        .open(path)
        .map_err(|error| recovery_filesystem("create replay file", path, error))?;
    file.write_all(bytes)
        .map_err(|error| recovery_filesystem("write replay file", path, error))?;
    file.sync_all()
        .map_err(|error| recovery_filesystem("sync replay file", path, error))?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777))
        .map_err(|error| recovery_filesystem("restore replay mode", path, error))
}

#[cfg(not(unix))]
fn write_regular(path: &Path, bytes: &[u8], _mode: u32) -> Result<(), Error> {
    atomic_bytes(path, bytes, "write replay file")
}

#[cfg(unix)]
fn write_symlink(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let target = std::ffi::OsString::from_vec(bytes.to_vec());
    std::os::unix::fs::symlink(target, path)
        .map_err(|error| recovery_filesystem("restore replay symlink", path, error))
}

#[cfg(not(unix))]
fn write_symlink(path: &Path, _bytes: &[u8]) -> Result<(), Error> {
    Err(recovery_refusal(
        "git::recovery-symlink-unsupported",
        format!("cannot restore symlink {} on this platform", path.display()),
        "resume recovery on a platform with native symlink support",
    ))
}

fn verify_replay_surface(
    repository: &Repository,
    manifest: &Manifest,
    boundary: &GitBoundary,
) -> Result<(), Error> {
    verify_replay_identity(repository, manifest, boundary)?;
    require_clean(repository.worktree_root(), "task recovery replay")
}

fn resume_replay_surface(
    repository: &Repository,
    manifest: &Manifest,
    boundary: &GitBoundary,
) -> Result<(), Error> {
    verify_replay_identity(repository, manifest, boundary)?;
    refuse_unsealed_dirty_paths(
        repository.worktree_root(),
        &manifest.payload,
        "git::recovery-replay-drift",
        "preserve the retained quarantine and inspect concurrent lane edits",
    )?;
    clean_manifest_paths(repository.worktree_root(), &manifest.payload)
}

fn verify_replay_identity(
    repository: &Repository,
    manifest: &Manifest,
    boundary: &GitBoundary,
) -> Result<(), Error> {
    refuse_unsupported_tree_entries(repository.worktree_root())?;
    verify_manifest_identity(repository, manifest)?;
    verify_text(
        "symbolic ref",
        &manifest.payload.reference,
        &required_read(
            repository.worktree_root(),
            &["symbolic-ref", "HEAD"],
            "verify replay symbolic HEAD",
        )?,
    )?;
    verify_text(
        "configuration",
        &manifest.payload.configuration,
        &configuration(repository.worktree_root())?,
    )?;
    verify_text(
        "synchronized HEAD",
        &boundary.head,
        &current_head(repository.worktree_root())?,
    )
}

fn refuse_untracked_collisions(worktree: &Path, paths: &[PathState]) -> Result<(), Error> {
    for state in paths {
        let path = admitted_path(worktree, &state.path)?;
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(recovery_refusal(
                    "git::recovery-untracked-collision",
                    format!(
                        "synchronized baseline already owns untracked handoff path {:?}",
                        state.path
                    ),
                    "inspect the baseline path and retained quarantine; no bytes were overwritten",
                ));
            }
            Err(error) => {
                return Err(recovery_filesystem(
                    "inspect replay collision",
                    &path,
                    error,
                ));
            }
        }
    }
    Ok(())
}

fn verify_replayed(
    repository: &Repository,
    root: &Path,
    manifest: &Manifest,
    boundary: &GitBoundary,
) -> Result<(), Error> {
    verify_text(
        "replayed HEAD",
        &boundary.head,
        &current_head(repository.worktree_root())?,
    )?;
    verify_list(
        "replayed staged paths",
        &manifest.payload.staged_paths,
        &changed_paths(
            repository.worktree_root(),
            &["diff", "--name-only", "-z", "--cached"],
        )?,
    )?;
    verify_list(
        "replayed unstaged paths",
        &manifest.payload.unstaged_paths,
        &changed_paths(repository.worktree_root(), &["diff", "--name-only", "-z"])?,
    )?;
    verify_list(
        "replayed untracked paths",
        &path_names(&manifest.payload.untracked),
        &changed_paths(
            repository.worktree_root(),
            &["ls-files", "--others", "--exclude-standard", "-z"],
        )?,
    )?;
    verify_paths(
        repository.worktree_root(),
        root,
        &manifest.payload.untracked,
    )?;
    verify_unchanged_base_patches(repository.worktree_root(), root, manifest, boundary)
}

fn verify_unchanged_base_patches(
    worktree: &Path,
    root: &Path,
    manifest: &Manifest,
    boundary: &GitBoundary,
) -> Result<(), Error> {
    let tracked = path_names(&manifest.payload.tracked);
    if tracked.is_empty()
        || base_changed(worktree, &manifest.payload.head, &boundary.head, &tracked)?
    {
        return Ok(());
    }
    let staged = git_bytes(
        worktree,
        &[
            "diff",
            "--binary",
            "--full-index",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--cached",
            "HEAD",
        ],
        "verify staged replay",
    )?;
    let unstaged = git_bytes(
        worktree,
        &[
            "diff",
            "--binary",
            "--full-index",
            "--src-prefix=a/",
            "--dst-prefix=b/",
        ],
        "verify unstaged replay",
    )?;
    verify_blob_bytes(root, &manifest.payload.staged_patch, &staged)?;
    verify_blob_bytes(root, &manifest.payload.unstaged_patch, &unstaged)
}

fn base_changed(worktree: &Path, old: &str, new: &str, paths: &[String]) -> Result<bool, Error> {
    let mut owned = vec![
        "diff".to_owned(),
        "--quiet".to_owned(),
        old.to_owned(),
        new.to_owned(),
        "--".to_owned(),
    ];
    owned.extend(paths.iter().cloned());
    let args = owned.iter().map(String::as_str).collect::<Vec<_>>();
    let output = run(worktree, &args)?;
    match output.status {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::fault(
            "git::recovery-verify-failed",
            output.detail("compare replay bases"),
        )),
    }
}
