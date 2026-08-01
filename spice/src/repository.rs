//! Resolve the one repository-common Spice state root from Git's own files.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// One repository resolved to its worktree and shared Git directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Repository {
    worktree: PathBuf,
    common: PathBuf,
}

impl Repository {
    /// Resolves the nearest repository containing `start`.
    ///
    /// The caller supplies the starting directory so this library acquires no
    /// ambient working directory. A normal worktree owns a `.git` directory;
    /// a linked worktree owns a `.git` pointer whose target may in turn carry
    /// Git's `commondir` file. Those are the only admitted shapes.
    pub fn discover(start: impl AsRef<Path>) -> Result<Self, ResolveError> {
        let requested = start.as_ref();
        let start = fs::canonicalize(requested)
            .map_err(|source| ResolveError::filesystem(requested, source))?;
        if !metadata(&start)?.is_dir() {
            return Err(ResolveError::StartNotDirectory { path: start });
        }

        let mut cursor = start.clone();
        loop {
            let marker = cursor.join(".git");
            match fs::metadata(&marker) {
                Ok(kind) if kind.is_dir() => {
                    return Ok(Self {
                        worktree: cursor,
                        common: common_directory(directory(&marker, DirectoryKind::Git)?)?,
                    });
                }
                Ok(kind) if kind.is_file() => {
                    let git = gitdir_pointer(&marker)?;
                    return Ok(Self {
                        worktree: cursor,
                        common: common_directory(git)?,
                    });
                }
                Ok(_) => return Err(ResolveError::MalformedGitdir { path: marker }),
                Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                Err(source) => return Err(ResolveError::filesystem(&marker, source)),
            }

            if !cursor.pop() {
                return Err(ResolveError::NoRepository { start });
            }
        }
    }

    /// Canonical root of the nearest worktree containing the requested start.
    #[must_use]
    pub fn worktree_root(&self) -> &Path {
        &self.worktree
    }

    /// Canonical Git directory shared by every linked worktree.
    #[must_use]
    pub fn common_dir(&self) -> &Path {
        &self.common
    }

    /// The sole authority database path under repository-common Spice state.
    #[must_use]
    pub fn journal_path(&self) -> PathBuf {
        self.common
            .join(".spice")
            .join("data")
            .join("spicejournal.sqlite3")
    }
}

fn gitdir_pointer(marker: &Path) -> Result<PathBuf, ResolveError> {
    let line = one_line(marker, PointerKind::Gitdir)?;
    let Some(value) = line.strip_prefix("gitdir: ") else {
        return Err(ResolveError::MalformedGitdir {
            path: marker.to_path_buf(),
        });
    };
    if value.is_empty() {
        return Err(ResolveError::MalformedGitdir {
            path: marker.to_path_buf(),
        });
    }
    let target = resolve_relative(marker.parent().expect(".git has a parent"), value);
    directory(&target, DirectoryKind::Git)
}

fn common_directory(git: PathBuf) -> Result<PathBuf, ResolveError> {
    let pointer = git.join("commondir");
    match fs::metadata(&pointer) {
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(git),
        Err(source) => return Err(ResolveError::filesystem(&pointer, source)),
        Ok(kind) if !kind.is_file() => {
            return Err(ResolveError::MalformedCommondir { path: pointer });
        }
        Ok(_) => {}
    }

    let line = one_line(&pointer, PointerKind::Commondir)?;
    if line.is_empty() {
        return Err(ResolveError::MalformedCommondir { path: pointer });
    }
    directory(&resolve_relative(&git, &line), DirectoryKind::Common)
}

fn one_line(path: &Path, kind: PointerKind) -> Result<String, ResolveError> {
    let mut bytes = fs::read(path).map_err(|source| ResolveError::filesystem(path, source))?;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.is_empty() || bytes.contains(&b'\n') || bytes.contains(&b'\r') {
        return Err(kind.malformed(path));
    }
    String::from_utf8(bytes).map_err(|_| kind.malformed(path))
}

fn resolve_relative(parent: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        parent.join(path)
    }
}

fn directory(path: &Path, kind: DirectoryKind) -> Result<PathBuf, ResolveError> {
    match fs::metadata(path) {
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Err(kind.missing(path)),
        Err(source) => return Err(ResolveError::filesystem(path, source)),
        Ok(metadata) if !metadata.is_dir() => return Err(kind.not_directory(path)),
        Ok(_) => {}
    }
    fs::canonicalize(path).map_err(|source| ResolveError::filesystem(path, source))
}

fn metadata(path: &Path) -> Result<fs::Metadata, ResolveError> {
    fs::metadata(path).map_err(|source| ResolveError::filesystem(path, source))
}

#[derive(Clone, Copy)]
enum DirectoryKind {
    Git,
    Common,
}

impl DirectoryKind {
    fn missing(self, path: &Path) -> ResolveError {
        match self {
            Self::Git => ResolveError::MissingGitdir {
                path: path.to_path_buf(),
            },
            Self::Common => ResolveError::MissingCommondir {
                path: path.to_path_buf(),
            },
        }
    }

