//! Canonical reusable-action fingerprints for bounded receipt lookup.

use serde::{Deserialize, Serialize};
use wire::Digest;

use crate::ChangeSelectionReport;

use super::{
    ActionKey, EvidenceStoreError, Layout, MAX_RECEIPT_BYTES, ReceiptExpectation,
    derive_action_key, publish_immutable,
};

const FINGERPRINT_VERSION: u64 = 3;
const FINGERPRINT_INDEX_VERSION: u64 = 1;
const FINGERPRINT_IDENTITY: &[u8] = b"spice.gate.action-fingerprint-manifest.v1\0";
const FINGERPRINT_INDEX_BYTES: u64 = 64 * 1024;
const MAX_FINGERPRINT_ACTIONS: usize = 128;
const MAX_FINGERPRINT_BYTES: usize = MAX_RECEIPT_BYTES;
const MAX_FINGERPRINT_READ_BYTES: u64 = 4 * 1024 * 1024;

/// Inputs that can be validated without reconstructing Cargo or Canvas plans.
#[derive(Clone, Debug)]
pub struct ActionFingerprintAuthorityFields {
    pub candidate_tree: String,
    pub baseline: Option<String>,
    pub source: Digest,
    pub policy: Digest,
    pub gate_set: Digest,
    pub tools: Digest,
    pub environment: Digest,
    pub cargo_source: Option<Digest>,
}

/// Complete fast-path authority for one reusable-action fingerprint manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionFingerprintAuthority {
    version: u64,
    candidate_tree: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    baseline: Option<String>,
    source: Digest,
    policy: Digest,
    gate_set: Digest,
    tools: Digest,
    environment: Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    cargo_source: Option<Digest>,
}

impl ActionFingerprintAuthority {
    pub fn new(fields: ActionFingerprintAuthorityFields) -> Result<Self, EvidenceStoreError> {
        let authority = Self {
            version: FINGERPRINT_VERSION,
            candidate_tree: fields.candidate_tree,
            baseline: fields.baseline,
            source: fields.source,
            policy: fields.policy,
            gate_set: fields.gate_set,
            tools: fields.tools,
            environment: fields.environment,
            cargo_source: fields.cargo_source,
        };
        authority.validate()?;
        Ok(authority)
    }

    #[must_use]
    pub fn candidate_tree(&self) -> &str {
        &self.candidate_tree
    }

    #[must_use]
    pub const fn source(&self) -> &Digest {
        &self.source
    }

    pub fn key(&self) -> Result<ActionKey, EvidenceStoreError> {
        let bytes = wire::encode(self).map_err(|error| {
            EvidenceStoreError::new(
                "gate-evidence::fingerprint-authority-invalid",
                error.to_string(),
            )
        })?;
        Ok(derive_action_key(&bytes))
    }

    fn validate(&self) -> Result<(), EvidenceStoreError> {
        if self.version != FINGERPRINT_VERSION {
            return Err(EvidenceStoreError::new(
                "gate-evidence::fingerprint-version-unsupported",
                format!(
                    "fingerprint authority version {} is not {FINGERPRINT_VERSION}",
                    self.version
                ),
            ));
        }
        if !canonical_object_id(&self.candidate_tree)
            || self
                .baseline
                .as_ref()
                .is_some_and(|value| !canonical_object_id(value))
        {
            return Err(EvidenceStoreError::new(
                "gate-evidence::fingerprint-source-invalid",
                "fingerprint candidate tree or baseline is not canonical",
            ));
        }
        Ok(())
    }
}

/// Fields supplied after explicit proof planning completes.
#[derive(Clone, Debug)]
pub struct ActionFingerprintManifestFields {
    pub authority: ActionFingerprintAuthority,
    pub selection: ChangeSelectionReport,
    pub cargo_tests: bool,
    pub actions: Vec<ReceiptExpectation>,
}

/// Exact reusable actions selected under one fast-path authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionFingerprintManifest {
    version: u64,
    authority: ActionFingerprintAuthority,
    selection: ChangeSelectionReport,
    cargo_tests: bool,
    actions: Vec<ReceiptExpectation>,
}

/// One manifest and its exact receipt lookup results read under the same lock.
pub struct ActionFingerprintResolution {
    manifest: ActionFingerprintManifest,
    receipts: Vec<Option<super::AdmissionAction>>,
    files_read: u64,
    bytes_read: u64,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ActionFingerprintIndex {
    version: u64,
    authority: ActionFingerprintAuthority,
    manifest: Digest,
}

impl ActionFingerprintResolution {
    pub(super) fn new(
        manifest: ActionFingerprintManifest,
        receipts: Vec<Option<super::AdmissionAction>>,
        budget: &super::ReadBudget,
    ) -> Self {
        Self {
            manifest,
            receipts,
            files_read: budget.files(),
            bytes_read: budget.bytes(),
        }
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        ActionFingerprintManifest,
        Vec<Option<super::AdmissionAction>>,
    ) {
        (self.manifest, self.receipts)
    }

