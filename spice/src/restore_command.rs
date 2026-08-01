use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::journal_lock::JournalLock;
use serde::Serialize;
use spice::repository::{Repository, StateError, StatePaths};
use wire::{
    Actor, Digest, Domain, Fault, FaultClass, JournalRestore, Outcome, Producer, Refusal, Reply,
    ReplyPosition, RestoreVerification, SeqRange,
};

pub(crate) fn restore(matches: &clap::ArgMatches) -> Outcome<JournalRestore> {
    let bundle = matches
        .get_one::<String>("BUNDLE")
        .expect("restore requires its generated BUNDLE operand");
    match restore_inner(Path::new(bundle), matches.get_one::<String>("apply")) {
        Ok((position, body)) => Outcome::Reply(Reply { position, body }),
        Err(failure) => failure.outcome(),
    }
}

pub(crate) fn verify(matches: &clap::ArgMatches) -> Outcome<RestoreVerification> {
    let bundle = matches
        .get_one::<String>("BUNDLE")
        .expect("restore verify requires its generated BUNDLE operand");
    match journal::verify_restore_bundle(Path::new(bundle)) {
        Ok(body) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body,
        }),
        Err(error) => failure(error),
    }
}

fn restore_inner(
    bundle: &Path,
    requested: Option<&String>,
) -> Result<(ReplyPosition, JournalRestore), Failure> {
    let prepared = prepare_restore(bundle)?;
    let Some(requested) = requested else {
        return Ok((ReplyPosition::Direct {}, prepared.report(false)));
    };
    let (position, body) = apply_restore(prepared, requested)?;
    Ok((ReplyPosition::Committed(position), body))
}

struct PreparedRestore {
    state: StatePaths,
    destination: PathBuf,
    bundle: String,
    capture: journal::RestoreCapture,
    previous: Option<Digest>,
    previous_bytes: Option<Vec<u8>>,
    plan: Digest,
}

impl PreparedRestore {
    fn report(self, applied: bool) -> JournalRestore {
        JournalRestore {
            plan: self.plan,
            destination: self.destination.display().to_string(),
            previous: self.previous,
            verification: self.capture.into_verification(),
            applied,
        }
    }
}

fn prepare_restore(bundle: &Path) -> Result<PreparedRestore, Failure> {
    let working = std::env::current_dir().map_err(|error| {
        fault(
            FaultClass::Environment,
            "restore::working-directory-unavailable",
            error.to_string(),
        )
    })?;
    let repository = Repository::discover(&working).map_err(|error| {
        fault(
            FaultClass::Environment,
            "restore::repository-unavailable",
            error.to_string(),
        )
    })?;
    let state = repository.admit_state().map_err(state_failure)?;
    let destination = state.journal().to_path_buf();
    let capture = journal::capture_restore_bundle(bundle).map_err(bundle_failure)?;
    let previous_bytes = current_bytes(&destination)?;
    let previous = previous_bytes
        .as_ref()
        .map(|bytes| Digest::compute(Domain::JournalBackupMember, bytes));
    let plan_document = RestorePlanIdentity {
        version: 1,
        bundle: capture.verification().bundle.clone(),
        destination: destination.display().to_string(),
        previous: previous.clone(),
    };
    let plan_bytes = wire::encode(&plan_document).map_err(|error| {
        fault(
            FaultClass::Protocol,
            "restore::plan-unencodable",
            error.to_string(),
        )
    })?;
    let plan = Digest::compute(Domain::JournalRestorePlan, &plan_bytes);
    Ok(PreparedRestore {
        state,
        destination,
        bundle: bundle.display().to_string(),
        capture,
        previous,
        previous_bytes,
        plan,
    })
}

fn apply_restore(
    prepared: PreparedRestore,
    requested: &str,
) -> Result<(SeqRange, JournalRestore), Failure> {
    let _lock = admit_apply(&prepared, requested)?;
    publish_authority(&prepared.destination, prepared.capture.authority_bytes())?;
    let finalized = verify_published(&prepared).and_then(|()| append_restored(&prepared));
    let position = match finalized {
        Ok(position) => position,
        Err(failure) => {
            rollback_authority(&prepared).map_err(|rollback| {
                fault(
                    FaultClass::Environment,
                    "restore::rollback-failed",
                    format!("{rollback:?}; replacement finalization also failed"),
                )
            })?;
            return Err(failure);
        }
    };
    Ok((position, prepared.report(true)))
}

