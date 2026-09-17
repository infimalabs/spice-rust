//! Runtime paths keyed by one lane identity.

use std::path::{Path, PathBuf};

use wire::CanonicalUuid;

use super::StatePaths;

impl StatePaths {
    /// Parent of per-lane detached supervisor records.
    #[must_use]
    pub fn supervisors(&self) -> &Path {
        &self.supervisors
    }

    /// Current RTK history database for one Rust lane identity.
    ///
    /// The path follows the typed-root/per-lane team used by `tape`
    /// and `degraded`. Unowned paths elsewhere in the runtime are irrelevant.
    #[must_use]
    pub fn rtk_history(&self, lane: CanonicalUuid) -> PathBuf {
        self.rtk().join(lane.to_string()).join("history.db")
    }

    /// Roots whose children are each named by one lane identity.
    ///
    /// Every family keys its child by the lane's canonical text, so residue
    /// for a lane the resident no longer seats is found by one directory read
    /// per family. The canvas family holds files; the rest hold directories.
    #[must_use]
    pub fn lane_families(&self) -> [&Path; 5] {
        [
            self.tape(),
            self.rtk(),
            self.supervisors(),
            self.steering_media(),
            self.lane_canvas(),
        ]
    }
}
