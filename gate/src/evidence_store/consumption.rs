//! Reference-transaction consumption of immutable candidate admissions.

use std::fs;

use serde::{Deserialize, Serialize};
use wire::{Digest, Domain, Instant};

use super::admission::{canonical_object_id, canonical_reference};
use super::{
    AdmissionBundle, AdmissionId, EvidenceStoreError, Layout, MAX_RECEIPT_BYTES, publish_immutable,
    read_optional_regular, read_receipt, remove_optional_file,
};

const CONSUMPTION_VERSION: u64 = 1;

/// Stable identity of the complete queued reference update set.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct AdmissionTransactionId(Digest);

impl AdmissionTransactionId {
    #[must_use]
    pub fn derive(canonical_updates: &[u8]) -> Self {
        let mut framed = b"reference-transaction-consumption-v1\0".to_vec();
        framed.extend_from_slice(canonical_updates);
        Self(Digest::compute(Domain::GateAdmission, &framed))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// One queued commit and the candidate tree it would install on a branch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmissionCandidate {
    branch: String,
    head: Option<String>,
    commit: String,
    tree: String,
}

impl AdmissionCandidate {
    pub fn new(
        branch: impl Into<String>,
        head: Option<String>,
        commit: impl Into<String>,
        tree: impl Into<String>,
    ) -> Result<Self, EvidenceStoreError> {
        let value = Self {
            branch: branch.into(),
            head,
            commit: commit.into(),
            tree: tree.into(),
        };
        validate_candidate(&value)?;
        Ok(value)
    }
}

fn validate_candidate(candidate: &AdmissionCandidate) -> Result<(), EvidenceStoreError> {
    if !canonical_reference(&candidate.branch)
        || !canonical_object_id(&candidate.commit)
        || !canonical_object_id(&candidate.tree)
        || candidate
            .head
            .as_deref()
            .is_some_and(|head| !canonical_object_id(head))
    {
        return Err(EvidenceStoreError::new(
            "gate-evidence::admission-consumption-candidate-invalid",
            "admission consumption candidate has a noncanonical branch, head, commit, or tree",
        ));
    }
    Ok(())
}

/// Durable credit proving which admission authorized one committed update.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionConsumption {
    version: u64,
    transaction: AdmissionTransactionId,
    entries: Vec<ConsumptionEntry>,
}

impl AdmissionConsumption {
    #[must_use]
    pub const fn transaction(&self) -> &AdmissionTransactionId {
        &self.transaction
    }

