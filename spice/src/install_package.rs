//! Verified local release-package acquisition for the installed appliance.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use spice::repository::Repository;
use wire::manifest::ReleaseTarget;

const MAX_PACKAGE_MEMBERS: usize = 16;
const MAX_PACKAGE_ENTRIES: usize = 64;
const MAX_MEMBER_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PACKAGE_BYTES: u64 = 1024 * 1024 * 1024;

pub(crate) fn run(
    repository: &Repository,
    source: &Path,
    apply: Option<&str>,
    json: bool,
) -> Result<ExitStatus, Error> {
    let acquired = acquire(source)?;
    delegate(repository.worktree_root(), &acquired, apply, json)
}

fn delegate(
    worktree: &Path,
    acquired: &[u8],
    apply: Option<&str>,
    json: bool,
) -> Result<ExitStatus, Error> {
    let staged = StagedExecutable::new(acquired)?;
    let mut command = Command::new(staged.path());
    command.current_dir(worktree).arg("install");
    if let Some(digest) = apply {
        command.arg(format!("--apply={digest}"));
    }
    if json {
        command.arg("--json");
    }
    command.status().map_err(|source| {
        Error::environment("launch verified package executable", staged.path(), source)
    })
}

fn acquire(root: &Path) -> Result<Vec<u8>, Error> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::input(format!(
                "package root does not exist: {}",
                root.display()
            )));
        }
        Err(source) => {
            return Err(Error::environment("inspect package root", root, source));
        }
    };
    if !metadata.file_type().is_dir() {
        return Err(Error::input(format!(
            "package root is not an unlinked directory: {}",
            root.display()
        )));
    }
    let mut members = Vec::new();
    let mut bytes = 0_u64;
    let mut entries = 0_usize;
    collect(root, root, &mut members, &mut bytes, &mut entries)?;
    members.sort_by(|left, right| left.path().cmp(right.path()));
    let admitted =
        proof::admit_package_tree(&members).map_err(|error| Error::input(error.to_string()))?;
    let candidate = admitted.document().product_version.as_str();
    let current = dist::current_product_version()
        .expect("the compiled distribution product version is canonical");
    let current = current.as_str();
    if !newer(candidate, current) {
        return Err(Error::input(format!(
            "package version {candidate} is not newer than installed appliance version {current}"
        )));
    }
    let target = current_target()?;
    let selected = admitted
        .document()
        .targets
        .iter()
        .find(|candidate| candidate.target == target)
        .ok_or_else(|| Error::input(format!("package has no executable for {target}")))?;
    members
        .iter()
        .find(|member| member.path() == selected.executable_path)
        .map(|member| member.bytes().to_vec())
        .ok_or_else(|| {
            Error::input(format!(
                "verified package omits selected executable {}",
                selected.executable_path
            ))
        })
}

fn newer(candidate: &str, current: &str) -> bool {
    let (candidate_core, candidate_prerelease) = version_parts(candidate);
    let (current_core, current_prerelease) = version_parts(current);
    candidate_core
        .cmp(&current_core)
        .then_with(|| match (candidate_prerelease, current_prerelease) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(candidate), Some(current)) => candidate.cmp(&current),
        })
        .is_gt()
}

fn version_parts(version: &str) -> (Vec<Number<'_>>, Option<Vec<Identifier<'_>>>) {
    let without_build = version
        .split_once('+')
        .map_or(version, |(version, _)| version);
    let (core, prerelease) = without_build
        .split_once('-')
        .map_or((without_build, None), |(core, prerelease)| {
            (core, Some(prerelease))
        });
    (
        core.split('.').map(Number).collect(),
        prerelease.map(|value| value.split('.').map(Identifier::new).collect()),
    )
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Number<'a>(&'a str);

impl Ord for Number<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.cmp(other.0))
    }
}

impl PartialOrd for Number<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Identifier<'a> {
    Numeric(Number<'a>),
    Text(&'a str),
}

impl<'a> Identifier<'a> {
    fn new(value: &'a str) -> Self {
        if value.bytes().all(|byte| byte.is_ascii_digit()) {
            Self::Numeric(Number(value))
        } else {
            Self::Text(value)
        }
    }
}

impl Ord for Identifier<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Numeric(left), Self::Numeric(right)) => left.cmp(right),
            (Self::Numeric(_), Self::Text(_)) => std::cmp::Ordering::Less,
            (Self::Text(_), Self::Numeric(_)) => std::cmp::Ordering::Greater,
            (Self::Text(left), Self::Text(right)) => left.cmp(right),
        }
    }
}

