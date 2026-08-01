//! What one current gate run decided, and every identity that pins its receipt.
//!
//! A verdict that only says pass or fail is a rumour. A publishable receipt is
//! minted only by [`Contract::adjudicate`]: it binds the selected contents, the
//! compiled gate set, every loaded declaration, every probed tool, and the
//! ordered outcome of the completed run. Decoded receipts remain evidence;
//! there is no path from their retained bytes back into executable inputs.

use std::fmt;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use crate::input::{Contract, ToolIdentity};
use crate::jurisdiction::{Jurisdiction, WalkError};
use crate::measure::{Bounds, MeasureError, Measurement, Policy, measurement_digest};
use crate::ratchet::{Finding, Ratchet};

/// The one receipt schema this build emits and admits.
pub const GATE_RECEIPT_VERSION: u64 = 1;

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

impl Standing {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Refused => "refused",
        }
    }
}

/// One compiled tool identity retained in a receipt without executable data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptTool {
    name: String,
    identity: Digest,
}

impl ReceiptTool {
    fn current(tool: &ToolIdentity) -> Self {
        Self {
            name: tool.name().to_owned(),
            identity: tool.digest().clone(),
        }
    }

    /// Stable compiled tool name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Identity of the declaration and executable bytes observed at preflight.
    #[must_use]
    pub const fn identity(&self) -> &Digest {
        &self.identity
    }
}

/// One completed current run's canonical, versioned receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verdict {
    version: u64,
    /// Digest of the admitted files' contents.
    tree: Digest,
    /// Digest of the complete compiled gate set.
    gate_set: Digest,
    /// Digest of the enforcement declaration.
    policy: Digest,
    /// Digest of every path reached, admitted or not.
    inventory: Digest,
    /// Digest of the measured quantities and the files behind them.
    measurement: Digest,
    /// Digest of the complete current process bounds.
    bounds: Digest,
    /// Digest of the complete versioned floor state.
    baseline: Digest,
    /// Every current compiled tool identity, in declaration order.
    tools: Vec<ReceiptTool>,
    /// Whether the run passed.
    standing: Standing,
    /// Where every metric stands, in metric order.
    findings: Vec<Finding>,
}

impl Verdict {
    fn current(contract: &Contract, run: &Run) -> Self {
        Self {
            version: GATE_RECEIPT_VERSION,
            tree: run.tree.clone(),
            gate_set: contract.gate_set().digest(),
            policy: run.policy.clone(),
            inventory: run.inventory.clone(),
            measurement: run.measurement.clone(),
            bounds: contract.bounds().digest(),
            baseline: contract.baseline().digest(),
            tools: contract.tools().iter().map(ReceiptTool::current).collect(),
            standing: run.standing,
            findings: run.findings.clone(),
        }
    }

    /// Receipt schema admitted by this build.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Digest of admitted file contents.
    #[must_use]
    pub const fn tree(&self) -> &Digest {
        &self.tree
    }

    /// Digest of the compiled executable gate set.
    #[must_use]
    pub const fn gate_set(&self) -> &Digest {
        &self.gate_set
    }

    /// Digest of effective policy.
    #[must_use]
    pub const fn policy(&self) -> &Digest {
        &self.policy
    }

    /// Digest of every reached path and its admission reason.
    #[must_use]
    pub const fn inventory(&self) -> &Digest {
        &self.inventory
    }

    /// Digest of all completed measurements.
    #[must_use]
    pub const fn measurement(&self) -> &Digest {
        &self.measurement
    }

    /// Digest of current process bounds.
    #[must_use]
    pub const fn bounds(&self) -> &Digest {
        &self.bounds
    }

    /// Digest of the complete versioned baseline.
    #[must_use]
    pub const fn baseline(&self) -> &Digest {
        &self.baseline
    }

    /// Current compiled tools and the identities observed at preflight.
    #[must_use]
    pub fn tools(&self) -> &[ReceiptTool] {
        &self.tools
    }

    /// Whether the run passed.
    #[must_use]
    pub const fn standing(&self) -> Standing {
        self.standing
    }

