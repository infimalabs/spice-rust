//! Canonical pre-commit admissions over reusable receipts and live checks.

use gix::bstr::ByteSlice as _;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use wire::{Digest, Domain, Instant};

use super::{
    ActionKey, EvidenceStoreError, Layout, ReceiptId, create_directory, publish_immutable,
};

const ADMISSION_VERSION: u64 = 1;

/// Content identity of one immutable admission bundle.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct AdmissionId(Digest);

impl AdmissionId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for AdmissionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Transient complete authority expected from one reusable receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiptExpectation {
    action: String,
    action_key: ActionKey,
    action_spec: Digest,
    input_manifest: Digest,
}

impl ReceiptExpectation {
    pub fn new(
        action: impl Into<String>,
        action_key: ActionKey,
        action_spec: Digest,
        input_manifest: Digest,
    ) -> Result<Self, EvidenceStoreError> {
        let value = Self {
            action: action.into(),
            action_key,
            action_spec,
            input_manifest,
        };
        value.validate()?;
        Ok(value)
    }

    #[must_use]
    pub fn action(&self) -> &str {
        &self.action
    }

    #[must_use]
    pub const fn action_key(&self) -> &ActionKey {
        &self.action_key
    }

    #[must_use]
    pub const fn action_spec(&self) -> &Digest {
        &self.action_spec
    }

    #[must_use]
    pub const fn input_manifest(&self) -> &Digest {
        &self.input_manifest
    }

    fn validate(&self) -> Result<(), EvidenceStoreError> {
        if self.action.is_empty()
            || self.action.trim() != self.action
            || self.action.contains(['\0', '\n', '\r'])
        {
            return Err(EvidenceStoreError::new(
                "gate-evidence::admission-action-invalid",
                format!("invalid admission action {:?}", self.action),
            ));
        }
        Ok(())
    }
}

pub(super) fn validate_receipt_expectations(
    actions: &[ReceiptExpectation],
) -> Result<(), EvidenceStoreError> {
    let mut names = BTreeSet::new();
    let mut keys = BTreeSet::new();
    for action in actions {
        action.validate()?;
        if !names.insert(action.action()) || !keys.insert(action.action_key()) {
            return Err(EvidenceStoreError::new(
                "gate-evidence::admission-actions-invalid",
                "receipt expectations must have unique action names and keys",
            ));
        }
    }
    Ok(())
}

/// One owed reusable action and the exact receipt admitted for it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionAction {
    action: String,
    action_key: ActionKey,
    receipt: ReceiptId,
}

impl AdmissionAction {
    pub(super) fn resolved(expected: &ReceiptExpectation, receipt: ReceiptId) -> Self {
        Self {
            action: expected.action.clone(),
            action_key: expected.action_key.clone(),
            receipt,
        }
    }

    #[must_use]
    pub fn action(&self) -> &str {
        &self.action
    }

    #[must_use]
    pub const fn action_key(&self) -> &ActionKey {
        &self.action_key
    }

    #[must_use]
    pub const fn receipt(&self) -> &ReceiptId {
        &self.receipt
    }

    fn validate(&self) -> Result<(), EvidenceStoreError> {
        if self.action.is_empty()
            || self.action.trim() != self.action
            || self.action.contains(['\0', '\n', '\r'])
        {
            return Err(EvidenceStoreError::new(
                "gate-evidence::admission-action-invalid",
                format!("invalid admission action {:?}", self.action),
            ));
        }
        Ok(())
    }
}

/// One live-only result executed against the candidate at admission time.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionLiveCheck {
    action: String,
    result: Digest,
}

impl AdmissionLiveCheck {
    pub fn new(action: impl Into<String>, result: Digest) -> Result<Self, EvidenceStoreError> {
        let value = Self {
            action: action.into(),
            result,
        };
        if value.action.is_empty()
            || value.action.trim() != value.action
            || value.action.contains(['\0', '\n', '\r'])
        {
            return Err(EvidenceStoreError::new(
                "gate-evidence::admission-live-check-invalid",
                format!("invalid live action {:?}", value.action),
            ));
        }
        Ok(value)
    }

    #[must_use]
    pub fn action(&self) -> &str {
        &self.action
    }
}

/// Fields captured by the pre-commit command rim for one candidate.
#[derive(Clone, Debug)]
pub struct AdmissionBundleFields {
    pub candidate_tree: String,
    pub branch: String,
    pub head: Option<String>,
    pub selection: Digest,
    pub policy: Digest,
    pub gate_set: Digest,
    pub actions: Vec<AdmissionAction>,
    pub live_checks: Vec<AdmissionLiveCheck>,
    pub claim: Digest,
    pub created_at: Instant,
    pub expires_at: Instant,
}

/// Complete immutable evidence admitting one prospective commit tree.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionBundle {
    version: u64,
    candidate_tree: String,
    branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    head: Option<String>,
    selection: Digest,
    policy: Digest,
    gate_set: Digest,
    actions: Vec<AdmissionAction>,
    live_checks: Vec<AdmissionLiveCheck>,
    claim: Digest,
    created_at: Instant,
    expires_at: Instant,
}

