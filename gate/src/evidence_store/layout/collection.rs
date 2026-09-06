use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::*;

type AdmissionReference = (String, AdmissionId);

struct ConsumptionRecord {
    path: PathBuf,
    record: AdmissionConsumption,
    old: bool,
}

struct AdmissionRecord {
    path: PathBuf,
    reference: AdmissionReference,
    receipts: Vec<ReceiptId>,
    old: bool,
}

pub(in crate::evidence_store) fn collect_layout(
    layout: &Layout,
    now: SystemTime,
    minimum_age: Duration,
    limit: usize,
) -> Result<CollectionReport, EvidenceStoreError> {
    let mut report = CollectionReport {
        removed_objects: 0,
        removed_leases: 0,
        removed_admissions: 0,
        removed_pending_consumptions: 0,
        removed_admission_credits: 0,
        retained: Vec::new(),
    };
    let mut remaining = limit;
    let (mut admissions, mut roots_ambiguous) = indexed_admissions(layout, &mut report)?;

    for (directory, kind) in [
        (&layout.pending_admissions, ConsumptionKind::Pending),
        (&layout.admission_credits, ConsumptionKind::Credit),
    ] {
        let (references, ambiguous) = collect_consumptions(
            directory,
            kind,
            now,
            minimum_age,
            &mut remaining,
            &mut report,
        )?;
        admissions.extend(references);
        roots_ambiguous |= ambiguous;
    }

    let (admission_receipts, admissions_ambiguous) = collect_admissions(
        layout,
        &admissions,
        roots_ambiguous,
        now,
        minimum_age,
        &mut remaining,
        &mut report,
    )?;
    let (mut receipts, indexes_ambiguous) = indexed_receipts(layout, &mut report)?;
    receipts.extend(admission_receipts);
    collect_receipts(
        layout,
        &receipts,
        roots_ambiguous || admissions_ambiguous || indexes_ambiguous,
        now,
        minimum_age,
        &mut remaining,
        &mut report,
    )?;
    collect_leases(layout, now, &mut remaining, &mut report)?;
    report.retained.sort();
    Ok(report)
}

#[derive(Clone, Copy)]
enum ConsumptionKind {
    Pending,
    Credit,
}

fn collect_consumptions(
    directory: &Path,
    kind: ConsumptionKind,
    now: SystemTime,
    minimum_age: Duration,
    remaining: &mut usize,
    report: &mut CollectionReport,
) -> Result<(BTreeSet<AdmissionReference>, bool), EvidenceStoreError> {
    let mut records = Vec::new();
    let mut ambiguous = false;
    for entry in read_directory(directory)? {
        match consumption_record(&entry, now, minimum_age) {
            Ok(record) => records.push(record),
            Err(error) => {
                ambiguous = true;
                retain_error(report, &entry.path(), error);
            }
        }
    }

    let mut references = BTreeSet::new();
    for record in records {
        if record.old && *remaining > 0 {
            remove_file(&record.path)?;
            *remaining -= 1;
            match kind {
                ConsumptionKind::Pending => report.removed_pending_consumptions += 1,
                ConsumptionKind::Credit => report.removed_admission_credits += 1,
            }
        } else {
            references.extend(
                record
                    .record
                    .admission_references()
                    .map(|(tree, admission)| (tree.to_owned(), admission.clone())),
            );
        }
    }
    Ok((references, ambiguous))
}

fn consumption_record(
    entry: &fs::DirEntry,
    now: SystemTime,
    minimum_age: Duration,
) -> Result<ConsumptionRecord, EvidenceStoreError> {
    let transaction = AdmissionTransactionId::from_digest(digest_from_entry(entry)?);
    let path = entry.path();
    let record = consumption::read_record(&path, &transaction)?.ok_or_else(|| {
        EvidenceStoreError::new(
            "gate-evidence::record-missing",
            format!("{} disappeared during collection", path.display()),
        )
    })?;
    let old = old_enough(&path, now, minimum_age)?;
    Ok(ConsumptionRecord { path, record, old })
}