impl PartialOrd for Identifier<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn collect(
    root: &Path,
    directory: &Path,
    members: &mut Vec<proof::PackageMember>,
    total: &mut u64,
    visited: &mut usize,
) -> Result<(), Error> {
    let entries = fs::read_dir(directory)
        .map_err(|source| Error::environment("read package directory", directory, source))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| Error::environment("read package entry", directory, source))?;
    for entry in entries {
        if *visited == MAX_PACKAGE_ENTRIES {
            return Err(Error::input(format!(
                "package has more than {MAX_PACKAGE_ENTRIES} filesystem entries"
            )));
        }
        *visited += 1;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|source| Error::environment("inspect package member", &path, source))?;
        if metadata.file_type().is_dir() {
            collect(root, &path, members, total, visited)?;
            continue;
        }
        if !metadata.file_type().is_file() {
            return Err(Error::input(format!(
                "package member is not an unlinked regular file: {}",
                path.display()
            )));
        }
        if members.len() == MAX_PACKAGE_MEMBERS {
            return Err(Error::input(format!(
                "package has more than {MAX_PACKAGE_MEMBERS} regular files"
            )));
        }
        if metadata.len() > MAX_MEMBER_BYTES {
            return Err(Error::input(format!(
                "package member exceeds {MAX_MEMBER_BYTES} bytes: {}",
                path.display()
            )));
        }
        *total = total
            .checked_add(metadata.len())
            .filter(|total| *total <= MAX_PACKAGE_BYTES)
            .ok_or_else(|| {
                Error::input(format!(
                    "package regular files exceed {MAX_PACKAGE_BYTES} bytes"
                ))
            })?;
        let relative = path
            .strip_prefix(root)
            .expect("a traversed package member remains below its root");
        let relative = relative.to_str().ok_or_else(|| {
            Error::input(format!("package member path is not UTF-8: {relative:?}"))
        })?;
        let mut file = File::open(&path)
            .map_err(|source| Error::environment("open package member", &path, source))?;
        let mut content = Vec::with_capacity(
            usize::try_from(metadata.len()).expect("bounded member length fits usize"),
        );
        file.read_to_end(&mut content)
            .map_err(|source| Error::environment("read package member", &path, source))?;
        if content.len() as u64 != metadata.len() {
            return Err(Error::input(format!(
                "package member changed while read: {}",
                path.display()
            )));
        }
        members.push(proof::PackageMember::new(
            relative.replace(std::path::MAIN_SEPARATOR, "/"),
            metadata.permissions().mode() & 0o7777,
            content,
        ));
    }
    Ok(())
}

fn current_target() -> Result<ReleaseTarget, Error> {
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    {
        Ok(ReleaseTarget::Aarch64AppleDarwin)
    }
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    {
        Ok(ReleaseTarget::X86_64UnknownLinuxMusl)
    }
    #[cfg(not(any(
        all(target_arch = "aarch64", target_os = "macos"),
        all(target_arch = "x86_64", target_os = "linux")
    )))]
    {
        Err(Error::input(format!(
            "this platform is not a supported release target: {}-{}",
            std::env::consts::ARCH,
            std::env::consts::OS
        )))
    }
}

struct StagedExecutable {
    path: PathBuf,
}

impl StagedExecutable {
    fn new(bytes: &[u8]) -> Result<Self, Error> {
        for _ in 0..32 {
            let path = std::env::temp_dir().join(format!(
                "spice-package-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o700)
                .open(&path);
            let mut file = match file {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(Error::environment(
                        "create verified package executable",
                        &path,
                        source,
                    ));
                }
            };
            let staged = Self { path };
            file.write_all(bytes).map_err(|source| {
                Error::environment("write verified package executable", staged.path(), source)
            })?;
            file.sync_all().map_err(|source| {
                Error::environment("sync verified package executable", staged.path(), source)
            })?;
            return Ok(staged);
        }
        Err(Error::input(
            "could not allocate a unique verified package executable",
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagedExecutable {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Debug)]
pub(crate) enum Error {
    Input(String),
    Environment {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
}

impl Error {
    pub(crate) fn input(detail: impl Into<String>) -> Self {
        Self::Input(detail.into())
    }

    fn environment(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self::Environment {
            operation,
            path: path.to_path_buf(),
            source,
        }
    }

    pub(crate) const fn is_environment(&self) -> bool {
        matches!(self, Self::Environment { .. })
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input(detail) => formatter.write_str(detail),
            Self::Environment {
                operation,
                path,
                source,
            } => write!(formatter, "{operation} at {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_executable_staging_is_exact_executable_and_ephemeral() {
        let bytes = b"#!/bin/sh\nexit 0\n";
        let staged = StagedExecutable::new(bytes).expect("verified executable stages");
        let path = staged.path().to_path_buf();
        assert_eq!(fs::read(&path).expect("staged executable reads"), bytes);
        assert_eq!(
            fs::metadata(&path)
                .expect("staged executable metadata reads")
                .permissions()
                .mode()
                & 0o777,
            0o700,
        );
        drop(staged);
        assert!(!path.exists());
    }

    #[test]
    fn package_root_must_be_an_unlinked_directory() {
        let path = std::env::temp_dir().join(format!(
            "spice-package-root-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::write(&path, b"not a package directory").expect("fixture file writes");
        let error = acquire(&path).expect_err("regular file cannot be a package root");
        fs::remove_file(&path).expect("fixture file removes");
        assert!(matches!(error, Error::Input(_)));
        assert!(error.to_string().contains("unlinked directory"));
    }

    #[test]
    fn verified_generation_receives_only_the_existing_install_transaction() {
        let root = std::env::temp_dir().join(format!(
            "spice-package-delegate-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&root).expect("delegate fixture root creates");
        let observed = root.join("arguments");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\n",
            observed.display()
        );
        let status = delegate(&root, script.as_bytes(), Some("abc123"), true)
            .expect("verified generation launches");
        assert!(status.success());
        assert_eq!(
            fs::read_to_string(&observed).expect("delegated argv reads"),
            "install --apply=abc123 --json\n"
        );
        fs::remove_dir_all(&root).expect("delegate fixture root removes");
    }

    #[test]
    fn package_version_must_advance_semantic_precedence() {
        for (candidate, current) in [
            ("1.2.4", "1.2.3"),
            ("1.10.0", "1.9.9"),
            ("2.0.0", "2.0.0-rc.1"),
            ("2.0.0-rc.10", "2.0.0-rc.2"),
        ] {
            assert!(
                newer(candidate, current),
                "{candidate} must advance {current}"
            );
        }
        for (candidate, current) in [
            ("1.2.3", "1.2.3"),
            ("1.2.3+new", "1.2.3+old"),
            ("1.9.9", "1.10.0"),
            ("2.0.0-rc.1", "2.0.0"),
        ] {
            assert!(
                !newer(candidate, current),
                "{candidate} must not advance {current}"
            );
        }
    }
}
