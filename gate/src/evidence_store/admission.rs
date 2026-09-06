//! Canonical pre-commit admissions over reusable receipts and live checks.

use gix::bstr::ByteSlice as _;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use wire::{Digest, Domain, Instant};

use super::{
    ActionKey, EvidenceStoreError, Layout, MAX_RECEIPT_BYTES, ReceiptId, create_directory,
    publish_immutable, publish_json, read_optional_regular,
};

const ADMISSION_VERSION: u64 = 1;
pub(super) const MAX_ADMISSION_ACTIONS: usize = 256;

/// Content identity of one immutable admission bundle.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct AdmissionId(Digest);

impl AdmissionId {
    pub(super) const fn from_digest(digest: Digest) -> Self {
        Self(digest)
    }

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
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
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
    pub fn branch(&self) -> &str {
        &self.branch
    }

    #[must_use]
    pub fn head(&self) -> Option<&str> {
        self.head.as_deref()
    }

    #[must_use]
    pub const fn created_at(&self) -> Instant {
        self.created_at
    }

    #[must_use]
    pub const fn expires_at(&self) -> Instant {
        self.expires_at
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
        if self.actions.len() > MAX_ADMISSION_ACTIONS {
            return Err(EvidenceStoreError::new(
                "gate-evidence::admission-actions-invalid",
                format!("admission has more than {MAX_ADMISSION_ACTIONS} actions"),
            ));
        }
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
    publish_index(layout, bundle, &id)?;
    Ok(id)
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AdmissionIndex {
    version: u64,
    candidate_tree: String,
    branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    head: Option<String>,
    admission: AdmissionId,
    created_at: Instant,
    expires_at: Instant,
}

impl AdmissionIndex {
    pub(super) fn reference(&self) -> (&str, &AdmissionId) {
        (&self.candidate_tree, &self.admission)
    }

    pub(super) fn key(&self) -> Result<Digest, EvidenceStoreError> {
        index_key(&self.candidate_tree, &self.branch, self.head.as_deref())
    }
}

fn publish_index(
    layout: &Layout,
    bundle: &AdmissionBundle,
    admission: &AdmissionId,
) -> Result<(), EvidenceStoreError> {
    let key = index_key(bundle.candidate_tree(), bundle.branch(), bundle.head())?;
    let path = layout.admission_index_path(&key);
    let next = AdmissionIndex {
        version: ADMISSION_VERSION,
        candidate_tree: bundle.candidate_tree.clone(),
        branch: bundle.branch.clone(),
        head: bundle.head.clone(),
        admission: admission.clone(),
        created_at: bundle.created_at,
        expires_at: bundle.expires_at,
    };
    let current = read_index(&path)?;
    let replace = current.as_ref().is_none_or(|current| {
        next.created_at > current.created_at
            || (next.created_at == current.created_at && next.admission < current.admission)
    });
    if replace {
        publish_json(&path, &next)?;
    }
    Ok(())
}

pub(super) fn lookup_current(
    layout: &Layout,
    candidate_tree: &str,
    branch: &str,
    head: Option<&str>,
) -> Result<Option<(AdmissionId, AdmissionBundle)>, EvidenceStoreError> {
    let key = index_key(candidate_tree, branch, head)?;
    let Some(index) = read_index(&layout.admission_index_path(&key))? else {
        return Ok(None);
    };
    if index.candidate_tree != candidate_tree
        || index.branch != branch
        || index.head.as_deref() != head
    {
        return Err(EvidenceStoreError::new(
            "gate-evidence::admission-index-authority-mismatch",
            "candidate admission index does not match its requested authority",
        ));
    }
    let path = layout
        .admissions
        .join(candidate_tree)
        .join(index.admission.as_str());
    let bytes = read_optional_regular(&path, MAX_RECEIPT_BYTES as u64)?.ok_or_else(|| {
        EvidenceStoreError::new(
            "gate-evidence::admission-index-dangling",
            format!(
                "candidate admission index points to missing {}",
                path.display()
            ),
        )
    })?;
    let bundle = AdmissionBundle::decode(&bytes)?;
    if bundle.id()? != index.admission
        || bundle.candidate_tree() != candidate_tree
        || bundle.branch() != branch
        || bundle.head() != head
        || bundle.created_at() != index.created_at
        || bundle.expires_at() != index.expires_at
    {
        return Err(EvidenceStoreError::new(
            "gate-evidence::admission-index-authority-mismatch",
            "candidate admission index disagrees with its immutable bundle",
        ));
    }
    Ok(Some((index.admission, bundle)))
}

fn index_key(
    candidate_tree: &str,
    branch: &str,
    head: Option<&str>,
) -> Result<Digest, EvidenceStoreError> {
    if !canonical_object_id(candidate_tree)
        || !canonical_reference(branch)
        || head.is_some_and(|head| !canonical_object_id(head))
    {
        return Err(EvidenceStoreError::new(
            "gate-evidence::admission-index-authority-invalid",
            "candidate admission index authority is not canonical",
        ));
    }
    let bytes = wire::encode(&(candidate_tree, branch, head)).map_err(|error| {
        EvidenceStoreError::new("gate-evidence::admission-index-invalid", error.to_string())
    })?;
    let mut framed = b"candidate-admission-index-v1\0".to_vec();
    framed.extend_from_slice(&bytes);
    Ok(Digest::compute(Domain::GateAdmission, &framed))
}

pub(super) fn read_index(
    path: &std::path::Path,
) -> Result<Option<AdmissionIndex>, EvidenceStoreError> {
    let Some(bytes) = read_optional_regular(path, MAX_RECEIPT_BYTES as u64)? else {
        return Ok(None);
    };
    let index: AdmissionIndex = wire::decode(&bytes).map_err(|error| {
        EvidenceStoreError::new("gate-evidence::admission-index-invalid", error.to_string())
    })?;
    if index.version != ADMISSION_VERSION {
        return Err(EvidenceStoreError::new(
            "gate-evidence::admission-index-version-unsupported",
            format!("admission index version {} is unsupported", index.version),
        ));
    }
    let canonical = wire::encode(&index).map_err(|error| {
        EvidenceStoreError::new("gate-evidence::admission-index-invalid", error.to_string())
    })?;
    if canonical != bytes {
        return Err(EvidenceStoreError::new(
            "gate-evidence::admission-index-noncanonical",
            "admission index bytes are not canonical",
        ));
    }
    Ok(Some(index))
}

pub(super) fn canonical_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(super) fn canonical_reference(value: &str) -> bool {
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