    fn not_directory(self, path: &Path) -> ResolveError {
        match self {
            Self::Git => ResolveError::GitdirNotDirectory {
                path: path.to_path_buf(),
            },
            Self::Common => ResolveError::CommondirNotDirectory {
                path: path.to_path_buf(),
            },
        }
    }
}

#[derive(Clone, Copy)]
enum PointerKind {
    Gitdir,
    Commondir,
}

impl PointerKind {
    fn malformed(self, path: &Path) -> ResolveError {
        match self {
            Self::Gitdir => ResolveError::MalformedGitdir {
                path: path.to_path_buf(),
            },
            Self::Commondir => ResolveError::MalformedCommondir {
                path: path.to_path_buf(),
            },
        }
    }
}

/// Why repository-common state could not be resolved exactly.
#[derive(Debug)]
pub enum ResolveError {
    /// The supplied starting path exists but is not a directory.
    StartNotDirectory { path: PathBuf },
    /// No `.git` entry exists at the start or any ancestor.
    NoRepository { start: PathBuf },
    /// A `.git` file is not one exact `gitdir: PATH` line.
    MalformedGitdir { path: PathBuf },
    /// A `.git` pointer names a path that does not exist.
    MissingGitdir { path: PathBuf },
    /// A `.git` pointer names something other than a directory.
    GitdirNotDirectory { path: PathBuf },
    /// A `commondir` file is not one nonempty path line.
    MalformedCommondir { path: PathBuf },
    /// A `commondir` pointer names a path that does not exist.
    MissingCommondir { path: PathBuf },
    /// A `commondir` pointer names something other than a directory.
    CommondirNotDirectory { path: PathBuf },
    /// A filesystem operation failed before a repository fact could be read.
    Filesystem { path: PathBuf, source: io::Error },
}

impl ResolveError {
    fn filesystem(path: &Path, source: io::Error) -> Self {
        Self::Filesystem {
            path: path.to_path_buf(),
            source,
        }
    }
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StartNotDirectory { path } => {
                write!(
                    formatter,
                    "repository start is not a directory: {}",
                    path.display()
                )
            }
            Self::NoRepository { start } => {
                write!(formatter, "no Git repository contains {}", start.display())
            }
            Self::MalformedGitdir { path } => write!(
                formatter,
                "Git directory pointer is malformed: {}",
                path.display()
            ),
            Self::MissingGitdir { path } => {
                write!(formatter, "Git directory is missing: {}", path.display())
            }
            Self::GitdirNotDirectory { path } => write!(
                formatter,
                "Git directory target is not a directory: {}",
                path.display()
            ),
            Self::MalformedCommondir { path } => write!(
                formatter,
                "Git common-directory pointer is malformed: {}",
                path.display()
            ),
            Self::MissingCommondir { path } => write!(
                formatter,
                "Git common directory is missing: {}",
                path.display()
            ),
            Self::CommondirNotDirectory { path } => write!(
                formatter,
                "Git common-directory target is not a directory: {}",
                path.display()
            ),
            Self::Filesystem { path, source } => write!(
                formatter,
                "repository filesystem access failed at {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for ResolveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Filesystem { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn main_and_nested_repositories_resolve_the_nearest_common_state() {
        let scratch = Scratch::new();
        let outer = scratch.join("outer repository");
        let inner = outer.join("nested repository");
        fs::create_dir_all(outer.join(".git")).expect("create outer Git directory");
        fs::create_dir_all(inner.join(".git")).expect("create inner Git directory");
        let start = inner.join("source tree").join("deep");
        fs::create_dir_all(&start).expect("create nested start");

        let repository = Repository::discover(&start).expect("resolve nearest repository");
        let worktree = fs::canonicalize(&inner).expect("canonical worktree root");
        let common = fs::canonicalize(inner.join(".git")).expect("canonical common directory");

        assert_eq!(repository.worktree_root(), worktree);
        assert_eq!(repository.common_dir(), common);
        assert_eq!(
            repository.journal_path(),
            common.join(".spice/data/spicejournal.sqlite3")
        );
        assert_eq!(
            proof::Layout::new(
                repository.worktree_root(),
                repository.common_dir(),
                wire::manifest::ReleaseTarget::Aarch64AppleDarwin,
            )
            .payload_path(),
            worktree.join("dist/stage/targets/aarch64-apple-darwin/payload.json")
        );
    }

