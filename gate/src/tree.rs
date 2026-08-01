//! Immutable Git tree selection for gate runs.
//!
//! A gate never measures the checkout. The bare selector captures one parsed
//! index, while a revision selector resolves one immutable commit identity.
//! Both sources pass through the same path, mode, object, and materialization
//! checks before a private read-only root is returned to the caller.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_family = "unix")]
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
#[cfg(not(target_family = "unix"))]
use std::time::SystemTime;

/// Which immutable Git source a gate should measure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    /// The repository index as captured at selection time.
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
    /// [`SelectedTree`] is dropped. Index and reference sources are rechecked
    /// after every selected object has been copied, so a moving source cannot
    /// publish a partial or mixed tree.
    ///
    /// # Errors
    ///
    /// Returns the named repository, revision, index, path, mode, object, or
    /// filesystem failure that prevented an immutable tree from being formed.
    pub fn materialize(&self, repository: &Path) -> Result<SelectedTree, TreeError> {
        self.materialize_with(repository, || {})
    }

    fn materialize_with(
        &self,
        repository: &Path,
        after_capture: impl FnOnce(),
    ) -> Result<SelectedTree, TreeError> {
        let repo = open_repository(repository)?;
        let mut build = BuildRoot::create()?;

        let (items, source) = match self {
            Self::Staged => {
                let before = IndexSeal::capture(&repo)?;
                let index = repo
                    .open_index()
                    .map_err(|source| TreeError::IndexUnreadable {
                        path: repo.index_path(),
                        evidence: source.to_string(),
                    })?;
                if IndexSeal::capture(&repo).ok().as_ref() != Some(&before) {
                    return Err(TreeError::IndexChanged {
                        path: repo.index_path(),
                    });
                }
                after_capture();
                let items = index_items(&index)?;
                materialize_items(&repo, &build.root, &items)?;
                (items, SelectionSeal::Index(before))
            }
            Self::Revision(revision) => {
                let before =
                    RevisionSeal::capture(&repo, revision).map_err(|source| TreeError::Io {
                        operation: "capturing revision sources",
                        path: repo.git_dir().to_path_buf(),
                        source,
                    })?;
                let commit_id = resolve_revision(&repo, revision)?;
                after_capture();
                let items = revision_items(&repo, revision, commit_id)?;
                materialize_items(&repo, &build.root, &items)?;
                (
                    items,
                    SelectionSeal::Revision {
                        before,
                        revision: revision.clone(),
                    },
                )
            }
        };

        make_immutable(&build.root)?;
        verify_immutable(&build.root, &items)?;
        source.verify(&repo)?;
        Ok(build.finish())
    }
}

/// One private materialization owned for the duration of a gate run.
#[derive(Debug)]
pub struct SelectedTree {
    workspace: PathBuf,
    root: PathBuf,
}

impl SelectedTree {
    /// Root beneath which every selected path is normalized and relative.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for SelectedTree {
    fn drop(&mut self) {
        make_removable(&self.workspace);
        let _ = fs::remove_dir_all(&self.workspace);
    }
}

#[derive(Debug)]
struct BuildRoot {
    workspace: PathBuf,
    root: PathBuf,
    finished: bool,
}

