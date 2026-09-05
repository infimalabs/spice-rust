use std::collections::BTreeSet;
use std::fs::{self, DirEntry, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::*;

const STATE_DIRECTORY: &str = ".spice-rs";
const STORE_DIRECTORY: &str = "gate-evidence";
const VERSION_DIRECTORY: &str = "v1";
const MARKER_FILE: &str = "store.json";
const RECORD_LIMIT: u64 = MAX_RECEIPT_BYTES as u64;
const METADATA_LIMIT: u64 = 64 * 1024;

#[derive(Clone, Debug)]
pub(super) struct Layout {
    pub(super) root: PathBuf,
    pub(super) objects: PathBuf,
    pub(super) actions: PathBuf,
    pub(super) leases: PathBuf,
    pub(super) locks: PathBuf,
}

impl Layout {
    pub(super) fn initialize(common_git: &Path) -> Result<Self, EvidenceStoreError> {
        let state = common_git.join(STATE_DIRECTORY);
        create_directory(&state)?;
        let store = state.join(STORE_DIRECTORY);
        create_directory(&store)?;
        let root = store.join(VERSION_DIRECTORY);
        create_directory(&root)?;
        let layout = Self {
            objects: root.join("objects"),
            actions: root.join("actions"),
            leases: root.join("leases"),
            locks: root.join("locks"),
            root,
        };
        for path in [
            &layout.objects,
            &layout.actions,
            &layout.leases,
            &layout.locks,
        ] {
            create_directory(path)?;
        }
        let marker = layout.root.join(MARKER_FILE);
        match fs::symlink_metadata(&marker) {
            Ok(_) => validate_marker(&marker)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                publish_json(
                    &marker,
                    &StoreMarker {
                        version: EVIDENCE_STORE_VERSION,
                    },
                )?;
                validate_marker(&marker)?;
            }
            Err(error) => {
                return Err(EvidenceStoreError::io(
                    "inspect store marker",
                    &marker,
                    error,
                ));
            }
        }
        layout.validate()?;
        Ok(layout)
    }

    pub(super) fn validate(&self) -> Result<(), EvidenceStoreError> {
        for path in [
            &self.root,
            &self.objects,
            &self.actions,
            &self.leases,
            &self.locks,
        ] {
            require_directory(path)?;
        }
        validate_marker(&self.root.join(MARKER_FILE))
    }

    pub(super) fn object_path(&self, receipt: &ReceiptId) -> PathBuf {
        self.objects.join(receipt.as_str())
    }
    pub(super) fn index_path(&self, action: &ActionKey) -> PathBuf {
        self.actions.join(action.as_str())
    }
    pub(super) fn lease_path(&self, action: &ActionKey) -> PathBuf {
        self.leases.join(action.as_str())
    }
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StoreMarker {
    version: u64,
}

fn validate_marker(path: &Path) -> Result<(), EvidenceStoreError> {
    let bytes = read_regular(path, METADATA_LIMIT)?;
    let marker: StoreMarker = decode_canonical(&bytes, "gate-evidence::store-marker-invalid")?;
    if marker.version != EVIDENCE_STORE_VERSION {
        return Err(EvidenceStoreError::new(
            "gate-evidence::store-version-unsupported",
            format!(
                "store version {} is not {EVIDENCE_STORE_VERSION}",
                marker.version
            ),
        ));
    }
    Ok(())
}

pub(super) fn read_index(
    layout: &Layout,
    action: &ActionKey,
) -> Result<Option<ActionIndex>, EvidenceStoreError> {
    let path = layout.index_path(action);
    let Some(bytes) = read_optional_regular(&path, METADATA_LIMIT)? else {
        return Ok(None);
    };
    let index: ActionIndex = decode_canonical(&bytes, "gate-evidence::index-invalid")?;
    if index.version != EVIDENCE_STORE_VERSION {
        return Err(EvidenceStoreError::new(
            "gate-evidence::index-version-unsupported",
            format!(
                "index version {} is not {EVIDENCE_STORE_VERSION}",
                index.version
            ),
        ));
    }
    if &index.action != action {
        return Err(EvidenceStoreError::new(
            "gate-evidence::index-action-mismatch",
            format!("index path names {action}, body names {}", index.action),
        ));
    }
    read_receipt(layout, &index.receipt, Some(action))?;
    Ok(Some(index))
}

pub(super) fn read_receipt(
    layout: &Layout,
    id: &ReceiptId,
    action: Option<&ActionKey>,
) -> Result<PassReceipt, EvidenceStoreError> {
    let path = layout.object_path(id);
    let bytes = read_regular(&path, RECORD_LIMIT)?;
    let observed = ReceiptId(Digest::compute(Domain::GatePassReceipt, &bytes));
    if &observed != id {
        return Err(EvidenceStoreError::new(
            "gate-evidence::receipt-digest-mismatch",
            format!("object path names {id}, bytes identify {observed}"),
        ));
    }
    let receipt = PassReceipt::decode(&bytes)?;
    if let Some(action) = action
        && receipt.action() != action
    {
        return Err(EvidenceStoreError::new(
            "gate-evidence::receipt-action-mismatch",
            format!("index names {action}, receipt names {}", receipt.action()),
        ));
    }
    Ok(receipt)
}

pub(super) fn read_lease(
    layout: &Layout,
    action: &ActionKey,
) -> Result<Option<LeaseRecord>, EvidenceStoreError> {
    let path = layout.lease_path(action);
    let Some(bytes) = read_optional_regular(&path, METADATA_LIMIT)? else {
        return Ok(None);
    };
    let record: LeaseRecord = decode_canonical(&bytes, "gate-evidence::lease-invalid")?;
    if record.version != LEASE_VERSION {
        return Err(EvidenceStoreError::new(
            "gate-evidence::lease-version-unsupported",
            format!("lease version {} is not {LEASE_VERSION}", record.version),
        ));
    }
    if &record.action != action {
        return Err(EvidenceStoreError::new(
            "gate-evidence::lease-action-mismatch",
            format!("lease path names {action}, body names {}", record.action),
        ));
    }
    validate_owner(&record.owner)?;
    Ok(Some(record))
}

pub(super) fn publish_immutable(path: &Path, bytes: &[u8]) -> Result<bool, EvidenceStoreError> {
    match read_optional_regular(path, RECORD_LIMIT)? {
        Some(existing) if existing == bytes => Ok(false),
        Some(_) => Err(EvidenceStoreError::new(
            "gate-evidence::immutable-object-mismatch",
            format!("existing object {} has different bytes", path.display()),
        )),
        None => {
            let parent = path.parent().expect("object path has a parent");
            let temporary = parent.join(format!(
                ".tmp-{}-{}",
                std::process::id(),
                NEXT_LEASE.fetch_add(1, Ordering::Relaxed)
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            let mut file = options.open(&temporary).map_err(|error| {
                EvidenceStoreError::io("create immutable temporary", &temporary, error)
            })?;
            if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
                let _ = fs::remove_file(&temporary);
                return Err(EvidenceStoreError::io(
                    "write immutable temporary",
                    &temporary,
                    error,
                ));
            }
            if let Err(error) = fs::rename(&temporary, path) {
                let _ = fs::remove_file(&temporary);
                return Err(EvidenceStoreError::io(
                    "publish immutable object",
                    path,
                    error,
                ));
            }
            sync_directory(parent)?;
            Ok(true)
        }
    }
}

pub(super) fn publish_json(path: &Path, value: &impl Serialize) -> Result<(), EvidenceStoreError> {
    let bytes = wire::encode(value).map_err(|error| {
        EvidenceStoreError::new("gate-evidence::record-invalid", error.to_string())
    })?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(EvidenceStoreError::new(
                "gate-evidence::record-type-invalid",
                format!("{} is not a regular file", path.display()),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(EvidenceStoreError::io(
                "inspect publication target",
                path,
                error,
            ));
        }
    }
    let destination = atomic::Destination::new(path).map_err(|error| {
        EvidenceStoreError::new("gate-evidence::publication-failed", error.to_string())
    })?;
    destination
        .publish(|file| file.write_all(&bytes))
        .map_err(|error| {
            EvidenceStoreError::new("gate-evidence::publication-failed", error.to_string())
        })
}

pub(super) fn remove_file(path: &Path) -> Result<(), EvidenceStoreError> {
    fs::remove_file(path).map_err(|error| EvidenceStoreError::io("remove record", path, error))?;
    sync_directory(path.parent().expect("record has a parent"))
}

pub(super) fn collect_layout(
    layout: &Layout,
    now: SystemTime,
    minimum_age: Duration,
    limit: usize,
) -> Result<CollectionReport, EvidenceStoreError> {
    let mut report = CollectionReport {
        removed_objects: 0,
        removed_leases: 0,
        retained: Vec::new(),
    };
    let mut remaining = limit;
    let mut referenced = BTreeSet::new();
    let mut indexes_ambiguous = false;
    for entry in read_directory(&layout.actions)? {
        match action_from_entry(&entry).and_then(|action| read_index(layout, &action)) {
            Ok(Some(index)) => {
                referenced.insert(index.receipt);
            }
            Ok(None) => unreachable!("directory entry disappeared while locked"),
            Err(error) => {
                indexes_ambiguous = true;
                report
                    .retained
                    .push(format!("{}: {error}", entry.path().display()));
            }
        }
    }
    if !indexes_ambiguous && remaining > 0 {
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
                    Err(error) => report
                        .retained
                        .push(format!("{}: {error}", entry.path().display())),
                },
                Err(error) => report
                    .retained
                    .push(format!("{}: {error}", entry.path().display())),
            }
        }
        candidates.sort();
        for path in candidates.into_iter().take(remaining) {
            remove_file(&path)?;
            report.removed_objects += 1;
            remaining -= 1;
        }
    }
    if remaining > 0 {
        let now_millis = millis(now)?;
        for entry in read_directory(&layout.leases)? {
            if remaining == 0 {
                break;
            }
            match action_from_entry(&entry).and_then(|action| read_lease(layout, &action)) {
                Ok(Some(record)) if record.expires_at_millis <= now_millis => {
                    remove_file(&entry.path())?;
                    report.removed_leases += 1;
                    remaining -= 1;
                }
                Ok(Some(_)) => {}
                Ok(None) => unreachable!("directory entry disappeared while locked"),
                Err(error) => report
                    .retained
                    .push(format!("{}: {error}", entry.path().display())),
            }
        }
    }
    report.retained.sort();
    Ok(report)
}

