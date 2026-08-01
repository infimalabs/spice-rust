//! What one gate run decided, and the four digests that pin it to its inputs.
//!
//! A verdict that only says pass or fail is a rumour. This one names the tree
//! it read, the policy it read it under, the jurisdiction that policy produced,
//! and the measurements taken inside it. Any of those four changing changes a
//! digest, so a receipt can never be quoted about a run it did not describe.

use std::fmt;
use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};
use wire::Digest;

use crate::jurisdiction::{Jurisdiction, WalkError};
use crate::measure::{Bounds, MeasureError, Measurement, Policy, measurement_digest};
use crate::ratchet::{Finding, Ratchet};

/// Whether a run passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Standing {
    /// Every metric held, shrank, or is newly declared.
    Clean,
    /// At least one metric regressed or lost the declaration behind its
    /// baseline.
    Refused,
}

/// One run's receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verdict {
    /// Digest of the admitted files' contents.
    pub tree: Digest,
    /// Digest of the enforcement declaration.
    pub policy: Digest,
    /// Digest of every path reached, admitted or not.
    pub inventory: Digest,
    /// Digest of the measured quantities and the files behind them.
    pub measurement: Digest,
    /// Whether the run passed.
    pub standing: Standing,
    /// Where every metric stands, in metric order.
    pub findings: Vec<Finding>,
}

impl Verdict {
    /// The canonical bytes of this receipt.
    ///
    /// # Errors
    ///
    /// Returns the encoder error when the receipt cannot be rendered.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        wire::encode(self)
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

/// Everything one gate run produced.
#[derive(Debug, Clone)]
pub struct Run {
    jurisdiction: Jurisdiction,
    measurements: Vec<Measurement>,
    verdict: Verdict,
}

impl Run {
    /// Walk a foreign repository, measure it, and judge it against a ratchet.
    ///
    /// # Errors
    ///
    /// Returns the failure that stopped the run. No verdict is produced from a
    /// partial run, because a receipt naming fewer metrics than the policy
    /// declares reads exactly like a run in which those metrics were fine.
    pub fn adjudicate(
        policy: &Policy,
        root: &Path,
        ratchet: &Ratchet,
        bounds: &Bounds,
    ) -> Result<Self, RunError> {
        let jurisdiction = Jurisdiction::walk(root, policy.ignore()).map_err(RunError::Walk)?;
        let inventory = jurisdiction.inventory_digest();
        let tree = jurisdiction.tree_digest().map_err(RunError::Walk)?;
        let measurements = policy
            .measure(&jurisdiction, bounds)
            .map_err(RunError::Measure)?;

        let observed = Jurisdiction::walk(root, policy.ignore()).map_err(RunError::Walk)?;
        let observed_inventory = observed.inventory_digest();
        let observed_tree = observed.tree_digest().map_err(RunError::Walk)?;
        if inventory != observed_inventory || tree != observed_tree {
            return Err(RunError::JurisdictionChanged {
                inventory,
                observed_inventory,
                tree,
                observed_tree,
            });
        }

        let findings = ratchet.compare(&measurements);
        let standing = if findings.iter().all(|finding| finding.movement.passes()) {
            Standing::Clean
        } else {
            Standing::Refused
        };

        let verdict = Verdict {
            tree,
            policy: policy.digest(),
            inventory,
            measurement: measurement_digest(&measurements),
            standing,
            findings,
        };

        Ok(Self {
            jurisdiction,
            measurements,
            verdict,
        })
    }

    /// The jurisdiction this run walked.
    #[must_use]
    pub const fn jurisdiction(&self) -> &Jurisdiction {
        &self.jurisdiction
    }

    /// What this run measured.
    #[must_use]
    pub fn measurements(&self) -> &[Measurement] {
        &self.measurements
    }

    /// This run's receipt.
    #[must_use]
    pub const fn verdict(&self) -> &Verdict {
        &self.verdict
    }

    /// The ratchet this run earns, absent when the run was refused.
    ///
    /// A refused run earns nothing. Letting one tighten would take the baseline
    /// of every metric that behaved and hand it to a tree that has already
    /// failed, which is how a regression buys itself a lower bar somewhere
    /// else.
    #[must_use]
    pub fn earned(&self, ratchet: &Ratchet) -> Option<Ratchet> {
        match self.verdict.standing {
            Standing::Clean => Some(ratchet.tighten(&self.measurements)),
            Standing::Refused => None,
        }
    }
}

/// What stopped a run.
#[derive(Debug)]
pub enum RunError {
    /// The jurisdiction could not be established.
    Walk(WalkError),
    /// A metric could not be measured.
    Measure(MeasureError),
    /// The inventory or admitted contents moved during measurement.
    JurisdictionChanged {
        /// Inventory established before measurement.
        inventory: Digest,
        /// Inventory observed after measurement.
        observed_inventory: Digest,
        /// Admitted contents established before measurement.
        tree: Digest,
        /// Admitted contents observed after measurement.
        observed_tree: Digest,
    },
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Walk(source) => write!(formatter, "{source}"),
            Self::Measure(source) => write!(formatter, "{source}"),
            Self::JurisdictionChanged {
                inventory,
                observed_inventory,
                tree,
                observed_tree,
            } => write!(
                formatter,
                "jurisdiction changed during measurement: inventory {inventory} became \
                 {observed_inventory}; tree {tree} became {observed_tree}"
            ),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Walk(source) => Some(source),
            Self::Measure(source) => Some(source),
            Self::JurisdictionChanged { .. } => None,
        }
    }
}

/// Why a receipt did not reach its destination.
#[derive(Debug)]
pub enum PublishError {
    /// The receipt could not be rendered.
    Encode(serde_json::Error),
    /// The rendered receipt could not be published.
    Publish(atomic::Error),
}

impl fmt::Display for PublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode(source) => write!(formatter, "rendering the receipt: {source}"),
            Self::Publish(source) => write!(formatter, "publishing the receipt: {source}"),
        }
    }
}

impl std::error::Error for PublishError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Encode(source) => Some(source),
            Self::Publish(source) => Some(source),
        }
    }
}
