use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

#[cfg(target_family = "unix")]
use std::os::unix::{ffi::OsStrExt as _, fs::MetadataExt as _};
#[cfg(not(target_family = "unix"))]
use std::time::SystemTime;

use super::{
    TreeError, create_private_directory_tree, create_private_workspace, normalized_path,
    open_repository, resolve_revision, write_private_source,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct IndexSeal {
    files: Vec<ObservedFile>,
    directory: ObservedDirectory,
}

impl IndexSeal {
    pub(super) fn capture(repo: &gix::Repository) -> Result<Self, TreeError> {
        let first = Self::capture_once(repo)?;
        let second = Self::capture_once(repo).map_err(|_| TreeError::IndexChanged {
            path: repo.index_path(),
        })?;
        if first != second {
            return Err(TreeError::IndexChanged {
                path: repo.index_path(),
            });
        }
        Ok(second)
    }

    fn capture_once(repo: &gix::Repository) -> Result<Self, TreeError> {
        let index = repo.index_path();
        let directory =
            index
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| TreeError::IndexUnreadable {
                    path: index.clone(),
                    evidence: "the index has no parent directory".to_owned(),
                })?;
        let mut paths = vec![index];
        let listing = fs::read_dir(&directory)
            .map_err(|source| TreeError::io("listing index sources", &directory, source))?;
        for entry in listing {
            let entry = entry
                .map_err(|source| TreeError::io("reading index sources", &directory, source))?;
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("sharedindex."))
            {
                paths.push(entry.path());
            }
        }
        paths.sort();
        let files = paths
            .into_iter()
            .map(|path| {
                observed_file(&path).map_err(|source| {
                    if path == repo.index_path() && source.kind() == io::ErrorKind::NotFound {
                        TreeError::IndexAbsent { path }
                    } else {
                        TreeError::io("capturing an index source", &path, source)
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let directory = observed_directory(&directory)
            .map_err(|source| TreeError::io("capturing the index directory", &directory, source))?;
        Ok(Self { files, directory })
    }

    pub(super) fn open(
        &self,
        repo: &gix::Repository,
        workspace: &Path,
    ) -> Result<gix::index::File, TreeError> {
        let source = workspace.join("index-source");
        create_private_workspace(&source).map_err(|error| {
            TreeError::io("creating a captured index directory", &source, error)
        })?;
        let original_index = repo.index_path();
        for file in &self.files {
            let name = if file.path == original_index {
                "index".into()
            } else {
                file.path
                    .file_name()
                    .ok_or_else(|| TreeError::IndexUnreadable {
                        path: file.path.clone(),
                        evidence: "an index source has no file name".to_owned(),
                    })?
                    .to_owned()
            };
            write_private_source(&source, &source.join(name), &file.bytes)?;
        }
        let captured_index = source.join("index");
        gix::index::File::at(
            &captured_index,
            repo.object_hash(),
            false,
            gix::index::decode::Options::default(),
        )
        .map_err(|error| TreeError::IndexUnreadable {
            path: original_index,
            evidence: error.to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RevisionSeal {
    ObjectId(gix::ObjectId),
    Metadata(RevisionMetadata),
}

impl RevisionSeal {
    pub(super) fn capture(repo: &gix::Repository, revision: &str) -> Result<Self, TreeError> {
        if let Ok(id) = revision.parse::<gix::ObjectId>()
            && id.kind() == repo.object_hash()
        {
            return Ok(Self::ObjectId(id));
        }
        let first = RevisionMetadata::capture_once(repo, revision).map_err(|source| {
            TreeError::io("capturing revision sources", repo.git_dir(), source)
        })?;
        let second = RevisionMetadata::capture_once(repo, revision).map_err(|_| {
            TreeError::RevisionChanged {
                revision: revision.to_owned(),
            }
        })?;
        if first != second {
            return Err(TreeError::RevisionChanged {
                revision: revision.to_owned(),
            });
        }
        Ok(Self::Metadata(second))
    }

    pub(super) fn resolve(
        &self,
        repo: &gix::Repository,
        revision: &str,
        workspace: &Path,
    ) -> Result<gix::ObjectId, TreeError> {
        match self {
            Self::ObjectId(id) => Ok(*id),
            Self::Metadata(metadata) => metadata.resolve(repo, revision, workspace),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RevisionMetadata {
    files: Vec<CapturedRevisionFile>,
    directories: Vec<CapturedRevisionDirectory>,
    linked: bool,
}

impl RevisionMetadata {
    fn capture_once(repo: &gix::Repository, revision: &str) -> io::Result<Self> {
        let (file_paths, directory_paths) = revision_source_paths(repo, revision)?;
        let files = file_paths
            .into_iter()
            .map(|source| {
                let path = revision_source_path(repo, &source);
                observed_file(&path).map(|observed| CapturedRevisionFile { source, observed })
            })
            .collect::<io::Result<Vec<_>>>()?;
        let directories = directory_paths
            .into_iter()
            .map(|source| {
                let path = revision_source_path(repo, &source);
                observed_directory(&path)
                    .map(|observed| CapturedRevisionDirectory { source, observed })
            })
            .collect::<io::Result<Vec<_>>>()?;
        Ok(Self {
            files,
            directories,
            linked: repo.common_dir() != repo.git_dir(),
        })
    }

    fn resolve(
        &self,
        repo: &gix::Repository,
        revision: &str,
        workspace: &Path,
    ) -> Result<gix::ObjectId, TreeError> {
        let source = workspace.join("revision-source");
        create_private_workspace(&source).map_err(|error| {
            TreeError::io("creating captured revision metadata", &source, error)
        })?;
        let common = source.join("common.git");
        create_private_directory_tree(&source, &common)?;
        let git = if self.linked {
            let git = common.join("worktrees/selected");
            create_private_directory_tree(&source, &git)?;
            write_private_source(&source, &git.join("commondir"), b"../..\n")?;
            let gitfile = alternate_path_bytes(&source.join("worktree/.git"))?;
            write_private_source(&source, &git.join("gitdir"), &gitfile)?;
            git
        } else {
            common.clone()
        };
        create_private_directory_tree(&source, &common.join("objects/info"))?;
        create_private_directory_tree(&source, &common.join("refs"))?;
        write_private_source(
            &source,
            &common.join("config"),
            if self.linked {
                b"[core]\nrepositoryformatversion = 0\nbare = false\n"
            } else {
                b"[core]\nrepositoryformatversion = 0\nbare = true\n"
            },
        )?;
        let alternates = alternate_path_bytes(&repo.common_dir().join("objects"))?;
        write_private_source(
            &source,
            &common.join("objects/info/alternates"),
            &alternates,
        )?;
        for file in &self.files {
            let base = match file.source.area {
                RevisionArea::Common => &common,
                RevisionArea::Worktree => &git,
            };
            write_private_source(
                &source,
                &base.join(&file.source.relative),
                &file.observed.bytes,
            )?;
        }

        let snapshot = open_repository(&git).map_err(|error| TreeError::RevisionUnresolved {
            revision: revision.to_owned(),
            evidence: error.to_string(),
        })?;
        resolve_revision(&snapshot, revision)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RevisionArea {
    Common,
    Worktree,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RevisionSourcePath {
    area: RevisionArea,
    relative: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedRevisionFile {
    source: RevisionSourcePath,
    observed: ObservedFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedRevisionDirectory {
    source: RevisionSourcePath,
    observed: ObservedDirectory,
}

fn revision_source_paths(
    repo: &gix::Repository,
    revision: &str,
) -> io::Result<(BTreeSet<RevisionSourcePath>, BTreeSet<RevisionSourcePath>)> {
    let linked = repo.common_dir() != repo.git_dir();
    let worktree_area = if linked {
        RevisionArea::Worktree
    } else {
        RevisionArea::Common
    };
    let mut files = BTreeSet::new();
    let mut directories = BTreeSet::new();
    directories.insert(RevisionSourcePath {
        area: worktree_area,
        relative: PathBuf::new(),
    });
    directories.insert(RevisionSourcePath {
        area: RevisionArea::Common,
        relative: PathBuf::new(),
    });
    files.insert(RevisionSourcePath {
        area: worktree_area,
        relative: PathBuf::from("HEAD"),
    });
    if linked {
        add_optional_revision_file(repo, RevisionArea::Common, Path::new("HEAD"), &mut files)?;
    }
    for area in [worktree_area, RevisionArea::Common] {
        collect_revision_directory(repo, area, Path::new("refs"), &mut files, &mut directories)?;
        collect_revision_directory(repo, area, Path::new("logs"), &mut files, &mut directories)?;
        for relative in ["packed-refs", "shallow"] {
            add_optional_revision_file(repo, area, Path::new(relative), &mut files)?;
        }
    }

    let atom_end = revision
        .char_indices()
        .find_map(|(index, character)| "~^:@".contains(character).then_some(index))
        .unwrap_or(revision.len());
    let atom = &revision[..atom_end];
    if normalized_path(atom.as_bytes()).is_ok() {
        add_optional_revision_file(repo, worktree_area, Path::new(atom), &mut files)?;
    }
    Ok((files, directories))
}

fn collect_revision_directory(
    repo: &gix::Repository,
    area: RevisionArea,
    relative: &Path,
    files: &mut BTreeSet<RevisionSourcePath>,
    directories: &mut BTreeSet<RevisionSourcePath>,
) -> io::Result<()> {
    let source = RevisionSourcePath {
        area,
        relative: relative.to_path_buf(),
    };
    let path = revision_source_path(repo, &source);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("revision source {} is not a directory", path.display()),
        ));
    }
    directories.insert(source);
    for entry in fs::read_dir(&path)? {
        let entry = entry?;
        let child_relative = relative.join(entry.file_name());
        let child = RevisionSourcePath {
            area,
            relative: child_relative.clone(),
        };
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.is_dir() {
            collect_revision_directory(repo, area, &child_relative, files, directories)?;
        } else if metadata.is_file() {
            files.insert(child);
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "revision source {} is not a regular file",
                    entry.path().display()
                ),
            ));
        }
    }
    Ok(())
}

fn add_optional_revision_file(
    repo: &gix::Repository,
    area: RevisionArea,
    relative: &Path,
    files: &mut BTreeSet<RevisionSourcePath>,
) -> io::Result<()> {
    let source = RevisionSourcePath {
        area,
        relative: relative.to_path_buf(),
    };
    let path = revision_source_path(repo, &source);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {
            files.insert(source);
            Ok(())
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("revision source {} is not a regular file", path.display()),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn revision_source_path(repo: &gix::Repository, source: &RevisionSourcePath) -> PathBuf {
    match source.area {
        RevisionArea::Common => repo.common_dir(),
        RevisionArea::Worktree => repo.git_dir(),
    }
    .join(&source.relative)
}

fn alternate_path_bytes(path: &Path) -> Result<Vec<u8>, TreeError> {
    #[cfg(target_family = "unix")]
    let mut bytes = path.as_os_str().as_bytes().to_vec();
    #[cfg(not(target_family = "unix"))]
    let mut bytes = path
        .to_str()
        .ok_or_else(|| {
            TreeError::io(
                "encoding an object directory",
                path,
                io::Error::new(io::ErrorKind::InvalidData, "object path is not Unicode"),
            )
        })?
        .as_bytes()
        .to_vec();
    bytes.push(b'\n');
    Ok(bytes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedFile {
    path: PathBuf,
    identity: FileIdentity,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedDirectory {
    path: PathBuf,
    identity: FileIdentity,
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

fn observed_directory(path: &Path) -> io::Result<ObservedDirectory> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source is not a directory",
        ));
    }
    Ok(ObservedDirectory {
        path: path.to_path_buf(),
        identity: file_identity(&metadata),
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