fn action_from_entry(entry: &DirEntry) -> Result<ActionKey, EvidenceStoreError> {
    let digest = digest_from_entry(entry)?;
    Ok(ActionKey(digest))
}

fn receipt_from_entry(entry: &DirEntry) -> Result<ReceiptId, EvidenceStoreError> {
    let digest = digest_from_entry(entry)?;
    Ok(ReceiptId(digest))
}

fn digest_from_entry(entry: &DirEntry) -> Result<Digest, EvidenceStoreError> {
    let name = entry.file_name().into_string().map_err(|_| {
        EvidenceStoreError::new(
            "gate-evidence::entry-name-invalid",
            format!("non-UTF-8 entry at {}", entry.path().display()),
        )
    })?;
    name.parse::<Digest>().map_err(|error| {
        EvidenceStoreError::new("gate-evidence::entry-name-invalid", error.to_string())
    })
}

fn old_enough(
    path: &Path,
    now: SystemTime,
    minimum_age: Duration,
) -> Result<bool, EvidenceStoreError> {
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| EvidenceStoreError::io("measure record age", path, error))?;
    Ok(now
        .duration_since(modified)
        .is_ok_and(|age| age >= minimum_age))
}

fn decode_canonical<T>(bytes: &[u8], reason: &'static str) -> Result<T, EvidenceStoreError>
where
    T: DeserializeOwned + Serialize,
{
    let value: T =
        wire::decode(bytes).map_err(|error| EvidenceStoreError::new(reason, error.to_string()))?;
    let canonical =
        wire::encode(&value).map_err(|error| EvidenceStoreError::new(reason, error.to_string()))?;
    if canonical != bytes {
        return Err(EvidenceStoreError::new(
            reason,
            "record is not canonically encoded",
        ));
    }
    Ok(value)
}

fn read_optional_regular(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, EvidenceStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                return Err(EvidenceStoreError::new(
                    "gate-evidence::record-type-invalid",
                    format!("{} is not a regular file", path.display()),
                ));
            }
            if metadata.len() > limit {
                return Err(EvidenceStoreError::new(
                    "gate-evidence::record-size-invalid",
                    format!(
                        "{} is {} bytes, over {limit}",
                        path.display(),
                        metadata.len()
                    ),
                ));
            }
            fs::read(path)
                .map(Some)
                .map_err(|error| EvidenceStoreError::io("read record", path, error))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(EvidenceStoreError::io("inspect record", path, error)),
    }
}

