use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::journal_lock::JournalLock;
use fold::{
    AnswerContent, Cursor, FoldId, Horizon, INVENTORY, Materialization, Reducer, Reduction, Source,
    Store,
};
use spice::repository::Repository;
use wire::manifest::{JOURNAL_BACKUP_VERSION, JournalBackupManifest, JournalBackupMember};
use wire::{
    Digest, Domain, Fault, FaultClass, FoldReport, FoldState, JournalBackup, JournalReindex,
    JournalReport, JournalVerification, Outcome, Refusal, Reply, ReplyPosition, ThreadReport,
    WireVersion,
};

const AUTHORITY_MEMBER: &str = "spicejournal.sqlite3";
const MANIFEST: &str = "manifest.json";
const PRIVATE_FILE: u32 = 0o600;
const PRIVATE_DIRECTORY: u32 = 0o700;
static NEXT_BACKUP: AtomicU64 = AtomicU64::new(0);

pub(crate) fn backup(matches: &clap::ArgMatches) -> Outcome<JournalBackup> {
    let output = matches
        .get_one::<String>("OUTPUT")
        .expect("the generated backup parser requires OUTPUT");
    match backup_inner(Path::new(output)) {
        Ok(body) => reply(ReplyPosition::Direct {}, body),
        Err(failure) => failure.outcome(),
    }
}

pub(crate) fn report() -> Outcome<JournalReport> {
    match report_inner() {
        Ok((position, body)) => reply(position, body),
        Err(failure) => failure.outcome(),
    }
}

pub(crate) fn verify() -> Outcome<JournalVerification> {
    match verify_inner() {
        Ok((position, body)) => reply(position, body),
        Err(failure) => failure.outcome(),
    }
}

pub(crate) fn reindex() -> Outcome<JournalReindex> {
    match reindex_inner() {
        Ok((position, body)) => reply(position, body),
        Err(failure) => failure.outcome(),
    }
}

pub(crate) fn thread(matches: &clap::ArgMatches) -> Outcome<ThreadReport> {
    let reference = matches
        .get_one::<String>("REF")
        .expect("the generated thread parser requires REF");
    match thread_inner(reference) {
        Ok((position, body)) => reply(position, body),
        Err(failure) => failure.outcome(),
    }
}

pub(crate) fn folds() -> Outcome<FoldReport> {
    match folds_inner() {
        Ok((position, body)) => reply(position, body),
        Err(failure) => failure.outcome(),
    }
}

pub(crate) fn replay(matches: &clap::ArgMatches) -> Outcome<FoldReport> {
    let selected = matches.get_one::<String>("FOLD");
    match replay_inner(selected.map(String::as_str)) {
        Ok((position, body)) => reply(position, body),
        Err(failure) => failure.outcome(),
    }
}

fn backup_inner(output: &Path) -> Result<JournalBackup, Failure> {
    let repository = repository()?;
    let _lock = acquire_lock(&repository)?;
    let reader = open_reader(&repository)?;
    reader.verify().map_err(verify_failure)?;
    let authority = reader.snapshot_bytes().map_err(open_failure)?;
    let output_name = output.file_name().ok_or_else(|| {
        Failure::refusal(
            "backup::output-refused",
            "a new bundle path with a filename",
            output.display().to_string(),
            "choose a nonexistent output path in an existing directory",
        )
    })?;
    let requested_parent = output.parent().unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(requested_parent).map_err(|error| {
        Failure::environment(
            "backup::parent-unavailable",
            format!("{}: {error}", requested_parent.display()),
        )
    })?;
    let destination = parent.join(output_name);
    require_absent(&destination)?;

    let (temporary, mut cleanup) = create_staging_directory(&parent)?;
    let authority_path = temporary.join(AUTHORITY_MEMBER);
    write_private_file(&authority_path, &authority)?;
    let member = JournalBackupMember {
        path: AUTHORITY_MEMBER.to_owned(),
        mode: mode(&authority_path)?,
        bytes: count(authority.len(), "authority bytes")?,
        digest: Digest::compute(Domain::JournalBackupMember, &authority),
    };
    let manifest = JournalBackupManifest {
        version: JOURNAL_BACKUP_VERSION,
        wire: WireVersion::CURRENT,
        journal_schema: journal::SCHEMA_VERSION,
        authority: AUTHORITY_MEMBER.to_owned(),
        members: vec![member],
    };
    let manifest_bytes = wire::encode(&manifest)
        .map_err(|error| Failure::protocol("backup::manifest-unencodable", error.to_string()))?;
    write_private_file(&temporary.join(MANIFEST), &manifest_bytes)?;
    sync_directory(&temporary)?;
    let verification = journal::verify_restore_bundle(&temporary).map_err(bundle_failure)?;
    require_absent(&destination)?;
    fs::rename(&temporary, &destination).map_err(|error| {
        Failure::environment(
            "backup::publish-failed",
            format!("{}: {error}", destination.display()),
        )
    })?;
    cleanup.disarm();
    sync_directory(&parent)?;
    Ok(JournalBackup {
        output: output.display().to_string(),
        verification,
    })
}

