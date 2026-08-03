//! One publishable receipt per complete battery run, and the explicit dispatch
//! that re-admits retained receipt bytes.
//!
//! A [`BatteryReport`] says what twenty-five arms decided. It does not say
//! which checked-in disposition document compiled them, which floors they
//! answered to, or what the run's own identity is, so retained report bytes
//! cannot be re-admitted without asking the reader to remember. A receipt binds
//! those identities to the run, and it is minted only from a run that finished.
//!
//! Re-admission is a lookup, never a branch. [`BatteryReceipt::verify`] hashes
//! the bytes it was handed before decoding anything, reads only a schema number
//! and the recorded gate-set digest back out of them, and matches that pair
//! against [`supported_pairs`]. A pair this build does not carry comes back
//! unread and byte-identical. Retiring a schema removes a row and freezing a
//! historical one adds a row, so the number of executable verification paths
//! stays at one however many releases accumulate.

use std::fmt;
use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use super::execution::{BatteryReport, gate_set_digest};
use super::manifest_contract::{BatteryManifest, CANONICAL_BATTERY_MANIFEST};
use super::{
    ActorEvidence, BatteryArmResult, BatteryError, BatteryScope, BatteryToolEvidence,
    BoundEvidence, ConfigurationEvidence, LatchEvidence,
};
use crate::verdict::PublishError;

/// The one battery receipt schema this build mints and admits.
pub const GATE_BATTERY_RECEIPT_VERSION: u64 = 1;

/// One complete battery run bound to every identity that pins it.
///
/// This type deliberately does not implement [`Deserialize`]. A derived one
/// would be a second door into retained bytes that skips dispatch, every
/// invariant, and the canonical byte rule, and a door that does not exist
/// cannot be walked through by mistake. [`BatteryReceipt::verify`] is the only
/// way in, and it decodes through a private shape declared below.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BatteryReceipt {
    version: u64,
    manifest: Digest,
    baseline: Digest,
    report: Digest,
    run: BatteryReport,
}

/// The receipt shape exactly as it is spelled on disk, admitted nowhere else.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReceipt {
    version: u64,
    manifest: Digest,
    baseline: Digest,
    report: Digest,
    run: BatteryReport,
}

impl From<RawReceipt> for BatteryReceipt {
    fn from(raw: RawReceipt) -> Self {
        Self {
            version: raw.version,
            manifest: raw.manifest,
            baseline: raw.baseline,
            report: raw.report,
            run: raw.run,
        }
    }
}

impl BatteryReceipt {
    /// Bind one completed battery run to the current receipt schema.
    ///
    /// A [`BatteryReport`] is the only way in, and it cannot be assembled by
    /// hand: its fields are private and both of its constructors validate, so a
    /// partial, reordered, duplicated, or digest-mismatched vector is already
    /// refused before it can reach a receipt. A second check here would be a
    /// guard no caller could ever trip, so the same invariants are proven where
    /// foreign bytes really do reach them, in
    /// `truncated_tampered_and_reordered_receipts_are_refused_by_the_invariant_they_break`,
    /// rather than restated here as unreachable code.
    #[must_use]
    pub fn mint(run: BatteryReport, baseline: &Digest) -> Self {
        Self {
            version: GATE_BATTERY_RECEIPT_VERSION,
            manifest: manifest_digest(),
            baseline: baseline.clone(),
            report: report_digest(&run),
            run,
        }
    }

    /// Receipt schema this build minted or admitted.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Identity of the checked-in disposition document behind the run.
    #[must_use]
    pub const fn manifest(&self) -> &Digest {
        &self.manifest
    }

    /// Identity of the complete compiled gate set the run executed.
    #[must_use]
    pub const fn gate_set(&self) -> &Digest {
        self.run.gate_set()
    }

    /// Identity of the captured tree every arm measured.
    #[must_use]
    pub const fn tree(&self) -> &Digest {
        self.run.context().source()
    }

    /// Identity of the complete compiled policy document.
    #[must_use]
    pub const fn policy(&self) -> &Digest {
        self.run.context().policy()
    }

    /// Identity of the complete state ledger the run read.
    #[must_use]
    pub const fn state(&self) -> &Digest {
        self.run.context().state()
    }

    /// Complete bound decisions supplied to the run.
    #[must_use]
    pub fn bounds(&self) -> &[BoundEvidence] {
        self.run.context().bounds()
    }

    /// Identity of the complete versioned floor state.
    #[must_use]
    pub const fn baseline(&self) -> &Digest {
        &self.baseline
    }

    /// Explicit resolved actor the run ran as.
    #[must_use]
    pub const fn actor(&self) -> &ActorEvidence {
        self.run.context().actor()
    }

