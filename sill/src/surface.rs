//! Fixed repository-common topology for daemonless resource locking.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Acquisition, Availability, ExitCode, LockConfig, LockFile, LockName, Possession};

/// Read-only rendering of the configured lock topology.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockReport {
    /// Repository-common directory under which lock files live.
    pub root: String,
    /// Configured process statuses.
    pub exits: LockExitReport,
    /// Named pools in stable name order.
    pub pools: Vec<LockPoolReport>,
}

/// Process statuses attached to the three contention outcomes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockExitReport {
    pub busy: i32,
    pub taken: i32,
    pub full: i32,
}

/// One configured lock pool and every fixed shard path it owns.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockPoolReport {
    pub name: String,
    pub shards: u32,
    pub entries: Vec<LockShardReport>,
}

/// One fixed shard path and its point-in-time kernel availability.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockShardReport {
    pub shard: u32,
    pub path: String,
    pub availability: LockAvailability,
}

/// Whether a nonblocking acquisition would have succeeded when observed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LockAvailability {
    Available,
    Held,
}

/// Repository-common lock topology plus its admitted policy.
#[derive(Clone, Debug)]
pub struct LockTopology {
    root: PathBuf,
    config: LockConfig,
}

impl LockTopology {
    /// Bind an effective configuration to its fixed state directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, config: &LockConfig) -> Self {
        Self {
            root: root.into(),
            config: config.clone(),
        }
    }

    /// Render every configured path and its availability without creating one.
    ///
    /// Existing shard files are opened only for a nonblocking kernel probe;
    /// absent shard files and directories remain absent.
    ///
    /// # Errors
    ///
    /// Returns a typed inspection failure when an existing shard cannot be
    /// opened or observed.
    pub fn report(&self) -> Result<LockReport, LockSurfaceError> {
        let exits = self.config.exits();
        let pools = self
            .config
            .named()
            .map(|(name, shards)| {
                let entries = (0..shards.get())
                    .map(|shard| self.observe_shard(name, shard))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(LockPoolReport {
                    name: name.as_str().to_owned(),
                    shards: shards.get(),
                    entries,
                })
            })
            .collect::<Result<Vec<_>, LockSurfaceError>>()?;
        Ok(LockReport {
            root: self.root.display().to_string(),
            exits: LockExitReport {
                busy: exits.busy().get(),
                taken: exits.taken().get(),
                full: exits.full().get(),
            },
            pools,
        })
    }

    /// Try one selected shard, or the first free shard in stable order.
    ///
    /// This is the first operation that mutates the topology: it creates the
    /// selected pool directory and advisory lock files as acquisition reaches
    /// them. Reporting the topology remains read-only.
    ///
    /// # Errors
    ///
    /// Returns a typed selection, directory, or advisory-lock failure.
    pub fn try_acquire(
        &self,
        name: &str,
        requested: Option<u32>,
    ) -> Result<LockAttempt, LockSurfaceError> {
        let name = LockName::new(name.to_owned())
            .map_err(|_| LockSurfaceError::InvalidName(name.to_owned()))?;
        let shards = self
            .config
            .shards(&name)
            .ok_or_else(|| LockSurfaceError::UnknownName(name.as_str().to_owned()))?;
        if let Some(shard) = requested
            && shard >= shards.get()
        {
            return Err(LockSurfaceError::ShardOutOfRange {
                name: name.as_str().to_owned(),
                shard,
                shards: shards.get(),
            });
        }

        let pool = self.root.join(name.as_str());
        ensure_private_directory(&pool)
            .map_err(|source| LockSurfaceError::Prepare { path: pool, source })?;
        if let Some(shard) = requested {
            return self
                .acquire_shard(name, shard)
                .map(|attempt| match attempt {
                    Some(held) => LockAttempt::Acquired(held),
                    None => LockAttempt::Contended {
                        kind: Contention::Taken,
                        code: self.config.exits().taken(),
                    },
                });
        }

        for shard in 0..shards.get() {
            if let Some(held) = self.acquire_shard(name.clone(), shard)? {
                return Ok(LockAttempt::Acquired(held));
            }
        }
        let (kind, code) = if shards.get() == 1 {
            (Contention::Busy, self.config.exits().busy())
        } else {
            (Contention::Full, self.config.exits().full())
        };
        Ok(LockAttempt::Contended { kind, code })
    }

    fn acquire_shard(
        &self,
        name: LockName,
        shard: u32,
    ) -> Result<Option<HeldShard>, LockSurfaceError> {
        let path = self.shard_path(&name, shard);
        let lock = LockFile::new(name, &path);
        match lock
            .try_acquire()
            .map_err(|source| LockSurfaceError::Acquire { path, source })?
        {
            Acquisition::Acquired(possession) => Ok(Some(HeldShard { shard, possession })),
            Acquisition::Contended => Ok(None),
        }
    }

    fn shard_path(&self, name: &LockName, shard: u32) -> PathBuf {
        self.root.join(name.as_str()).join(shard.to_string())
    }

    fn observe_shard(
        &self,
        name: &LockName,
        shard: u32,
    ) -> Result<LockShardReport, LockSurfaceError> {
        let path = self.shard_path(name, shard);
        let lock = LockFile::new(name.clone(), &path);
        let availability = lock
            .availability()
            .map_err(|source| LockSurfaceError::Inspect {
                path: path.clone(),
                source,
            })?;
        Ok(LockShardReport {
            shard,
            path: path.display().to_string(),
            availability: match availability {
                Availability::Available => LockAvailability::Available,
                Availability::Held => LockAvailability::Held,
            },
        })
    }
}