    /// Every finding, in metric order.
    #[must_use]
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// Human-readable canonical text carrying every receipt field in order.
    #[must_use]
    pub fn render(&self) -> String {
        let mut rendered = format!(
            "gate-receipt {}\ntree {}\ngate-set {}\npolicy {}\ninventory {}\n\
             measurement {}\nbounds {}\nbaseline {}\n",
            self.version,
            self.tree,
            self.gate_set,
            self.policy,
            self.inventory,
            self.measurement,
            self.bounds,
            self.baseline,
        );
        for tool in &self.tools {
            let _ = writeln!(rendered, "tool {} {}", tool.name, tool.identity);
        }
        let _ = writeln!(rendered, "standing {}", self.standing.as_str());
        for finding in &self.findings {
            let _ = writeln!(rendered, "finding {} {}", finding.metric, finding.movement);
        }
        rendered
    }

    /// Identity of the entire frozen receipt schema and its current values.
    #[must_use]
    pub fn digest(&self) -> Digest {
        let bytes = self
            .encode()
            .expect("the receipt schema contains only infallible JSON values");
        Digest::compute(Domain::GateReceipt, &bytes)
    }

    /// The canonical bytes of this receipt.
    ///
    /// # Errors
    ///
    /// Returns the encoder error when the receipt cannot be rendered.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        wire::encode(self)
    }

    /// Decode only the current receipt schema and only its canonical JSON.
    ///
    /// # Errors
    ///
    /// Returns malformed fields, an unsupported version with the observed
    /// number retained as evidence, or the canonical bytes for a second JSON
    /// spelling of an otherwise valid receipt.
    pub fn decode(bytes: &[u8]) -> Result<Self, ReceiptDecodeError> {
        let evidence: VersionEvidence = wire::decode(bytes).map_err(ReceiptDecodeError::Json)?;
        if evidence.version != GATE_RECEIPT_VERSION {
            return Err(ReceiptDecodeError::UnsupportedVersion {
                observed: evidence.version,
            });
        }
        let receipt: Self = wire::decode(bytes).map_err(ReceiptDecodeError::Json)?;
        let canonical = receipt.encode().map_err(ReceiptDecodeError::Json)?;
        if bytes != canonical {
            return Err(ReceiptDecodeError::NonCanonical { canonical });
        }
        Ok(receipt)
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

#[derive(Debug, Deserialize)]
struct VersionEvidence {
    version: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawVerdict {
    version: u64,
    tree: Digest,
    gate_set: Digest,
    policy: Digest,
    inventory: Digest,
    measurement: Digest,
    bounds: Digest,
    baseline: Digest,
    tools: Vec<ReceiptTool>,
    standing: Standing,
    findings: Vec<Finding>,
}

impl From<RawVerdict> for Verdict {
    fn from(raw: RawVerdict) -> Self {
        Self {
            version: raw.version,
            tree: raw.tree,
            gate_set: raw.gate_set,
            policy: raw.policy,
            inventory: raw.inventory,
            measurement: raw.measurement,
            bounds: raw.bounds,
            baseline: raw.baseline,
            tools: raw.tools,
            standing: raw.standing,
            findings: raw.findings,
        }
    }
}

impl<'de> Deserialize<'de> for Verdict {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let raw = RawVerdict::deserialize(deserializer)?;
        if raw.version != GATE_RECEIPT_VERSION {
            return Err(D::Error::custom(format!(
                "receipt version {} is unsupported; current version is {}",
                raw.version, GATE_RECEIPT_VERSION
            )));
        }
        Ok(Self::from(raw))
    }
}

/// Everything one gate run produced.
#[derive(Debug, Clone)]
pub struct Run {
    jurisdiction: Jurisdiction,
    measurements: Vec<Measurement>,
    tree: Digest,
    policy: Digest,
    inventory: Digest,
    measurement: Digest,
    standing: Standing,
    findings: Vec<Finding>,
}

/// A completed run bound to the exact current contract that executed it.
#[derive(Debug, Clone)]
pub struct CurrentRun {
    run: Run,
    receipt: Verdict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotIntegrity {
    KernelChangeIdentity,
    Unsupported,
}

impl SnapshotIntegrity {
    const fn native() -> Self {
        if cfg!(target_family = "unix") {
            Self::KernelChangeIdentity
        } else {
            Self::Unsupported
        }
    }
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
        Self::adjudicate_with_integrity(policy, root, ratchet, bounds, SnapshotIntegrity::native())
    }

    fn adjudicate_with_integrity(
        policy: &Policy,
        root: &Path,
        ratchet: &Ratchet,
        bounds: &Bounds,
        snapshot_integrity: SnapshotIntegrity,
    ) -> Result<Self, RunError> {
        if snapshot_integrity == SnapshotIntegrity::Unsupported {
            return Err(RunError::SnapshotIntegrityUnsupported);
        }

        let jurisdiction = Jurisdiction::walk(root, policy.ignore()).map_err(RunError::Walk)?;
        let inventory = jurisdiction.inventory_digest();
        let snapshot = jurisdiction.snapshot().map_err(RunError::Walk)?;
        let tree = snapshot.tree();
        let mut measurements = Vec::with_capacity(policy.metrics().len());
        for metric in policy.metrics() {
            let measurement = metric.measure(snapshot.jurisdiction(), bounds);
            if let Some(changed) = snapshot.verify().map_err(RunError::Walk)? {
                return Err(RunError::SnapshotChanged {
                    metric: metric.name().to_owned(),
                    measured: measurement.as_ref().ok().map(Measurement::value),
                    path: changed.path,
                    tree: changed.tree,
                    observed_tree: changed.observed_tree,
                });
            }
            measurements.push(measurement.map_err(RunError::Measure)?);
        }

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
        let measurement = measurement_digest(&measurements);

        Ok(Self {
            jurisdiction,
            measurements,
            tree,
            policy: policy.digest(),
            inventory,
            measurement,
            standing,
            findings,
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

    /// Digest of admitted file contents measured by this run.
    #[must_use]
    pub const fn tree(&self) -> &Digest {
        &self.tree
    }

    /// Digest of effective policy used by this run.
    #[must_use]
    pub const fn policy(&self) -> &Digest {
        &self.policy
    }

    /// Digest of every path reached by this run.
    #[must_use]
    pub const fn inventory(&self) -> &Digest {
        &self.inventory
    }

    /// Digest of the completed measurements.
    #[must_use]
    pub const fn measurement(&self) -> &Digest {
        &self.measurement
    }

    /// Whether the completed low-level run passed its ratchet.
    #[must_use]
    pub const fn standing(&self) -> Standing {
        self.standing
    }

    /// Ordered findings produced by the ratchet comparison.
    #[must_use]
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// The ratchet this run earns, absent when the run was refused.
    ///
    /// A refused run earns nothing. Letting one tighten would take the baseline
    /// of every metric that behaved and hand it to a tree that has already
    /// failed, which is how a regression buys itself a lower bar somewhere
    /// else.
    #[must_use]
    pub fn earned(&self, ratchet: &Ratchet) -> Option<Ratchet> {
        match self.standing {
            Standing::Clean => Some(ratchet.tighten(&self.measurements)),
            Standing::Refused => None,
        }
    }
}

impl Contract {
    /// Execute this current contract and bind its complete identity to the
    /// completed run in one receipt.
    ///
    /// # Errors
    ///
    /// Returns the run failure. A partial run never reaches receipt
    /// construction.
    pub fn adjudicate(&self, root: &Path) -> Result<CurrentRun, RunError> {
        let run = Run::adjudicate(self.policy(), root, self.ratchet(), self.bounds())?;
        let receipt = Verdict::current(self, &run);
        Ok(CurrentRun { run, receipt })
    }
}

impl CurrentRun {
    /// Completed low-level run evidence.
    #[must_use]
    pub const fn run(&self) -> &Run {
        &self.run
    }

    /// Complete current receipt, constructed only after the run completed.
    #[must_use]
    pub const fn receipt(&self) -> &Verdict {
        &self.receipt
    }

    /// The jurisdiction this current run walked.
    #[must_use]
    pub const fn jurisdiction(&self) -> &Jurisdiction {
        self.run.jurisdiction()
    }

    /// What this current run measured.
    #[must_use]
    pub fn measurements(&self) -> &[Measurement] {
        self.run.measurements()
    }

    /// The ratchet this current run earns, absent when it was refused.
    #[must_use]
    pub fn earned(&self, ratchet: &Ratchet) -> Option<Ratchet> {
        self.run.earned(ratchet)
    }
}

/// What stopped a run.
#[derive(Debug)]
pub enum RunError {
    /// This target cannot prove that a parser left its snapshot unchanged.
    SnapshotIntegrityUnsupported,
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
    /// A parser changed the private jurisdiction snapshot it was measuring.
    SnapshotChanged {
        /// The metric whose parser changed the snapshot.
        metric: String,
        /// The number it printed, when it produced one trustworthy count.
        measured: Option<u64>,
        /// The first snapshot path whose kernel state moved.
        path: String,
        /// Admitted contents captured before any parser ran.
        tree: Digest,
        /// Admitted contents after the parser, when they remained readable.
        observed_tree: Option<Digest>,
    },
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SnapshotIntegrityUnsupported => write!(
                formatter,
                "snapshot integrity is unsupported on this target; no parser was launched"
            ),
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
            Self::SnapshotChanged {
                metric,
                measured,
                path,
                tree,
                observed_tree,
            } => write!(
                formatter,
                "{metric} parser changed snapshot path {path} after measuring {measured:?}: \
                 tree {tree} became {observed_tree:?}"
            ),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Walk(source) => Some(source),
            Self::Measure(source) => Some(source),
            Self::SnapshotIntegrityUnsupported
            | Self::JurisdictionChanged { .. }
            | Self::SnapshotChanged { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use sill::Deadlines;
    use wire::{Outcome, Reply, ReplyPosition};

    use super::*;
    use crate::jurisdiction::Ignore;
    use crate::measure::{Metric, Parser};
    use crate::ratchet::Movement;

    fn receipt_digest(label: &str) -> Digest {
        Digest::compute(Domain::GateFile, label.as_bytes())
    }

    fn sample_receipt() -> Verdict {
        Verdict {
            version: GATE_RECEIPT_VERSION,
            tree: receipt_digest("tree"),
            gate_set: receipt_digest("gate-set"),
            policy: receipt_digest("policy"),
            inventory: receipt_digest("inventory"),
            measurement: receipt_digest("measurement"),
            bounds: receipt_digest("bounds"),
            baseline: receipt_digest("baseline"),
            tools: vec![ReceiptTool {
                name: "awk".to_owned(),
                identity: receipt_digest("awk"),
            }],
            standing: Standing::Clean,
            findings: vec![Finding {
                metric: "rust.long-lines".to_owned(),
                movement: Movement::Held { baseline: 70 },
            }],
        }
    }

    #[test]
    fn canonical_text_and_json_round_trip_without_losing_ordered_evidence() {
        let receipt = sample_receipt();
        let expected_text = format!(
            "gate-receipt 1\ntree {}\ngate-set {}\npolicy {}\ninventory {}\n\
             measurement {}\nbounds {}\nbaseline {}\ntool awk {}\nstanding clean\n\
             finding rust.long-lines held at 70\n",
            receipt.tree,
            receipt.gate_set,
            receipt.policy,
            receipt.inventory,
            receipt.measurement,
            receipt.bounds,
            receipt.baseline,
            receipt.tools[0].identity,
        );
        assert_eq!(receipt.render(), expected_text);

        let encoded = receipt.encode().expect("encode canonical receipt");
        assert_eq!(receipt.encode().expect("encode again"), encoded);
        let decoded = Verdict::decode(&encoded).expect("decode canonical receipt");
        assert_eq!(decoded, receipt);
        assert_eq!(decoded.render(), expected_text);
        let outcome = Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body: receipt,
        });
        assert_eq!(
            wire::encode(&outcome).expect("encode direct verdict body"),
            wire::encode(&outcome).expect("encode direct verdict body again")
        );
    }

    #[test]
    fn receipt_json_refuses_unknown_missing_and_second_spellings() {
        let receipt = sample_receipt();
        let encoded = receipt.encode().expect("encode canonical receipt");

        let mut unknown: serde_json::Value =
            serde_json::from_slice(&encoded).expect("decode fixture value");
        unknown
            .as_object_mut()
            .expect("receipt is an object")
            .insert("argv".to_owned(), serde_json::json!(["untrusted"]));
        assert!(matches!(
            Verdict::decode(&serde_json::to_vec(&unknown).expect("encode unknown field")),
            Err(ReceiptDecodeError::Json(_))
        ));

        let mut nested_unknown: serde_json::Value =
            serde_json::from_slice(&encoded).expect("decode nested fixture value");
        nested_unknown["findings"][0]["movement"]
            .as_object_mut()
            .expect("movement is an object")
            .insert("unknown".to_owned(), serde_json::json!(true));
        assert!(matches!(
            Verdict::decode(
                &serde_json::to_vec(&nested_unknown).expect("encode nested unknown field")
            ),
            Err(ReceiptDecodeError::Json(_))
        ));

        let mut missing: serde_json::Value =
            serde_json::from_slice(&encoded).expect("decode fixture value");
        missing
            .as_object_mut()
            .expect("receipt is an object")
            .remove("bounds");
        assert!(matches!(
            Verdict::decode(&serde_json::to_vec(&missing).expect("encode missing field")),
            Err(ReceiptDecodeError::Json(_))
        ));

        let mut padded = Vec::with_capacity(encoded.len() + 1);
        padded.push(b' ');
        padded.extend_from_slice(&encoded);
        let refused = Verdict::decode(&padded).expect_err("a second JSON spelling is refused");
        assert_eq!(refused.canonical(), Some(encoded.as_slice()));
    }

    #[test]
    fn a_foreign_receipt_version_is_named_but_never_admitted() {
        let mut foreign: serde_json::Value =
            serde_json::from_slice(&sample_receipt().encode().expect("encode canonical receipt"))
                .expect("decode fixture value");
        foreign
            .as_object_mut()
            .expect("receipt is an object")
            .insert("version".to_owned(), serde_json::json!(77));
        foreign
            .as_object_mut()
            .expect("receipt is an object")
            .insert("future-executable".to_owned(), serde_json::json!("never"));
        foreign
            .as_object_mut()
            .expect("receipt is an object")
            .insert("standing".to_owned(), serde_json::json!("future-standing"));

        let refused = Verdict::decode(
            &serde_json::to_vec(&foreign).expect("encode foreign receipt evidence"),
        )
        .expect_err("foreign receipt stays evidence");
        assert_eq!(refused.unsupported_version(), Some(77));
        assert!(refused.to_string().contains("version 77"));
    }

    #[test]
    fn every_bound_value_moves_the_complete_receipt_identity() {
        let receipt = sample_receipt();
        let identity = receipt.digest();
        let assert_moved = |changed: Verdict, field: &str| {
            assert_ne!(
                changed.digest(),
                identity,
                "changing {field} did not move the receipt identity"
            );
        };

        let mut changed = receipt.clone();
        changed.version += 1;
        assert_moved(changed, "version");
        let mut changed = receipt.clone();
        changed.tree = receipt_digest("other-tree");
        assert_moved(changed, "tree");
        let mut changed = receipt.clone();
        changed.gate_set = receipt_digest("other-gate-set");
        assert_moved(changed, "gate set");
        let mut changed = receipt.clone();
        changed.policy = receipt_digest("other-policy");
        assert_moved(changed, "policy");
        let mut changed = receipt.clone();
        changed.inventory = receipt_digest("other-inventory");
        assert_moved(changed, "inventory");
        let mut changed = receipt.clone();
        changed.measurement = receipt_digest("other-measurement");
        assert_moved(changed, "measurement");
        let mut changed = receipt.clone();
        changed.bounds = receipt_digest("other-bounds");
        assert_moved(changed, "bounds");
        let mut changed = receipt.clone();
        changed.baseline = receipt_digest("other-baseline");
        assert_moved(changed, "baseline");
        let mut changed = receipt.clone();
        changed.tools[0].identity = receipt_digest("other-awk");
        assert_moved(changed, "tool identity");
        let mut changed = receipt.clone();
        changed.tools[0].name = "other-tool".to_owned();
        assert_moved(changed, "tool name");
        let mut changed = receipt.clone();
        changed.standing = Standing::Refused;
        assert_moved(changed, "standing");
        let mut changed = receipt;
        changed.findings[0].movement = Movement::Regressed {
            baseline: 70,
            measured: 71,
        };
        assert_moved(changed, "ordered findings");
    }

    #[cfg(not(target_family = "unix"))]
    #[test]
    fn a_non_unix_target_selects_the_fail_closed_snapshot_path() {
        assert_eq!(SnapshotIntegrity::native(), SnapshotIntegrity::Unsupported);
    }

    #[test]
    fn unsupported_snapshot_integrity_refuses_before_a_restoring_parser_can_run() {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "gate-unsupported-snapshot-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("src")).expect("create fixture tree");
        fs::write(root.join("src/lib.rs"), "one\n").expect("write fixture source");

        let metric = Metric::new(
            "rust.lines",
            "rust",
            "transient lines restored with their metadata",
            Parser::new(
                "same-length-restore-parser-must-not-launch",
                std::iter::empty::<&str>(),
            )
            .expect("declare parser"),
            "*.rs\n",
            "",
        )
        .expect("declare metric");
        let policy = Policy::new(Ignore::parse("").expect("parse ignore"), [metric])
            .expect("declare policy");
        let bounds = Bounds::new(
            "unsupported-snapshot-fixture",
            Deadlines::new(
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .expect("declare deadlines"),
            Duration::from_millis(10),
            1024,
        )
        .expect("declare bounds");

        let refused = Run::adjudicate_with_integrity(
            &policy,
            &root,
            &Ratchet::parse("rust.lines 1\n").expect("parse ratchet"),
            &bounds,
            SnapshotIntegrity::Unsupported,
        )
        .expect_err("unsupported snapshot integrity must fail before parser launch");

        assert!(matches!(refused, RunError::SnapshotIntegrityUnsupported));
        fs::remove_dir_all(root).expect("remove fixture tree");
    }
}

/// Why retained receipt bytes were not admitted as current canonical evidence.
#[derive(Debug)]
pub enum ReceiptDecodeError {
    /// The JSON shape or one typed field was malformed.
    Json(serde_json::Error),
    /// The bytes name a schema this build does not interpret.
    UnsupportedVersion {
        /// Exact foreign version retained as evidence.
        observed: u64,
    },
    /// A valid value used a second JSON spelling.
    NonCanonical {
        /// The one JSON spelling this build admits.
        canonical: Vec<u8>,
    },
}

impl ReceiptDecodeError {
    /// Foreign schema number, when the refusal was a version boundary.
    #[must_use]
    pub const fn unsupported_version(&self) -> Option<u64> {
        match self {
            Self::UnsupportedVersion { observed } => Some(*observed),
            Self::Json(_) | Self::NonCanonical { .. } => None,
        }
    }

    /// Canonical replacement bytes, when only the JSON spelling differed.
    #[must_use]
    pub fn canonical(&self) -> Option<&[u8]> {
        match self {
            Self::NonCanonical { canonical } => Some(canonical),
            Self::Json(_) | Self::UnsupportedVersion { .. } => None,
        }
    }
}

impl fmt::Display for ReceiptDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(source) => write!(formatter, "decoding the receipt: {source}"),
            Self::UnsupportedVersion { observed } => write!(
                formatter,
                "receipt version {observed} is unsupported; current version is \
                 {GATE_RECEIPT_VERSION}"
            ),
            Self::NonCanonical { .. } => {
                formatter.write_str("receipt JSON is not the canonical byte spelling")
            }
        }
    }
}

impl std::error::Error for ReceiptDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(source) => Some(source),
            Self::UnsupportedVersion { .. } | Self::NonCanonical { .. } => None,
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
