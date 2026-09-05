//! Immutable Git tree selection for gate runs.
//!
//! A gate never measures the checkout. The staged selector captures one parsed
//! index plus the exact `HEAD` tree that gives staged renames their lineage,
//! while a revision selector resolves one immutable commit identity. Both
//! sources pass through the same path, mode, object, and materialization checks
//! before a private read-only root is returned to the caller, named together
//! with the repository it was taken from so that an arm which must observe that
//! repository cannot be pointed at a different one.
//!
//! Each materialization is a real directory of real bytes below the temporary
//! directory, and a run that unwinds removes its own. A run that is *killed*
//! unwinds nothing, so every workspace is held under the advisory lock the
//! crate's `held_root` module supplies, for exactly as long as it is live. That
//! lock is what lets a later run tell a killed run's wreckage from a tree a live
//! gate is measuring, and reclaim only the first.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use sill::Possession;

use crate::state::RenameMap;

/// Stable gate-owned policy for deciding that a staged delete/add pair is a move.
const STAGED_RENAME_SIMILARITY: f32 = 0.5;
/// Zero is the plumbing API's explicit unlimited-candidate setting.
const STAGED_RENAME_LIMIT: usize = 0;

#[cfg(target_family = "unix")]
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};

mod error;
mod source;
#[cfg(test)]
mod tests;
mod workspace;

pub use error::TreeError;
use source::{IndexSeal, RevisionSeal, SelectionSeal, head_seal};
use workspace::{BuildRoot, release_workspace};
#[cfg(test)]
use workspace::{reclaim_abandoned, workspace_lock};

/// Which immutable Git source a gate should measure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    /// The repository index and its `HEAD` rename base as captured at selection time.
    Staged,
    /// The one commit named by this revision at selection time.
    Revision(String),
}

impl Selector {
    /// Select the staged index.
    #[must_use]
    pub const fn staged() -> Self {
        Self::Staged
    }

    /// Select one named revision.
    #[must_use]
    pub fn revision(revision: impl Into<String>) -> Self {
        Self::Revision(revision.into())
    }

    /// Materialize this source below a private immutable root.
    ///
    /// The returned root owns no checkout state and remains valid until the
    /// [`SelectedTree`] is dropped. Index and reference sources, including the
    /// staged index's `HEAD` rename base, are rechecked after every selected
    /// object has been copied, so a moving source cannot publish a partial or
    /// mixed tree or mismatched rename lineage.
    ///
    /// # Errors
    ///
    /// Returns the named repository, revision, index, path, mode, object, or
    /// filesystem failure that prevented an immutable tree from being formed.
    pub fn materialize(&self, repository: &Path) -> Result<SelectedTree, TreeError> {
        self.materialize_with(repository, |_| {})
    }

    fn materialize_with(
        &self,
        repository: &Path,
        after_capture: impl FnOnce(&Path),
    ) -> Result<SelectedTree, TreeError> {
        let repo = open_repository(repository)?;
        let mut build = BuildRoot::create()?;

        let (items, source, renames, staged_authority, publishes_state) = match self {
            Self::Staged => {
                let before = IndexSeal::capture(&repo)?;
                let head = head_seal(&repo)?;
                after_capture(&build.workspace);
                let head_commit = head
                    .as_ref()
                    .map(|seal| seal.resolve(&repo, "HEAD", &build.workspace))
                    .transpose()?;
                let index = before.open(&repo, &build.workspace)?;
                let items = index_items(&index)?;
                materialize_items(&repo, &build.root, &items)?;
                let renames = staged_renames(&repo, head_commit, &index)?;
                let authority = StagedAuthority {
                    entries: items.clone(),
                    rename_base: head_commit,
                    renames: renames.clone(),
                };
                (
                    items,
                    SelectionSeal::Index { before, head },
                    renames,
                    Some(authority),
                    true,
                )
            }
            Self::Revision(revision) => {
                let before = RevisionSeal::capture(&repo, revision)?;
                after_capture(&build.workspace);
                let commit_id = before.resolve(&repo, revision, &build.workspace)?;
                let items = revision_items(&repo, revision, commit_id)?;
                materialize_items(&repo, &build.root, &items)?;
                (
                    items,
                    SelectionSeal::Revision {
                        before,
                        revision: revision.clone(),
                    },
                    RenameMap::default(),
                    None,
                    false,
                )
            }
        };

        make_immutable(&build.root)?;
        verify_immutable(&build.root, &items)?;
        source.verify(&repo)?;
        let files = SelectedFiles {
            paths: items.iter().map(|item| item.path.clone()).collect(),
            modes: items.iter().map(|item| item.mode.into()).collect(),
        };
        Ok(build.finish(
            repository,
            source,
            files,
            renames,
            staged_authority,
            publishes_state,
        ))
    }
}

