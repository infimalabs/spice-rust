//! Pure admission of tracked lane supervision policy.

use serde::de::{Deserializer, Error as _};
use serde::{Deserialize, Serialize};
use wire::{DurationWord, Name};

use crate::life::Life;

/// The defaultable tracked lane policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    life: Life,
    lease: DurationWord,
    vendor: Name,
    #[serde(deserialize_with = "nonzero_deaths")]
    deaths: u32,
    young: DurationWord,
    hold: DurationWord,
}

impl Config {
    /// Policy assigned to a newly seated lane.
    #[must_use]
    pub const fn life(&self) -> Life {
        self.life
    }

    /// Claim deadline.
    #[must_use]
    pub const fn lease(&self) -> &DurationWord {
        &self.lease
    }

    /// Vendor launched for a new occupant.
    #[must_use]
    pub const fn vendor(&self) -> &Name {
        &self.vendor
    }

    /// Consecutive young deaths that hold a lane.
    #[must_use]
    pub const fn deaths(&self) -> u32 {
        self.deaths
    }

    /// Lifetime below which an occupant death is young.
    #[must_use]
    pub const fn young(&self) -> &DurationWord {
        &self.young
    }

    /// Automatic lane hold duration.
    #[must_use]
    pub const fn hold(&self) -> &DurationWord {
        &self.hold
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            life: Life::Drive,
            lease: "15m".parse().expect("the compiled lane lease is valid"),
            vendor: "codex"
                .parse()
                .expect("the compiled lane vendor is a valid name"),
            deaths: 3,
            young: "30s"
                .parse()
                .expect("the compiled young threshold is valid"),
            hold: "30m".parse().expect("the compiled lane hold is valid"),
        }
    }
}

fn nonzero_deaths<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let value = u32::deserialize(deserializer)?;
    if value == 0 {
        Err(D::Error::custom("young death limit must be nonzero"))
    } else {
        Ok(value)
    }
}
