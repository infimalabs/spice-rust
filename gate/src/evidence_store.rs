//! Git-common immutable storage for proof-aware gate receipts.
//!
//! Receipt objects are content-addressed and never replaced. An action index is
//! the only mutable pointer and is published atomically under one cross-process
//! lock. Durable production leases use the same lock, so expiry takeover,
//! renewal, publication, and garbage collection have one serial history across
//! every linked worktree.

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sill::{LockFile, LockName};
use wire::{Actor, CanonicalUuid, Digest, Domain, Instant, TaskHandle};

mod layout;
use layout::*;

/// Current on-disk evidence-store schema.
pub const EVIDENCE_STORE_VERSION: u64 = 1;
/// Current immutable receipt envelope schema.
pub const EVIDENCE_RECEIPT_VERSION: u64 = 1;

const LEASE_VERSION: u64 = 1;
const MAX_RECEIPT_BYTES: usize = 4 * 1024 * 1024;
static NEXT_LEASE: AtomicU64 = AtomicU64::new(0);

/// Identity of one canonical action specification and complete input closure.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ActionKey(Digest);

impl ActionKey {
    /// Digest spelling used for paths and crossing records.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for ActionKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Derive an action key from an already-canonical action contract.
///
/// Canonical field ordering and closure completeness belong to the action
/// contract. This boundary supplies the fixed domain separator.
#[must_use]
pub fn derive_action_key(canonical_action: &[u8]) -> ActionKey {
    ActionKey(Digest::compute(Domain::GateActionKey, canonical_action))
}

/// Content identity of one immutable receipt envelope.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ReceiptId(Digest);

impl ReceiptId {
    /// Digest spelling used for paths and crossing records.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for ReceiptId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Audit and credit metadata attached by the proof-aware runner.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProducerRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lane: Option<CanonicalUuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<CanonicalUuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task: Option<TaskHandle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    actor: Option<Actor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    appliance_generation: Option<Digest>,
}

impl ProducerRecord {
    #[must_use]
    pub const fn new(
        lane: Option<CanonicalUuid>,
        session: Option<CanonicalUuid>,
        task: Option<TaskHandle>,
        actor: Option<Actor>,
        appliance_generation: Option<Digest>,
    ) -> Self {
        Self {
            lane,
            session,
            task,
            actor,
            appliance_generation,
        }
    }
}

/// Fields supplied by one trusted proof-aware execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PassReceiptFields {
    pub action_key: ActionKey,
    pub action_spec: Digest,
    pub input_manifest: Digest,
    pub typed_result: Digest,
    pub started_at: Instant,
    pub finished_at: Instant,
    pub duration_millis: u64,
    pub artifacts: Vec<Digest>,
    pub producer: ProducerRecord,
}

/// Immutable passing receipt validated against the approved evidence contract.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PassReceipt {
    version: u64,
    action_key: ActionKey,
    action_spec: Digest,
    input_manifest: Digest,
    typed_result: Digest,
    passed: bool,
    started_at: Instant,
    finished_at: Instant,
    duration_millis: u64,
    artifacts: Vec<Digest>,
    producer: ProducerRecord,
}