impl BuildRoot {
    fn create() -> Result<Self, TreeError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let temporary = std::env::temp_dir();
        for _ in 0..128 {
            let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
            let workspace =
                temporary.join(format!("spice-gate-tree-{}-{nonce}", std::process::id()));
            match create_private_workspace(&workspace) {
                Ok(()) => {
                    let root = workspace.join("root");
                    let build = Self {
                        workspace,
                        root,
                        finished: false,
                    };
                    private_directory(&build.workspace)?;
                    fs::create_dir(&build.root).map_err(|source| {
                        TreeError::io("creating a selected tree root", &build.root, source)
                    })?;
                    private_directory(&build.root)?;
                    return Ok(build);
                }
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(TreeError::io(
                        "creating a selected tree workspace",
                        &workspace,
                        source,
                    ));
                }
            }
        }
        Err(TreeError::io(
            "creating a selected tree workspace",
            &temporary,
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "selected tree name attempts exhausted",
            ),
        ))
    }

    fn finish(&mut self) -> SelectedTree {
        self.finished = true;
        SelectedTree {
            workspace: self.workspace.clone(),
            root: self.root.clone(),
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

impl Drop for BuildRoot {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        make_removable(&self.workspace);
        let _ = fs::remove_dir_all(&self.workspace);
    }
}

fn open_repository(path: &Path) -> Result<gix::Repository, TreeError> {
    let options = gix::open::Options::isolated()
        .strict_config(true)
        .config_overrides(["gitoxide.objects.noReplace=true", "index.skipHash=false"]);
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    path: String,
    id: gix::ObjectId,
    mode: FileMode,
}

enum SelectionSeal {
    Index(IndexSeal),
    Revision {
        before: RevisionSeal,
        revision: String,
    },
}

impl SelectionSeal {
    fn verify(&self, repo: &gix::Repository) -> Result<(), TreeError> {
        match self {
            Self::Index(before) => {
                if IndexSeal::capture(repo).ok().as_ref() != Some(before) {
                    return Err(TreeError::IndexChanged {
                        path: repo.index_path(),
                    });
                }
            }
            Self::Revision { before, revision } => {
                if RevisionSeal::capture(repo, revision).ok().as_ref() != Some(before) {
                    return Err(TreeError::RevisionChanged {
                        revision: revision.clone(),
                    });
                }
            }
        }
        Ok(())
    }
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
    refuse_ambiguous_reference(repo, revision)?;
    let options = gix::revision::spec::parse::Options {
        refs_hint: gix::revision::spec::parse::RefsHint::Fail,
        object_kind_hint: None,
    };
    let spec = gix::revision::Spec::from_bstr(revision, repo, options).map_err(|source| {
        TreeError::RevisionUnresolved {
            revision: revision.to_owned(),
            evidence: source.to_string(),
        }
    })?;
    spec.single()
        .map(gix::Id::detach)
        .ok_or_else(|| TreeError::RevisionUnresolved {
            revision: revision.to_owned(),
            evidence: "the revision names a range rather than one object".to_owned(),
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
    let tree_id =
        commit
            .tree_id()
            .map(gix::Id::detach)
            .map_err(|source| TreeError::MalformedObject {
                object: commit_id.to_string(),
                evidence: source.to_string(),
            })?;

    let mut items = Vec::new();
    collect_tree(repo, tree_id, "", &mut items)?;
    normalize_items(items)
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

fn normalized_component(component: &[u8]) -> Result<String, TreeError> {
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

fn verified_object<'repo>(
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexSeal(Vec<ObservedFile>);

impl IndexSeal {
    fn capture(repo: &gix::Repository) -> Result<Self, TreeError> {
        let index = repo.index_path();
        if !index.exists() {
            return Err(TreeError::IndexAbsent { path: index });
        }
        let mut paths = vec![index];
        let listing = fs::read_dir(repo.git_dir())
            .map_err(|source| TreeError::io("listing index sources", repo.git_dir(), source))?;
        for entry in listing {
            let entry = entry
                .map_err(|source| TreeError::io("reading index sources", repo.git_dir(), source))?;
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("sharedindex."))
            {
                paths.push(entry.path());
            }
        }
        paths.sort();
        paths
            .into_iter()
            .map(|path| {
                observed_file(&path)
                    .map_err(|source| TreeError::io("capturing an index source", &path, source))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RevisionSeal(Vec<ObservedFile>);

impl RevisionSeal {
    fn capture(repo: &gix::Repository, revision: &str) -> io::Result<Self> {
        let mut paths = Vec::new();
        paths.push(repo.git_dir().join("HEAD"));
        if repo.common_dir() != repo.git_dir() {
            paths.push(repo.common_dir().join("HEAD"));
        }
        for directory in [repo.git_dir().join("refs"), repo.common_dir().join("refs")] {
            collect_files(&directory, &mut paths)?;
        }
        for directory in [repo.git_dir().join("logs"), repo.common_dir().join("logs")] {
            collect_files(&directory, &mut paths)?;
        }
        let atom_end = revision
            .char_indices()
            .find_map(|(index, character)| "~^:@".contains(character).then_some(index))
            .unwrap_or(revision.len());
        let atom = &revision[..atom_end];
        if normalized_path(atom.as_bytes()).is_ok() {
            let pseudoref = repo.git_dir().join(atom);
            if pseudoref.is_file() {
                paths.push(pseudoref);
            }
        }
        for packed in [
            repo.git_dir().join("packed-refs"),
            repo.common_dir().join("packed-refs"),
        ] {
            if packed.exists() {
                paths.push(packed);
            }
        }
        paths.sort();
        paths.dedup();
        paths
            .into_iter()
            .map(|path| observed_file(&path))
            .collect::<io::Result<Vec<_>>>()
            .map(Self)
    }
}

fn collect_files(directory: &Path, into: &mut Vec<PathBuf>) -> io::Result<()> {
    match fs::read_dir(directory) {
        Ok(listing) => {
            for entry in listing {
                let entry = entry?;
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path)?;
                if metadata.is_dir() {
                    collect_files(&path, into)?;
                } else if metadata.is_file() {
                    into.push(path);
                } else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("revision source {} is not a regular file", path.display()),
                    ));
                }
            }
            Ok(())
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(source),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedFile {
    path: PathBuf,
    identity: FileIdentity,
    bytes: Vec<u8>,
}

fn observed_file(path: &Path) -> io::Result<ObservedFile> {
    let before = fs::symlink_metadata(path)?;
    if !before.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source is not a regular file",
        ));
    }
    let before = file_identity(&before);
    let mut file = File::open(path)?;
    let opened = file_identity(&file.metadata()?);
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let read = file_identity(&file.metadata()?);
    let after_metadata = fs::symlink_metadata(path)?;
    if !after_metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source stopped being a regular file",
        ));
    }
    let after = file_identity(&after_metadata);
    if before != opened || opened != read || read != after {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source changed while it was captured",
        ));
    }
    Ok(ObservedFile {
        path: path.to_path_buf(),
        identity: after,
        bytes,
    })
}