fn report_inner() -> Result<(ReplyPosition, JournalReport), Failure> {
    let repository = repository()?;
    let reader = open_reader(&repository)?;
    let schema = reader.schema_version().map_err(open_failure)?;
    let snapshot = reader.fact_snapshot().map_err(snapshot_failure)?;
    let cursor = snapshot.horizon();
    Ok((
        snapshot_position(cursor),
        JournalReport {
            schema,
            cursor,
            facts: count(snapshot.facts().len(), "facts")?,
        },
    ))
}

fn verify_inner() -> Result<(ReplyPosition, JournalVerification), Failure> {
    let repository = repository()?;
    let reader = open_reader(&repository)?;
    let schema = reader.schema_version().map_err(open_failure)?;
    let cursor = reader.cursor().map_err(open_failure)?;
    let verified = reader.verify().map_err(verify_failure)?;
    Ok((
        snapshot_position(cursor),
        JournalVerification {
            schema,
            facts: count(verified.facts, "facts")?,
            receipts: count(verified.receipts, "receipts")?,
            heads: count(verified.heads, "heads")?,
            threads: count(verified.threads, "threads")?,
        },
    ))
}

fn reindex_inner() -> Result<(ReplyPosition, JournalReindex), Failure> {
    let repository = repository()?;
    let _lock = acquire_lock(&repository)?;
    let mut journal = journal::Journal::open(repository.journal_path()).map_err(open_failure)?;
    let rebuilt = journal.reindex().map_err(|error| match error {
        journal::ReindexError::Violations(_) => Failure::refusal(
            "journal::history-invalid",
            "canonical contiguous immutable facts",
            error.to_string(),
            "run `spice journal verify` and restore valid authority before reindexing",
        ),
        journal::ReindexError::Sqlite(_) => {
            Failure::store("journal::reindex-failed", error.to_string())
        }
    })?;
    let cursor = journal.fact_snapshot().map_err(snapshot_failure)?.horizon();
    Ok((
        snapshot_position(cursor),
        JournalReindex {
            facts: count(rebuilt.facts, "facts")?,
            threads: count(rebuilt.threads, "threads")?,
        },
    ))
}

fn thread_inner(reference: &str) -> Result<(ReplyPosition, ThreadReport), Failure> {
    let repository = repository()?;
    let reader = open_reader(&repository)?;
    let cursor = reader.cursor().map_err(open_failure)?;
    let thread = reader.thread(reference).map_err(|error| match error {
        journal::ThreadError::NotFound { .. } | journal::ThreadError::Ambiguous { .. } => {
            Failure::refusal(
                "thread::reference-refused",
                "one existing sequence or unambiguous exact origin",
                error.to_string(),
                "select a fact sequence, or an origin that names exactly one root",
            )
        }
        journal::ThreadError::Verify(_) | journal::ThreadError::Snapshot(_) => Failure::refusal(
            "thread::authority-invalid",
            "verified immutable history and thread index",
            error.to_string(),
            "run `spice journal verify`, then restore or reindex as directed",
        ),
        journal::ThreadError::Sqlite(_) => Failure::store("thread::read-failed", error.to_string()),
    })?;
    Ok((
        snapshot_position(cursor),
        ThreadReport {
            reference: thread.reference,
            root: thread.root,
            entries: thread.entries,
        },
    ))
}