impl PassReceipt {
    /// Admit the canonical result of one successful proof-aware execution.
    pub fn new(fields: PassReceiptFields) -> Result<Self, EvidenceStoreError> {
        let receipt = Self {
            version: EVIDENCE_RECEIPT_VERSION,
            action_key: fields.action_key,
            action_spec: fields.action_spec,
            input_manifest: fields.input_manifest,
            typed_result: fields.typed_result,
            passed: true,
            started_at: fields.started_at,
            finished_at: fields.finished_at,
            duration_millis: fields.duration_millis,
            artifacts: fields.artifacts,
            producer: fields.producer,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    /// Action this receipt proves.
    #[must_use]
    pub const fn action(&self) -> &ActionKey {
        &self.action_key
    }

    /// Canonical identity of the typed successful result.
    #[must_use]
    pub const fn typed_result(&self) -> &Digest {
        &self.typed_result
    }

    /// Canonical receipt envelope bytes.
    pub fn encode(&self) -> Result<Vec<u8>, EvidenceStoreError> {
        self.validate()?;
        let bytes = wire::encode(self).map_err(|error| {
            EvidenceStoreError::new("gate-evidence::receipt-invalid", error.to_string())
        })?;
        if bytes.len() > MAX_RECEIPT_BYTES {
            return Err(EvidenceStoreError::new(
                "gate-evidence::receipt-size-invalid",
                format!("receipt envelope length {} is over the limit", bytes.len()),
            ));
        }
        Ok(bytes)
    }

    /// Admit only the current schema in its one canonical spelling.
    pub fn decode(bytes: &[u8]) -> Result<Self, EvidenceStoreError> {
        if bytes.len() > MAX_RECEIPT_BYTES {
            return Err(EvidenceStoreError::new(
                "gate-evidence::receipt-size-invalid",
                format!("receipt envelope length {} is over the limit", bytes.len()),
            ));
        }
        let receipt: Self = wire::decode(bytes).map_err(|error| {
            EvidenceStoreError::new("gate-evidence::receipt-invalid", error.to_string())
        })?;
        receipt.validate()?;
        if receipt.encode()? != bytes {
            return Err(EvidenceStoreError::new(
                "gate-evidence::receipt-noncanonical",
                "receipt bytes do not match their canonical encoding",
            ));
        }
        Ok(receipt)
    }

    fn validate(&self) -> Result<(), EvidenceStoreError> {
        if self.version != EVIDENCE_RECEIPT_VERSION {
            return Err(EvidenceStoreError::new(
                "gate-evidence::receipt-version-unsupported",
                format!(
                    "receipt version {} is not {EVIDENCE_RECEIPT_VERSION}",
                    self.version
                ),
            ));
        }
        if !self.passed {
            return Err(EvidenceStoreError::new(
                "gate-evidence::receipt-not-passing",
                "only passing evidence may enter the reusable store",
            ));
        }
        if self.finished_at < self.started_at {
            return Err(EvidenceStoreError::new(
                "gate-evidence::receipt-time-invalid",
                "receipt finish precedes its start",
            ));
        }
        if self.artifacts.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(EvidenceStoreError::new(
                "gate-evidence::receipt-artifacts-invalid",
                "receipt artifact digests must be sorted and unique",
            ));
        }
        Ok(())
    }

    /// Content identity of the complete envelope.
    pub fn id(&self) -> Result<ReceiptId, EvidenceStoreError> {
        Ok(ReceiptId(Digest::compute(
            Domain::GatePassReceipt,
            &self.encode()?,
        )))
    }
}

/// Result of publishing one receipt and resolving its action index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Publication {
    receipt: ReceiptId,
    selected: ReceiptId,
    object_created: bool,
    index_updated: bool,
}

impl Publication {
    #[must_use]
    pub const fn receipt(&self) -> &ReceiptId {
        &self.receipt
    }
    #[must_use]
    pub const fn selected(&self) -> &ReceiptId {
        &self.selected
    }
    #[must_use]
    pub const fn object_created(&self) -> bool {
        self.object_created
    }
    #[must_use]
    pub const fn index_updated(&self) -> bool {
        self.index_updated
    }
}

/// Durable authority to produce evidence for one action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceLease {
    action: ActionKey,
    token: Digest,
    owner: String,
    expires_at_millis: u64,
}

impl EvidenceLease {
    #[must_use]
    pub const fn action(&self) -> &ActionKey {
        &self.action
    }
    #[must_use]
    pub fn token(&self) -> &str {
        self.token.as_str()
    }
    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }
    #[must_use]
    pub const fn expires_at_millis(&self) -> u64 {
        self.expires_at_millis
    }
}

/// One deterministic lease acquisition outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LeaseAcquisition {
    Acquired {
        lease: EvidenceLease,
        recovered: bool,
    },
    Busy {
        owner: String,
        expires_at_millis: u64,
    },
}

/// Bounded collection result. Ambiguous bytes are retained and named.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionReport {
    removed_objects: usize,
    removed_leases: usize,
    retained: Vec<String>,
}

impl CollectionReport {
    #[must_use]
    pub const fn removed_objects(&self) -> usize {
        self.removed_objects
    }
    #[must_use]
    pub const fn removed_leases(&self) -> usize {
        self.removed_leases
    }
    #[must_use]
    pub fn retained(&self) -> &[String] {
        &self.retained
    }
}

/// One Git-common evidence store shared by linked worktrees.
#[derive(Clone, Debug)]
pub struct EvidenceStore {
    layout: Layout,
}

impl EvidenceStore {
    /// Open or initialize the current store below an exact Git common directory.
    pub fn open(common_git: &Path) -> Result<Self, EvidenceStoreError> {
        Ok(Self {
            layout: Layout::initialize(common_git)?,
        })
    }