fn admit_apply(prepared: &PreparedRestore, requested: &str) -> Result<JournalLock, Failure> {
    let requested = requested.parse::<Digest>().map_err(|error| {
        refusal(
            "restore::plan-digest-refused",
            "the exact digest printed by `spice restore BUNDLE`",
            error.to_string(),
            "preview the unchanged bundle again and pass its canonical plan digest",
        )
    })?;
    if requested != prepared.plan {
        return Err(refusal(
            "restore::stale-plan",
            prepared.plan.to_string(),
            requested.to_string(),
            "preview the unchanged bundle against the current authority and apply that digest",
        ));
    }
    let lock = JournalLock::acquire(&prepared.state).map_err(|error| match error {
        crate::journal_lock::Error::Contended => refusal(
            "restore::writer-active",
            "the resident writer lock to be unheld",
            "the resident or another maintenance command holds the lock",
            "quiesce the resident before applying restore",
        ),
        crate::journal_lock::Error::Io(error) => fault(
            FaultClass::Environment,
            "restore::lock-unavailable",
            error.to_string(),
        ),
    })?;
    refuse_sidecars(&prepared.destination)?;
    let observed = current_identity(&prepared.destination)?;
    if observed != prepared.previous {
        return Err(refusal(
            "restore::authority-changed",
            format!("predecessor identity {:?}", prepared.previous),
            format!("predecessor identity {observed:?}"),
            "preview again after the authority writer is quiescent",
        ));
    }
    Ok(lock)
}

fn publish_authority(destination: &Path, bytes: &[u8]) -> Result<(), Failure> {
    let parent = destination
        .parent()
        .expect("repository journal path has a data parent");
    fs::create_dir_all(parent).map_err(|error| {
        fault(
            FaultClass::Environment,
            "restore::destination-unavailable",
            format!("{}: {error}", parent.display()),
        )
    })?;
    let destination_file = atomic::Destination::new(destination).map_err(|error| {
        fault(
            FaultClass::Environment,
            "restore::destination-unavailable",
            error.to_string(),
        )
    })?;
    let mut staged = destination_file.stage().map_err(|error| {
        fault(
            FaultClass::Environment,
            "restore::stage-failed",
            error.to_string(),
        )
    })?;
    set_private(staged.temporary_path())?;
    staged.write_all(bytes).map_err(|error| {
        fault(
            FaultClass::Environment,
            "restore::stage-failed",
            error.to_string(),
        )
    })?;
    staged.commit().map_err(|error| {
        fault(
            FaultClass::Environment,
            "restore::publish-failed",
            error.to_string(),
        )
    })?;
    sync_parent(destination)
}

fn verify_published(prepared: &PreparedRestore) -> Result<(), Failure> {
    let reader = journal::JournalReader::open(&prepared.destination).map_err(|error| {
        fault(
            FaultClass::Store,
            "restore::reopen-failed",
            error.to_string(),
        )
    })?;
    let verified = reader.verify().map_err(|error| {
        fault(
            FaultClass::Store,
            "restore::post-verification-failed",
            error.to_string(),
        )
    })?;
    let expected = prepared.capture.verification();
    let agreed = u64::try_from(verified.facts).ok() == Some(expected.facts)
        && u64::try_from(verified.receipts).ok() == Some(expected.receipts)
        && u64::try_from(verified.heads).ok() == Some(expected.heads)
        && u64::try_from(verified.threads).ok() == Some(expected.threads)
        && current_identity(&prepared.destination)? == Some(expected.authority.clone());
    if !agreed {
        return Err(fault(
            FaultClass::Store,
            "restore::post-verification-disagreed",
            "published authority does not reproduce the captured verification receipt",
        ));
    }
    Ok(())
}