    #[must_use]
    pub const fn files_read(&self) -> u64 {
        self.files_read
    }

    #[must_use]
    pub const fn bytes_read(&self) -> u64 {
        self.bytes_read
    }
}

impl ActionFingerprintManifest {
    /// Maximum reusable actions admitted by one fingerprint manifest.
    pub const MAX_ACTIONS: usize = MAX_FINGERPRINT_ACTIONS;
    /// Maximum canonical bytes admitted by one fingerprint manifest.
    pub const MAX_BYTES: usize = MAX_FINGERPRINT_BYTES;
    /// Maximum aggregate bytes read while resolving a fingerprint and its receipts.
    pub const MAX_READ_BYTES: u64 = MAX_FINGERPRINT_READ_BYTES;

    pub fn new(fields: ActionFingerprintManifestFields) -> Result<Self, EvidenceStoreError> {
        let manifest = Self {
            version: FINGERPRINT_VERSION,
            authority: fields.authority,
            selection: fields.selection,
            cargo_tests: fields.cargo_tests,
            actions: fields.actions,
        };
        manifest.encode()?;
        Ok(manifest)
    }

    #[must_use]
    pub const fn authority(&self) -> &ActionFingerprintAuthority {
        &self.authority
    }

    #[must_use]
    pub const fn selection(&self) -> &ChangeSelectionReport {
        &self.selection
    }

    #[must_use]
    pub const fn cargo_tests(&self) -> bool {
        self.cargo_tests
    }

    #[must_use]
    pub fn actions(&self) -> &[ReceiptExpectation] {
        &self.actions
    }