    /// Store root, primarily for inspection and diagnostics.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.layout.root
    }

    /// Publish one immutable object and atomically select it for its action.
    pub fn publish(&self, receipt: &PassReceipt) -> Result<Publication, EvidenceStoreError> {
        self.publish_with(receipt, || Ok(()))
    }

    fn publish_with(
        &self,
        receipt: &PassReceipt,
        before_index: impl FnOnce() -> Result<(), EvidenceStoreError>,
    ) -> Result<Publication, EvidenceStoreError> {
        self.with_lock(|layout| {
            let bytes = receipt.encode()?;
            let id = receipt.id()?;
            let object_created = publish_immutable(&layout.object_path(&id), &bytes)?;
            before_index()?;
            let current = read_index(layout, receipt.action())?;
            let selected = current
                .as_ref()
                .map_or_else(|| id.clone(), |index| index.receipt.clone().min(id.clone()));
            let index_updated = current
                .as_ref()
                .is_none_or(|index| index.receipt != selected);
            if index_updated {
                publish_json(
                    &layout.index_path(receipt.action()),
                    &ActionIndex::new(receipt.action().clone(), selected.clone()),
                )?;
            }
            Ok(Publication {
                receipt: id,
                selected,
                object_created,
                index_updated,
            })
        })
    }

    /// Read the currently selected receipt for an action.
    pub fn lookup(&self, action: &ActionKey) -> Result<Option<PassReceipt>, EvidenceStoreError> {
        self.with_lock(|layout| {
            let Some(index) = read_index(layout, action)? else {
                return Ok(None);
            };
            read_receipt(layout, &index.receipt, Some(action)).map(Some)
        })
    }

    /// Acquire a durable single-flight lease, replacing it only at expiry.
    pub fn acquire_lease(
        &self,
        action: &ActionKey,
        owner: &str,
        now: SystemTime,
        ttl: Duration,
    ) -> Result<LeaseAcquisition, EvidenceStoreError> {
        validate_owner(owner)?;
        let now = millis(now)?;
        let ttl = u64::try_from(ttl.as_millis()).map_err(|_| {
            EvidenceStoreError::new(
                "gate-evidence::lease-duration-invalid",
                "lease duration exceeds u64 milliseconds",
            )
        })?;
        if ttl == 0 {
            return Err(EvidenceStoreError::new(
                "gate-evidence::lease-duration-invalid",
                "lease duration must be nonzero",
            ));
        }
        let expires = now.checked_add(ttl).ok_or_else(|| {
            EvidenceStoreError::new(
                "gate-evidence::lease-duration-invalid",
                "lease expiry overflows",
            )
        })?;
        self.with_lock(|layout| {
            let existing = read_lease(layout, action)?;
            if let Some(record) = &existing
                && record.expires_at_millis > now
            {
                return Ok(LeaseAcquisition::Busy {
                    owner: record.owner.clone(),
                    expires_at_millis: record.expires_at_millis,
                });
            }
            let recovered = existing.is_some();
            let token = lease_token(action, owner, now, expires);
            let record = LeaseRecord::new(action.clone(), token.clone(), owner.to_owned(), expires);
            publish_json(&layout.lease_path(action), &record)?;
            Ok(LeaseAcquisition::Acquired {
                lease: record.into(),
                recovered,
            })
        })
    }

    /// Extend the exact live lease without changing its token.
    pub fn renew_lease(
        &self,
        lease: &EvidenceLease,
        now: SystemTime,
        ttl: Duration,
    ) -> Result<EvidenceLease, EvidenceStoreError> {
        let now = millis(now)?;
        let ttl = u64::try_from(ttl.as_millis()).map_err(|_| {
            EvidenceStoreError::new(
                "gate-evidence::lease-duration-invalid",
                "lease duration exceeds u64 milliseconds",
            )
        })?;
        if ttl == 0 {
            return Err(EvidenceStoreError::new(
                "gate-evidence::lease-duration-invalid",
                "lease duration must be nonzero",
            ));
        }
        let expires = now.checked_add(ttl).ok_or_else(|| {
            EvidenceStoreError::new(
                "gate-evidence::lease-duration-invalid",
                "lease expiry overflows",
            )
        })?;
        self.with_lock(|layout| {
            let record = require_lease(layout, lease)?;
            if record.expires_at_millis <= now {
                return Err(EvidenceStoreError::new(
                    "gate-evidence::lease-expired",
                    format!("lease expired at {}", record.expires_at_millis),
                ));
            }
            let renewed = LeaseRecord::new(
                lease.action.clone(),
                lease.token.clone(),
                lease.owner.clone(),
                expires,
            );
            publish_json(&layout.lease_path(&lease.action), &renewed)?;
            Ok(renewed.into())
        })
    }

    /// Release only the lease carrying the caller's exact token.
    pub fn release_lease(&self, lease: &EvidenceLease) -> Result<(), EvidenceStoreError> {
        self.with_lock(|layout| {
            require_lease(layout, lease)?;
            remove_file(&layout.lease_path(&lease.action))
        })
    }

    /// Remove at most `limit` expired leases and old unreferenced receipts.
    pub fn collect(
        &self,
        now: SystemTime,
        minimum_age: Duration,
        limit: usize,
    ) -> Result<CollectionReport, EvidenceStoreError> {
        self.with_lock(|layout| collect_layout(layout, now, minimum_age, limit))
    }

    fn with_lock<T>(
        &self,
        operation: impl FnOnce(&Layout) -> Result<T, EvidenceStoreError>,
    ) -> Result<T, EvidenceStoreError> {
        self.layout.validate()?;
        let name = LockName::new("gate-evidence-store").expect("static lock name is valid");
        let lock = LockFile::new(name, self.layout.locks.join("store.lock"));
        let _possession = lock
            .acquire()
            .map_err(|error| EvidenceStoreError::io("acquire store lock", lock.path(), error))?;
        self.layout.validate()?;
        operation(&self.layout)
    }
}

