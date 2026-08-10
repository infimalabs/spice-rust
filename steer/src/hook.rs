//! Installing the structural shell-interception boundary.
//!
//! Interception puts the wrapper in front of every agent-owned top-level shell
//! command. It is installed as two distinct executable stages: the outer stage
//! always enters the Rust wrapper, and the inner stage restores and execs the
//! user's native shell. The launch environment points the agent at the outer
//! stage; the inner stage points descendants at the native shell. Exactly-once
//! routing is therefore structural rather than a marker descendants can forge,
//! lose, or accidentally inherit.
//!
//! Both stages are published from bytes compiled into this crate, so what is on
//! disk either matches what this build expects or is named as divergent. That
//! matters because a stage that silently drifted would intercept in some other
//! way than the one this code reasons about.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The variable naming the installed hooks directory.
pub const HOOKS: &str = wire::SpiceVariable::Hooks.as_str();

/// The variable naming the lane's rendered direction.
pub const INBOX: &str = wire::SpiceVariable::Inbox.as_str();

/// The variable naming the lane a shell is bound to.
pub const LANE: &str = wire::SpiceVariable::Lane.as_str();

/// The variable carrying the user shell restored by the inner stage.
pub const NATIVE_SHELL: &str = wire::SpiceVariable::NativeShell.as_str();

/// One interception stage and the bytes it must carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stage {
    path: &'static str,
    bytes: &'static str,
}

impl Stage {
    /// Where this stage sits under the hooks directory.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        self.path
    }

    /// The bytes this build expects to find there.
    #[must_use]
    pub const fn bytes(&self) -> &'static str {
        self.bytes
    }
}

/// Every stage installation places, in the order a shell reaches them.
pub const STAGES: [Stage; 2] = [
    Stage {
        path: "entry.sh",
        bytes: include_str!("../hooks/entry.sh"),
    },
    Stage {
        path: "static/stage.sh",
        bytes: include_str!("../hooks/static/stage.sh"),
    },
];

/// An installed hooks directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hooks {
    root: PathBuf,
}

impl Hooks {
    /// The hooks directory at this path.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Where the hooks directory sits.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where one stage sits.
    #[must_use]
    pub fn stage(&self, stage: Stage) -> PathBuf {
        self.root.join(stage.path())
    }

    /// Installs every stage, replacing each whole.
    ///
    /// Installing again over the same build writes the same bytes, so this is
    /// repeatable rather than something a caller must check first.
    ///
    /// # Errors
    ///
    /// Returns [`InstallError::Directory`] when a stage's directory cannot be
    /// created, [`InstallError::Publication`] when a stage cannot be placed,
    /// and [`InstallError::Permission`] when a placed stage cannot be made
    /// executable.
    pub fn install(&self) -> Result<(), InstallError> {
        for stage in STAGES {
            let path = self.stage(stage);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(InstallError::Directory)?;
            }
            let destination = atomic::Destination::new(&path).map_err(InstallError::Publication)?;
            destination
                .publish(|staged| staged.write_all(stage.bytes().as_bytes()))
                .map_err(InstallError::Publication)?;
            make_executable(&path)?;
        }
        Ok(())
    }

    /// Verifies interception is installed as this build expects.
    ///
    /// # Errors
    ///
    /// Returns [`Refusal::Absent`] when a stage is missing, [`Refusal::Unread`]
    /// when one cannot be read, and [`Refusal::Divergent`] when one differs from
    /// this build.
    pub fn verify(&self) -> Result<(), Refusal> {
        for stage in STAGES {
            let path = self.stage(stage);
            let found = match fs::read_to_string(&path) {
                Ok(found) => found,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(Refusal::Absent { stage: path });
                }
                Err(error) => {
                    return Err(Refusal::Unread {
                        stage: path,
                        reason: error.to_string(),
                    });
                }
            };
            if found != stage.bytes() {
                return Err(Refusal::Divergent { stage: path });
            }
        }
        Ok(())
    }
}

/// Why interception could not be installed.
#[derive(Debug)]
pub enum InstallError {
    /// A stage's directory could not be created.
    Directory(std::io::Error),
    /// A stage could not replace its destination.
    Publication(atomic::Error),
    /// A placed stage could not be made executable.
    Permission(std::io::Error),
}

impl fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Directory(error) => write!(formatter, "hook directory: {error}"),
            Self::Publication(error) => write!(formatter, "hook installation: {error}"),
            Self::Permission(error) => write!(formatter, "hook permission: {error}"),
        }
    }
}

impl Error for InstallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Directory(error) | Self::Permission(error) => Some(error),
            Self::Publication(error) => Some(error),
        }
    }
}

/// Why the installed structural stages could not be verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A stage is not installed.
    Absent {
        /// Where the stage was expected.
        stage: PathBuf,
    },
    /// A stage is installed but could not be read.
    Unread {
        /// Where the stage sits.
        stage: PathBuf,
        /// What the read reported.
        reason: String,
    },
    /// A stage differs from the bytes this build installs.
    Divergent {
        /// Where the stage sits.
        stage: PathBuf,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absent { stage } => write!(
                formatter,
                "interception is not installed: {} is missing",
                stage.display()
            ),
            Self::Unread { stage, reason } => write!(
                formatter,
                "interception cannot be proven: {} could not be read: {reason}",
                stage.display()
            ),
            Self::Divergent { stage } => write!(
                formatter,
                "interception differs from this build: {} is not the stage it installs",
                stage.display()
            ),
        }
    }
}

impl Error for Refusal {}

/// Makes a placed stage executable where the platform has the notion.
#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), InstallError> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .map_err(InstallError::Permission)?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).map_err(InstallError::Permission)
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), InstallError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{HOOKS, NATIVE_SHELL, STAGES};
    use std::collections::BTreeSet;

    /// The prefix every name in this vocabulary carries.
    const PREFIX: &str = "SPICE_";

    /// The stages spell exactly the vocabulary this module declares.
    ///
    /// The names live in two languages, and nothing in either one refers to the
    /// other: a stage could reach for `SPICE_ANYTHING` and a shell would hand it
    /// the empty string rather than complain. Reading the compiled stage bytes
    /// against the declared constants closes that, in both directions — an
    /// undeclared name in a stage fails here, and so does a constant no stage
    /// uses.
    #[test]
    fn the_stages_spell_the_vocabulary_this_module_declares() {
        let declared: BTreeSet<&str> = [HOOKS, NATIVE_SHELL].into_iter().collect();
        let mut spelled = BTreeSet::new();
        for stage in STAGES {
            spelled.extend(names(stage.bytes()));
        }

        assert_eq!(spelled, declared);
    }

    /// Every `SPICE_`-prefixed name one stage reads or writes.
    fn names(script: &str) -> Vec<&str> {
        let mut found = Vec::new();
        let mut rest = script;
        while let Some(begin) = rest.find(PREFIX) {
            let name = &rest[begin..];
            let width = name
                .bytes()
                .take_while(|byte| byte.is_ascii_uppercase() || *byte == b'_')
                .count();
            found.push(&name[..width]);
            rest = &name[width..];
        }
        found
    }
}