    pub fn encode(&self) -> Result<Vec<u8>, EvidenceStoreError> {
        self.validate()?;
        let bytes = wire::encode(self).map_err(|error| {
            EvidenceStoreError::new("gate-evidence::fingerprint-invalid", error.to_string())
        })?;
        if bytes.len() > MAX_FINGERPRINT_BYTES {
            return Err(EvidenceStoreError::new(
                "gate-evidence::fingerprint-size-invalid",
                format!(
                    "fingerprint manifest length {} is over the limit",
                    bytes.len()
                ),
            ));
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, EvidenceStoreError> {
        if bytes.len() > MAX_FINGERPRINT_BYTES {
            return Err(EvidenceStoreError::new(
                "gate-evidence::fingerprint-size-invalid",
                format!(
                    "fingerprint manifest length {} is over the limit",
                    bytes.len()
                ),
            ));
        }
        let manifest: Self = wire::decode(bytes).map_err(|error| {
            EvidenceStoreError::new("gate-evidence::fingerprint-invalid", error.to_string())
        })?;
        manifest.validate()?;
        if manifest.encode()? != bytes {
            return Err(EvidenceStoreError::new(
                "gate-evidence::fingerprint-noncanonical",
                "fingerprint bytes do not match their canonical encoding",
            ));
        }
        Ok(manifest)
    }

    pub(super) fn id(&self) -> Result<Digest, EvidenceStoreError> {
        let bytes = self.encode()?;
        Ok(manifest_id(&bytes))
    }

    fn validate(&self) -> Result<(), EvidenceStoreError> {
        if self.version != FINGERPRINT_VERSION {
            return Err(EvidenceStoreError::new(
                "gate-evidence::fingerprint-version-unsupported",
                format!(
                    "fingerprint version {} is not {FINGERPRINT_VERSION}",
                    self.version
                ),
            ));
        }
        self.authority.validate()?;
        if self.actions.len() > Self::MAX_ACTIONS {
            return Err(EvidenceStoreError::new(
                "gate-evidence::fingerprint-action-count-invalid",
                format!(
                    "fingerprint manifest has {} actions over the {} action limit",
                    self.actions.len(),
                    Self::MAX_ACTIONS,
                ),
            ));
        }
        let selection = self.selection.encode().map_err(|error| {
            EvidenceStoreError::new(
                "gate-evidence::fingerprint-selection-invalid",
                error.to_string(),
            )
        })?;
        ChangeSelectionReport::decode(&selection).map_err(|error| {
            EvidenceStoreError::new(
                "gate-evidence::fingerprint-selection-invalid",
                error.to_string(),
            )
        })?;
        if self.selection.source() != self.authority.source() {
            return Err(EvidenceStoreError::new(
                "gate-evidence::fingerprint-selection-mismatch",
                "fingerprint selection names a different staged source",
            ));
        }
        super::admission::validate_receipt_expectations(&self.actions)?;
        if self
            .actions
            .windows(2)
            .any(|pair| pair[0].action() >= pair[1].action())
        {
            return Err(EvidenceStoreError::new(
                "gate-evidence::fingerprint-actions-invalid",
                "fingerprint actions must be sorted by action name",
            ));
        }
        Ok(())
    }
}

impl ActionFingerprintIndex {
    pub(super) fn new(authority: ActionFingerprintAuthority, manifest: Digest) -> Self {
        Self {
            version: FINGERPRINT_INDEX_VERSION,
            authority,
            manifest,
        }
    }

    pub(super) fn encode(&self) -> Result<Vec<u8>, EvidenceStoreError> {
        wire::encode(self).map_err(|error| {
            EvidenceStoreError::new(
                "gate-evidence::fingerprint-index-invalid",
                error.to_string(),
            )
        })
    }

    fn decode(bytes: &[u8]) -> Result<Self, EvidenceStoreError> {
        let index: Self = wire::decode(bytes).map_err(|error| {
            EvidenceStoreError::new(
                "gate-evidence::fingerprint-index-invalid",
                error.to_string(),
            )
        })?;
        if index.version != FINGERPRINT_INDEX_VERSION {
            return Err(EvidenceStoreError::new(
                "gate-evidence::fingerprint-index-version-unsupported",
                format!(
                    "fingerprint index version {} is not {FINGERPRINT_INDEX_VERSION}",
                    index.version
                ),
            ));
        }
        let canonical = index.encode()?;
        if canonical != bytes {
            return Err(EvidenceStoreError::new(
                "gate-evidence::fingerprint-index-noncanonical",
                "fingerprint index bytes do not match their canonical encoding",
            ));
        }
        Ok(index)
    }
}

pub(super) fn publish(
    layout: &Layout,
    manifest: &ActionFingerprintManifest,
) -> Result<ActionKey, EvidenceStoreError> {
    publish_with(layout, manifest, || Ok(()))
}

pub(super) fn publish_with(
    layout: &Layout,
    manifest: &ActionFingerprintManifest,
    after_object: impl FnOnce() -> Result<(), EvidenceStoreError>,
) -> Result<ActionKey, EvidenceStoreError> {
    let bytes = manifest.encode()?;
    let id = manifest.id()?;
    let key = manifest.authority.key()?;
    publish_immutable(&layout.fingerprint_objects.join(id.as_str()), &bytes)?;
    after_object()?;
    let index = ActionFingerprintIndex::new(manifest.authority.clone(), id).encode()?;
    publish_immutable(&layout.fingerprints.join(key.as_str()), &index)?;
    Ok(key)
}

pub(super) fn lookup(
    layout: &Layout,
    authority: &ActionFingerprintAuthority,
) -> Result<Option<ActionFingerprintManifest>, EvidenceStoreError> {
    let mut budget = super::ReadBudget::new(MAX_FINGERPRINT_READ_BYTES);
    lookup_bounded(layout, authority, &mut budget)
}

pub(super) fn lookup_bounded(
    layout: &Layout,
    authority: &ActionFingerprintAuthority,
    budget: &mut super::ReadBudget,
) -> Result<Option<ActionFingerprintManifest>, EvidenceStoreError> {
    authority.validate()?;
    let key = authority.key()?;
    let path = layout.fingerprints.join(key.as_str());
    let Some(bytes) = budget.read_optional(&path, FINGERPRINT_INDEX_BYTES)? else {
        return Ok(None);
    };
    let index = ActionFingerprintIndex::decode(&bytes)?;
    if &index.authority != authority {
        return Err(EvidenceStoreError::new(
            "gate-evidence::fingerprint-authority-mismatch",
            format!("fingerprint path names authority {key}, body names another authority"),
        ));
    }
    let object = layout.fingerprint_objects.join(index.manifest.as_str());
    let bytes = budget
        .read_optional(&object, MAX_FINGERPRINT_BYTES as u64)?
        .ok_or_else(|| {
            EvidenceStoreError::new(
                "gate-evidence::fingerprint-object-missing",
                format!("fingerprint object {} does not exist", object.display()),
            )
        })?;
    let observed = manifest_id(&bytes);
    if observed != index.manifest {
        return Err(EvidenceStoreError::new(
            "gate-evidence::fingerprint-digest-mismatch",
            format!(
                "fingerprint index names {}, object bytes identify {observed}",
                index.manifest
            ),
        ));
    }
    let manifest = ActionFingerprintManifest::decode(&bytes)?;
    if manifest.authority() != authority {
        return Err(EvidenceStoreError::new(
            "gate-evidence::fingerprint-authority-mismatch",
            "fingerprint manifest names another authority",
        ));
    }
    Ok(Some(manifest))
}

fn manifest_id(bytes: &[u8]) -> Digest {
    let mut identity = Vec::with_capacity(FINGERPRINT_IDENTITY.len() + bytes.len());
    identity.extend_from_slice(FINGERPRINT_IDENTITY);
    identity.extend_from_slice(bytes);
    Digest::compute(wire::Domain::GateInputManifest, &identity)
}

fn canonical_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
