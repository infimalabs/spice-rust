//! Distribution payloads and release evidence shared by dist and proof.

use std::fmt;
use std::str::FromStr;

use serde::de::{Deserializer, Error as _};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::{Digest, WireVersion};

const OCI_SHA256_HEX_LEN: usize = 64;

/// The sole current journal backup bundle schema.
pub const JOURNAL_BACKUP_VERSION: u16 = 2;

/// One regular file authenticated by a journal backup manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JournalBackupMember {
    /// Canonical slash-separated path relative to the bundle root.
    pub path: String,
    /// POSIX permission bits, masked to `0o7777`.
    pub mode: u32,
    /// Exact byte length of the member.
    pub bytes: u64,
    /// Domain-separated identity of the member bytes.
    pub digest: Digest,
}

/// Complete immutable inventory for one journal backup bundle.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JournalBackupManifest {
    /// Current bundle schema.
    pub version: u16,
    /// Current crossing schema required to interpret the authority.
    pub wire: WireVersion,
    /// Finalized journal schema carried by the authority member.
    pub journal_schema: u32,
    /// Member path containing the SQLite authority.
    pub authority: String,
    /// Every regular file other than this manifest, ordered by path.
    pub members: Vec<JournalBackupMember>,
}

/// The sole current detached proof receipt schema.
pub const PROOF_RECEIPT_VERSION: u16 = 3;
/// The sole current target payload schema.
pub const TARGET_PAYLOAD_VERSION: u16 = 1;
/// The sole current multi-target release manifest schema.
pub const RELEASE_MANIFEST_VERSION: u16 = 1;
/// The sole current deterministic release-note document schema.
pub const RELEASE_NOTES_VERSION: u16 = 1;

/// An OCI image identity that is pinned by one canonical SHA-256 digest.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProofImage(String);

impl ProofImage {
    /// Exact admitted OCI identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this text is one admitted digest-pinned OCI identity.
    #[must_use]
    pub fn admits(text: &str) -> bool {
        let digest = if let Some(digest) = text.strip_prefix("sha256:") {
            Some(digest)
        } else if let Some((repository, digest)) = text.rsplit_once("@sha256:") {
            canonical_repository(repository).then_some(digest)
        } else {
            None
        };
        digest.is_some_and(|digest| {
            digest.len() == OCI_SHA256_HEX_LEN
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    }
}

fn canonical_repository(repository: &str) -> bool {
    let components = repository.split('/').collect::<Vec<_>>();
    if components.iter().any(|component| component.is_empty()) {
        return false;
    }
    let path_start = if components.len() > 1
        && (components[0].contains(['.', ':']) || components[0] == "localhost")
    {
        if !canonical_registry(components[0]) {
            return false;
        }
        1
    } else {
        0
    };
    components[path_start..]
        .iter()
        .all(|component| canonical_repository_component(component))
}

fn canonical_registry(registry: &str) -> bool {
    let (host, port) = match registry.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (registry, None),
    };
    if port.is_some_and(|port| port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit())) {
        return false;
    }
    !host.is_empty()
        && host.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn canonical_repository_component(component: &str) -> bool {
    let bytes = component.as_bytes();
    if bytes.is_empty()
        || !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit()
        || !bytes[bytes.len() - 1].is_ascii_lowercase() && !bytes[bytes.len() - 1].is_ascii_digit()
    {
        return false;
    }
    let mut previous_separator = false;
    for &byte in bytes {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            previous_separator = false;
        } else if matches!(byte, b'.' | b'_' | b'-') && !previous_separator {
            previous_separator = true;
        } else {
            return false;
        }
    }
    true
}

/// Why an OCI proof image identity was refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("proof image must be a canonical lower-case sha256 digest reference: {0:?}")]
pub struct ProofImageError(String);

impl FromStr for ProofImage {
    type Err = ProofImageError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if Self::admits(text) {
            Ok(Self(text.to_owned()))
        } else {
            Err(ProofImageError(text.to_owned()))
        }
    }
}

impl fmt::Display for ProofImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for ProofImage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ProofImage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(D::Error::custom)
    }
}

/// The one sealed execution identity carried by replay evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum ProofEnvironment {
    /// A digest-pinned OCI image executed through the sealed container route.
    Oci { image: ProofImage },
    /// The domain-separated digest of one admitted native runner executable.
    Native { runner: Digest },
}