fn indexed_admissions(
    layout: &Layout,
    report: &mut CollectionReport,
) -> Result<(BTreeSet<AdmissionReference>, bool), EvidenceStoreError> {
    let mut references = BTreeSet::new();
    let mut ambiguous = false;
    for entry in read_directory(&layout.admission_indexes)? {
        let result = (|| {
            let key = digest_from_entry(&entry)?;
            let index = admission::read_index(&entry.path())?.ok_or_else(|| {
                EvidenceStoreError::new(
                    "gate-evidence::record-missing",
                    format!("{} disappeared during collection", entry.path().display()),
                )
            })?;
            if index.key()? != key {
                return Err(EvidenceStoreError::new(
                    "gate-evidence::admission-index-identity-mismatch",
                    "admission index authority does not match its path",
                ));
            }
            let (tree, admission) = index.reference();
            Ok((tree.to_owned(), admission.clone()))
        })();
        match result {
            Ok(reference) => {
                references.insert(reference);
            }
            Err(error) => {
                ambiguous = true;
                retain_error(report, &entry.path(), error);
            }
        }
    }
    Ok((references, ambiguous))
}

fn collect_admissions(
    layout: &Layout,
    referenced: &BTreeSet<AdmissionReference>,
    mut protect_all: bool,
    now: SystemTime,
    minimum_age: Duration,
    remaining: &mut usize,
    report: &mut CollectionReport,
) -> Result<(BTreeSet<ReceiptId>, bool), EvidenceStoreError> {
    let mut records = Vec::new();
    let mut found = BTreeSet::new();
    let mut ambiguous = false;
    for tree_entry in read_directory(&layout.admissions)? {
        let tree = match admission_tree(&tree_entry) {
            Ok(tree) => tree,
            Err(error) => {
                ambiguous = true;
                retain_error(report, &tree_entry.path(), error);
                continue;
            }
        };
        for entry in read_directory(&tree_entry.path())? {
            match admission_record(&entry, &tree, now, minimum_age) {
                Ok(record) => {
                    found.insert(record.reference.clone());
                    records.push(record);
                }
                Err(error) => {
                    ambiguous = true;
                    retain_error(report, &entry.path(), error);
                }
            }
        }
    }
    for missing in referenced.difference(&found) {
        protect_all = true;
        report.retained.push(format!(
            "{}: gate-evidence::admission-reference-missing",
            layout
                .admissions
                .join(&missing.0)
                .join(missing.1.as_str())
                .display()
        ));
    }

    let mut receipts = BTreeSet::new();
    for record in records {
        if !protect_all && !referenced.contains(&record.reference) && record.old && *remaining > 0 {
            remove_file(&record.path)?;
            report.removed_admissions += 1;
            *remaining -= 1;
        } else {
            receipts.extend(record.receipts);
        }
    }
    Ok((receipts, ambiguous))
}

fn admission_tree(entry: &fs::DirEntry) -> Result<String, EvidenceStoreError> {
    let tree = entry.file_name().into_string().map_err(|_| {
        EvidenceStoreError::new(
            "gate-evidence::entry-name-invalid",
            format!("non-UTF-8 entry at {}", entry.path().display()),
        )
    })?;
    if !admission::canonical_object_id(&tree) {
        return Err(EvidenceStoreError::new(
            "gate-evidence::admission-candidate-invalid",
            format!("admission directory names noncanonical tree {tree:?}"),
        ));
    }
    let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
        EvidenceStoreError::io("inspect admission directory", &entry.path(), error)
    })?;
    if !metadata.file_type().is_dir() {
        return Err(EvidenceStoreError::new(
            "gate-evidence::admission-entry-invalid",
            format!("{} is not a directory", entry.path().display()),
        ));
    }
    Ok(tree)
}