impl AdmissionBundle {
    pub fn new(fields: AdmissionBundleFields) -> Result<Self, EvidenceStoreError> {
        let bundle = Self {
            version: ADMISSION_VERSION,
            candidate_tree: fields.candidate_tree,
            branch: fields.branch,
            head: fields.head,
            selection: fields.selection,
            policy: fields.policy,
            gate_set: fields.gate_set,
            actions: fields.actions,
            live_checks: fields.live_checks,
            claim: fields.claim,
            created_at: fields.created_at,
            expires_at: fields.expires_at,
        };
        bundle.validate()?;
        Ok(bundle)
    }

    #[must_use]
    pub fn candidate_tree(&self) -> &str {
        &self.candidate_tree
    }

    #[must_use]
    pub fn actions(&self) -> &[AdmissionAction] {
        &self.actions
    }

    #[must_use]
    pub fn live_checks(&self) -> &[AdmissionLiveCheck] {
        &self.live_checks
    }

    pub fn id(&self) -> Result<AdmissionId, EvidenceStoreError> {
        Ok(AdmissionId(Digest::compute(
            Domain::GateAdmission,
            &self.encode()?,
        )))
    }

    pub fn encode(&self) -> Result<Vec<u8>, EvidenceStoreError> {
        self.validate()?;
        wire::encode(self).map_err(|error| {
            EvidenceStoreError::new("gate-evidence::admission-invalid", error.to_string())
        })
    }

    /// Decode only the current canonical admission schema.
    pub fn decode(bytes: &[u8]) -> Result<Self, EvidenceStoreError> {
        let bundle: Self = wire::decode(bytes).map_err(|error| {
            EvidenceStoreError::new("gate-evidence::admission-invalid", error.to_string())
        })?;
        bundle.validate()?;
        if bundle.encode()? != bytes {
            return Err(EvidenceStoreError::new(
                "gate-evidence::admission-noncanonical",
                "admission bytes do not match their canonical encoding",
            ));
        }
        Ok(bundle)
    }

    fn validate(&self) -> Result<(), EvidenceStoreError> {
        if self.version != ADMISSION_VERSION {
            return Err(EvidenceStoreError::new(
                "gate-evidence::admission-version-unsupported",
                format!(
                    "admission version {} is not {ADMISSION_VERSION}",
                    self.version
                ),
            ));
        }
        if !canonical_object_id(&self.candidate_tree) {
            return Err(EvidenceStoreError::new(
                "gate-evidence::admission-candidate-invalid",
                format!("candidate tree {:?} is not canonical", self.candidate_tree),
            ));
        }
        if !canonical_reference(&self.branch)
            || self
                .head
                .as_ref()
                .is_some_and(|head| !canonical_object_id(head))
        {
            return Err(EvidenceStoreError::new(
                "gate-evidence::admission-reference-invalid",
                "admission branch or HEAD is not canonical",
            ));
        }
        if self.expires_at <= self.created_at {
            return Err(EvidenceStoreError::new(
                "gate-evidence::admission-time-invalid",
                "admission expiry must follow creation",
            ));
        }
        let mut names = BTreeSet::new();
        let mut keys = BTreeSet::new();
        for action in &self.actions {
            action.validate()?;
            if !names.insert(action.action()) || !keys.insert(action.action_key()) {
                return Err(EvidenceStoreError::new(
                    "gate-evidence::admission-actions-invalid",
                    "admission actions must have unique names and keys",
                ));
            }
        }
        if self
            .actions
            .windows(2)
            .any(|pair| pair[0].action >= pair[1].action)
        {
            return Err(EvidenceStoreError::new(
                "gate-evidence::admission-actions-invalid",
                "admission actions must be sorted by action name",
            ));
        }
        let mut live = BTreeSet::new();
        for check in &self.live_checks {
            if !live.insert(check.action()) {
                return Err(EvidenceStoreError::new(
                    "gate-evidence::admission-live-checks-invalid",
                    "admission live checks repeat an action",
                ));
            }
        }
        if self
            .live_checks
            .windows(2)
            .any(|pair| pair[0].action >= pair[1].action)
        {
            return Err(EvidenceStoreError::new(
                "gate-evidence::admission-live-checks-invalid",
                "admission live checks must be sorted by action name",
            ));
        }
        Ok(())
    }
}

pub(super) fn publish(
    layout: &Layout,
    bundle: &AdmissionBundle,
) -> Result<AdmissionId, EvidenceStoreError> {
    let bytes = bundle.encode()?;
    let id = bundle.id()?;
    let candidate = layout.admissions.join(bundle.candidate_tree());
    create_directory(&candidate)?;
    publish_immutable(&candidate.join(id.as_str()), &bytes)?;
    Ok(id)
}

fn canonical_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn canonical_reference(value: &str) -> bool {
    value.starts_with("refs/heads/")
        && gix::validate::reference::branch_name(value.as_bytes().as_bstr()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::canonical_reference;

    #[test]
    fn admission_branch_uses_the_complete_git_reference_contract() {
        assert!(canonical_reference("refs/heads/main"));
        for invalid in [
            "main",
            "refs/heads/HEAD",
            "refs/heads/topic.lock",
            "refs/heads/topic@{1}",
            "refs/heads/topic..next",
        ] {
            assert!(!canonical_reference(invalid), "admitted {invalid:?}");
        }
    }
}
