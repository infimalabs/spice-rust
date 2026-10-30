//! The owner-only record of one installed lifecycle owner.
//!
//! The record carries everything the keeper needs to relaunch the resident
//! exactly as it was installed, including the bearer token, which is why it
//! is a private file in the repository's runtime root and never an argument
//! of the registered task.

use std::fs;
use std::io::{self, Write as _};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sill::authority::{create_private_file, prepare_replacement};

/// Scratch name the record is written to before replacing the current one.
const REPLACEMENT_SUFFIX: &str = ".next";

/// One installed lifecycle owner.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KeeperRecord {
    /// Task Scheduler path of the owner's task.
    pub(crate) task: String,
    /// The appliance registered as the task's action.
    pub(crate) executable: PathBuf,
    /// Directory the task starts the keeper in.
    pub(crate) working_directory: PathBuf,
    /// Address the kept resident listens on; port 0 until the first resident
    /// bound one, which is then pinned so relaunches keep the endpoint.
    pub(crate) listen: SocketAddr,
    /// Whether a non-loopback bind was consented to at install.
    pub(crate) allow_insecure_bind: bool,
    /// Bearer token every kept resident requires, handed over privately.
    pub(crate) auth_token: Option<String>,
}

impl KeeperRecord {
    /// The argument vector the keeper launches the resident with; the token
    /// travels over the launch handoff instead.
    pub(crate) fn launch_argv(&self) -> Vec<String> {
        let mut argv = vec![
            "spice".to_owned(),
            "--json".to_owned(),
            "up".to_owned(),
            "--host".to_owned(),
            self.listen.ip().to_string(),
            "--port".to_owned(),
            self.listen.port().to_string(),
        ];
        if self.allow_insecure_bind {
            argv.push("--allow-insecure-bind".to_owned());
        }
        argv
    }

    /// Pin the port a resident actually bound when the install left it to
    /// the system; a record that already names a port is unchanged.
    pub(crate) fn pin_port(&mut self, endpoint: &str) -> bool {
        if self.listen.port() != 0 {
            return false;
        }
        let Some(port) = endpoint_port(endpoint) else {
            return false;
        };
        self.listen.set_port(port);
        true
    }
}

/// The port of one `scheme://host:port` endpoint.
pub(crate) fn endpoint_port(endpoint: &str) -> Option<u16> {
    let authority = endpoint
        .split_once("://")
        .map_or(endpoint, |(_, rest)| rest);
    let authority = authority.split('/').next()?;
    let (_, port) = authority.rsplit_once(':')?;
    port.parse().ok()
}

/// Read the record at `path`; `None` when no owner is installed.
///
/// # Errors
///
/// Returns the read failure or the malformed record.
pub(crate) fn read(path: &Path) -> io::Result<Option<KeeperRecord>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Write `record` to `path` privately and atomically.
///
/// # Errors
///
/// Returns the failure creating, writing, or renaming the private file.
pub(crate) fn write(path: &Path, record: &KeeperRecord) -> io::Result<()> {
    let mut scratch = path.as_os_str().to_owned();
    scratch.push(REPLACEMENT_SUFFIX);
    let scratch = PathBuf::from(scratch);
    match fs::remove_file(&scratch) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut file = create_private_file(&scratch)?;
    let encoded = serde_json::to_vec_pretty(record)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    drop(file);
    if path.exists() {
        prepare_replacement(path)?;
    }
    fs::rename(&scratch, path)
}