    #[test]
    fn linked_worktree_resolves_relative_gitdir_and_commondir_with_spaces() {
        let scratch = Scratch::new();
        let origin = scratch.join("origin repository");
        let common = origin.join(".git");
        let git = common.join("worktrees").join("lane one");
        let worktree = scratch.join("linked worktree");
        fs::create_dir_all(&git).expect("create linked Git directory");
        fs::create_dir_all(worktree.join("nested")).expect("create linked worktree");
        fs::write(
            worktree.join(".git"),
            b"gitdir: ../origin repository/.git/worktrees/lane one\n",
        )
        .expect("write Git directory pointer");
        fs::write(git.join("commondir"), b"../..\n").expect("write common pointer");

        let repository =
            Repository::discover(worktree.join("nested")).expect("resolve linked worktree");

        assert_eq!(
            repository.worktree_root(),
            fs::canonicalize(&worktree).expect("canonical linked worktree")
        );
        assert_eq!(
            repository.common_dir(),
            fs::canonicalize(&common).expect("canonical common directory")
        );
        assert_eq!(
            proof::Layout::new(
                repository.worktree_root(),
                repository.common_dir(),
                wire::manifest::ReleaseTarget::X86_64UnknownLinuxMusl,
            )
            .payload_path(),
            fs::canonicalize(&worktree)
                .expect("canonical linked worktree")
                .join("dist/stage/targets/x86_64-unknown-linux-musl/payload.json")
        );

        let absolute_git = origin.join("absolute git directory");
        let absolute_worktree = scratch.join("absolute linked worktree");
        fs::create_dir(&absolute_git).expect("create absolute Git directory");
        fs::create_dir(&absolute_worktree).expect("create absolute linked worktree");
        fs::write(
            absolute_worktree.join(".git"),
            format!("gitdir: {}\n", absolute_git.display()),
        )
        .expect("write absolute Git pointer");
        fs::write(
            absolute_git.join("commondir"),
            format!("{}\n", common.display()),
        )
        .expect("write absolute common pointer");

        assert_eq!(
            Repository::discover(absolute_worktree)
                .expect("resolve absolute pointers")
                .common_dir(),
            fs::canonicalize(&common).expect("canonical absolute common directory")
        );
    }

    #[test]
    fn every_repository_shape_failure_is_named_at_its_path() {
        let scratch = Scratch::new();

        let no_repository = scratch.join("no repository").join("nested");
        fs::create_dir_all(&no_repository).expect("create ordinary directory");
        assert!(matches!(
            Repository::discover(&no_repository),
            Err(ResolveError::NoRepository { start })
                if start == fs::canonicalize(&no_repository).expect("canonical start")
        ));

        let malformed = case(&scratch, "malformed gitdir");
        fs::write(malformed.join(".git"), b"gitdir: target\nsecond line\n")
            .expect("write malformed pointer");
        assert!(matches!(
            Repository::discover(&malformed),
            Err(ResolveError::MalformedGitdir { path }) if path == malformed.join(".git")
        ));

        let missing = case(&scratch, "missing gitdir");
        fs::write(missing.join(".git"), b"gitdir: absent\n").expect("write missing pointer");
        assert!(matches!(
            Repository::discover(&missing),
            Err(ResolveError::MissingGitdir { path }) if path == missing.join("absent")
        ));

        let not_directory = case(&scratch, "file gitdir");
        fs::write(not_directory.join("target"), b"ordinary file").expect("write Git target");
        fs::write(not_directory.join(".git"), b"gitdir: target\n").expect("write Git pointer");
        assert!(matches!(
            Repository::discover(&not_directory),
            Err(ResolveError::GitdirNotDirectory { path }) if path == not_directory.join("target")
        ));

        let malformed_common = case(&scratch, "malformed commondir");
        fs::create_dir(malformed_common.join(".git")).expect("create Git directory");
        fs::write(
            malformed_common.join(".git/commondir"),
            b"target\nsecond line\n",
        )
        .expect("write malformed common pointer");
        assert!(matches!(
            Repository::discover(&malformed_common),
            Err(ResolveError::MalformedCommondir { path })
                if path == malformed_common.join(".git/commondir")
        ));

        let missing_common = case(&scratch, "missing commondir");
        fs::create_dir(missing_common.join(".git")).expect("create Git directory");
        fs::write(missing_common.join(".git/commondir"), b"absent\n")
            .expect("write missing common pointer");
        assert!(matches!(
            Repository::discover(&missing_common),
            Err(ResolveError::MissingCommondir { path })
                if path == missing_common.join(".git/absent")
        ));

        let common_file = case(&scratch, "file commondir");
        fs::create_dir(common_file.join(".git")).expect("create Git directory");
        fs::write(common_file.join(".git/target"), b"ordinary file").expect("write common target");
        fs::write(common_file.join(".git/commondir"), b"target\n").expect("write common pointer");
        assert!(matches!(
            Repository::discover(&common_file),
            Err(ResolveError::CommondirNotDirectory { path })
                if path == common_file.join(".git/target")
        ));

        let absent_start = scratch.join("absent start");
        assert!(matches!(
            Repository::discover(&absent_start),
            Err(ResolveError::Filesystem { path, .. }) if path == absent_start
        ));

        let file_start = scratch.join("file start");
        fs::write(&file_start, b"ordinary file").expect("write non-directory start");
        assert!(matches!(
            Repository::discover(&file_start),
            Err(ResolveError::StartNotDirectory { path }) if path == file_start
        ));
    }

    fn case(scratch: &Scratch, name: &str) -> PathBuf {
        let path = scratch.join(name);
        fs::create_dir(&path).expect("create refusal case");
        path
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "spice-repository-resolver-{}-{}",
                std::process::id(),
                NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create repository scratch");
            Self(fs::canonicalize(path).expect("canonical repository scratch"))
        }

        fn join(&self, path: impl AsRef<Path>) -> PathBuf {
            self.0.join(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("remove repository scratch");
        }
    }
}