fn read_regular(path: &Path, limit: u64) -> Result<Vec<u8>, EvidenceStoreError> {
    read_optional_regular(path, limit)?.ok_or_else(|| {
        EvidenceStoreError::new(
            "gate-evidence::record-missing",
            format!("{} does not exist", path.display()),
        )
    })
}

fn read_directory(path: &Path) -> Result<Vec<DirEntry>, EvidenceStoreError> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| EvidenceStoreError::io("read directory", path, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| EvidenceStoreError::io("read directory entry", path, error))?;
    entries.sort_by_key(DirEntry::file_name);
    Ok(entries)
}

fn create_directory(path: &Path) -> Result<(), EvidenceStoreError> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(EvidenceStoreError::io("create directory", path, error)),
    }
    require_directory(path)
}

fn require_directory(path: &Path) -> Result<(), EvidenceStoreError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| EvidenceStoreError::io("inspect directory", path, error))?;
    if metadata.file_type().is_dir() {
        Ok(())
    } else {
        Err(EvidenceStoreError::new(
            "gate-evidence::directory-type-invalid",
            format!("{} is not a directory", path.display()),
        ))
    }
}

fn sync_directory(path: &Path) -> Result<(), EvidenceStoreError> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| EvidenceStoreError::io("sync directory", path, error))
}
