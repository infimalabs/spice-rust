//! Pure admission of proof policy and machine-local executable locations.

use std::path::{Path, PathBuf};

use serde::de::{Deserializer, Error as _};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use wire::manifest::ProofImage;

/// Proof image sealed into this build's compiled configuration defaults.
///
/// This is the exact OCI identity of the `linux/amd64` appliance assembled by
/// `spice dev xtask -- proof-image build`, and `dist/proof-image/identity.txt` is the
/// same digest in the one spelling a shell can read. The two are proven equal
/// by `the_shell_readable_identity_names_the_compiled_image`, so the image a
/// host loads and the image this build demands cannot drift apart silently.
pub const DEFAULT_IMAGE: &str =
    "sha256:078dcf61392c9b24ad00dc8b293dd9acefc91f15c866762fa5423693efeaca21";

/// An exact lower-case SHA-256 image reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Image(ProofImage);

impl Image {
    /// Exact digest reference.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Default for Image {
    fn default() -> Self {
        Self(
            DEFAULT_IMAGE
                .parse()
                .expect("compiled proof image is a canonical digest reference"),
        )
    }
}

impl<'de> Deserialize<'de> for Image {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        ProofImage::deserialize(deserializer).map(Self)
    }
}

impl Serialize for Image {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

/// One lexically absolute executable location supplied by local configuration.
///
/// Filesystem shape, executable permission, runtime identity, and process
/// behavior remain the consuming proof boundary's responsibility.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutablePath(PathBuf);

impl ExecutablePath {
    /// Exact platform path admitted from configuration.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for ExecutablePath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl<'de> Deserialize<'de> for ExecutablePath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            return Err(D::Error::custom(
                "proof executable path must be an absolute platform path",
            ));
        }
        Ok(Self(path))
    }
}

impl Serialize for ExecutablePath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

/// Tracked proof policy plus optional machine-local executable locations.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    image: Image,
    #[serde(skip_serializing_if = "Option::is_none")]
    container: Option<ExecutablePath>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runner: Option<ExecutablePath>,
}

impl Config {
    /// Approved proof container identity.
    #[must_use]
    pub const fn image(&self) -> &Image {
        &self.image
    }

    /// Configured Docker or Podman executable location, when supplied.
    #[must_use]
    pub const fn container(&self) -> Option<&ExecutablePath> {
        self.container.as_ref()
    }

    /// Configured native sealed-runner executable location, when supplied.
    #[must_use]
    pub const fn runner(&self) -> Option<&ExecutablePath> {
        self.runner.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn fixture(container: Option<PathBuf>, runner: Option<PathBuf>) -> Self {
        Self {
            image: Image::default(),
            container: container.map(ExecutablePath),
            runner: runner.map(ExecutablePath),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity both installed-Spice proof-image xtask actions compare against.
    const SHELL_READABLE_IDENTITY: &str =
        ::std::include_str!("../../dist/proof-image/identity.txt").trim_ascii();

    #[test]
    fn the_shell_readable_identity_names_the_compiled_image() {
        assert_eq!(
            SHELL_READABLE_IDENTITY, DEFAULT_IMAGE,
            "dist/proof-image/identity.txt and proof::DEFAULT_IMAGE name different images, so a \
             host would load one appliance and this build would demand another"
        );
    }

    #[test]
    fn the_compiled_image_is_a_canonical_digest_reference() {
        assert_eq!(Image::default().as_str(), DEFAULT_IMAGE);
    }
}