#[cfg(target_family = "unix")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    mode: u32,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

#[cfg(target_family = "unix")]
fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        length: metadata.len(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    }
}

#[cfg(not(target_family = "unix"))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    length: u64,
    readonly: bool,
    modified: Option<SystemTime>,
}

#[cfg(not(target_family = "unix"))]
fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
    FileIdentity {
        length: metadata.len(),
        readonly: metadata.permissions().readonly(),
        modified: metadata.modified().ok(),
    }
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

/// What prevented a Git source from becoming one immutable gate tree.
#[derive(Debug)]
pub enum TreeError {
    /// The requested path could not be opened as a repository.
    Repository {
        /// Requested repository path.
        path: PathBuf,
        /// What the Git implementation reported.
        evidence: String,
    },
    /// The repository has no index to capture.
    IndexAbsent {
        /// Expected index path.
        path: PathBuf,
    },
    /// The captured index could not be decoded.
    IndexUnreadable {
        /// Index path.
        path: PathBuf,
        /// What the Git implementation reported.
        evidence: String,
    },
    /// The index was replaced or changed during selection.
    IndexChanged {
        /// Index path.
        path: PathBuf,
    },
    /// The index carries unresolved merge stages.
    UnmergedIndex {
        /// Conflicted path.
        path: String,
        /// Nonzero index stage.
        stage: u32,
    },
    /// The named revision did not resolve uniquely to one object.
    RevisionUnresolved {
        /// Exact requested revision.
        revision: String,
        /// What the Git implementation reported.
        evidence: String,
    },
    /// The named revision resolved to an object other than a commit.
    RevisionNotCommit {
        /// Exact requested revision.
        revision: String,
        /// Actual object kind.
        actual: String,
    },
    /// A reference changed while its selected commit was materialized.
    RevisionChanged {
        /// Exact requested revision.
        revision: String,
    },
    /// A selected path is not a stable normalized UTF-8 relative path.
    UnnameablePath {
        /// Lossy rendering of the refused bytes.
        path: String,
    },
    /// Two selected paths cannot coexist in one filesystem tree.
    PathCollision {
        /// First colliding path.
        first: String,
        /// Second colliding path.
        second: String,
    },
    /// A selected path is a symlink, which could escape the private root.
    Symlink {
        /// Refused path.
        path: String,
    },
    /// A selected path is a submodule commit rather than local bytes.
    Submodule {
        /// Refused path.
        path: String,
    },
    /// A selected path carries a mode the gate cannot faithfully reproduce.
    UnsupportedMode {
        /// Refused path.
        path: String,
        /// Raw Git mode.
        mode: u32,
    },
    /// A selected Git object was absent or unreadable.
    MissingObject {
        /// Path or revision that required the object.
        path: String,
        /// Requested object identity.
        object: String,
        /// What the object database reported.
        evidence: String,
    },
    /// A selected object's bytes did not reproduce its identity.
    ObjectChanged {
        /// Path or revision that required the object.
        path: String,
        /// Requested object identity.
        expected: String,
        /// Identity of the bytes actually read.
        actual: String,
    },
    /// A selected object had the wrong Git kind for its mode or parent.
    UnexpectedObject {
        /// Path that required the object.
        path: String,
        /// Requested object identity.
        object: String,
        /// Required Git object kind.
        expected: &'static str,
        /// Actual Git object kind.
        actual: String,
    },
    /// A selected commit or tree object was malformed.
    MalformedObject {
        /// Object identity.
        object: String,
        /// What decoding or hashing reported.
        evidence: String,
    },
    /// The private output changed before it could be returned.
    MaterializationChanged {
        /// First affected relative path.
        path: String,
    },
    /// The filesystem refused an operation needed by selection.
    Io {
        /// Operation being attempted.
        operation: &'static str,
        /// Path at which it failed.
        path: PathBuf,
        /// What the filesystem reported.
        source: io::Error,
    },
}

