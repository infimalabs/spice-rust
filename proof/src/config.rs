//! Pure admission of the tracked proof container identity.

use serde::de::{Deserializer, Error as _};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::container::is_digest_pinned;

/// Proof image sealed into the compiled defaults documented by `CONFIG.md`.
pub const DEFAULT_IMAGE: &str =
    "sha256:2f1f7a4c9e6d0b835a1c47e9d20b6f83c15d9a047e6b3a92c8d05e714f6a9b3c";

/// An exact lower-case SHA-256 image reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Image(String);

impl Image {
    /// Exact digest reference.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for Image {
    fn default() -> Self {
        Self(DEFAULT_IMAGE.to_owned())
    }
}

impl<'de> Deserialize<'de> for Image {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if is_digest_pinned(&value) {
            Ok(Self(value))
        } else {
            Err(D::Error::custom(
                "proof image must be an exact lower-case sha256 digest reference",
            ))
        }
    }
}

impl Serialize for Image {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// The defaultable tracked proof policy.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    image: Image,
}

impl Config {
    /// Approved proof container identity.
    #[must_use]
    pub const fn image(&self) -> &Image {
        &self.image
    }
}