impl ProofEnvironment {
    /// Construct an OCI identity from its strict wire spelling.
    ///
    /// # Errors
    ///
    /// Refuses tags, upper-case digests, and noncanonical repository spellings.
    pub fn oci(image: impl AsRef<str>) -> Result<Self, ProofImageError> {
        Ok(Self::Oci {
            image: image.as_ref().parse()?,
        })
    }

    /// Construct a native identity from observed runner bytes.
    #[must_use]
    pub const fn native(runner: Digest) -> Self {
        Self::Native { runner }
    }

    /// OCI identity, when this is the container variant.
    #[must_use]
    pub const fn oci_image(&self) -> Option<&ProofImage> {
        match self {
            Self::Oci { image } => Some(image),
            Self::Native { .. } => None,
        }
    }

    /// Native runner digest, when this is the native variant.
    #[must_use]
    pub const fn native_runner(&self) -> Option<&Digest> {
        match self {
            Self::Native { runner } => Some(runner),
            Self::Oci { .. } => None,
        }
    }
}

/// A release tuple with a distinct artifact identity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum ReleaseTarget {
    #[serde(rename = "aarch64-apple-darwin")]
    Aarch64AppleDarwin,
    #[serde(rename = "x86_64-unknown-linux-musl")]
    X86_64UnknownLinuxMusl,
}

impl ReleaseTarget {
    /// The canonical Rust target-triple spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Aarch64AppleDarwin => "aarch64-apple-darwin",
            Self::X86_64UnknownLinuxMusl => "x86_64-unknown-linux-musl",
        }
    }
}

impl fmt::Display for ReleaseTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Exact identities required before distribution may build.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildPins {
    pub rust: String,
    pub wasm_target: String,
    pub trunk: String,
    pub cargo_mutants: String,
    pub proof_container: String,
}

/// One staged file bound into a target payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryEntry {
    pub path: String,
    pub mode: u32,
    pub bytes: u64,
    pub digest: Digest,
}

/// The receipt-free object submitted to proof for one release target.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TargetPayload {
    pub version: u16,
    pub target: ReleaseTarget,
    /// Domain-separated identity of the immutable Git commit built.
    pub source: Digest,
    pub app: Digest,
    pub embedded_app: Digest,
    pub native: Digest,
    pub pins: BuildPins,
    pub pins_digest: Digest,
    pub inventory: Vec<InventoryEntry>,
    pub inventory_digest: Digest,
}

/// Whether the complete sealed proof battery passed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProofStanding {
    /// Every declared battery member completed and its evidence was whole.
    Clean,
    /// At least one battery member failed, timed out, or produced incomplete evidence.
    Refused,
}

/// Detached evidence binding one sealed replay to one receipt-free target payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProofReceipt {
    /// Current detached-receipt schema.
    pub version: u16,
    /// Release tuple the payload and replay both cover.
    pub target: ReleaseTarget,
    /// Canonical identity of the receipt-free target payload.
    pub payload: Digest,
    /// Canonical identity of the ordered battery command inventory.
    pub battery: Digest,
    /// Canonical identity of the complete typed replay result.
    pub replay: Digest,
    /// Digest-pinned container or native runner identity that executed the replay.
    pub environment: ProofEnvironment,
    /// One standing for the complete battery.
    pub standing: ProofStanding,
}

/// One proof-admitted target in the complete release manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifestTarget {
    /// Release tuple carried by both the payload and receipt.
    pub target: ReleaseTarget,
    /// Canonical receipt-free target-payload identity.
    pub payload: Digest,
    /// Canonical detached proof-receipt identity.
    pub receipt: Digest,
    /// Current generated proof battery identity.
    pub battery: Digest,
    /// Complete replay-result identity.
    pub replay: Digest,
    /// Exact sealed environment that executed the replay.
    pub environment: ProofEnvironment,
    /// Explicit clean standing verified before this entry was constructed.
    pub standing: ProofStanding,
}

/// The canonical complete release assembled from both supported targets.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    /// Current release-manifest schema.
    pub version: u16,
    /// One exact source identity shared by every target payload.
    pub source: Digest,
    /// Both supported targets in [`ReleaseTarget`] declaration order.
    pub targets: Vec<ReleaseManifestTarget>,
}