impl TreeError {
    fn io(operation: &'static str, path: &Path, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.to_path_buf(),
            source,
        }
    }
}

impl fmt::Display for TreeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository { path, evidence } => write!(
                formatter,
                "{} is not a readable Git repository: {evidence}",
                path.display()
            ),
            Self::IndexAbsent { path } => {
                write!(formatter, "Git index {} is absent", path.display())
            }
            Self::IndexUnreadable { path, evidence } => write!(
                formatter,
                "Git index {} could not be captured: {evidence}",
                path.display()
            ),
            Self::IndexChanged { path } => write!(
                formatter,
                "Git index {} changed while its staged tree was selected",
                path.display()
            ),
            Self::UnmergedIndex { path, stage } => {
                write!(
                    formatter,
                    "staged path {path:?} remains at merge stage {stage}"
                )
            }
            Self::RevisionUnresolved { revision, evidence } => write!(
                formatter,
                "revision {revision:?} did not name exactly one object: {evidence}"
            ),
            Self::RevisionNotCommit { revision, actual } => write!(
                formatter,
                "revision {revision:?} names a {actual}, not a commit"
            ),
            Self::RevisionChanged { revision } => write!(
                formatter,
                "revision source for {revision:?} moved while its tree was selected"
            ),
            Self::UnnameablePath { path } => write!(
                formatter,
                "Git path {path:?} is not a normalized UTF-8 relative path"
            ),
            Self::PathCollision { first, second } => write!(
                formatter,
                "Git paths {first:?} and {second:?} cannot coexist in one tree"
            ),
            Self::Symlink { path } => write!(
                formatter,
                "Git path {path:?} is a symlink, which the gate does not follow"
            ),
            Self::Submodule { path } => write!(
                formatter,
                "Git path {path:?} is a submodule, which has no local blob to measure"
            ),
            Self::UnsupportedMode { path, mode } => write!(
                formatter,
                "Git path {path:?} carries unsupported mode {mode:o}"
            ),
            Self::MissingObject {
                path,
                object,
                evidence,
            } => write!(
                formatter,
                "Git object {object} for {path:?} is unavailable: {evidence}"
            ),
            Self::ObjectChanged {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "Git object {expected} for {path:?} read back as {actual}"
            ),
            Self::UnexpectedObject {
                path,
                object,
                expected,
                actual,
            } => write!(
                formatter,
                "Git object {object} for {path:?} is {actual}, not {expected}"
            ),
            Self::MalformedObject { object, evidence } => {
                write!(formatter, "Git object {object} is malformed: {evidence}")
            }
            Self::MaterializationChanged { path } => write!(
                formatter,
                "selected tree path {path:?} changed before publication"
            ),
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "{operation} at {} failed: {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for TreeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{Selector, TreeError};

    struct Repository {
        root: PathBuf,
    }

    impl Repository {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "gate-tree-unit-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("create a unit repository");
            let repository = Self { root };
            repository.git(&["init", "-q", "-b", "main"]);
            repository.git(&["config", "user.name", "Gate Unit"]);
            repository.git(&["config", "user.email", "gate@example.invalid"]);
            repository
        }

        fn write(&self, relative: &str, bytes: &[u8]) {
            let path = self.root.join(relative);
            fs::create_dir_all(path.parent().expect("fixture file has a parent"))
                .expect("create a fixture directory");
            fs::write(path, bytes).expect("write a fixture file");
        }

        fn git(&self, arguments: &[&str]) -> String {
            let output = Command::new("/usr/bin/git")
                .args(arguments)
                .current_dir(&self.root)
                .env_clear()
                .env("HOME", &self.root)
                .output()
                .unwrap_or_else(|source| panic!("run Git {arguments:?}: {source}"));
            assert!(
                output.status.success(),
                "Git {arguments:?} exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout)
                .expect("Git output is UTF-8")
                .trim()
                .to_owned()
        }

        fn commit(&self, message: &str) -> String {
            self.git(&["add", "--all"]);
            self.git(&["commit", "-q", "-m", message]);
            self.git(&["rev-parse", "HEAD"])
        }
    }

    impl Drop for Repository {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn byte_identical_index_replacement_is_still_a_changed_source() {
        let repository = Repository::new("index-replacement");
        repository.write("selected.txt", b"selected\n");
        repository.git(&["add", "selected.txt"]);
        let index = repository.root.join(".git/index");

        let refused = Selector::staged()
            .materialize_with(&repository.root, || {
                let replacement = repository.root.join(".git/index.replacement");
                fs::copy(&index, &replacement).expect("copy an identical index");
                fs::rename(&replacement, &index).expect("replace the captured index");
            })
            .expect_err("index replacement must refuse the selection");

        assert!(matches!(refused, TreeError::IndexChanged { .. }));
    }

    #[test]
    fn a_branch_move_after_resolution_refuses_the_captured_commit() {
        let repository = Repository::new("moving-revision");
        repository.write("selected.txt", b"first\n");
        let first = repository.commit("first");
        repository.write("selected.txt", b"second\n");
        repository.commit("second");

        let refused = Selector::revision("main")
            .materialize_with(&repository.root, || {
                repository.git(&["update-ref", "refs/heads/main", &first]);
            })
            .expect_err("a moving branch must refuse the selection");

        assert!(matches!(
            refused,
            TreeError::RevisionChanged { revision } if revision == "main"
        ));
    }
}
