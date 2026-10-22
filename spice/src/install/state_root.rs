//! The one move of a live predecessor state root to its current name.
//!
//! The root carried [`wire::PREDECESSOR_STATE_SUFFIX`] while Python Spice
//! held [`wire::STATE_DIRECTORY`].
//! Every admission refuses a symlinked component, so no link can bridge the
//! two names: the live root moves as a real directory, exactly once, and the
//! install that puts this build in place is where it moves. The move is a
//! rename of the Git-common root and of each linked worktree's root, so every
//! child keeps its relative name and its inode; the journal, gate evidence,
//! and supervisors inside are untouched.
//!
//! A resident possesses the common root through its lock, and a lane's
//! supervisor keeps writing below it for as long as the supervisor runs.
//! Renaming the root under either would leave the process writing into a
//! path no current admission opens, so the crossing refuses and names each
//! holder instead.

use std::fmt;
use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sill::{Availability, LockFile, LockName};
use wire::CanonicalUuid;

use super::InstallError;
use crate::repository::Repository;
use crate::resident_lock;

/// Names the archived predecessor left below the common root that no
/// current writer owns.
const COMMON_RESIDUE: [&str; 3] = ["learnings", "install-recovery", "data/spicefolds.sqlite3"];
/// Names the archived predecessor left below each worktree root.
const WORKTREE_RESIDUE: [&str; 2] = ["inbox", "full-verifier-evidence.json"];
/// Largest resident lock record the crossing reads to name its holder.
const RESIDENT_RECORD_BYTES: u64 = 4096;
/// Largest supervisor state document the crossing reads to name a lane.
const SUPERVISOR_STATE_BYTES: u64 = 64 * 1024;

/// One directory rename the crossing performs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateRootMove {
    /// The live predecessor root.
    pub from: PathBuf,
    /// Its current name beside it.
    pub to: PathBuf,
}

/// The pending move of every live predecessor root in one repository.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateRootCrossing {
    /// The Git-common root, when it still carries the predecessor name.
    pub common: Option<StateRootMove>,
    /// Each linked worktree's Git-directory root still carrying that name.
    pub worktrees: Vec<StateRootMove>,
    /// Retired residue below those roots, deleted before the move.
    pub residue: Vec<PathBuf>,
    /// Whoever holds the common root now; apply refuses while any remain.
    pub holders: Vec<String>,
}

impl fmt::Display for StateRootCrossing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for moved in self.common.iter().chain(&self.worktrees) {
            writeln!(
                formatter,
                "  move {} to {}",
                moved.from.display(),
                moved.to.display()
            )?;
        }
        for residue in &self.residue {
            writeln!(formatter, "  delete {}", residue.display())?;
        }
        if !self.holders.is_empty() {
            writeln!(
                formatter,
                "  blocked: the state root is held by {}",
                self.holders.join(", ")
            )?;
        }
        Ok(())
    }
}

/// The crossing an install of this build would perform, if any root still
/// carries the predecessor name.
///
/// Reading changes nothing. Both names occupied at one Git directory is
/// refused: the resolver would silently prefer the current name and strand
/// whatever lives under the other.
pub fn pending(repository: &Repository) -> Result<Option<StateRootCrossing>, InstallError> {
    let common = pending_move(repository.common_dir())?;
    let mut worktrees = Vec::new();
    for git_directory in linked_worktree_git_directories(repository.common_dir())? {
        worktrees.extend(pending_move(&git_directory)?);
    }
    if common.is_none() && worktrees.is_empty() {
        return Ok(None);
    }
    let mut residue = Vec::new();
    if let Some(common) = &common {
        residue.extend(present_residue(&common.from, &COMMON_RESIDUE)?);
    }
    for moved in &worktrees {
        residue.extend(present_residue(&moved.from, &WORKTREE_RESIDUE)?);
    }
    let holders = match &common {
        Some(common) => holders(&common.from)?,
        None => Vec::new(),
    };
    Ok(Some(StateRootCrossing {
        common,
        worktrees,
        residue,
        holders,
    }))
}

/// Perform `crossing`, refusing while a holder remains.
///
/// Holders are probed again here rather than trusted from the preview: a
/// resident may have started since.
pub(super) fn cross(crossing: &StateRootCrossing) -> Result<(), InstallError> {
    if let Some(common) = &crossing.common {
        let holders = holders(&common.from)?;
        if !holders.is_empty() {
            return Err(InstallError::StateRootHeld {
                root: common.from.clone(),
                holders,
            });
        }
    }
    for path in &crossing.residue {
        remove_residue(path)?;
    }
    for moved in crossing.common.iter().chain(&crossing.worktrees) {
        fs::rename(&moved.from, &moved.to)
            .map_err(|source| InstallError::io("move state root", &moved.from, source))?;
    }
    Ok(())
}