    /// Explicit selected scope.
    #[must_use]
    pub const fn scope(&self) -> &BatteryScope {
        self.run.context().scope()
    }

    /// Resolved configuration values and the provenance that won each one.
    #[must_use]
    pub fn configuration(&self) -> &[ConfigurationEvidence] {
        self.run.context().configuration()
    }

    /// Complete sticky-state inventory supplied to the run.
    #[must_use]
    pub fn latches(&self) -> &[LatchEvidence] {
        self.run.context().latches()
    }

    /// Exactly twenty-five typed results in canonical order.
    #[must_use]
    pub fn results(&self) -> &[BatteryArmResult] {
        self.run.results()
    }

    /// Every discovered executable identity behind the run.
    #[must_use]
    pub fn tools(&self) -> &[BatteryToolEvidence] {
        self.run.context().tools()
    }

    /// Identity of the complete report this receipt was minted from.
    #[must_use]
    pub const fn report(&self) -> &Digest {
        &self.report
    }

    /// The complete run this receipt binds.
    #[must_use]
    pub const fn run(&self) -> &BatteryReport {
        &self.run
    }

    /// Canonical JSON bytes for the complete receipt.
    ///
    /// # Errors
    ///
    /// Returns the encoder error when the receipt cannot be rendered.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        wire::encode(self)
    }

    /// Identity of these exact retained bytes.
    #[must_use]
    pub fn digest(&self) -> Digest {
        let bytes = self
            .encode()
            .expect("the receipt schema contains only finite JSON values");
        Digest::compute(Domain::GateBatteryReceipt, &bytes)
    }

    /// Re-admit retained bytes as current evidence, or say exactly why not.
    ///
    /// The bytes are hashed before anything is decoded, so the identity in a
    /// refusal describes what arrived rather than what survived parsing.
    ///
    /// # Errors
    ///
    /// Returns [`ReceiptRefusal::UnsupportedVersion`], carrying the observed
    /// pair and the bytes untouched, when the recorded pair is not one this
    /// build interprets; returns [`ReceiptRefusal::Malformed`] when a pair it
    /// does interpret is not admissible evidence.
    pub fn verify(bytes: &[u8]) -> Result<Self, ReceiptRefusal> {
        let digest = Digest::compute(Domain::GateBatteryReceipt, bytes);
        let envelope: ReceiptEnvelope = wire::decode(bytes).map_err(|error| {
            ReceiptRefusal::Malformed(BatteryError::new(format!(
                "battery receipt has no readable version envelope: {error}"
            )))
        })?;
        if !supported(envelope.version, &envelope.run.gate_set) {
            return Err(ReceiptRefusal::UnsupportedVersion(UnsupportedReceipt {
                version: envelope.version,
                gate_set: envelope.run.gate_set,
                digest,
                bytes: bytes.to_vec(),
            }));
        }
        let raw: RawReceipt = wire::decode(bytes).map_err(|error| {
            ReceiptRefusal::Malformed(BatteryError::new(format!(
                "invalid battery receipt: {error}"
            )))
        })?;
        let receipt = Self::from(raw);
        receipt.validate().map_err(ReceiptRefusal::Malformed)?;
        let canonical = receipt.encode().map_err(|error| {
            ReceiptRefusal::Malformed(BatteryError::new(format!(
                "cannot encode battery receipt: {error}"
            )))
        })?;
        if canonical != bytes {
            return Err(ReceiptRefusal::Malformed(BatteryError::new(
                "battery receipt bytes are not canonical",
            )));
        }
        Ok(receipt)
    }

    /// Every invariant a receipt must hold, checked before its bytes are
    /// compared.
    ///
    /// Ordering is the whole point: a forged receipt that violates one of these
    /// would also fail the byte comparison, and refusing it there first would
    /// report every mutation as merely noncanonical and prove nothing about the
    /// invariant it actually broke.
    fn validate(&self) -> Result<(), BatteryError> {
        self.run.validate()?;
        if self.manifest != manifest_digest() {
            return Err(BatteryError::new(
                "battery receipt names a foreign canonical manifest digest",
            ));
        }
        if self.report != report_digest(&self.run) {
            return Err(BatteryError::new(
                "battery receipt report digest disagrees with its own run",
            ));
        }
        Ok(())
    }

    /// Publish this receipt so no reader ever sees a partial one.
    ///
    /// The bytes are staged beside the destination and swapped in one step. A
    /// run interrupted before the swap leaves the previous receipt exactly as
    /// it was, which is the only honest thing an unfinished run can leave.
    ///
    /// # Errors
    ///
    /// Returns the encoder or publication failure.
    pub fn publish(&self, destination: &Path) -> Result<(), PublishError> {
        let bytes = self.encode().map_err(PublishError::Encode)?;
        atomic::Destination::new(destination)
            .map_err(PublishError::Publish)?
            .publish(|staged| staged.write_all(&bytes))
            .map_err(PublishError::Publish)
    }
}

