//! Pure admission of allocator tuning under `[board.next]`.

use serde::de::{Deserializer, Error as _};
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

/// The two allocator coefficients exposed by release one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NextConfig {
    #[serde(deserialize_with = "finite")]
    review: f64,
    #[serde(deserialize_with = "band")]
    band: f64,
}

impl NextConfig {
    /// Anti-self-review coefficient.
    #[must_use]
    pub const fn review(&self) -> f64 {
        self.review
    }

    /// Urgency tie-break band.
    #[must_use]
    pub const fn band(&self) -> f64 {
        self.band
    }
}

impl Default for NextConfig {
    fn default() -> Self {
        Self {
            review: -100.0,
            band: 2.0,
        }
    }
}

fn finite<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    let value = f64::deserialize(deserializer)?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(D::Error::custom("coefficient must be finite"))
    }
}

fn band<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    let value = finite(deserializer)?;
    if value >= 0.0 {
        Ok(value)
    } else {
        Err(D::Error::custom("urgency band must not be negative"))
    }
}
