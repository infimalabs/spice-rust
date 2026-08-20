//! Pure admission of tracked steering horizons.

use serde::de::{Deserializer, Error as _};
use serde::{Deserialize, Serialize};
use wire::DurationWord;

/// Repository-owned shell command routes admitted by the current runtime.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Wrappers {
    #[serde(skip_serializing_if = "Option::is_none")]
    spice: Option<WrapperReference>,
}

impl Wrappers {
    /// Optional route for the ambient `spice` command in supervised shells.
    #[must_use]
    pub const fn spice(&self) -> Option<WrapperReference> {
        self.spice
    }
}

/// Closed destinations a tracked shell command route may name.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WrapperReference {
    /// The installed shell route backed by the exact Git-common appliance.
    Appliance,
}

/// The defaultable tracked steering policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Config {
    expiry: DurationWord,
    resend: Vec<DurationWord>,
}

impl Config {
    /// Directive lapse horizon.
    #[must_use]
    pub const fn expiry(&self) -> &DurationWord {
        &self.expiry
    }

    /// Escalation ladder in document order.
    #[must_use]
    pub fn resend(&self) -> &[DurationWord] {
        &self.resend
    }

    /// The three absolute silence horizons from original publication.
    #[must_use]
    pub fn stage_horizons(&self) -> [std::time::Duration; 3] {
        [
            self.resend[0].get(),
            self.resend[1].get(),
            self.resend[2].get(),
        ]
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            expiry: "24h"
                .parse()
                .expect("the compiled steering expiry is valid"),
            resend: ["15s", "60s", "300s"]
                .into_iter()
                .map(|word| word.parse().expect("compiled resend words are valid"))
                .collect(),
        }
    }
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawConfig {
    expiry: DurationWord,
    resend: Vec<DurationWord>,
}

impl Default for RawConfig {
    fn default() -> Self {
        let config = Config::default();
        Self {
            expiry: config.expiry,
            resend: config.resend,
        }
    }
}

impl<'de> Deserialize<'de> for Config {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawConfig::deserialize(deserializer)?;
        let [reminded, escalated, overdue] = raw.resend.as_slice() else {
            return Err(D::Error::custom(format!(
                "steer.resend requires exactly three horizons, found {}",
                raw.resend.len()
            )));
        };
        if !(reminded.get() < escalated.get()
            && escalated.get() < overdue.get()
            && overdue.get() < raw.expiry.get())
        {
            return Err(D::Error::custom(
                "steer horizons must satisfy reminded < escalated < overdue < expiry",
            ));
        }
        Ok(Self {
            expiry: raw.expiry,
            resend: raw.resend,
        })
    }
}