fn folds_inner() -> Result<(ReplyPosition, FoldReport), Failure> {
    let repository = repository()?;
    let reader = open_reader(&repository)?;
    let cursor = reader.cursor().map_err(open_failure)?;
    let path = repository.folds_path();
    let states = match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => missing_fold_states(),
        Err(error) => {
            return Err(Failure::environment(
                "fold::store-unavailable",
                format!("{}: {error}", path.display()),
            ));
        }
        Ok(metadata) if metadata.file_type().is_file() => {
            let store = Store::open_read_only(&path).map_err(fold_failure)?;
            current_fold_states(&store)?
        }
        Ok(_) => {
            return Err(Failure::refusal(
                "fold::store-refused",
                "a regular disposable folds database",
                path.display().to_string(),
                "remove the non-file obstruction and replay the folds",
            ));
        }
    };
    Ok((snapshot_position(cursor), FoldReport { folds: states }))
}

fn replay_inner(selected: Option<&str>) -> Result<(ReplyPosition, FoldReport), Failure> {
    let repository = repository()?;
    let reader = open_reader(&repository)?;
    reader.verify().map_err(verify_failure)?;
    let snapshot = reader.fact_snapshot().map_err(snapshot_failure)?;
    let selected = selected
        .map(|name| {
            name.parse::<FoldId>().map_err(|error| {
                Failure::refusal(
                    "fold::selection-refused",
                    "one fold from the closed inventory",
                    error.to_string(),
                    "run `spice fold` to list every admitted fold name",
                )
            })
        })
        .transpose()?;
    let folds = selected.as_ref().map_or(FoldId::ALL, std::slice::from_ref);
    let path = repository.folds_path();
    let parent = path.parent().expect("folds path has a data parent");
    fs::create_dir_all(parent).map_err(|error| {
        Failure::environment(
            "fold::parent-unavailable",
            format!("{}: {error}", parent.display()),
        )
    })?;
    let mut store = Store::open(&path).map_err(fold_failure)?;
    for fold in folds {
        store
            .read_or_rebuild_snapshot(&snapshot, &AuthorityReducer { fold: *fold })
            .map_err(fold_failure)?;
    }
    Ok((
        snapshot_position(snapshot.horizon()),
        FoldReport {
            folds: current_fold_states(&store)?,
        },
    ))
}

struct AuthorityReducer {
    fold: FoldId,
}

impl Reducer for AuthorityReducer {
    fn fold(&self) -> FoldId {
        self.fold
    }

    fn version(&self) -> u32 {
        1
    }

    fn rebuild(&self, facts: &journal::FactSnapshot) -> Result<Reduction, String> {
        let definition = INVENTORY.definition(self.fold);
        let selected = facts
            .facts()
            .iter()
            .filter(|envelope| {
                definition
                    .answer_content
                    .iter()
                    .any(|content| match content {
                        AnswerContent::Fact(kind) => *kind == envelope.fact.kind(),
                        AnswerContent::Facts(plane) => plane.contains(envelope.fact.kind()),
                        AnswerContent::Head(_) => false,
                    })
            })
            .collect::<Vec<_>>();
        let payload = serde_json::json!({
            "answer": definition.answer,
            "facts": selected,
        });
        if definition
            .sources
            .iter()
            .all(|source| matches!(source, Source::Fact(_) | Source::Facts(_)))
        {
            Ok(Reduction::complete(facts, payload))
        } else {
            Reduction::bounded(
                facts.horizon().map_or_else(Cursor::empty, Cursor::at),
                Horizon::empty(),
                payload,
            )
            .map_err(|error| format!("{error:?}"))
        }
    }
}

fn current_fold_states(store: &Store) -> Result<Vec<FoldState>, Failure> {
    INVENTORY
        .definitions()
        .iter()
        .map(|definition| {
            store
                .current(definition.fold)
                .map_err(fold_failure)
                .map(|current| fold_state(definition.fold, current.as_ref()))
        })
        .collect()
}