fn append_restored(prepared: &PreparedRestore) -> Result<SeqRange, Failure> {
    let epoch = wire::CanonicalUuid::new(uuid::Uuid::new_v4());
    let origin = format!("recovery:{epoch}")
        .parse::<wire::Origin>()
        .map_err(|error| {
            fault(
                FaultClass::Protocol,
                "restore::origin-invalid",
                error.to_string(),
            )
        })?;
    let at = wire::Instant::new(jiff::Timestamp::now()).map_err(|error| {
        fault(
            FaultClass::Environment,
            "restore::clock-invalid",
            error.to_string(),
        )
    })?;
    let mut write =
        journal::JournalWrite::new(Actor::Spice, origin, at, Producer::ResidentRecognition);
    write
        .fact(
            wire::body::JournalRestored {
                bundle: prepared.bundle.clone(),
                epoch,
                authority: prepared.capture.verification().authority.clone(),
            },
            journal::FactCause::Root,
        )
        .map_err(|error| {
            fault(
                FaultClass::Protocol,
                "restore::fact-invalid",
                error.to_string(),
            )
        })?;
    let mut authority = journal::Journal::open(&prepared.destination).map_err(|error| {
        fault(
            FaultClass::Store,
            "restore::reopen-failed",
            error.to_string(),
        )
    })?;
    let committed = authority.apply(write).map_err(|error| match error {
        journal::CommitError::Refused(_) => refusal(
            "restore::recognition-refused",
            "one current restored authority",
            error.to_string(),
            "verify the replacement and retry the digest-pinned restore",
        ),
        journal::CommitError::Fault(_) => fault(
            FaultClass::Store,
            "restore::recognition-failed",
            error.to_string(),
        ),
    })?;
    Ok(SeqRange {
        first: committed.first(),
        last: committed.last(),
    })
}

fn rollback_authority(prepared: &PreparedRestore) -> Result<(), Failure> {
    match prepared.previous_bytes.as_deref() {
        Some(bytes) => publish_authority(&prepared.destination, bytes),
        None => match fs::remove_file(&prepared.destination) {
            Ok(()) => sync_parent(&prepared.destination),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(fault(
                FaultClass::Environment,
                "restore::rollback-failed",
                format!("{}: {error}", prepared.destination.display()),
            )),
        },
    }
}

fn sync_parent(path: &Path) -> Result<(), Failure> {
    let parent = path
        .parent()
        .expect("repository journal path has a data parent");
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            fault(
                FaultClass::Environment,
                "restore::sync-failed",
                format!("{}: {error}", parent.display()),
            )
        })
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct RestorePlanIdentity {
    version: u16,
    bundle: Digest,
    destination: String,
    previous: Option<Digest>,
}

fn current_identity(path: &Path) -> Result<Option<Digest>, Failure> {
    Ok(current_bytes(path)?
        .as_ref()
        .map(|bytes| Digest::compute(Domain::JournalBackupMember, bytes)))
}

fn current_bytes(path: &Path) -> Result<Option<Vec<u8>>, Failure> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(fault(
                FaultClass::Environment,
                "restore::authority-unreadable",
                format!("{}: {error}", path.display()),
            ));
        }
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(refusal(
                "restore::authority-refused",
                "an absent or regular-file authority destination",
                path.display().to_string(),
                "remove the non-file obstruction before previewing restore",
            ));
        }
        Ok(_) => {}
    }
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let mut file = options.open(path).map_err(|error| {
        fault(
            FaultClass::Environment,
            "restore::authority-unreadable",
            format!("{}: {error}", path.display()),
        )
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|error| {
        fault(
            FaultClass::Environment,
            "restore::authority-unreadable",
            format!("{}: {error}", path.display()),
        )
    })?;
    Ok(Some(bytes))
}

fn refuse_sidecars(destination: &Path) -> Result<(), Failure> {
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut name = OsString::from(destination.as_os_str());
        name.push(suffix);
        let path = PathBuf::from(name);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(fault(
                    FaultClass::Environment,
                    "restore::sidecar-unreadable",
                    format!("{}: {error}", path.display()),
                ));
            }
            Ok(_) => {
                return Err(refusal(
                    "restore::authority-active",
                    "no live or residual SQLite sidecars",
                    path.display().to_string(),
                    "quiesce the resident and resolve the sidecar before applying restore",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_private(path: &Path) -> Result<(), Failure> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        fault(
            FaultClass::Environment,
            "restore::stage-mode-failed",
            format!("{}: {error}", path.display()),
        )
    })
}