#[derive(Debug)]
pub struct EvidenceStoreError {
    reason: &'static str,
    detail: String,
}

impl EvidenceStoreError {
    fn new(reason: &'static str, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }
    fn io(operation: &str, path: &Path, error: std::io::Error) -> Self {
        Self::new(
            "gate-evidence::filesystem",
            format!("{operation} at {}: {error}", path.display()),
        )
    }
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        self.reason
    }
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for EvidenceStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.reason, self.detail)
    }
}

impl std::error::Error for EvidenceStoreError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ActionIndex {
    version: u64,
    action: ActionKey,
    receipt: ReceiptId,
}

impl ActionIndex {
    fn new(action: ActionKey, receipt: ReceiptId) -> Self {
        Self {
            version: EVIDENCE_STORE_VERSION,
            action,
            receipt,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LeaseRecord {
    version: u64,
    action: ActionKey,
    token: Digest,
    owner: String,
    expires_at_millis: u64,
}

impl LeaseRecord {
    fn new(action: ActionKey, token: Digest, owner: String, expires_at_millis: u64) -> Self {
        Self {
            version: LEASE_VERSION,
            action,
            token,
            owner,
            expires_at_millis,
        }
    }
}

impl From<LeaseRecord> for EvidenceLease {
    fn from(record: LeaseRecord) -> Self {
        Self {
            action: record.action,
            token: record.token,
            owner: record.owner,
            expires_at_millis: record.expires_at_millis,
        }
    }
}

fn validate_owner(owner: &str) -> Result<(), EvidenceStoreError> {
    if owner.is_empty() || owner.len() > 256 || owner.chars().any(char::is_control) {
        return Err(EvidenceStoreError::new(
            "gate-evidence::lease-owner-invalid",
            "lease owner must be 1..=256 printable bytes",
        ));
    }
    Ok(())
}

fn millis(time: SystemTime) -> Result<u64, EvidenceStoreError> {
    let value = time
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            EvidenceStoreError::new("gate-evidence::clock-invalid", error.to_string())
        })?
        .as_millis();
    u64::try_from(value).map_err(|_| {
        EvidenceStoreError::new(
            "gate-evidence::clock-invalid",
            "clock exceeds u64 milliseconds",
        )
    })
}

fn lease_token(action: &ActionKey, owner: &str, now: u64, expires: u64) -> Digest {
    let sequence = NEXT_LEASE.fetch_add(1, Ordering::Relaxed);
    let bytes = format!(
        "{}\0{owner}\0{now}\0{expires}\0{}\0{sequence}",
        action,
        std::process::id()
    );
    Digest::compute(Domain::GateEvidenceLease, bytes.as_bytes())
}

fn require_lease(
    layout: &Layout,
    lease: &EvidenceLease,
) -> Result<LeaseRecord, EvidenceStoreError> {
    let record = read_lease(layout, &lease.action)?.ok_or_else(|| {
        EvidenceStoreError::new(
            "gate-evidence::lease-missing",
            format!("no lease exists for {}", lease.action),
        )
    })?;
    if record.token != lease.token {
        return Err(EvidenceStoreError::new(
            "gate-evidence::lease-token-mismatch",
            format!("another lease owns {}", lease.action),
        ));
    }
    Ok(record)
}

#[cfg(test)]
mod tests;