/// One nonblocking pool acquisition.
#[derive(Debug)]
pub enum LockAttempt {
    Acquired(HeldShard),
    Contended { kind: Contention, code: ExitCode },
}

/// Possession of one exact configured shard.
#[derive(Debug)]
pub struct HeldShard {
    shard: u32,
    possession: Possession,
}

impl HeldShard {
    #[must_use]
    pub const fn shard(&self) -> u32 {
        self.shard
    }

    #[must_use]
    pub const fn possession(&self) -> &Possession {
        &self.possession
    }
}

/// Why an otherwise valid lock request could not acquire a shard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Contention {
    Busy,
    Taken,
    Full,
}

impl Contention {
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Busy => "lock::busy",
            Self::Taken => "lock::taken",
            Self::Full => "lock::full",
        }
    }
}

/// Why a lock request could not reach an acquisition decision.
#[derive(Debug)]
pub enum LockSurfaceError {
    InvalidName(String),
    UnknownName(String),
    ShardOutOfRange {
        name: String,
        shard: u32,
        shards: u32,
    },
    Prepare {
        path: PathBuf,
        source: io::Error,
    },
    Acquire {
        path: PathBuf,
        source: io::Error,
    },
    Inspect {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for LockSurfaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(name) => write!(formatter, "lock name is invalid: {name:?}"),
            Self::UnknownName(name) => write!(formatter, "lock is not configured: {name:?}"),
            Self::ShardOutOfRange {
                name,
                shard,
                shards,
            } => write!(
                formatter,
                "lock {name:?} shard {shard} is outside configured capacity {shards}"
            ),
            Self::Prepare { path, source } => {
                write!(formatter, "cannot prepare {}: {source}", path.display())
            }
            Self::Acquire { path, source } => {
                write!(formatter, "cannot acquire {}: {source}", path.display())
            }
            Self::Inspect { path, source } => {
                write!(formatter, "cannot inspect {}: {source}", path.display())
            }
        }
    }
}

impl Error for LockSurfaceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Prepare { source, .. }
            | Self::Acquire { source, .. }
            | Self::Inspect { source, .. } => Some(source),
            Self::InvalidName(_) | Self::UnknownName(_) | Self::ShardOutOfRange { .. } => None,
        }
    }
}