#[cfg(not(unix))]
fn set_private(_path: &Path) -> Result<(), Failure> {
    Ok(())
}

fn bundle_failure(error: journal::BackupVerifyError) -> Failure {
    use journal::BackupVerifyError;

    match error {
        missing @ (BackupVerifyError::Missing { .. } | BackupVerifyError::Incomplete { .. }) => {
            Failure::Refusal(Refusal {
                reason: "restore::bundle-incomplete".to_owned(),
                witness: Some("backup-inventory".to_owned()),
                expected: Some("one complete manifest-bound bundle".to_owned()),
                observed: Some(missing.to_string()),
                remedy: "restore every member named by the canonical backup manifest".to_owned(),
            })
        }
        version @ BackupVerifyError::UnsupportedVersion { .. } => Failure::Refusal(Refusal {
            reason: "restore::bundle-version".to_owned(),
            witness: Some("backup-schema".to_owned()),
            expected: Some(wire::manifest::JOURNAL_BACKUP_VERSION.to_string()),
            observed: Some(version.to_string()),
            remedy: "verify the bundle with the Spice release that owns its backup schema"
                .to_owned(),
        }),
        version @ BackupVerifyError::UnsupportedWire { .. } => Failure::Refusal(Refusal {
            reason: "restore::wire-version".to_owned(),
            witness: Some("wire-schema".to_owned()),
            expected: Some(wire::WireVersion::CURRENT.to_string()),
            observed: Some(version.to_string()),
            remedy: "verify the bundle with the Spice release that owns its wire schema".to_owned(),
        }),
        schema @ (BackupVerifyError::ForwardSchema { .. }
        | BackupVerifyError::Authority(journal::OpenError::Migration(
            journal::MigrationError::ForwardSchema { .. },
        ))) => Failure::Refusal(Refusal {
            reason: "restore::schema-forward".to_owned(),
            witness: Some("journal-schema".to_owned()),
            expected: Some(journal::SCHEMA_VERSION.to_string()),
            observed: Some(schema.to_string()),
            remedy: "verify the bundle with the Spice release that owns its journal schema"
                .to_owned(),
        }),
        unreadable @ BackupVerifyError::Io { .. } => Failure::Fault(Fault {
            class: FaultClass::Environment,
            reason: "restore::bundle-unreadable".to_owned(),
            detail: unreadable.to_string(),
        }),
        invalid => Failure::Fault(Fault {
            class: FaultClass::Store,
            reason: "restore::bundle-invalid".to_owned(),
            detail: invalid.to_string(),
        }),
    }
}

fn state_failure(error: StateError) -> Failure {
    match error {
        StateError::Filesystem { .. } => fault(
            FaultClass::Environment,
            "repository::state-path-unavailable",
            error.to_string(),
        ),
        StateError::Linked { .. }
        | StateError::NotDirectory { .. }
        | StateError::NotRegularFile { .. } => refusal(
            "repository::state-path-refused",
            "real fixed components beneath the canonical Git-common .spice-rust directory",
            error.to_string(),
            format!(
                "remove or repair the linked or malformed Rust state component at {}",
                error.path().display()
            ),
        ),
    }
}

fn refusal(
    reason: impl Into<String>,
    expected: impl Into<String>,
    observed: impl Into<String>,
    remedy: impl Into<String>,
) -> Failure {
    Failure::Refusal(Refusal {
        reason: reason.into(),
        witness: None,
        expected: Some(expected.into()),
        observed: Some(observed.into()),
        remedy: remedy.into(),
    })
}

fn fault(class: FaultClass, reason: impl Into<String>, detail: impl Into<String>) -> Failure {
    Failure::Fault(Fault {
        class,
        reason: reason.into(),
        detail: detail.into(),
    })
}

fn failure<B>(error: journal::BackupVerifyError) -> Outcome<B> {
    bundle_failure(error).outcome()
}

#[derive(Debug)]
enum Failure {
    Refusal(Refusal),
    Fault(Fault),
}

impl Failure {
    fn outcome<B>(self) -> Outcome<B> {
        match self {
            Self::Refusal(refusal) => Outcome::Refusal(refusal),
            Self::Fault(fault) => Outcome::Fault(fault),
        }
    }
}