/// Semantic authority carried by one staged index independently of its on-disk encoding.
///
/// Canonical entries retain path, object, and executable mode. Rename authority
/// additionally retains the exact `HEAD` commit used as the comparison base and
/// the complete detected old-to-new map.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedAuthority {
    entries: Vec<Item>,
    rename_base: Option<gix::ObjectId>,
    renames: RenameMap,
}

/// One semantic component that differs between staged authority captures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StagedAuthorityDifference {
    /// A canonical path, object identity, or executable mode moved.
    Tree,
    /// The exact `HEAD` commit used for staged rename detection moved.
    RenameBase,
    /// The canonical staged rename map moved.
    RenameLineage,
}

impl StagedAuthority {
    /// Exact commit used as the staged rename and ancestry baseline.
    #[must_use]
    pub fn baseline_identity(&self) -> Option<String> {
        self.rename_base.map(|identity| identity.to_string())
    }

    /// Name every semantic component that differs in another staged capture.
    #[must_use]
    pub fn differences(&self, observed: &Self) -> Vec<StagedAuthorityDifference> {
        let mut differences = Vec::new();
        if self.entries != observed.entries {
            differences.push(StagedAuthorityDifference::Tree);
        }
        if self.rename_base != observed.rename_base {
            differences.push(StagedAuthorityDifference::RenameBase);
        }
        if self.renames != observed.renames {
            differences.push(StagedAuthorityDifference::RenameLineage);
        }
        differences
    }
}

/// Result of checking a retained source at a boundary allowed to recapture a
/// semantic-equivalent Git index representation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshableSourceState {
    /// The retained byte-sensitive source seal remains current.
    Current,
    /// Only the index seal reported movement; semantic recapture is required.
    IndexChanged,
}

/// One private materialization owned for the duration of a gate run.
///
/// The possession is kept in an [`Option`] so [`Drop`] can take it: the removal
/// this tree owes runs while its lock is still held, which is the invariant
/// `held_root::remove_held_root` carries in its signature.
#[derive(Debug)]
pub struct SelectedTree {
    workspace: PathBuf,
    root: PathBuf,
    repository: PathBuf,
    source: SelectionSeal,
    files: SelectedFiles,
    renames: RenameMap,
    staged_authority: Option<StagedAuthority>,
    publishes_state: bool,
    possession: Option<Possession>,
}

/// Git file mode retained for one regular selected-tree entry.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SelectedFileMode {
    /// Git regular blob mode `100644`.
    Regular,
    /// Git executable blob mode `100755`.
    Executable,
}

#[derive(Debug)]
struct SelectedFiles {
    paths: Vec<String>,
    modes: Vec<SelectedFileMode>,
}

impl SelectedTree {
    /// Root beneath which every selected path is normalized and relative.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Repository this selection was materialized from.
    ///
    /// Carried with the tree because the root alone cannot name it: the root is
    /// a private copy in a scratch directory, and anything that must observe the
    /// live repository the identity was taken from has no other way to reach the
    /// right one. A caller that supplied it separately could supply a different
    /// one, and the observation would be labelled with a tree it never saw.
    #[must_use]
    pub fn repository(&self) -> &Path {
        &self.repository
    }

    /// Materialize the exact `HEAD` commit captured beside a staged index.
    ///
    /// The retained object identity, rather than a fresh `HEAD` resolution,
    /// makes this safe for ancestry-dependent inventories acquired after the
    /// selected tree itself was sealed. Revision selections and unborn staged
    /// repositories have no staged base.
    ///
    /// # Errors
    ///
    /// Returns the same repository, object, path, or filesystem failures as
    /// [`Selector::materialize`].
    pub fn materialize_staged_base(&self) -> Result<Option<Self>, TreeError> {
        let Some(base) = self
            .staged_authority
            .as_ref()
            .and_then(|authority| authority.rename_base)
        else {
            return Ok(None);
        };
        Selector::revision(base.to_string())
            .materialize(&self.repository)
            .map(Some)
    }

