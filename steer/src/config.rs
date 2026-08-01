//! Pure admission of tracked steering horizons.

use serde::{Deserialize, Serialize};
use wire::DurationWord;

/// The defaultable tracked steering policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            expiry: "24h"
                .parse()
                .expect("the compiled steering expiry is valid"),
            resend: ["5m", "15m", "1h"]
                .into_iter()
                .map(|word| word.parse().expect("compiled resend words are valid"))
                .collect(),
        }
    }
}