    pub fn commits(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.commit.as_str())
    }

    pub fn admissions(&self) -> impl Iterator<Item = &AdmissionId> {
        self.entries.iter().map(|entry| &entry.admission)
    }

    fn encode(&self) -> Result<Vec<u8>, EvidenceStoreError> {
        validate(self)?;
        wire::encode(self).map_err(|error| {
            EvidenceStoreError::new(
                "gate-evidence::admission-consumption-invalid",
                error.to_string(),
            )
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ConsumptionEntry {
    branch: String,
    head: Option<String>,
    commit: String,
    tree: String,
    admission: AdmissionId,
}

pub(super) fn prepare(
    layout: &Layout,
    transaction: &AdmissionTransactionId,
    candidates: &[AdmissionCandidate],
    now: Instant,
) -> Result<bool, EvidenceStoreError> {
    let mut candidates = candidates.to_vec();
    candidates.sort_by(|left, right| {
        (&left.branch, &left.commit, &left.tree).cmp(&(&right.branch, &right.commit, &right.tree))
    });
    if candidates.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(EvidenceStoreError::new(
            "gate-evidence::admission-consumption-candidates-invalid",
            "reference transaction repeats one admission candidate",
        ));
    }

    let mut entries = Vec::new();
    for candidate in &candidates {
        let Some((admission, bundle)) = matching_admission(layout, candidate, now)? else {
            continue;
        };
        for action in bundle.actions() {
            read_receipt(layout, action.receipt(), Some(action.action_key()))?;
        }
        entries.push(ConsumptionEntry {
            branch: candidate.branch.clone(),
            head: candidate.head.clone(),
            commit: candidate.commit.clone(),
            tree: candidate.tree.clone(),
            admission,
        });
    }
    if entries.is_empty() {
        return Ok(false);
    }
    let pending = AdmissionConsumption {
        version: CONSUMPTION_VERSION,
        transaction: transaction.clone(),
        entries,
    };
    let bytes = pending.encode()?;
    if let Some(credit) = read_record(&layout.admission_credit_path(transaction), transaction)? {
        if credit != pending {
            return Err(EvidenceStoreError::new(
                "gate-evidence::admission-credit-mismatch",
                "durable credit differs from the repeated prepared transaction",
            ));
        }
        return Ok(true);
    }
    publish_immutable(&layout.pending_admission_path(transaction), &bytes)?;
    Ok(true)
}

pub(super) fn commit(
    layout: &Layout,
    transaction: &AdmissionTransactionId,
) -> Result<Option<AdmissionConsumption>, EvidenceStoreError> {
    commit_with(layout, transaction, || Ok(()))
}

pub(super) fn commit_with(
    layout: &Layout,
    transaction: &AdmissionTransactionId,
    after_credit: impl FnOnce() -> Result<(), EvidenceStoreError>,
) -> Result<Option<AdmissionConsumption>, EvidenceStoreError> {
    let credit_path = layout.admission_credit_path(transaction);
    let existing = read_record(&credit_path, transaction)?;
    let pending_path = layout.pending_admission_path(transaction);
    let Some(pending) = read_record(&pending_path, transaction)? else {
        return Ok(existing);
    };
    if let Some(existing) = &existing {
        if existing != &pending {
            return Err(EvidenceStoreError::new(
                "gate-evidence::admission-credit-mismatch",
                "durable credit differs from its retained pending transaction",
            ));
        }
    } else {
        publish_immutable(&credit_path, &pending.encode()?)?;
    }
    after_credit()?;
    remove_optional_file(&pending_path)?;
    Ok(Some(pending))
}

pub(super) fn abort(
    layout: &Layout,
    transaction: &AdmissionTransactionId,
) -> Result<bool, EvidenceStoreError> {
    let path = layout.pending_admission_path(transaction);
    let existed = read_record(&path, transaction)?.is_some();
    remove_optional_file(&path)?;
    Ok(existed)
}

pub(super) fn credit(
    layout: &Layout,
    transaction: &AdmissionTransactionId,
) -> Result<Option<AdmissionConsumption>, EvidenceStoreError> {
    read_record(&layout.admission_credit_path(transaction), transaction)
}

fn matching_admission(
    layout: &Layout,
    candidate: &AdmissionCandidate,
    now: Instant,
) -> Result<Option<(AdmissionId, AdmissionBundle)>, EvidenceStoreError> {
    let Some((id, bundle)) = super::admission::lookup_current(
        layout,
        &candidate.tree,
        &candidate.branch,
        candidate.head.as_deref(),
    )?
    else {
        let directory = layout.admissions.join(&candidate.tree);
        return match fs::symlink_metadata(&directory) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Ok(metadata) if metadata.is_dir() => Err(EvidenceStoreError::new(
                "gate-evidence::admission-candidate-mismatch",
                format!(
                    "candidate {} has admissions, but none match {} at its current head",
                    candidate.commit, candidate.branch
                ),
            )),
            Ok(_) => Err(EvidenceStoreError::new(
                "gate-evidence::admission-entry-invalid",
                format!(
                    "candidate admission path {} is not a directory",
                    directory.display()
                ),
            )),
            Err(error) => Err(EvidenceStoreError::io(
                "inspect candidate admissions",
                &directory,
                error,
            )),
        };
    };
    if bundle.created_at() > now || now >= bundle.expires_at() {
        return Err(EvidenceStoreError::new(
            "gate-evidence::admission-candidate-mismatch",
            format!(
                "candidate {} on {} has no current admission",
                candidate.commit, candidate.branch
            ),
        ));
    }
    Ok(Some((id, bundle)))
}

fn read_record(
    path: &std::path::Path,
    transaction: &AdmissionTransactionId,
) -> Result<Option<AdmissionConsumption>, EvidenceStoreError> {
    let Some(bytes) = read_optional_regular(path, MAX_RECEIPT_BYTES as u64)? else {
        return Ok(None);
    };
    let record: AdmissionConsumption = wire::decode(&bytes).map_err(|error| {
        EvidenceStoreError::new(
            "gate-evidence::admission-consumption-invalid",
            error.to_string(),
        )
    })?;
    validate(&record)?;
    if record.transaction != *transaction {
        return Err(EvidenceStoreError::new(
            "gate-evidence::admission-consumption-identity-mismatch",
            "admission consumption record does not match its transaction path",
        ));
    }
    if record.encode()? != bytes {
        return Err(EvidenceStoreError::new(
            "gate-evidence::admission-consumption-noncanonical",
            "admission consumption bytes are not canonical",
        ));
    }
    Ok(Some(record))
}

fn validate(record: &AdmissionConsumption) -> Result<(), EvidenceStoreError> {
    if record.version != CONSUMPTION_VERSION || record.entries.is_empty() {
        return Err(EvidenceStoreError::new(
            "gate-evidence::admission-consumption-invalid",
            "admission consumption has an unsupported version or no entries",
        ));
    }
    for entry in &record.entries {
        validate_candidate(&AdmissionCandidate {
            branch: entry.branch.clone(),
            head: entry.head.clone(),
            commit: entry.commit.clone(),
            tree: entry.tree.clone(),
        })?;
    }
    if record.entries.windows(2).any(|pair| {
        (&pair[0].branch, &pair[0].commit, &pair[0].tree)
            >= (&pair[1].branch, &pair[1].commit, &pair[1].tree)
    }) {
        return Err(EvidenceStoreError::new(
            "gate-evidence::admission-consumption-invalid",
            "admission consumption entries must be sorted and unique",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_identity_is_stable_and_order_sensitive() {
        let first = AdmissionTransactionId::derive(b"a\0b");
        assert_eq!(first, AdmissionTransactionId::derive(b"a\0b"));
        assert_ne!(first, AdmissionTransactionId::derive(b"b\0a"));
    }

    #[test]
    fn candidate_paths_and_git_identities_are_canonical_before_store_access() {
        for candidate in [
            AdmissionCandidate::new(
                "main",
                None,
                "1111111111111111111111111111111111111111",
                "2222222222222222222222222222222222222222",
            ),
            AdmissionCandidate::new("refs/heads/main", None, "not-an-object", "../../outside"),
        ] {
            assert_eq!(
                candidate.unwrap_err().reason(),
                "gate-evidence::admission-consumption-candidate-invalid"
            );
        }
    }
}
