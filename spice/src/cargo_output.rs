//! A per-worktree bound on Cargo output.
//!
//! Every lane compiles into its own `target/`, and Cargo never removes output
//! a later build stopped using, so without a bound each lane grows until the
//! shared volume fills. This module holds a worktree's `target/` to
//! [`LANE_OUTPUT_BOUND`] by emptying its least recently used Cargo units.
//!
//! A unit is one directory that directly holds Cargo's `.cargo-lock`,
//! `.cargo-build-lock` or `.cargo-artifact-lock`: a profile directory of a
//! target or build directory. Cargo holds one of those locks for as long as it
//! reads or writes the unit, so a unit is emptied only while this process holds
//! every one of its locks exclusively. A Cargo that starts meanwhile waits on
//! the same lock files, which are kept, and then rebuilds what it needs. The
//! unit's `.fingerprint` directory is removed first, so a pass that fails part
//! way leaves Cargo rebuilding rather than trusting a partial unit.
//!
//! Some users of a unit hold no Cargo lock: the gate runs prebuilt test
//! executables after Cargo exits. Each such run holds the worktree's
//! [`OutputLease`] shared for its whole length, and reclamation runs only while
//! the lease is held exclusively, so it never overlaps a gate or developer
//! Cargo run in the same worktree.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File, Metadata, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

#[cfg(test)]
mod tests;

/// Most physical bytes one worktree's `target/` holds after a reclamation pass.
pub const LANE_OUTPUT_BOUND: u64 = 24 << 30;

/// Shortest pause between two reclamation passes over one worktree.
pub const RECLAIM_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// How long a unit stays out of reclamation after its last use.
///
/// One gate prove's build cache can exceed [`LANE_OUTPUT_BOUND`] on its own,
/// so a pass ordered only by least recent use would empty what a prove built
/// minutes earlier, and a retry after a timed-out prebuild would start cold. A
/// unit used within this window is kept even while the footprint stays over
/// the bound.
pub const RECENT_USE: Duration = Duration::from_secs(60 * 60);

/// Cargo's per-profile locks; a directory directly holding any of them is a unit.
const CARGO_LOCKS: [&str; 3] = [".cargo-lock", ".cargo-build-lock", ".cargo-artifact-lock"];

/// Cargo's freshness records, removed first so a partial unit is never fresh.
const FINGERPRINTS: &str = ".fingerprint";

const LEASE_FILE: &str = "cargo-output.lock";

/// The Cargo output directory this bound governs for one worktree.
#[must_use]
pub fn worktree_target(worktree: &Path) -> PathBuf {
    worktree.join("target")
}

/// Physical bytes below one worktree's Cargo output directory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Footprint {
    pub target: PathBuf,
    pub bound: u64,
    pub bytes: u64,
    /// Bytes outside every unit, which reclamation never removes.
    pub unowned_bytes: u64,
    pub units: Vec<Unit>,
}

/// One Cargo profile directory and the bytes it alone owns.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Unit {
    pub path: PathBuf,
    /// Bytes below this unit, excluding units nested inside it.
    pub bytes: u64,
    /// Newest modification or status change below the unit.
    pub last_use_epoch_seconds: u64,
    #[serde(skip)]
    last_use: SystemTime,
    #[serde(skip)]
    locks: Vec<PathBuf>,
    #[serde(skip)]
    nested: Vec<PathBuf>,
}

/// What one reclamation pass removed and what it had to leave.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Reclamation {
    pub bound: u64,
    pub before: u64,
    pub after: u64,
    pub removed: Vec<Removed>,
    /// Units a live Cargo held while the footprint was still over the bound.
    pub busy: Vec<PathBuf>,
    /// Units used within [`RECENT_USE`] while the footprint was still over the bound.
    pub recent: Vec<PathBuf>,
}

/// One unit a pass emptied.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Removed {
    pub path: PathBuf,
    pub bytes: u64,
}

/// Measure `target` without following symbolic links.
///
/// A missing directory is an empty footprint. Hard-linked files count once.
///
/// # Errors
///
/// Returns the first filesystem refusal other than an entry vanishing
/// mid-walk.
pub fn measure(target: &Path, bound: u64) -> io::Result<Footprint> {
    let mut walk = Walk::default();
    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.is_dir() => walk.directory(target, None, &metadata)?,
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a directory", target.display()),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    for unit in &mut walk.units {
        unit.last_use_epoch_seconds = unit
            .last_use
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
    }
    Ok(Footprint {
        target: target.to_path_buf(),
        bound,
        bytes: walk.bytes,
        unowned_bytes: walk.unowned,
        units: walk.units,
    })
}