fn pending_move(git_directory: &Path) -> Result<Option<StateRootMove>, InstallError> {
    let current = git_directory.join(wire::STATE_DIRECTORY);
    let predecessor = git_directory.join(wire::predecessor_state_directory());
    let Some(metadata) = occupant(&predecessor)? else {
        return Ok(None);
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(InstallError::PathShape(predecessor));
    }
    if occupant(&current)?.is_some() {
        return Err(InstallError::StateRootAmbiguous {
            current,
            predecessor,
        });
    }
    Ok(Some(StateRootMove {
        from: predecessor,
        to: current,
    }))
}

fn occupant(path: &Path) -> Result<Option<fs::Metadata>, InstallError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(InstallError::io("inspect state root", path, source)),
    }
}

/// Every `<common>/worktrees/<name>` Git directory, each a real directory.
fn linked_worktree_git_directories(common: &Path) -> Result<Vec<PathBuf>, InstallError> {
    let worktrees = common.join("worktrees");
    let entries = match fs::read_dir(&worktrees) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(InstallError::io(
                "list linked worktrees",
                &worktrees,
                source,
            ));
        }
    };
    let mut directories = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|source| InstallError::io("list linked worktrees", &worktrees, source))?;
        let file_type = entry
            .file_type()
            .map_err(|source| InstallError::io("inspect linked worktree", entry.path(), source))?;
        if file_type.is_dir() {
            directories.push(entry.path());
        }
    }
    directories.sort();
    Ok(directories)
}

fn present_residue(root: &Path, names: &[&str]) -> Result<Vec<PathBuf>, InstallError> {
    let mut present = Vec::new();
    for name in names {
        let path = root.join(name);
        if occupant(&path)?.is_some() {
            present.push(path);
        }
    }
    Ok(present)
}

fn remove_residue(path: &Path) -> Result<(), InstallError> {
    let Some(metadata) = occupant(path)? else {
        return Ok(());
    };
    let removed = if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    removed.map_err(|source| InstallError::io("delete retired state residue", path, source))
}

/// Whoever holds `root` now: the resident possessing its lock, and every lane
/// whose supervisor process is still running.
fn holders(root: &Path) -> Result<Vec<String>, InstallError> {
    let mut holders = Vec::new();
    holders.extend(resident_holder(&root.join("resident"))?);
    holders.extend(live_lanes(&root.join("supervisors"))?);
    Ok(holders)
}

fn resident_holder(lock: &Path) -> Result<Option<String>, InstallError> {
    if occupant(lock)?.is_none() {
        return Ok(None);
    }
    let name = LockName::new("resident").expect("resident is a fixed lock name");
    match LockFile::new(name, lock)
        .availability()
        .map_err(|source| InstallError::io("probe resident possession", lock, source))?
    {
        Availability::Available => Ok(None),
        Availability::Held => {
            let record = read_bounded(lock, RESIDENT_RECORD_BYTES)
                .map_err(|source| InstallError::io("read resident holder record", lock, source))?;
            Ok(Some(match resident_lock::holder_pid(&record) {
                Some(pid) => format!("resident pid {pid}"),
                None => "a resident whose lock record names no pid".to_owned(),
            }))
        }
    }
}

/// The fields of a supervisor's published state the crossing needs: the
/// lane it serves, its own process, and whether it has finished.
#[derive(Deserialize)]
struct SupervisorState {
    supervisor: u32,
    lane: CanonicalUuid,
    phase: SupervisorPhase,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum SupervisorPhase {
    Starting,
    Running,
    Exited,
    Failed,
}

/// Every lane below `supervisors` whose supervisor is still running: a state
/// that has not ended, published by a process that still exists.
fn live_lanes(supervisors: &Path) -> Result<Vec<String>, InstallError> {
    let mut lanes = Vec::new();
    for lane in directories(supervisors)? {
        for seat in directories(&lane)? {
            let state = seat.join("state.json");
            if occupant(&state)?.is_none() {
                continue;
            }
            let bytes = read_bounded(&state, SUPERVISOR_STATE_BYTES)
                .map_err(|source| InstallError::io("read supervisor state", &state, source))?;
            let Ok(state) = serde_json::from_slice::<SupervisorState>(&bytes) else {
                continue;
            };
            if matches!(
                state.phase,
                SupervisorPhase::Exited | SupervisorPhase::Failed
            ) {
                continue;
            }
            let running = sill::process_running(state.supervisor)
                .map_err(|source| InstallError::io("probe supervisor liveness", &seat, source))?;
            if running {
                lanes.push(format!(
                    "lane {} (supervisor pid {})",
                    state.lane, state.supervisor
                ));
            }
        }
    }
    Ok(lanes)
}

fn directories(parent: &Path) -> Result<Vec<PathBuf>, InstallError> {
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(InstallError::io("list state root", parent, source)),
    };
    let mut directories = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| InstallError::io("list state root", parent, source))?;
        if entry
            .file_type()
            .map_err(|source| InstallError::io("inspect state root entry", entry.path(), source))?
            .is_dir()
        {
            directories.push(entry.path());
        }
    }
    directories.sort();
    Ok(directories)
}

fn read_bounded(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}