fn admission_record(
    entry: &fs::DirEntry,
    tree: &str,
    now: SystemTime,
    minimum_age: Duration,
) -> Result<AdmissionRecord, EvidenceStoreError> {
    let id = AdmissionId::from_digest(digest_from_entry(entry)?);
    let path = entry.path();
    let bytes = read_optional_regular(&path, MAX_RECEIPT_BYTES as u64)?.ok_or_else(|| {
        EvidenceStoreError::new(
            "gate-evidence::record-missing",
            format!("{} disappeared during collection", path.display()),
        )
    })?;
    let bundle = AdmissionBundle::decode(&bytes)?;
    if bundle.id()? != id || bundle.candidate_tree() != tree {
        return Err(EvidenceStoreError::new(
            "gate-evidence::admission-identity-mismatch",
            "admission bundle identity or candidate tree does not match its path",
        ));
    }
    let receipts = bundle
        .actions()
        .iter()
        .map(|action| action.receipt().clone())
        .collect();
    let old = old_enough(&path, now, minimum_age)?;
    Ok(AdmissionRecord {
        path,
        reference: (tree.to_owned(), id),
        receipts,
        old,
    })
}

fn indexed_receipts(
    layout: &Layout,
    report: &mut CollectionReport,
) -> Result<(BTreeSet<ReceiptId>, bool), EvidenceStoreError> {
    let mut referenced = BTreeSet::new();
    let mut ambiguous = false;
    for entry in read_directory(&layout.actions)? {
        match action_from_entry(&entry).and_then(|action| read_index(layout, &action)) {
            Ok(Some(index)) => {
                referenced.insert(index.receipt);
            }
            Ok(None) => unreachable!("directory entry disappeared while locked"),
            Err(error) => {
                ambiguous = true;
                retain_error(report, &entry.path(), error);
            }
        }
    }
    Ok((referenced, ambiguous))
}

fn collect_receipts(
    layout: &Layout,
    referenced: &BTreeSet<ReceiptId>,
    protect_all: bool,
    now: SystemTime,
    minimum_age: Duration,
    remaining: &mut usize,
    report: &mut CollectionReport,
) -> Result<(), EvidenceStoreError> {
    if protect_all || *remaining == 0 {
        return Ok(());
    }
    let mut candidates = Vec::new();
    for entry in read_directory(&layout.objects)? {
        match receipt_from_entry(&entry).and_then(|id| {
            read_receipt(layout, &id, None)?;
            Ok(id)
        }) {
            Ok(id) if referenced.contains(&id) => {}
            Ok(_) => match old_enough(&entry.path(), now, minimum_age) {
                Ok(true) => candidates.push(entry.path()),
                Ok(false) => {}
                Err(error) => retain_error(report, &entry.path(), error),
            },
            Err(error) => retain_error(report, &entry.path(), error),
        }
    }
    for path in candidates.into_iter().take(*remaining) {
        remove_file(&path)?;
        report.removed_objects += 1;
        *remaining -= 1;
    }
    Ok(())
}

fn collect_leases(
    layout: &Layout,
    now: SystemTime,
    remaining: &mut usize,
    report: &mut CollectionReport,
) -> Result<(), EvidenceStoreError> {
    if *remaining == 0 {
        return Ok(());
    }
    let now_millis = millis(now)?;
    for entry in read_directory(&layout.leases)? {
        if *remaining == 0 {
            break;
        }
        match action_from_entry(&entry).and_then(|action| read_lease(layout, &action)) {
            Ok(Some(record)) if record.expires_at_millis <= now_millis => {
                remove_file(&entry.path())?;
                report.removed_leases += 1;
                *remaining -= 1;
            }
            Ok(Some(_)) => {}
            Ok(None) => unreachable!("directory entry disappeared while locked"),
            Err(error) => retain_error(report, &entry.path(), error),
        }
    }
    Ok(())
}

fn retain_error(report: &mut CollectionReport, path: &Path, error: EvidenceStoreError) {
    report.retained.push(format!("{}: {error}", path.display()));
}