fn missing_fold_states() -> Vec<FoldState> {
    INVENTORY
        .definitions()
        .iter()
        .map(|definition| fold_state(definition.fold, None))
        .collect()
}

fn fold_state(fold: FoldId, materialization: Option<&Materialization>) -> FoldState {
    let recovery = INVENTORY
        .definition(fold)
        .recovery_command()
        .expect("the release-one fold inventory assigns replay recovery");
    FoldState {
        fold: fold.as_str().to_owned(),
        generation: materialization.map(|state| state.generation),
        reducer_version: materialization.map(|state| state.reducer_version),
        cursor: materialization.and_then(|state| state.cursor.position()),
        horizon: materialization.and_then(|state| state.horizon.position()),
        recovery,
        payload: materialization.map(|state| state.payload().clone()),
    }
}

fn repository() -> Result<Repository, Failure> {
    let working = std::env::current_dir().map_err(|error| {
        Failure::environment("journal::working-directory-unavailable", error.to_string())
    })?;
    Repository::discover(&working)
        .map_err(|error| Failure::environment("journal::repository-unavailable", error.to_string()))
}

fn open_reader(repository: &Repository) -> Result<journal::JournalReader, Failure> {
    journal::JournalReader::open(repository.journal_path()).map_err(open_failure)
}

fn acquire_lock(repository: &Repository) -> Result<JournalLock, Failure> {
    JournalLock::acquire(repository).map_err(|error| match error {
        crate::journal_lock::Error::Contended => Failure::refusal(
            "journal::writer-active",
            "the resident writer lock to be unheld",
            "the resident or another maintenance command holds the lock",
            "quiesce the resident and retry the offline journal operation",
        ),
        crate::journal_lock::Error::Io(error) => {
            Failure::environment("journal::lock-unavailable", error.to_string())
        }
    })
}

fn open_failure(error: journal::OpenError) -> Failure {
    match error {
        journal::OpenError::Migration(_) => Failure::refusal(
            "journal::schema-refused",
            format!("finalized journal schema {}", journal::SCHEMA_VERSION),
            error.to_string(),
            "finish or replace incompatible authority before retrying",
        ),
        _ => Failure::store("journal::open-failed", error.to_string()),
    }
}

fn snapshot_failure(error: journal::SnapshotError) -> Failure {
    match error {
        journal::SnapshotError::Invalid { .. } => Failure::refusal(
            "journal::history-invalid",
            "canonical contiguous immutable facts",
            error.to_string(),
            "run `spice journal verify` and restore valid authority",
        ),
        journal::SnapshotError::Sqlite(_) => {
            Failure::store("journal::read-failed", error.to_string())
        }
    }
}

fn verify_failure(error: journal::VerifyError) -> Failure {
    match error {
        journal::VerifyError::Violations(_) | journal::VerifyError::Open(_) => Failure::refusal(
            "journal::verification-failed",
            "one valid schema, history, index, heads, receipts, and cursor closure",
            error.to_string(),
            "restore valid authority or use the named recovery command for a derived index",
        ),
        journal::VerifyError::Sqlite(_) => {
            Failure::store("journal::verify-failed", error.to_string())
        }
    }
}

fn fold_failure(error: fold::StoreError) -> Failure {
    match error {
        fold::StoreError::Corrupt { .. }
        | fold::StoreError::Refused(_)
        | fold::StoreError::SchemaVersion { .. } => Failure::refusal(
            "fold::materialization-refused",
            "one current deterministic materialization",
            error.to_string(),
            "run `spice journal replay` to rebuild disposable folds",
        ),
        _ => Failure::store("fold::store-failed", error.to_string()),
    }
}

fn bundle_failure(error: journal::BackupVerifyError) -> Failure {
    Failure::store("backup::self-verification-failed", error.to_string())
}

fn count(value: usize, subject: &str) -> Result<u64, Failure> {
    u64::try_from(value).map_err(|error| {
        Failure::protocol(
            "journal::count-overflow",
            format!("{subject} cannot cross as u64: {error}"),
        )
    })
}