/// Only the pair verification dispatches on, read out of bytes this build may
/// not understand.
///
/// Unknown fields are deliberately admitted here and nowhere else. A foreign
/// schema is exactly the case this envelope exists to name, and it cannot be
/// named by a decoder that refuses to read it.
#[derive(Debug, Deserialize)]
struct ReceiptEnvelope {
    version: u64,
    run: GateSetEnvelope,
}

/// The recorded gate-set digest, read from the run that carries it.
///
/// The receipt does not repeat this digest at its top level. One recorded
/// gate set means one field, which cannot disagree with itself.
#[derive(Debug, Deserialize)]
struct GateSetEnvelope {
    gate_set: Digest,
}

/// Every receipt schema and gate set this build interprets.
///
/// This is data, not control flow. Today exactly one pair is current and no
/// historical pair has been frozen, so the table has one row; a build that
/// froze a predecessor contract would carry two rows and still one decoder.
#[must_use]
pub fn supported_pairs() -> [(u64, Digest); 1] {
    [(GATE_BATTERY_RECEIPT_VERSION, current_gate_set())]
}

fn supported(version: u64, gate_set: &Digest) -> bool {
    supported_pairs()
        .iter()
        .any(|(schema, digest)| *schema == version && digest == gate_set)
}

fn current_gate_set() -> Digest {
    gate_set_digest(&BatteryManifest::canonical().expect("the checked-in manifest is canonical"))
}

fn manifest_digest() -> Digest {
    Digest::compute(
        Domain::GateBatteryManifest,
        CANONICAL_BATTERY_MANIFEST.as_bytes(),
    )
}

fn report_digest(run: &BatteryReport) -> Digest {
    let bytes = run
        .encode()
        .expect("the report schema contains only finite JSON values");
    Digest::compute(Domain::GateBatteryReport, &bytes)
}

/// Bytes naming a receipt schema and gate set this build does not interpret.
///
/// Nothing here is normalized, migrated, or reinterpreted. The retained bytes
/// are the ones that arrived, and the only current claim made about them is
/// their identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnsupportedReceipt {
    version: u64,
    gate_set: Digest,
    digest: Digest,
    bytes: Vec<u8>,
}

impl UnsupportedReceipt {
    /// Exact foreign schema number retained as evidence.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Exact foreign gate-set digest retained as evidence.
    #[must_use]
    pub const fn gate_set(&self) -> &Digest {
        &self.gate_set
    }

    /// Identity of the original bytes, computed before they were decoded.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }

    /// The original bytes exactly as they arrived.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Why retained receipt bytes were not admitted as current evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReceiptRefusal {
    /// The bytes name a schema and gate set this build does not interpret.
    UnsupportedVersion(UnsupportedReceipt),
    /// The bytes name the current pair but are not admissible evidence.
    Malformed(BatteryError),
}

impl ReceiptRefusal {
    /// The retained foreign bytes, when the refusal was a version boundary.
    #[must_use]
    pub const fn unsupported(&self) -> Option<&UnsupportedReceipt> {
        match self {
            Self::UnsupportedVersion(receipt) => Some(receipt),
            Self::Malformed(_) => None,
        }
    }
}

impl fmt::Display for ReceiptRefusal {
    /// Render the identifiers and never the bytes.
    ///
    /// Spelling foreign bytes into a current message is the quietest way to
    /// launder them into evidence, so an unsupported receipt renders only what
    /// it observed and what this build carries instead. What it carries is read
    /// out of [`supported_pairs`] rather than restated, so a frozen historical
    /// row cannot be added to the table and forgotten by the message.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(receipt) => {
                write!(
                    formatter,
                    "battery receipt schema {} recorded gate set {} and is unsupported; this \
                     build interprets",
                    receipt.version, receipt.gate_set,
                )?;
                for (index, (version, gate_set)) in supported_pairs().iter().enumerate() {
                    let separator = if index == 0 { " " } else { ", " };
                    write!(
                        formatter,
                        "{separator}schema {version} with gate set {gate_set}"
                    )?;
                }
                Ok(())
            }
            Self::Malformed(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for ReceiptRefusal {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Malformed(error) => Some(error),
            Self::UnsupportedVersion(_) => None,
        }
    }
}