fn ensure_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{Contention, LockAttempt, LockAvailability, LockSurfaceError, LockTopology};
    use crate::LockConfig;

    const BUSY_EXIT: u64 = 71;
    const TAKEN_EXIT: u64 = 72;
    const FULL_EXIT: u64 = 73;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "sill-surface-{label}-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("exclusive scratch root is created");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn topology(root: &Path) -> LockTopology {
        let config = LockConfig::new(
            BUSY_EXIT,
            TAKEN_EXIT,
            FULL_EXIT,
            [("single".to_owned(), 1), ("pool".to_owned(), 2)],
        )
        .expect("fixture configuration is admitted");
        LockTopology::new(root, &config)
    }

    #[test]
    fn report_is_stable_and_does_not_materialize_the_topology() {
        let scratch = Scratch::new("report");
        let root = scratch.path().join("state/locks");
        let report = topology(&root).report().expect("topology is observable");

        assert_eq!(report.root, root.display().to_string());
        assert_eq!(report.exits.busy, BUSY_EXIT as i32);
        assert_eq!(report.exits.taken, TAKEN_EXIT as i32);
        assert_eq!(report.exits.full, FULL_EXIT as i32);
        assert_eq!(
            report
                .pools
                .iter()
                .map(|pool| (pool.name.as_str(), pool.shards))
                .collect::<Vec<_>>(),
            [("pool", 2), ("single", 1)]
        );
        assert_eq!(
            report.pools[0]
                .entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
            [
                root.join("pool/0").display().to_string(),
                root.join("pool/1").display().to_string(),
            ]
        );
        assert!(
            report
                .pools
                .iter()
                .flat_map(|pool| &pool.entries)
                .all(|entry| entry.availability == LockAvailability::Available)
        );
        assert!(!root.exists(), "reporting created lock state");
    }

    #[test]
    fn report_observes_held_and_released_shards_without_creating_missing_ones() {
        let scratch = Scratch::new("availability");
        let root = scratch.path().join("locks");
        let topology = topology(&root);
        let held = match topology
            .try_acquire("pool", Some(0))
            .expect("selected shard is observable")
        {
            LockAttempt::Acquired(held) => held,
            LockAttempt::Contended { .. } => panic!("fresh shard was contended"),
        };

        let report = topology.report().expect("held topology is observable");
        let pool = report
            .pools
            .iter()
            .find(|pool| pool.name == "pool")
            .expect("pool is reported");
        assert_eq!(pool.entries[0].availability, LockAvailability::Held);
        assert_eq!(pool.entries[1].availability, LockAvailability::Available);
        assert!(root.join("pool/0").is_file());
        assert!(
            !root.join("pool/1").exists(),
            "observing an absent shard created it"
        );

        drop(held);
        let released = topology.report().expect("released topology is observable");
        let pool = released
            .pools
            .iter()
            .find(|pool| pool.name == "pool")
            .expect("pool is reported");
        assert_eq!(pool.entries[0].availability, LockAvailability::Available);
    }

    #[test]
    fn stable_selection_and_each_contention_class_use_configured_codes() {
        let scratch = Scratch::new("contention");
        let topology = topology(&scratch.path().join("locks"));

        let first = match topology.try_acquire("pool", None).expect("first shard") {
            LockAttempt::Acquired(held) => held,
            LockAttempt::Contended { .. } => panic!("fresh pool was contended"),
        };
        assert_eq!(first.shard(), 0);
        let second = match topology.try_acquire("pool", None).expect("second shard") {
            LockAttempt::Acquired(held) => held,
            LockAttempt::Contended { .. } => panic!("second shard was contended"),
        };
        assert_eq!(second.shard(), 1);

        assert!(matches!(
            topology.try_acquire("pool", Some(0)).expect("chosen contention"),
            LockAttempt::Contended {
                kind: Contention::Taken,
                code
            } if code.get() == TAKEN_EXIT as i32
        ));
        assert!(matches!(
            topology.try_acquire("pool", None).expect("pool exhaustion"),
            LockAttempt::Contended {
                kind: Contention::Full,
                code
            } if code.get() == FULL_EXIT as i32
        ));

        let single = match topology.try_acquire("single", None).expect("single lock") {
            LockAttempt::Acquired(held) => held,
            LockAttempt::Contended { .. } => panic!("fresh single lock was contended"),
        };
        assert!(matches!(
            topology.try_acquire("single", None).expect("single contention"),
            LockAttempt::Contended {
                kind: Contention::Busy,
                code
            } if code.get() == BUSY_EXIT as i32
        ));

        drop(first);
        assert!(matches!(
            topology.try_acquire("pool", Some(0)).expect("released shard"),
            LockAttempt::Acquired(held) if held.shard() == 0
        ));
        drop(second);
        drop(single);
    }

    #[test]
    fn invalid_unknown_and_out_of_range_requests_do_not_create_state() {
        let scratch = Scratch::new("selection");
        let root = scratch.path().join("locks");
        let topology = topology(&root);

        assert!(matches!(
            topology.try_acquire("../outside", None),
            Err(LockSurfaceError::InvalidName(_))
        ));
        assert!(matches!(
            topology.try_acquire("missing", None),
            Err(LockSurfaceError::UnknownName(_))
        ));
        assert!(matches!(
            topology.try_acquire("pool", Some(2)),
            Err(LockSurfaceError::ShardOutOfRange { .. })
        ));
        assert!(!root.exists(), "a refused selection created lock state");
    }
}