/// Empty least recently used units until `footprint` fits its bound.
///
/// A unit whose Cargo locks are not all free is left whole and reported busy.
/// A unit used within [`RECENT_USE`] of `now` is left whole and reported recent.
///
/// # Errors
///
/// Returns the first refusal opening a lock or removing an entry; units
/// emptied before it stay emptied.
pub fn reclaim(footprint: &Footprint, now: SystemTime) -> io::Result<Reclamation> {
    let mut reclamation = Reclamation {
        bound: footprint.bound,
        before: footprint.bytes,
        after: footprint.bytes,
        removed: Vec::new(),
        busy: Vec::new(),
        recent: Vec::new(),
    };
    let recent_since = now.checked_sub(RECENT_USE).unwrap_or(UNIX_EPOCH);
    let mut order = footprint.units.iter().collect::<Vec<_>>();
    order.sort_by(|left, right| {
        left.last_use
            .cmp(&right.last_use)
            .then_with(|| left.path.cmp(&right.path))
    });
    for unit in order {
        if reclamation.after <= footprint.bound {
            break;
        }
        if unit.last_use >= recent_since {
            reclamation.recent.push(unit.path.clone());
            continue;
        }
        let Some(_held) = hold(&unit.locks)? else {
            reclamation.busy.push(unit.path.clone());
            continue;
        };
        let keep = unit
            .locks
            .iter()
            .chain(&unit.nested)
            .map(PathBuf::as_path)
            .collect::<Vec<_>>();
        clear(&unit.path, &keep)?;
        reclamation.after = reclamation.after.saturating_sub(unit.bytes);
        reclamation.removed.push(Removed {
            path: unit.path.clone(),
            bytes: unit.bytes,
        });
    }
    Ok(reclamation)
}

/// What entering the lease did about reclamation.
#[derive(Debug)]
pub enum Pass {
    /// The last pass over this worktree is more recent than [`RECLAIM_INTERVAL`].
    NotDue,
    /// Another run holds the lease, so this one could not reclaim.
    Shared,
    /// This run reclaimed under the exclusive lease.
    Ran(io::Result<Reclamation>),
}

/// Shared possession of one worktree's Cargo output for a run's whole length.
#[derive(Debug)]
pub struct OutputLease {
    _file: File,
}

impl OutputLease {
    /// Take the worktree's output lease, reclaiming `target` down to `bound`
    /// first when nothing else holds the lease and the last pass is older than
    /// [`RECLAIM_INTERVAL`].
    ///
    /// `locks` is the worktree's lock directory. A reclamation failure is
    /// carried in the returned [`Pass`]; the lease is still held.
    ///
    /// # Errors
    ///
    /// Returns a refusal creating, opening or locking the lease file.
    pub fn enter(
        locks: &Path,
        target: &Path,
        bound: u64,
        now: SystemTime,
    ) -> io::Result<(Self, Pass)> {
        fs::create_dir_all(locks)?;
        let path = locks.join(LEASE_FILE);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)?;
        let pass = match file.try_lock() {
            Ok(()) => {
                let pass = if due(&path, now)? {
                    let reclaimed =
                        measure(target, bound).and_then(|footprint| reclaim(&footprint, now));
                    record_pass(&mut file, now)?;
                    Pass::Ran(reclaimed)
                } else {
                    Pass::NotDue
                };
                file.unlock()?;
                pass
            }
            Err(TryLockError::WouldBlock) => Pass::Shared,
            Err(TryLockError::Error(error)) => return Err(error),
        };
        file.lock_shared()?;
        Ok((Self { _file: file }, pass))
    }
}

/// Whether the lease file records no pass, or one older than the interval.
fn due(path: &Path, now: SystemTime) -> io::Result<bool> {
    let recorded = fs::read_to_string(path)?;
    let Ok(last) = recorded.trim().parse::<u64>() else {
        return Ok(true);
    };
    let last = UNIX_EPOCH + Duration::from_secs(last);
    Ok(now
        .duration_since(last)
        .is_ok_and(|elapsed| elapsed >= RECLAIM_INTERVAL))
}

fn record_pass(file: &mut File, now: SystemTime) -> io::Result<()> {
    let seconds = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    file.set_len(0)?;
    io::Seek::rewind(file)?;
    writeln!(file, "{seconds}")
}

#[derive(Default)]
struct Walk {
    bytes: u64,
    unowned: u64,
    units: Vec<Unit>,
    linked: HashSet<(u64, u64)>,
}

