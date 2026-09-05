//! Resolution of declarative agent-shell command routes.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use crate::repository::Repository;

/// Resolve one tracked wrapper reference against the admitted repository.
///
/// # Errors
///
/// Refuses a redirected, escaping, missing, non-file, or non-executable
/// appliance or installed shell-wrapper command.
pub fn resolve(
    reference: steer::WrapperReference,
    repository: &Repository,
) -> Result<Resolved, ResolveError> {
    match reference {
        steer::WrapperReference::Appliance => resolve_appliance(repository),
    }
}

fn resolve_appliance(repository: &Repository) -> Result<Resolved, ResolveError> {
    let root =
        fs::canonicalize(repository.common_dir()).map_err(|source| ResolveError::Unavailable {
            path: repository.common_dir().to_path_buf(),
            source: Some(source),
        })?;
    let executable = crate::install::installed_binary(repository);
    validate_executable(executable.clone(), &root)?;
    let command = crate::install::installed_spice_wrapper(repository);
    validate_executable(command.clone(), &root)?;
    Ok(Resolved {
        executable,
        command,
    })
}

fn validate_executable(path: PathBuf, repository: &Path) -> Result<(), ResolveError> {
    let metadata = fs::symlink_metadata(&path).map_err(|source| ResolveError::Unavailable {
        path: path.clone(),
        source: Some(source),
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ResolveError::Redirected { path });
    }
    #[cfg(unix)]
    let executable = metadata.permissions().mode() & 0o111 != 0;
    #[cfg(not(unix))]
    let executable = true;
    if !metadata.is_file() || !executable {
        return Err(ResolveError::Unavailable { path, source: None });
    }
    let physical = fs::canonicalize(&path).map_err(|source| ResolveError::Unavailable {
        path: path.clone(),
        source: Some(source),
    })?;
    if !physical.starts_with(repository) {
        return Err(ResolveError::OutsideRepository { path, physical });
    }
    Ok(())
}

/// The two deliberately separate paths selected by one configured route.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Resolved {
    executable: PathBuf,
    command: PathBuf,
}

impl Resolved {
    /// Exact Rust appliance path used by hooks and MCP commands.
    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Shell-wrapper command whose directory may lead agent `PATH`.
    #[must_use]
    pub fn command(&self) -> &Path {
        &self.command
    }
}

/// Why a configured wrapper reference could not become one admitted route.
#[derive(Debug)]
pub enum ResolveError {
    /// The installed command path is a symbolic link.
    Redirected { path: PathBuf },
    /// The installed command is absent or not one executable file.
    Unavailable {
        path: PathBuf,
        source: Option<io::Error>,
    },
    /// The command's parent chain redirects it beyond Git-common authority.
    OutsideRepository { path: PathBuf, physical: PathBuf },
}

impl ResolveError {
    /// Stable launch refusal emitted at the resident boundary.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::Redirected { .. } => "lane::wrapper-command-redirected",
            Self::Unavailable { .. } => "lane::wrapper-command-unavailable",
            Self::OutsideRepository { .. } => "lane::wrapper-command-outside-repository",
        }
    }

    /// Exact configured path that failed admission.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Redirected { path }
            | Self::Unavailable { path, .. }
            | Self::OutsideRepository { path, .. } => path,
        }
    }
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Redirected { path } => write!(
                formatter,
                "installed Spice command is a symbolic link: {}",
                path.display()
            ),
            Self::Unavailable { path, .. } => write!(
                formatter,
                "installed Spice command is not one executable file: {}",
                path.display()
            ),
            Self::OutsideRepository { path, physical } => write!(
                formatter,
                "installed Spice command resolves outside the repository: {} -> {}",
                path.display(),
                physical.display(),
            ),
        }
    }
}

