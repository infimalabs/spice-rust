//! Canonical reusable-action fingerprints for bounded receipt lookup.

use serde::{Deserialize, Serialize};
use wire::Digest;

use crate::ChangeSelectionReport;

use super::{
    ActionKey, EvidenceStoreError, Layout, MAX_RECEIPT_BYTES, ReceiptExpectation,
    derive_action_key, publish_immutable, read_optional_regular,
};

const FINGERPRINT_VERSION: u64 = 2;

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
}

impl ActionFingerprintResolution {
    pub(super) fn new(
        manifest: ActionFingerprintManifest,
        receipts: Vec<Option<super::AdmissionAction>>,
    ) -> Self {
        Self { manifest, receipts }
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
}

impl ActionFingerprintManifest {
    pub fn new(fields: ActionFingerprintManifestFields) -> Result<Self, EvidenceStoreError> {
        let manifest = Self {
            version: FINGERPRINT_VERSION,
            authority: fields.authority,
            selection: fields.selection,
            cargo_tests: fields.cargo_tests,
            actions: fields.actions,
        };
        manifest.validate()?;
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
        if bytes.len() > MAX_RECEIPT_BYTES {
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
        if bytes.len() > MAX_RECEIPT_BYTES {
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

pub(super) fn publish(
    layout: &Layout,
    manifest: &ActionFingerprintManifest,
) -> Result<ActionKey, EvidenceStoreError> {
    let bytes = manifest.encode()?;
    let key = manifest.authority.key()?;
    publish_immutable(&layout.fingerprints.join(key.as_str()), &bytes)?;
    Ok(key)
}

pub(super) fn lookup(
    layout: &Layout,
    authority: &ActionFingerprintAuthority,
) -> Result<Option<ActionFingerprintManifest>, EvidenceStoreError> {
    authority.validate()?;
    let key = authority.key()?;
    let path = layout.fingerprints.join(key.as_str());
    let Some(bytes) = read_optional_regular(&path, MAX_RECEIPT_BYTES as u64)? else {
        return Ok(None);
    };
    let manifest = ActionFingerprintManifest::decode(&bytes)?;
    if manifest.authority() != authority {
        return Err(EvidenceStoreError::new(
            "gate-evidence::fingerprint-authority-mismatch",
            format!("fingerprint path names authority {key}, body names another authority"),
        ));
    }
    Ok(Some(manifest))
}

fn canonical_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
