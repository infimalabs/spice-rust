use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::cli::Contract;
use crate::journal_lock::JournalLock;
use fold::{
    FoldId, INVENTORY, Materialization, Source, Store, TaskEffortReducer, select_task_effort,
    task_effort_answer,
};
use spice::repository::{Repository, StateError, StatePaths};
use wire::manifest::{JOURNAL_BACKUP_VERSION, JournalBackupManifest, JournalBackupMember};
use wire::{
    Actor, Digest, Domain, Fault, FaultClass, FoldReport, FoldState, JournalBackup, JournalReindex,
    JournalReport, JournalVerification, Outcome, Overflow, Packed, Producer, Refusal, Reply,
    ReplyPosition, Seq, SeqRange, ThreadReport, WireVersion,
};

const AUTHORITY_MEMBER: &str = "spicejournal.sqlite3";
const ARTIFACT_MEMBERS: &str = "artifacts/tasks";
const MANIFEST: &str = "manifest.json";
const PRIVATE_FILE: u32 = 0o600;
const PRIVATE_DIRECTORY: u32 = 0o700;
const MAX_REPORT_FACTS: usize = 256;
static NEXT_BACKUP: AtomicU64 = AtomicU64::new(0);

pub(crate) fn backup(matches: &clap::ArgMatches) -> Outcome<JournalBackup> {
    let output = matches
        .get_one::<String>("OUTPUT")
        .expect("the generated backup parser requires OUTPUT");
    match backup_inner(Path::new(output)) {
        Ok((position, body)) => reply(ReplyPosition::Committed(position), body),
        Err(failure) => failure.outcome(),
    }
}

pub(crate) fn report(contract: &Contract) -> Outcome<JournalReport> {
    match report_inner(contract) {
        Ok((position, body)) => reply(position, body),
        Err(failure) => failure.outcome(),
    }
}