impl Error for ResolveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unavailable {
                source: Some(source),
                ..
            } => Some(source),
            Self::Redirected { .. }
            | Self::Unavailable { source: None, .. }
            | Self::OutsideRepository { .. } => None,
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "spice-agent-wrapper-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir_all(root.join(".git")).expect("create wrapper fixture repository");
            Self { root }
        }

        fn repository(&self) -> Repository {
            Repository::discover(&self.root).expect("wrapper fixture repository discovers")
        }

        fn installed(&self) -> PathBuf {
            crate::install::installed_binary(&self.repository())
        }

        fn command(&self) -> PathBuf {
            crate::install::installed_spice_wrapper(&self.repository())
        }

        fn publish_executable(path: &Path) {
            fs::create_dir_all(path.parent().expect("installed command has a parent"))
                .expect("create installed command parent");
            fs::write(path, b"#!/bin/sh\nexit 0\n").expect("write installed command");
            fs::set_permissions(path, fs::Permissions::from_mode(0o755))
                .expect("make installed command executable");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.root)
                && !std::thread::panicking()
            {
                panic!("remove {}: {error}", self.root.display());
            }
        }
    }

    #[test]
    fn appliance_reference_resolves_one_executable_under_git_common() {
        let scratch = Scratch::new("admitted");
        let installed = scratch.installed();
        let command = scratch.command();
        Scratch::publish_executable(&installed);
        Scratch::publish_executable(&command);

        let resolved = resolve(steer::WrapperReference::Appliance, &scratch.repository())
            .expect("appliance reference resolves");
        assert_eq!(resolved.executable(), installed);
        assert_eq!(resolved.command(), command);
    }

    #[test]
    fn appliance_reference_refuses_missing_nonregular_nonexecutable_and_redirected_commands() {
        let scratch = Scratch::new("refused");
        let installed = scratch.installed();
        Scratch::publish_executable(&scratch.command());
        let repository = scratch.repository();
        assert!(matches!(
            resolve(steer::WrapperReference::Appliance, &repository),
            Err(ResolveError::Unavailable { .. })
        ));

        fs::create_dir_all(installed.parent().expect("installed command has a parent"))
            .expect("create appliance bin");
        fs::write(&installed, b"not executable\n").expect("write inert command");
        fs::set_permissions(&installed, fs::Permissions::from_mode(0o644))
            .expect("keep installed command inert");
        assert!(matches!(
            resolve(steer::WrapperReference::Appliance, &repository),
            Err(ResolveError::Unavailable { .. })
        ));

        fs::remove_file(&installed).expect("remove inert command");
        fs::create_dir(&installed).expect("create non-regular command");
        assert!(matches!(
            resolve(steer::WrapperReference::Appliance, &repository),
            Err(ResolveError::Unavailable { .. })
        ));

        fs::remove_dir(&installed).expect("remove non-regular command");
        let target = scratch.root.join("redirected-spice");
        fs::write(&target, b"#!/bin/sh\nexit 0\n").expect("write redirect target");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755))
            .expect("make redirect target executable");
        symlink(&target, &installed).expect("redirect installed command");
        assert!(matches!(
            resolve(steer::WrapperReference::Appliance, &repository),
            Err(ResolveError::Redirected { .. })
        ));
    }

    #[test]
    fn appliance_reference_refuses_an_unusable_shell_wrapper_separately() {
        let scratch = Scratch::new("wrapper-refused");
        Scratch::publish_executable(&scratch.installed());
        let command = scratch.command();

        let error = resolve(steer::WrapperReference::Appliance, &scratch.repository())
            .expect_err("missing shell wrapper refuses the route");
        assert!(matches!(error, ResolveError::Unavailable { .. }));
        assert_eq!(error.path(), command);
    }

    #[test]
    fn appliance_reference_refuses_a_parent_redirect_outside_git_common() {
        let scratch = Scratch::new("outside");
        let external = Scratch::new("outside-target");
        let installed = scratch.installed();
        let redirected_parent = installed
            .parent()
            .expect("installed appliance has a parent");
        fs::create_dir_all(
            redirected_parent
                .parent()
                .expect("bin has an appliance parent"),
        )
        .expect("create appliance root");
        let external_bin = external.root.join("bin");
        fs::create_dir(&external_bin).expect("create external bin");
        symlink(&external_bin, redirected_parent).expect("redirect appliance bin");
        Scratch::publish_executable(&installed);

        let error = resolve(steer::WrapperReference::Appliance, &scratch.repository())
            .expect_err("a parent redirect outside Git-common refuses");
        assert!(matches!(error, ResolveError::OutsideRepository { .. }));
        assert_eq!(error.path(), installed);
    }
}