    /// Prove that the live Git source has not moved since materialization.
    ///
    /// The staged seal retains its live index watch, so a write followed by a
    /// byte-for-byte restoration still counts as movement rather than silently
    /// becoming a fresh authority with the old digest.
    ///
    /// # Errors
    ///
    /// Returns the exact index, `HEAD`, revision, or repository movement that
    /// invalidated this selected source.
    pub fn verify_source(&self) -> Result<(), TreeError> {
        let repository = open_repository(&self.repository)?;
        self.source.verify(&repository)
    }

    /// Refresh the staged byte seal after a gate-owned Git observation writes
    /// index cache data without changing staged semantics.
    ///
    /// Canonical entries, the exact `HEAD` rename base, and rename lineage are
    /// independently recaptured and compared before the new seal is accepted.
    pub(crate) fn refresh_staged_seal(&mut self) -> Result<(), TreeError> {
        let Some(expected) = self.staged_authority.as_ref() else {
            return self.verify_source();
        };
        let repository = open_repository(&self.repository)?;
        let before = IndexSeal::capture(&repository)?;
        let head = head_seal(&repository)?;
        let scratch = BuildRoot::create()?;
        let head_commit = head
            .as_ref()
            .map(|seal| seal.resolve(&repository, "HEAD", &scratch.workspace))
            .transpose()?;
        let index = before.open(&repository, &scratch.workspace)?;
        let items = index_items(&index)?;
        let renames = staged_renames(&repository, head_commit, &index)?;
        let observed = StagedAuthority {
            entries: items,
            rename_base: head_commit,
            renames,
        };
        if !expected.differences(&observed).is_empty() {
            return Err(TreeError::IndexChanged {
                path: repository.index_path(),
            });
        }
        self.source = SelectionSeal::Index { before, head };
        Ok(())
    }

    /// Check this source while preserving index movement as a recapture signal.
    ///
    /// Revision and rename-base movement remains an error. Only an index seal
    /// failure is classified for the enclosing pre-commit reservation to
    /// compare against a fresh semantic staged authority.
    pub(crate) fn verify_refreshable_source(&self) -> Result<RefreshableSourceState, TreeError> {
        let repository = open_repository(&self.repository)?;
        match self.source.verify(&repository) {
            Ok(()) => Ok(RefreshableSourceState::Current),
            Err(TreeError::IndexChanged { .. }) => Ok(RefreshableSourceState::IndexChanged),
            Err(error) => Err(error),
        }
    }

    /// Canonical regular paths carried by this exact selected source.
    #[must_use]
    pub fn paths(&self) -> &[String] {
        &self.files.paths
    }

    /// Git mode for one canonical selected path.
    #[must_use]
    pub fn mode(&self, path: &str) -> Option<SelectedFileMode> {
        self.files
            .paths
            .binary_search_by(|candidate| candidate.as_str().cmp(path))
            .ok()
            .map(|index| self.files.modes[index])
    }

    /// Canonical old-to-new paths detected from this exact selected source.
    #[must_use]
    pub(crate) const fn renames(&self) -> &RenameMap {
        &self.renames
    }

    /// Canonical staged entries and their exact rename lineage.
    #[must_use]
    pub(crate) const fn staged_authority(&self) -> Option<&StagedAuthority> {
        self.staged_authority.as_ref()
    }

    /// Whether acceptance of this source may publish current gate state.
    #[must_use]
    pub(crate) const fn publishes_state(&self) -> bool {
        self.publishes_state
    }
}

impl Drop for SelectedTree {
    fn drop(&mut self) {
        if let Some(possession) = self.possession.take() {
            let _ = release_workspace(&self.workspace, &possession);
        }
    }
}

fn create_private_workspace(path: &Path) -> io::Result<()> {
    #[cfg(target_family = "unix")]
    {
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700).create(path)
    }
    #[cfg(not(target_family = "unix"))]
    {
        fs::create_dir(path)
    }
}

/// The Git directories one live repository publishes durable state below.
///
/// Both are named because a linked worktree has two: its own directory, which
/// is where a predecessor wrote per-worktree state, and the common directory
/// every linked worktree shares, which is where one authority lives. A caller
/// that resolved only one of them would either give every worktree a private
/// ledger or give them all one worktree's imports.
pub(crate) fn git_directories(repository: &Path) -> Result<(PathBuf, PathBuf), TreeError> {
    let repo = open_repository(repository)?;
    Ok((repo.path().to_path_buf(), repo.common_dir().to_path_buf()))
}