impl Walk {
    fn directory(
        &mut self,
        path: &Path,
        owner: Option<usize>,
        metadata: &Metadata,
    ) -> io::Result<()> {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut children = Vec::new();
        let mut locks = Vec::new();
        for entry in entries {
            let entry = entry?;
            let child = entry.path();
            if CARGO_LOCKS.iter().any(|lock| entry.file_name() == *lock)
                && entry.file_type()?.is_file()
            {
                locks.push(child.clone());
            }
            children.push(child);
        }
        let owner = if locks.is_empty() {
            owner
        } else {
            if let Some(parent) = owner {
                self.units[parent].nested.push(path.to_path_buf());
            }
            locks.sort();
            self.units.push(Unit {
                path: path.to_path_buf(),
                bytes: 0,
                last_use_epoch_seconds: 0,
                last_use: UNIX_EPOCH,
                locks,
                nested: Vec::new(),
            });
            Some(self.units.len() - 1)
        };
        self.record(owner, metadata);
        for child in children {
            let metadata = match fs::symlink_metadata(&child) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if metadata.is_dir() {
                self.directory(&child, owner, &metadata)?;
            } else {
                self.record(owner, &metadata);
            }
        }
        Ok(())
    }

    fn record(&mut self, owner: Option<usize>, metadata: &Metadata) {
        if metadata.nlink() > 1
            && !metadata.is_dir()
            && !self.linked.insert((metadata.dev(), metadata.ino()))
        {
            return;
        }
        let bytes = metadata.blocks().saturating_mul(512);
        self.bytes = self.bytes.saturating_add(bytes);
        let Some(owner) = owner else {
            self.unowned = self.unowned.saturating_add(bytes);
            return;
        };
        let unit = &mut self.units[owner];
        unit.bytes = unit.bytes.saturating_add(bytes);
        unit.last_use = unit.last_use.max(last_use(metadata));
    }
}

/// The later of a modification and a status change; Cargo re-links uplifted
/// outputs on a fresh build, which changes status without modifying content.
fn last_use(metadata: &Metadata) -> SystemTime {
    let changed = u64::try_from(metadata.ctime())
        .ok()
        .and_then(|seconds| {
            let nanos = u32::try_from(metadata.ctime_nsec()).unwrap_or(0);
            UNIX_EPOCH.checked_add(Duration::new(seconds, nanos))
        })
        .unwrap_or(UNIX_EPOCH);
    metadata
        .modified()
        .map_or(changed, |modified| modified.max(changed))
}

/// Exclusively hold every lock, or none when any is held elsewhere.
fn hold(locks: &[PathBuf]) -> io::Result<Option<Vec<File>>> {
    let mut held = Vec::with_capacity(locks.len());
    for path in locks {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => held.push(file),
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
    Ok(Some(held))
}

/// Remove everything below `directory` except `keep` and the directories
/// leading to it, fingerprints first.
fn clear(directory: &Path, keep: &[&Path]) -> io::Result<()> {
    let mut children = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    children.sort_by_key(|child| child.file_name() != Some(OsStr::new(FINGERPRINTS)));
    for child in children {
        if keep.contains(&child.as_path()) {
            continue;
        }
        if keep.iter().any(|kept| kept.starts_with(&child)) {
            clear(&child, keep)?;
            continue;
        }
        let removed = match fs::symlink_metadata(&child) {
            Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&child),
            Ok(_) => fs::remove_file(&child),
            Err(error) => Err(error),
        };
        match removed {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

impl fmt::Display for Footprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            formatter,
            "cargo output {} of {} bound in {}",
            Bytes(self.bytes),
            Bytes(self.bound),
            self.target.display()
        )?;
        let mut units = self.units.iter().collect::<Vec<_>>();
        units.sort_by_key(|unit| std::cmp::Reverse(unit.bytes));
        for unit in units {
            let path = unit.path.strip_prefix(&self.target).unwrap_or(&unit.path);
            writeln!(formatter, "  {:>9}  {}", Bytes(unit.bytes), path.display())?;
        }
        write!(
            formatter,
            "  {:>9}  outside Cargo units, never reclaimed",
            Bytes(self.unowned_bytes)
        )
    }
}

impl fmt::Display for Reclamation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let removed = self.before.saturating_sub(self.after);
        write!(
            formatter,
            "reclaimed {} by emptying {} least recently used units: {} -> {} of {} bound",
            Bytes(removed),
            self.removed.len(),
            Bytes(self.before),
            Bytes(self.after),
            Bytes(self.bound),
        )?;
        if !self.busy.is_empty() {
            write!(
                formatter,
                "; left {} units Cargo was using",
                self.busy.len()
            )?;
        }
        if !self.recent.is_empty() {
            write!(
                formatter,
                "; kept {} units used within the last hour",
                self.recent.len()
            )?;
        }
        Ok(())
    }
}

struct Bytes(u64);

impl fmt::Display for Bytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
        const STEP: u64 = 1024;
        let mut value = self.0;
        let mut tenths = 0;
        let mut unit = 0;
        while value >= STEP && unit + 1 < UNITS.len() {
            tenths = (value % STEP) * 10 / STEP;
            value /= STEP;
            unit += 1;
        }
        let rendered = if unit == 0 {
            format!("{value} {}", UNITS[unit])
        } else {
            format!("{value}.{tenths} {}", UNITS[unit])
        };
        formatter.pad(&rendered)
    }
}
