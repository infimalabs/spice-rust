//! Pure admission of allocator tuning under `[board.next]`.

use serde::{Deserialize, Serialize};

/// The defaultable tracked board configuration.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    next: NextConfig,
}

impl Config {
    /// Allocator tuning values.
    #[must_use]
    pub const fn next(&self) -> &NextConfig {
        &self.next
    }
}

/// The sole allocator coefficient exposed by this release.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NextConfig {
    band: u64,
}

impl NextConfig {
    /// Construct one admitted allocator policy for an explicit urgency band.
    #[must_use]
    pub const fn new(band: u64) -> Self {
        Self { band }
    }

    /// Integer urgency distance treated as an equivalent tie band.
    #[must_use]
    pub const fn band(&self) -> u64 {
        self.band
    }
}

impl Default for NextConfig {
    fn default() -> Self {
        Self::new(2)
    }
}