pub(crate) fn verify(contract: &Contract) -> Outcome<JournalVerification> {
    match verify_inner(contract) {
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

pub(crate) fn thread(matches: &clap::ArgMatches, contract: &Contract) -> Outcome<ThreadReport> {
    let reference = matches
        .get_one::<String>("REF")
        .expect("the generated thread parser requires REF");
    match thread_inner(reference, contract) {
        Ok((position, body)) => reply(position, body),
        Err(failure) => failure.outcome(),
    }
}

pub(crate) fn folds(contract: &Contract) -> Outcome<FoldReport> {
    match folds_inner(contract) {
        Ok((position, body)) => reply(position, body),
        Err(failure) => failure.outcome(),
    }
}

pub(crate) fn replay(matches: &clap::ArgMatches, contract: &Contract) -> Outcome<FoldReport> {
    let selected = matches.get_one::<String>("FOLD");
    match replay_inner(selected.map(String::as_str), contract) {
        Ok((position, body)) => reply(position, body),
        Err(failure) => failure.outcome(),
    }
}

pub(crate) fn task_effort(
    matches: &clap::ArgMatches,
    contract: &Contract,
) -> Outcome<wire::intent::EffortAnswer> {
    let read = wire::intent::EffortRead {
        task: match matches.get_one::<String>("HANDLE") {
            Some(handle) => match handle.parse() {
                Ok(handle) => Some(handle),
                Err(error) => {
                    return Failure::refusal(
                        "task::effort-handle-refused",
                        "one canonical task handle",
                        error.to_string(),
                        "pass HANDLE as <UPPER-STEM>-<key>, or omit it to read every task",
                    )
                    .outcome();
                }
            },
            None => None,
        },
    };
    match task_effort_inner(&read, contract) {
        Ok((position, body)) => reply(position, body),
        Err(failure) => failure.outcome(),
    }
}

fn task_effort_inner(
    read: &wire::intent::EffortRead,
    contract: &Contract,
) -> Result<(ReplyPosition, wire::intent::EffortAnswer), Failure> {
    let state = state()?;
    let reader = open_reader(&state)?;
    let (cursor, snapshot) = read_authenticated(&state, &reader, contract, |read| {
        read.fact_snapshot().map_err(snapshot_failure)
    })?;
    let parent = state
        .folds()
        .parent()
        .expect("folds path has a data parent");
    fs::create_dir_all(parent).map_err(|error| {
        Failure::environment(
            "fold::parent-unavailable",
            format!("{}: {error}", parent.display()),
        )
    })?;
    let mut store = Store::open(state.folds()).map_err(fold_failure)?;
    let materialization = store
        .read_or_rebuild_snapshot(&snapshot, &TaskEffortReducer)
        .map_err(fold_failure)?
        .materialization;
    let answer = task_effort_answer(&materialization)
        .map_err(|error| Failure::store("fold::payload-invalid", error))?;
    let selected = select_task_effort(answer, read)
        .map_err(|error| Failure::store("fold::selection-failed", error))?;
    Ok((ReplyPosition::Snapshot(cursor), selected))
}

fn backup_inner(output: &Path) -> Result<(SeqRange, JournalBackup), Failure> {
    let state = state()?;
    let _lock = acquire_lock(&state)?;
    let reader = open_reader(&state)?;
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
    let authority_member = JournalBackupMember {
        path: AUTHORITY_MEMBER.to_owned(),
        mode: mode(&authority_path)?,
        bytes: count(authority.len(), "authority bytes")?,
        digest: Digest::compute(Domain::JournalBackupMember, &authority),
    };
    let mut members = vec![authority_member];
    members.extend(stage_artifact_members(&state, &temporary)?);
    members.sort_by(|left, right| left.path.cmp(&right.path));
    let manifest = JournalBackupManifest {
        version: JOURNAL_BACKUP_VERSION,
        wire: WireVersion::CURRENT,
        journal_schema: journal::SCHEMA_VERSION,
        authority: AUTHORITY_MEMBER.to_owned(),
        members,
    };
    let manifest_bytes = wire::encode(&manifest)
        .map_err(|error| Failure::protocol("backup::manifest-unencodable", error.to_string()))?;
    write_private_file(&temporary.join(MANIFEST), &manifest_bytes)?;
    sync_tree_directories(&temporary)?;
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
    let body = JournalBackup {
        output: output.display().to_string(),
        verification,
    };
    let position = match append_recovery_fact(
        &state,
        "backup",
        wire::body::JournalBacked {
            bundle: destination.display().to_string(),
            manifest: manifest
                .members
                .iter()
                .map(|member| member.path.clone())
                .collect(),
        },
    ) {
        Ok(position) => position,
        Err(failure) => {
            if let Err(error) = fs::remove_dir_all(&destination) {
                return Err(Failure::environment(
                    "backup::rollback-failed",
                    format!(
                        "{}: {error}; recovery fact was not committed",
                        destination.display()
                    ),
                ));
            }
            sync_directory(&parent)?;
            return Err(failure);
        }
    };
    Ok((position, body))
}

fn stage_artifact_members(
    state: &StatePaths,
    temporary: &Path,
) -> Result<Vec<JournalBackupMember>, Failure> {
    let store = artifact::Store::new(state.artifacts(), state.locks().join("task-artifacts.lock"));
    let image = store.capture().map_err(artifact_failure)?;
    let mut members = Vec::with_capacity(image.files().len());
    for artifact in image.files() {
        let relative = Path::new(ARTIFACT_MEMBERS).join(&artifact.path);
        let destination = temporary.join(&relative);
        let directory = destination
            .parent()
            .expect("artifact backup member has a parent");
        create_private_directories(directory)?;
        write_backup_member(&destination, &artifact.bytes, artifact.mode)?;
        members.push(JournalBackupMember {
            path: relative_spelling(&relative)?,
            mode: mode(&destination)?,
            bytes: count(artifact.bytes.len(), "artifact member bytes")?,
            digest: Digest::compute(Domain::JournalBackupMember, &artifact.bytes),
        });
    }
    Ok(members)
}

fn report_inner(contract: &Contract) -> Result<(ReplyPosition, JournalReport), Failure> {
    let state = state()?;
    let reader = open_reader(&state)?;
    let schema = reader.schema_version().map_err(open_failure)?;
    let (cursor, snapshot) = read_authenticated(&state, &reader, contract, |read| {
        read.fact_snapshot().map_err(snapshot_failure)
    })?;
    let facts = count(snapshot.facts().len(), "facts")?;
    let entries = pack_facts(snapshot.facts().to_vec(), cursor)?;
    Ok((
        ReplyPosition::Snapshot(cursor),
        JournalReport {
            schema,
            cursor: Some(cursor),
            facts,
            entries,
        },
    ))
}

fn verify_inner(contract: &Contract) -> Result<(ReplyPosition, JournalVerification), Failure> {
    let state = state()?;
    let reader = open_reader(&state)?;
    let schema = reader.schema_version().map_err(open_failure)?;
    let (cursor, verified) = read_authenticated(&state, &reader, contract, |read| {
        read.verify().map_err(verify_failure)
    })?;
    Ok((
        ReplyPosition::Snapshot(cursor),
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
    let state = state()?;
    let _lock = acquire_lock(&state)?;
    let mut journal = journal::Journal::open(state.journal()).map_err(open_failure)?;
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
    let cursor = journal
        .fact_snapshot()
        .map_err(snapshot_failure)?
        .horizon()
        .ok_or_else(cursor_missing)?;
    Ok((
        ReplyPosition::Snapshot(cursor),
        JournalReindex {
            facts: count(rebuilt.facts, "facts")?,
            threads: count(rebuilt.threads, "threads")?,
        },
    ))
}

fn thread_inner(
    reference: &str,
    contract: &Contract,
) -> Result<(ReplyPosition, ThreadReport), Failure> {
    let state = state()?;
    let reader = open_reader(&state)?;
    let (cursor, thread) = read_authenticated(&state, &reader, contract, |read| {
        read.thread(reference).map_err(thread_failure)
    })?;
    Ok((
        ReplyPosition::Snapshot(cursor),
        ThreadReport {
            reference: thread.reference,
            root: thread.root,
            entries: pack_facts(thread.entries, cursor)?,
        },
    ))
}

fn folds_inner(contract: &Contract) -> Result<(ReplyPosition, FoldReport), Failure> {
    let state = state()?;
    let reader = open_reader(&state)?;
    let (cursor, ()) = read_authenticated(&state, &reader, contract, |_| Ok(()))?;
    let path = state.folds();
    let states = match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => missing_fold_states(),
        Err(error) => {
            return Err(Failure::environment(
                "fold::store-unavailable",
                format!("{}: {error}", path.display()),
            ));
        }
        Ok(metadata) if metadata.file_type().is_file() => {
            let store = Store::open_read_only(path).map_err(fold_failure)?;
            current_fold_states(&store, cursor)?
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
    Ok((
        ReplyPosition::Snapshot(cursor),
        FoldReport { folds: states },
    ))
}

fn replay_inner(
    selected: Option<&str>,
    contract: &Contract,
) -> Result<(ReplyPosition, FoldReport), Failure> {
    let state = state()?;
    let reader = open_reader(&state)?;
    let (cursor, snapshot) = read_authenticated(&state, &reader, contract, |read| {
        read.verify().map_err(verify_failure)?;
        read.fact_snapshot().map_err(snapshot_failure)
    })?;
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
    let path = state.folds();
    let parent = path.parent().expect("folds path has a data parent");
    fs::create_dir_all(parent).map_err(|error| {
        Failure::environment(
            "fold::parent-unavailable",
            format!("{}: {error}", parent.display()),
        )
    })?;
    let mut store = Store::open(path).map_err(fold_failure)?;
    for fold in folds {
        rebuild_registered_fold(&mut store, &snapshot, *fold)?;
    }
    Ok((
        ReplyPosition::Snapshot(cursor),
        FoldReport {
            folds: current_fold_states(&store, cursor)?,
        },
    ))
}

/// Exhaustive typed replay dispatch for the closed fold inventory.
///
/// Adding a registry row without its operational reducer makes this match fail
/// to compile. There is deliberately no generic fact-bag branch.
fn rebuild_registered_fold(
    store: &mut Store,
    snapshot: &journal::FactSnapshot,
    fold: FoldId,
) -> Result<(), Failure> {
    match fold {
        FoldId::TaskEffort => {
            store
                .read_or_rebuild_snapshot(snapshot, &TaskEffortReducer)
                .map_err(fold_failure)?;
        }
    }
    Ok(())
}

fn current_fold_states(store: &Store, authority: Seq) -> Result<Vec<FoldState>, Failure> {
    store
        .snapshot()
        .map_err(fold_failure)?
        .into_iter()
        .map(|(fold, current)| {
            if current
                .as_ref()
                .and_then(|state| state.cursor.position())
                .is_some_and(|cursor| cursor > authority)
            {
                return Err(Failure::refusal(
                    "fold::generation-ahead",
                    format!("a fold cursor no later than authority snapshot {authority}"),
                    format!("{fold} materialization is ahead of that snapshot"),
                    "retry against current resident authority",
                ));
            }
            Ok(fold_state(fold, current.as_ref()))
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
    let definition = INVENTORY.definition(fold);
    let recovery = definition.recovery_command();
    FoldState {
        fold: fold.as_str().to_owned(),
        generation: materialization.map(|state| state.generation),
        reducer_version: materialization.map(|state| state.reducer_version),
        cursor: materialization.and_then(|state| state.cursor.position()),
        horizon: materialization.and_then(|state| state.horizon.position()),
        recovery,
        sources: definition
            .sources
            .iter()
            .copied()
            .map(Source::provenance)
            .collect(),
        payload: materialization.map(|state| state.payload().clone()),
    }
}

fn read_authenticated<T, F>(
    state: &StatePaths,
    reader: &journal::JournalReader,
    contract: &Contract,
    answer: F,
) -> Result<(Seq, T), Failure>
where
    F: for<'a> FnOnce(&journal::JournalRead<'a>) -> Result<T, Failure>,
{
    let epoch = crate::command_client::authenticate(state.canvas(), contract)
        .map_err(authentication_failure)?;
    reader
        .read_with(|read| {
            let cursor = read
                .cursor()
                .map_err(|error| Failure::store("journal::read-failed", error.to_string()))?
                .ok_or_else(cursor_missing)?;
            let residency = read
                .residency_head()
                .map_err(|error| Failure::store("journal::read-failed", error.to_string()))?
                .ok_or_else(|| {
                    Failure::store(
                        "journal::residency-missing",
                        "the authenticated resident has no durable residency head",
                    )
                })?;
            if residency.body().epoch != epoch {
                return Err(Failure::refusal(
                    "journal::epoch-moved",
                    epoch.to_string(),
                    residency.body().epoch.to_string(),
                    "reconnect through current resident discovery and retry",
                ));
            }
            answer(read).map(|body| (cursor, body))
        })
        .map_err(|error| match error {
            journal::ReadError::Journal(error) => {
                Failure::store("journal::read-failed", error.to_string())
            }
            journal::ReadError::Answer(failure) => failure,
        })
}

fn authentication_failure(outcome: Outcome<wire::intent::CommandAnswer>) -> Failure {
    match outcome {
        Outcome::Refusal(refusal) => Failure::Refusal(refusal),
        Outcome::Fault(fault) => Failure::Fault(fault),
        Outcome::Reply(_) => Failure::protocol(
            "journal::authentication-invalid",
            "resident authentication returned a command reply as an error",
        ),
    }
}

fn cursor_missing() -> Failure {
    Failure::store(
        "journal::cursor-missing",
        "a resident or recovery authority has no durable journal position",
    )
}

fn thread_failure(error: journal::ThreadError) -> Failure {
    match error {
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
    }
}

fn append_recovery_fact<F: wire::Body>(
    state: &StatePaths,
    operation: &str,
    body: F,
) -> Result<SeqRange, Failure> {
    let origin = format!("recovery:{operation}")
        .parse::<wire::Origin>()
        .map_err(|error| Failure::protocol("journal::origin-invalid", error.to_string()))?;
    let at = wire::Instant::new(jiff::Timestamp::now())
        .map_err(|error| Failure::environment("journal::clock-invalid", error.to_string()))?;
    let mut write =
        journal::JournalWrite::new(Actor::Spice, origin, at, Producer::ResidentRecognition);
    write
        .fact(body, journal::FactCause::Root)
        .map_err(|error| Failure::protocol("journal::recovery-fact-invalid", error.to_string()))?;
    let mut journal = journal::Journal::open(state.journal()).map_err(open_failure)?;
    let committed = journal.apply(write).map_err(commit_failure)?;
    Ok(SeqRange {
        first: committed.first(),
        last: committed.last(),
    })
}

fn state() -> Result<StatePaths, Failure> {
    let working = std::env::current_dir().map_err(|error| {
        Failure::environment("journal::working-directory-unavailable", error.to_string())
    })?;
    let repository = Repository::discover(&working).map_err(|error| {
        Failure::environment("journal::repository-unavailable", error.to_string())
    })?;
    repository.admit_state().map_err(state_failure)
}

fn open_reader(state: &StatePaths) -> Result<journal::JournalReader, Failure> {
    journal::JournalReader::open(state.journal()).map_err(open_failure)
}

fn acquire_lock(state: &StatePaths) -> Result<JournalLock, Failure> {
    JournalLock::acquire(state).map_err(|error| match error {
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

fn state_failure(error: StateError) -> Failure {
    match error {
        StateError::Filesystem { .. } => {
            Failure::environment("repository::state-path-unavailable", error.to_string())
        }
        StateError::Linked { .. }
        | StateError::NotDirectory { .. }
        | StateError::NotRegularFile { .. }
        | StateError::RetiredState { .. } => Failure::refusal(
            "repository::state-path-refused",
            "real fixed components beneath the canonical Git-common .spice-rs directory",
            error.to_string(),
            format!(
                "follow the named archive/remove action or repair the malformed state component at {}",
                error.path().display()
            ),
        ),
    }
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
        journal::VerifyError::Violations(violations) => Failure::refusal(
            "journal::verification-failed",
            "one valid schema, history, index, heads, receipts, and cursor closure",
            violations
                .iter()
                .map(|violation| {
                    format!(
                        "{:?}:{}:{}",
                        violation.class, violation.subject, violation.detail
                    )
                })
                .collect::<Vec<_>>()
                .join(" | "),
            "restore valid authority or use the named recovery command for a derived index",
        ),
        journal::VerifyError::Open(error) => Failure::refusal(
            "journal::verification-failed",
            "one finalized current journal schema",
            error.to_string(),
            "finish or replace incompatible authority before retrying",
        ),
        journal::VerifyError::Sqlite(_) => {
            Failure::store("journal::verify-failed", error.to_string())
        }
    }
}

fn commit_failure(error: journal::CommitError) -> Failure {
    match error {
        journal::CommitError::Refused(_) => Failure::refusal(
            "journal::recovery-refused",
            "one current exclusive recovery transaction",
            error.to_string(),
            "verify the current authority and retry while the resident is quiescent",
        ),
        journal::CommitError::Fault(_) => {
            Failure::store("journal::recovery-failed", error.to_string())
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

fn pack_facts(
    mut entries: Vec<wire::Envelope<wire::AnyFact>>,
    horizon: Seq,
) -> Result<Packed<wire::Envelope<wire::AnyFact>>, Failure> {
    let overflow = if entries.len() > MAX_REPORT_FACTS {
        let continuation = entries[MAX_REPORT_FACTS].seq;
        let omitted = count(entries.len() - MAX_REPORT_FACTS, "omitted facts")?;
        entries.truncate(MAX_REPORT_FACTS);
        Some(Overflow {
            omitted,
            continuation,
        })
    } else {
        None
    };
    Ok(Packed {
        rows: entries,
        overflow,
        horizon,
    })
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
        match create_private_directory(&path) {
            Ok(()) => {
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

fn create_private_directories(path: &Path) -> Result<(), Failure> {
    #[cfg(unix)]
    use std::os::unix::fs::DirBuilderExt;

    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(PRIVATE_DIRECTORY);
    builder.create(path).map_err(|error| {
        Failure::environment(
            "backup::stage-failed",
            format!("{}: {error}", path.display()),
        )
    })
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let mut builder = fs::DirBuilder::new();
    builder.mode(PRIVATE_DIRECTORY).create(path)
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(PRIVATE_FILE);
    }
    let mut file = options.open(path).map_err(|error| {
        Failure::environment(
            "backup::write-failed",
            format!("{}: {error}", path.display()),
        )
    })?;
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

fn write_backup_member(path: &Path, bytes: &[u8], requested_mode: u32) -> Result<(), Failure> {
    write_private_file(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(requested_mode)).map_err(|error| {
            Failure::environment(
                "backup::mode-failed",
                format!("{}: {error}", path.display()),
            )
        })?;
        File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|error| {
                Failure::environment(
                    "backup::mode-failed",
                    format!("{}: {error}", path.display()),
                )
            })?;
    }
    #[cfg(not(unix))]
    let _ = requested_mode;
    Ok(())
}

fn relative_spelling(path: &Path) -> Result<String, Failure> {
    path.components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()
        .map(|parts| parts.join("/"))
        .ok_or_else(|| {
            Failure::protocol(
                "backup::member-path-invalid",
                format!("{} is not UTF-8", path.display()),
            )
        })
}

fn sync_tree_directories(root: &Path) -> Result<(), Failure> {
    let mut directories = vec![root.to_path_buf()];
    let mut index = 0;
    while index < directories.len() {
        let directory = directories[index].clone();
        for entry in fs::read_dir(&directory).map_err(|error| {
            Failure::environment(
                "backup::sync-failed",
                format!("{}: {error}", directory.display()),
            )
        })? {
            let entry = entry.map_err(|error| {
                Failure::environment(
                    "backup::sync-failed",
                    format!("{}: {error}", directory.display()),
                )
            })?;
            if entry
                .file_type()
                .map_err(|error| {
                    Failure::environment(
                        "backup::sync-failed",
                        format!("{}: {error}", entry.path().display()),
                    )
                })?
                .is_dir()
            {
                directories.push(entry.path());
            }
        }
        index += 1;
    }
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        sync_directory(&directory)?;
    }
    Ok(())
}

fn artifact_failure(error: artifact::Error) -> Failure {
    match error.class() {
        artifact::ErrorClass::Refusal => Failure::refusal(
            "backup::artifact-refused",
            "one current artifact store generation",
            error.to_string(),
            "repair or migrate the artifact store with the release that owns it",
        ),
        artifact::ErrorClass::Corrupt => {
            Failure::store("backup::artifact-corrupt", error.to_string())
        }
        artifact::ErrorClass::Environment => {
            Failure::environment("backup::artifact-unavailable", error.to_string())
        }
    }
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