/// Capture the one stable set of regular paths named by the current Git index.
///
/// The index sources are sealed before they are opened and rechecked after
/// every entry is admitted, just as staged-tree materialization does. Object
/// identities are deliberately discarded: a live contract measures current
/// checkout bytes, while the index supplies only the closed set of path names.
pub(crate) fn tracked_paths(repository: &Path) -> Result<Vec<String>, TreeError> {
    let repo = open_repository(repository)?;
    let seal = IndexSeal::capture(&repo)?;
    let build = BuildRoot::create()?;
    let index = seal.open(&repo, &build.workspace)?;
    let items = index_items(&index)?;
    seal.verify(&repo)?;
    Ok(items.into_iter().map(|item| item.path).collect())
}

pub(crate) fn open_repository(path: &Path) -> Result<gix::Repository, TreeError> {
    let options = gix::open::Options::isolated()
        .strict_config(true)
        .config_overrides([
            "gitoxide.objects.noReplace=true",
            "index.skipHash=false",
            "diff.algorithm=myers",
        ]);
    gix::open_opts(path, options).map_err(|source| TreeError::Repository {
        path: path.to_path_buf(),
        evidence: source.to_string(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileMode {
    Regular,
    Executable,
}

impl From<FileMode> for SelectedFileMode {
    fn from(value: FileMode) -> Self {
        match value {
            FileMode::Regular => Self::Regular,
            FileMode::Executable => Self::Executable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    path: String,
    id: gix::ObjectId,
    mode: FileMode,
}

fn staged_renames(
    repo: &gix::Repository,
    head_commit: Option<gix::ObjectId>,
    index: &gix::index::State,
) -> Result<RenameMap, TreeError> {
    let Some(head_commit) = head_commit else {
        return Ok(RenameMap::default());
    };
    let tree = commit_tree_id(repo, "HEAD", head_commit)?;
    let tree_index = repo
        .index_from_tree(&tree)
        .map_err(|source| TreeError::Repository {
            path: repository_path(repo),
            evidence: format!("cannot expand sealed HEAD for rename detection: {source}"),
        })?;
    let mut resource_cache =
        repo.diff_resource_cache_for_tree_diff()
            .map_err(|source| TreeError::Repository {
                path: repository_path(repo),
                evidence: format!("cannot initialize staged rename detection: {source}"),
            })?;
    let mut pathspec = gix::pathspec::Search::from_specs(
        std::iter::empty::<gix::pathspec::Pattern>(),
        None,
        repo.workdir().unwrap_or_else(|| repo.git_dir()),
    )
    .map_err(|source| TreeError::Repository {
        path: repository_path(repo),
        evidence: format!("cannot initialize the staged rename path matcher: {source}"),
    })?;
    let mut pairs = Vec::new();
    gix_diff::index(
        &tree_index,
        index,
        |change| {
            if let gix_diff::index::ChangeRef::Rewrite {
                source_location,
                location,
                copy: false,
                ..
            } = change
            {
                pairs.push((
                    normalized_path(source_location.as_ref())?,
                    normalized_path(location.as_ref())?,
                ));
            }
            Ok::<_, TreeError>(std::ops::ControlFlow::Continue(()))
        },
        Some(gix_diff::index::RewriteOptions {
            resource_cache: &mut resource_cache,
            find: repo,
            rewrites: gix_diff::Rewrites {
                copies: None,
                percentage: Some(STAGED_RENAME_SIMILARITY),
                limit: STAGED_RENAME_LIMIT,
                track_empty: false,
            },
        }),
        &mut pathspec,
        &mut |_, _, _, _| true,
    )
    .map_err(|source| TreeError::Repository {
        path: repository_path(repo),
        evidence: format!("cannot capture staged renames from the sealed index: {source}"),
    })?;
    RenameMap::new(pairs).map_err(|source| TreeError::Repository {
        path: repository_path(repo),
        evidence: format!("staged rename map is invalid: {source}"),
    })
}

fn repository_path(repo: &gix::Repository) -> PathBuf {
    repo.workdir()
        .unwrap_or_else(|| repo.git_dir())
        .to_path_buf()
}

fn index_items(index: &gix::index::File) -> Result<Vec<Item>, TreeError> {
    let mut items = Vec::with_capacity(index.entries().len());
    for entry in index.entries() {
        let path = normalized_path(entry.path(index))?;
        if entry.stage() != gix::index::entry::Stage::Unconflicted {
            return Err(TreeError::UnmergedIndex {
                path,
                stage: entry.stage_raw(),
            });
        }
        let mode = if entry.mode == gix::index::entry::Mode::FILE {
            FileMode::Regular
        } else if entry.mode == gix::index::entry::Mode::FILE_EXECUTABLE {
            FileMode::Executable
        } else if entry.mode == gix::index::entry::Mode::SYMLINK {
            return Err(TreeError::Symlink { path });
        } else if entry.mode == gix::index::entry::Mode::COMMIT {
            return Err(TreeError::Submodule { path });
        } else {
            return Err(TreeError::UnsupportedMode {
                path,
                mode: entry.mode.bits(),
            });
        };
        items.push(Item {
            path,
            id: entry.id,
            mode,
        });
    }
    normalize_items(items)
}

fn resolve_revision(repo: &gix::Repository, revision: &str) -> Result<gix::ObjectId, TreeError> {
    revision_spec(repo, revision)?
        .single()
        .map(gix::Id::detach)
        .ok_or_else(|| TreeError::RevisionUnresolved {
            revision: revision.to_owned(),
            evidence: "the revision names a range rather than one object".to_owned(),
        })
}

fn revision_spec<'repo>(
    repo: &'repo gix::Repository,
    revision: &str,
) -> Result<gix::revision::Spec<'repo>, TreeError> {
    refuse_ambiguous_reference(repo, revision)?;
    let options = gix::revision::spec::parse::Options {
        refs_hint: gix::revision::spec::parse::RefsHint::Fail,
        object_kind_hint: None,
    };
    gix::revision::Spec::from_bstr(revision, repo, options).map_err(|source| {
        TreeError::RevisionUnresolved {
            revision: revision.to_owned(),
            evidence: source.to_string(),
        }
    })
}

fn refuse_ambiguous_reference(repo: &gix::Repository, revision: &str) -> Result<(), TreeError> {
    let atom_end = revision
        .char_indices()
        .find_map(|(index, character)| "~^:@".contains(character).then_some(index))
        .unwrap_or(revision.len());
    let atom = &revision[..atom_end];
    if atom.is_empty() || atom == "HEAD" || atom.starts_with("refs/") {
        return Ok(());
    }

    let candidates = [
        format!("refs/{atom}"),
        format!("refs/tags/{atom}"),
        format!("refs/heads/{atom}"),
        format!("refs/remotes/{atom}"),
        format!("refs/remotes/{atom}/HEAD"),
    ];
    let mut matched = Vec::new();
    for candidate in candidates {
        match repo.try_find_reference(&candidate) {
            Ok(Some(_)) => matched.push(candidate),
            Ok(None) => {}
            Err(source) => {
                return Err(TreeError::RevisionUnresolved {
                    revision: revision.to_owned(),
                    evidence: source.to_string(),
                });
            }
        }
    }
    if matched.len() > 1 {
        return Err(TreeError::RevisionUnresolved {
            revision: revision.to_owned(),
            evidence: format!(
                "the reference name is ambiguous among {}",
                matched.join(", ")
            ),
        });
    }
    Ok(())
}

fn revision_items(
    repo: &gix::Repository,
    revision: &str,
    commit_id: gix::ObjectId,
) -> Result<Vec<Item>, TreeError> {
    let tree_id = commit_tree_id(repo, revision, commit_id)?;
    let mut items = Vec::new();
    collect_tree(repo, tree_id, "", &mut items)?;
    normalize_items(items)
}

fn commit_tree_id(
    repo: &gix::Repository,
    revision: &str,
    commit_id: gix::ObjectId,
) -> Result<gix::ObjectId, TreeError> {
    let commit = verified_object(repo, commit_id, revision)?;
    if commit.kind != gix::objs::Kind::Commit {
        return Err(TreeError::RevisionNotCommit {
            revision: revision.to_owned(),
            actual: commit.kind.to_string(),
        });
    }
    let commit = commit
        .try_into_commit()
        .map_err(|source| TreeError::RevisionNotCommit {
            revision: revision.to_owned(),
            actual: source.actual.to_string(),
        })?;
    commit
        .tree_id()
        .map(gix::Id::detach)
        .map_err(|source| TreeError::MalformedObject {
            object: commit_id.to_string(),
            evidence: source.to_string(),
        })
}

fn collect_tree(
    repo: &gix::Repository,
    tree_id: gix::ObjectId,
    prefix: &str,
    items: &mut Vec<Item>,
) -> Result<(), TreeError> {
    let label = if prefix.is_empty() { "." } else { prefix };
    let object = verified_object(repo, tree_id, label)?;
    if object.kind != gix::objs::Kind::Tree {
        return Err(TreeError::UnexpectedObject {
            path: label.to_owned(),
            object: tree_id.to_string(),
            expected: "tree",
            actual: object.kind.to_string(),
        });
    }
    let tree = object
        .try_into_tree()
        .map_err(|source| TreeError::UnexpectedObject {
            path: label.to_owned(),
            object: tree_id.to_string(),
            expected: "tree",
            actual: source.actual.to_string(),
        })?;
    let entries = tree
        .iter()
        .map(|entry| {
            entry
                .map(|entry| (entry.filename().to_vec(), entry.object_id(), entry.mode()))
                .map_err(|source| TreeError::MalformedObject {
                    object: tree_id.to_string(),
                    evidence: source.to_string(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;

    for (name, id, mode) in entries {
        let name = normalized_component(&name)?;
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        match mode.value() {
            0o040000 => collect_tree(repo, id, &path, items)?,
            0o100644 => items.push(Item {
                path,
                id,
                mode: FileMode::Regular,
            }),
            0o100755 => items.push(Item {
                path,
                id,
                mode: FileMode::Executable,
            }),
            0o120000 => return Err(TreeError::Symlink { path }),
            0o160000 => return Err(TreeError::Submodule { path }),
            unsupported => {
                return Err(TreeError::UnsupportedMode {
                    path,
                    mode: u32::from(unsupported),
                });
            }
        }
    }
    Ok(())
}

fn normalize_items(mut items: Vec<Item>) -> Result<Vec<Item>, TreeError> {
    items.sort_by(|left, right| left.path.cmp(&right.path));
    for pair in items.windows(2) {
        let left = &pair[0].path;
        let right = &pair[1].path;
        if left == right
            || right
                .strip_prefix(left)
                .is_some_and(|tail| tail.starts_with('/'))
        {
            return Err(TreeError::PathCollision {
                first: left.clone(),
                second: right.clone(),
            });
        }
    }
    Ok(items)
}

fn normalized_path(path: &[u8]) -> Result<String, TreeError> {
    let path = std::str::from_utf8(path).map_err(|_| TreeError::UnnameablePath {
        path: String::from_utf8_lossy(path).into_owned(),
    })?;
    if path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains(['\\', '\n', '\r', '\0'])
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(TreeError::UnnameablePath {
            path: path.to_owned(),
        });
    }
    Ok(path.to_owned())
}

pub(crate) fn normalized_component(component: &[u8]) -> Result<String, TreeError> {
    let component = std::str::from_utf8(component).map_err(|_| TreeError::UnnameablePath {
        path: String::from_utf8_lossy(component).into_owned(),
    })?;
    if component.is_empty()
        || component == "."
        || component == ".."
        || component.contains(['/', '\\', '\n', '\r', '\0'])
    {
        return Err(TreeError::UnnameablePath {
            path: component.to_owned(),
        });
    }
    Ok(component.to_owned())
}

fn materialize_items(repo: &gix::Repository, root: &Path, items: &[Item]) -> Result<(), TreeError> {
    let mut directories = BTreeSet::new();
    directories.insert(root.to_path_buf());
    for item in items {
        let destination = root.join(&item.path);
        let parent = destination
            .parent()
            .expect("a normalized relative path has a parent below its root");
        if directories.insert(parent.to_path_buf()) {
            fs::create_dir_all(parent).map_err(|source| {
                TreeError::io("creating a selected tree directory", parent, source)
            })?;
            private_directory_chain(root, parent)?;
        }

        let object = verified_object(repo, item.id, &item.path)?;
        if object.kind != gix::objs::Kind::Blob {
            return Err(TreeError::UnexpectedObject {
                path: item.path.clone(),
                object: item.id.to_string(),
                expected: "blob",
                actual: object.kind.to_string(),
            });
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|source| {
                TreeError::io("creating a selected tree file", &destination, source)
            })?;
        file.write_all(&object.data).map_err(|source| {
            TreeError::io("writing a selected tree file", &destination, source)
        })?;
        drop(file);
        immutable_file(&destination, item.mode)?;
    }
    Ok(())
}

pub(crate) fn verified_object<'repo>(
    repo: &'repo gix::Repository,
    id: gix::ObjectId,
    path: &str,
) -> Result<gix::Object<'repo>, TreeError> {
    let object = repo
        .find_object(id)
        .map_err(|source| TreeError::MissingObject {
            path: path.to_owned(),
            object: id.to_string(),
            evidence: source.to_string(),
        })?;
    let actual =
        gix::objs::compute_hash(id.kind(), object.kind, &object.data).map_err(|source| {
            TreeError::MalformedObject {
                object: id.to_string(),
                evidence: source.to_string(),
            }
        })?;
    if actual != id {
        return Err(TreeError::ObjectChanged {
            path: path.to_owned(),
            expected: id.to_string(),
            actual: actual.to_string(),
        });
    }
    Ok(object)
}

fn private_directory(path: &Path) -> Result<(), TreeError> {
    #[cfg(target_family = "unix")]
    let permissions = fs::Permissions::from_mode(0o700);
    #[cfg(not(target_family = "unix"))]
    let permissions = {
        let mut permissions = fs::metadata(path)
            .map_err(|source| TreeError::io("inspecting a private directory", path, source))?
            .permissions();
        permissions.set_readonly(false);
        permissions
    };
    fs::set_permissions(path, permissions)
        .map_err(|source| TreeError::io("making a selected directory private", path, source))
}

fn private_directory_chain(root: &Path, directory: &Path) -> Result<(), TreeError> {
    let relative = directory
        .strip_prefix(root)
        .expect("selected directory remains below its private root");
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        private_directory(&current)?;
    }
    Ok(())
}

fn create_private_directory_tree(root: &Path, directory: &Path) -> Result<(), TreeError> {
    fs::create_dir_all(directory).map_err(|source| {
        TreeError::io("creating captured source directories", directory, source)
    })?;
    private_directory_chain(root, directory)
}

fn write_private_source(root: &Path, path: &Path, bytes: &[u8]) -> Result<(), TreeError> {
    let parent = path
        .parent()
        .expect("a captured source file has a parent below its root");
    create_private_directory_tree(root, parent)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| TreeError::io("creating a captured source file", path, source))?;
    file.write_all(bytes)
        .map_err(|source| TreeError::io("writing a captured source file", path, source))?;
    drop(file);
    immutable_file(path, FileMode::Regular)
}

fn immutable_file(path: &Path, mode: FileMode) -> Result<(), TreeError> {
    #[cfg(target_family = "unix")]
    let permissions = fs::Permissions::from_mode(match mode {
        FileMode::Regular => 0o400,
        FileMode::Executable => 0o500,
    });
    #[cfg(not(target_family = "unix"))]
    let permissions = {
        let mut permissions = fs::metadata(path)
            .map_err(|source| TreeError::io("inspecting a selected file", path, source))?
            .permissions();
        permissions.set_readonly(true);
        permissions
    };
    fs::set_permissions(path, permissions)
        .map_err(|source| TreeError::io("making a selected file immutable", path, source))
}

fn make_immutable(root: &Path) -> Result<(), TreeError> {
    let mut directories = vec![root.to_path_buf()];
    let mut index = 0;
    while index < directories.len() {
        let directory = directories[index].clone();
        index += 1;
        let listing = fs::read_dir(&directory)
            .map_err(|source| TreeError::io("sealing a selected directory", &directory, source))?;
        for entry in listing {
            let entry = entry.map_err(|source| {
                TreeError::io("reading a selected directory", &directory, source)
            })?;
            if entry
                .file_type()
                .map_err(|source| {
                    TreeError::io("inspecting a selected path", &entry.path(), source)
                })?
                .is_dir()
            {
                directories.push(entry.path());
            }
        }
    }
    for directory in directories.into_iter().rev() {
        #[cfg(target_family = "unix")]
        let permissions = fs::Permissions::from_mode(0o500);
        #[cfg(not(target_family = "unix"))]
        let permissions = {
            let mut permissions = fs::metadata(&directory)
                .map_err(|source| {
                    TreeError::io("inspecting a selected directory", &directory, source)
                })?
                .permissions();
            permissions.set_readonly(true);
            permissions
        };
        fs::set_permissions(&directory, permissions).map_err(|source| {
            TreeError::io("making a selected directory immutable", &directory, source)
        })?;
    }
    Ok(())
}

fn verify_immutable(root: &Path, items: &[Item]) -> Result<(), TreeError> {
    let expected_files = items
        .iter()
        .map(|item| item.path.clone())
        .collect::<BTreeSet<_>>();
    let mut expected_directories = BTreeSet::from([String::new()]);
    for item in items {
        let mut parent = Path::new(&item.path).parent();
        while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
            expected_directories.insert(path.to_string_lossy().into_owned());
            parent = path.parent();
        }
    }
    let mut observed_files = BTreeSet::new();
    let mut observed_directories = BTreeSet::new();
    let mut directories = vec![(root.to_path_buf(), String::new())];
    while let Some((directory, relative)) = directories.pop() {
        observed_directories.insert(relative.clone());
        verify_directory(
            &directory,
            if relative.is_empty() { "." } else { &relative },
        )?;
        let listing = fs::read_dir(&directory).map_err(|source| {
            TreeError::io("verifying a selected directory", &directory, source)
        })?;
        for entry in listing {
            let entry = entry.map_err(|source| {
                TreeError::io("reading a selected directory", &directory, source)
            })?;
            let name = entry.file_name().into_string().map_err(|name| {
                TreeError::MaterializationChanged {
                    path: directory.join(name).display().to_string(),
                }
            })?;
            let path = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            let file_type = entry.file_type().map_err(|source| {
                TreeError::io("inspecting a selected path", &entry.path(), source)
            })?;
            if file_type.is_dir() {
                directories.push((entry.path(), path));
            } else if file_type.is_file() {
                observed_files.insert(path);
            } else {
                return Err(TreeError::MaterializationChanged { path });
            }
        }
    }
    if expected_directories != observed_directories {
        let path = expected_directories
            .symmetric_difference(&observed_directories)
            .next()
            .cloned()
            .unwrap_or_else(|| "selected tree".to_owned());
        return Err(TreeError::MaterializationChanged { path });
    }
    if expected_files != observed_files {
        let path = expected_files
            .symmetric_difference(&observed_files)
            .next()
            .cloned()
            .unwrap_or_else(|| "selected tree".to_owned());
        return Err(TreeError::MaterializationChanged { path });
    }

    for item in items {
        let path = root.join(&item.path);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|source| TreeError::io("verifying a selected file", &path, source))?;
        if !metadata.is_file() {
            return Err(TreeError::MaterializationChanged {
                path: item.path.clone(),
            });
        }
        let bytes = fs::read(&path)
            .map_err(|source| TreeError::io("verifying selected bytes", &path, source))?;
        let actual = gix::objs::compute_hash(item.id.kind(), gix::objs::Kind::Blob, &bytes)
            .map_err(|_| TreeError::MaterializationChanged {
                path: item.path.clone(),
            })?;
        if actual != item.id {
            return Err(TreeError::MaterializationChanged {
                path: item.path.clone(),
            });
        }
        #[cfg(target_family = "unix")]
        {
            let expected = match item.mode {
                FileMode::Regular => 0o400,
                FileMode::Executable => 0o500,
            };
            if metadata.mode() & 0o777 != expected {
                return Err(TreeError::MaterializationChanged {
                    path: item.path.clone(),
                });
            }
        }
        #[cfg(not(target_family = "unix"))]
        if !metadata.permissions().readonly() {
            return Err(TreeError::MaterializationChanged {
                path: item.path.clone(),
            });
        }
    }
    Ok(())
}

fn verify_directory(path: &Path, relative: &str) -> Result<(), TreeError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| TreeError::io("verifying a selected directory", path, source))?;
    if !metadata.is_dir() {
        return Err(TreeError::MaterializationChanged {
            path: relative.to_owned(),
        });
    }
    #[cfg(target_family = "unix")]
    if metadata.mode() & 0o777 != 0o500 {
        return Err(TreeError::MaterializationChanged {
            path: relative.to_owned(),
        });
    }
    #[cfg(not(target_family = "unix"))]
    if !metadata.permissions().readonly() {
        return Err(TreeError::MaterializationChanged {
            path: relative.to_owned(),
        });
    }
    Ok(())
}

fn make_removable(root: &Path) {
    let Ok(metadata) = fs::symlink_metadata(root) else {
        return;
    };
    if metadata.is_dir() {
        set_owner_writable(root, &metadata);
        if let Ok(listing) = fs::read_dir(root) {
            for entry in listing.flatten() {
                make_removable(&entry.path());
            }
        }
    } else if metadata.is_file() {
        set_owner_writable(root, &metadata);
    }
}

fn set_owner_writable(path: &Path, metadata: &fs::Metadata) {
    #[cfg(target_family = "unix")]
    {
        let mode = if metadata.is_dir() { 0o700 } else { 0o600 };
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
    #[cfg(not(target_family = "unix"))]
    {
        let mut permissions = metadata.permissions();
        permissions.set_readonly(false);
        let _ = fs::set_permissions(path, permissions);
    }
}