fn snapshot_position(cursor: Option<wire::Seq>) -> ReplyPosition {
    cursor.map_or(ReplyPosition::Direct {}, ReplyPosition::Snapshot)
}

fn reply<B>(position: ReplyPosition, body: B) -> Outcome<B> {
    Outcome::Reply(Reply { position, body })
}

fn require_absent(path: &Path) -> Result<(), Failure> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Failure::environment(
            "backup::output-unavailable",
            format!("{}: {error}", path.display()),
        )),
        Ok(_) => Err(Failure::refusal(
            "backup::output-exists",
            "a nonexistent output path",
            path.display().to_string(),
            "choose a new bundle destination; backups never overwrite",
        )),
    }
}

fn create_staging_directory(parent: &Path) -> Result<(PathBuf, DirectoryCleanup), Failure> {
    for _ in 0..128 {
        let path = parent.join(format!(
            ".spice-backup-{}-{}",
            std::process::id(),
            NEXT_BACKUP.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&path) {
            Ok(()) => {
                set_mode(&path, PRIVATE_DIRECTORY)?;
                return Ok((path.clone(), DirectoryCleanup(Some(path))));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(Failure::environment(
                    "backup::stage-failed",
                    format!("{}: {error}", path.display()),
                ));
            }
        }
    }
    Err(Failure::environment(
        "backup::stage-exhausted",
        parent.display().to_string(),
    ))
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            Failure::environment(
                "backup::write-failed",
                format!("{}: {error}", path.display()),
            )
        })?;
    set_mode(path, PRIVATE_FILE)?;
    file.write_all(bytes).map_err(|error| {
        Failure::environment(
            "backup::write-failed",
            format!("{}: {error}", path.display()),
        )
    })?;
    file.sync_all().map_err(|error| {
        Failure::environment(
            "backup::sync-failed",
            format!("{}: {error}", path.display()),
        )
    })
}

fn sync_directory(path: &Path) -> Result<(), Failure> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            Failure::environment(
                "backup::sync-failed",
                format!("{}: {error}", path.display()),
            )
        })
}

#[cfg(unix)]
fn mode(path: &Path) -> Result<u32, Failure> {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o7777)
        .map_err(|error| {
            Failure::environment(
                "backup::metadata-failed",
                format!("{}: {error}", path.display()),
            )
        })
}

#[cfg(not(unix))]
fn mode(path: &Path) -> Result<u32, Failure> {
    Err(Failure::environment(
        "backup::mode-unavailable",
        path.display().to_string(),
    ))
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), Failure> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|error| {
        Failure::environment(
            "backup::mode-failed",
            format!("{}: {error}", path.display()),
        )
    })
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), Failure> {
    Ok(())
}

struct DirectoryCleanup(Option<PathBuf>);

impl DirectoryCleanup {
    fn disarm(&mut self) {
        self.0.take();
    }
}

impl Drop for DirectoryCleanup {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

enum Failure {
    Refusal(Refusal),
    Fault(Fault),
}

impl Failure {
    fn refusal(
        reason: impl Into<String>,
        expected: impl Into<String>,
        observed: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Self {
        Self::Refusal(Refusal {
            reason: reason.into(),
            witness: None,
            expected: Some(expected.into()),
            observed: Some(observed.into()),
            remedy: remedy.into(),
        })
    }

    fn environment(reason: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Fault(Fault {
            class: FaultClass::Environment,
            reason: reason.into(),
            detail: detail.into(),
        })
    }

    fn store(reason: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Fault(Fault {
            class: FaultClass::Store,
            reason: reason.into(),
            detail: detail.into(),
        })
    }

    fn protocol(reason: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Fault(Fault {
            class: FaultClass::Protocol,
            reason: reason.into(),
            detail: detail.into(),
        })
    }

    fn outcome<B>(self) -> Outcome<B> {
        match self {
            Self::Refusal(refusal) => Outcome::Refusal(refusal),
            Self::Fault(fault) => Outcome::Fault(fault),
        }
    }
}
