//! Immutable Git tree selection for gate runs.
//!
//! A gate never measures the checkout. The bare selector captures one parsed
//! index, while a revision selector resolves one immutable commit identity.
//! Both sources pass through the same path, mode, object, and materialization
//! checks before a private read-only root is returned to the caller.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_family = "unix")]
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};

mod error;
mod source;
#[cfg(test)]
mod tests;

pub use error::TreeError;
use source::{IndexSeal, RevisionSeal};

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
        self.materialize_with(repository, |_| {})
    }

    fn materialize_with(
        &self,
        repository: &Path,
        after_capture: impl FnOnce(&Path),
    ) -> Result<SelectedTree, TreeError> {
        let repo = open_repository(repository)?;
        let mut build = BuildRoot::create()?;

        let (items, source) = match self {
            Self::Staged => {
                let before = IndexSeal::capture(&repo)?;
                after_capture(&build.workspace);
                let index = before.open(&repo, &build.workspace)?;
                let items = index_items(&index)?;
                materialize_items(&repo, &build.root, &items)?;
                (items, SelectionSeal::Index(before))
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
