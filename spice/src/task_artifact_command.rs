use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use artifact::{AddRequest, ArtifactAnswer, ArtifactContent, ErrorClass, Retention, Store};
use spice::cli::Contract;
use spice::config::LoadError;
use spice::repository::{Repository, StateError, StatePaths};
use wire::{Fault, FaultClass, Instant, Outcome, Refusal, Reply, ReplyPosition, Seq, TaskHandle};

pub(crate) fn run(matches: &clap::ArgMatches, contract: &Contract) -> Outcome<ArtifactAnswer> {
    match run_inner(matches, contract) {
        Ok((position, body)) => Outcome::Reply(Reply { position, body }),
        Err(failure) => failure.outcome(),
    }
}

fn run_inner(
    matches: &clap::ArgMatches,
    contract: &Contract,
) -> Result<(ReplyPosition, ArtifactAnswer), Failure> {
    let snapshot = snapshot()?;
    let store = Store::new(
        snapshot.state.artifacts(),
        snapshot.state.locks().join("task-artifacts.lock"),
    );
    let position = ReplyPosition::Snapshot(snapshot.cursor);
    match contract.path {
        ["task", "artifact", "add"] => {
            let task = selected_task(matches, &snapshot)?;
            let source = matches
                .get_one::<String>("PATH")
                .expect("generated artifact add parser requires PATH");
            let retention = matches
                .get_one::<String>("retention")
                .map_or(Ok(Retention::Permanent), |value| value.parse())
                .map_err(|error: artifact::VocabularyError| {
                    Failure::refusal(
                        "artifact::retention-invalid",
                        "permanent or prunable",
                        error.to_string(),
                        "choose one named retention policy",
                    )
                })?;
            let created_at = now()?;
            let artifact = store
                .add(AddRequest {
                    task: &task,
                    source: Path::new(source),
                    name: matches.get_one::<String>("name").map(String::as_str),
                    media_type: matches.get_one::<String>("type").map(String::as_str),
                    retention,
                    created_at,
                })
                .map_err(artifact_failure)?;
            Ok((position, ArtifactAnswer::Added { task, artifact }))
        }
        ["task", "artifact", "list"] => {
            let task = selected_task(matches, &snapshot)?;
            let artifacts = store.list(&task).map_err(artifact_failure)?;
            Ok((position, ArtifactAnswer::Listed { task, artifacts }))
        }
        ["task", "artifact", "show"] => {
            let task = selected_task(matches, &snapshot)?;
            let artifact_id = matches
                .get_one::<String>("ARTIFACT")
                .expect("generated artifact show parser requires ARTIFACT");
            let (artifact, bytes) = store.show(&task, artifact_id).map_err(artifact_failure)?;
            let content = if artifact.media_type.is_text() {
                ArtifactContent::Text {
                    text: String::from_utf8(bytes).expect("store proved text artifact UTF-8"),
                }
            } else {
                ArtifactContent::Binary {
                    path: payload_path(store.root(), &task, &artifact.digest)
                        .display()
                        .to_string(),
                }
            };
            Ok((
                position,
                ArtifactAnswer::Shown {
                    task,
                    artifact,
                    content,
                },
            ))
        }
        ["task", "artifact", "prune"] => {
            let older = matches
                .get_one::<String>("older-than")
                .map(|value| artifact_age(value))
                .transpose()?;
            let cutoff = older.as_ref().map(cutoff).transpose()?;
            let older_than_seconds = older.as_ref().map(Duration::as_secs);
            let plan = store
                .plan_prune(&snapshot.statuses, cutoff, older_than_seconds)
                .map_err(artifact_failure)?;
            let applied = matches.get_flag("apply");
            if applied {
                store.apply_prune(&plan).map_err(artifact_failure)?;
            }
            Ok((position, ArtifactAnswer::Pruned { plan, applied }))
        }
        _ => unreachable!("artifact dispatcher receives only artifact leaves"),
    }
}

struct Snapshot {
    state: StatePaths,
    cursor: Seq,
    statuses: BTreeMap<TaskHandle, String>,
}

fn snapshot() -> Result<Snapshot, Failure> {
    let working = std::env::current_dir().map_err(|error| {
        Failure::fault(
            FaultClass::Environment,
            "artifact::working-directory-unavailable",
            error.to_string(),
        )
    })?;
    let repository = Repository::discover(&working).map_err(|error| {
        Failure::fault(
            FaultClass::Environment,
            "artifact::repository-unavailable",
            error.to_string(),
        )
    })?;
    let projects = task_project_policy(&repository)?;
    let state = repository.admit_state().map_err(state_failure)?;
    let reader = journal::JournalReader::open(state.journal()).map_err(|error| {
        Failure::fault(
            FaultClass::Store,
            "artifact::journal-unreadable",
            error.to_string(),
        )
    })?;
    let (cursor, heads) = reader
        .read(|read| Ok((read.cursor()?, read.task_heads()?)))
        .map_err(|error| {
            Failure::fault(
                FaultClass::Store,
                "artifact::task-heads-unreadable",
                error.to_string(),
            )
        })?;
    let cursor = cursor.ok_or_else(|| {
        Failure::fault(
            FaultClass::Store,
            "artifact::cursor-missing",
            "journal has no current task snapshot",
        )
    })?;
    let mut statuses = BTreeMap::new();
    for heads in heads {
        let (statement, workflow, claim, lease) = heads.into_bodies();
        let entry =
            board::Entry::new(statement, workflow, claim, lease, &projects).map_err(|error| {
                Failure::fault(
                    FaultClass::Store,
                    "artifact::task-head-invalid",
                    error.to_string(),
                )
            })?;
        statuses.insert(
            entry.statement().handle.clone(),
            entry.workflow().status.clone(),
        );
    }
    Ok(Snapshot {
        state,
        cursor,
        statuses,
    })
}

fn task_project_policy(repository: &Repository) -> Result<board::ProjectPolicy, Failure> {
    let resolved =
        spice::config::load_repository(repository, &[]).map_err(Failure::configuration)?;
    Ok(resolved.root().tasks().project_policy())
}

fn selected_task(matches: &clap::ArgMatches, snapshot: &Snapshot) -> Result<TaskHandle, Failure> {
    let text = matches
        .get_one::<String>("HANDLE")
        .expect("generated artifact parser requires HANDLE");
    let task = text.parse::<TaskHandle>().map_err(|error| {
        Failure::refusal(
            "artifact::task-invalid",
            "one canonical current task handle",
            error.to_string(),
            "read board and pass the exact task handle",
        )
    })?;
    if snapshot.statuses.contains_key(&task) {
        Ok(task)
    } else {
        Err(Failure::refusal(
            "artifact::task-unknown",
            "one canonical current task handle",
            task.to_string(),
            "read board and pass the exact task handle",
        ))
    }
}

fn now() -> Result<Instant, Failure> {
    Instant::new(jiff::Timestamp::now()).map_err(|error| {
        Failure::fault(
            FaultClass::Environment,
            "artifact::clock-invalid",
            error.to_string(),
        )
    })
}

fn artifact_age(word: &str) -> Result<Duration, Failure> {
    let parsed = || {
        let (&suffix, digits) = word.as_bytes().split_last()?;
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let unit = match suffix {
            b's' => 1_u64,
            b'm' => 60,
            b'h' => 60 * 60,
            b'd' => 24 * 60 * 60,
            b'w' => 7 * 24 * 60 * 60,
            _ => return None,
        };
        word[..word.len() - 1]
            .parse::<u64>()
            .ok()?
            .checked_mul(unit)
            .map(Duration::from_secs)
    };
    parsed().ok_or_else(|| {
        Failure::refusal(
            "artifact::age-invalid",
            "an unsigned duration ending in s, m, h, d, or w",
            word,
            "choose a bounded duration such as 30d or omit --older-than",
        )
    })
}

fn cutoff(duration: &Duration) -> Result<Instant, Failure> {
    let duration = jiff::SignedDuration::try_from(*duration).map_err(|error| {
        Failure::refusal(
            "artifact::age-out-of-range",
            "a duration representable by the system clock",
            error.to_string(),
            "choose a smaller --older-than duration",
        )
    })?;
    let cutoff = jiff::Timestamp::now()
        .checked_sub(duration)
        .map_err(|error| {
            Failure::refusal(
                "artifact::age-out-of-range",
                "a duration representable by the system clock",
                error.to_string(),
                "choose a smaller --older-than duration",
            )
        })?;
    Instant::new(cutoff).map_err(|error| {
        Failure::fault(
            FaultClass::Environment,
            "artifact::clock-invalid",
            error.to_string(),
        )
    })
}

fn payload_path(root: &Path, task: &TaskHandle, digest: &wire::Digest) -> PathBuf {
    root.join(task.as_str())
        .join("objects")
        .join(digest.as_str())
        .join("payload")
}

fn artifact_failure(error: artifact::Error) -> Failure {
    match error.class() {
        ErrorClass::Refusal => Failure::refusal(
            "artifact::operation-refused",
            "one admitted task artifact operation",
            error.to_string(),
            "correct the named artifact operand or preview and retry",
        ),
        ErrorClass::Corrupt => Failure::fault(
            FaultClass::Store,
            "artifact::store-corrupt",
            error.to_string(),
        ),
        ErrorClass::Environment => Failure::fault(
            FaultClass::Environment,
            "artifact::store-unavailable",
            error.to_string(),
        ),
    }
}

fn state_failure(error: StateError) -> Failure {
    match error {
        StateError::Filesystem { .. } => Failure::fault(
            FaultClass::Environment,
            "repository::state-path-unavailable",
            error.to_string(),
        ),
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
            witness: Some("task-artifact".to_owned()),
            expected: Some(expected.into()),
            observed: Some(observed.into()),
            remedy: remedy.into(),
        })
    }

    fn fault(class: FaultClass, reason: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Fault(Fault {
            class,
            reason: reason.into(),
            detail: detail.into(),
        })
    }

    fn configuration(error: LoadError) -> Self {
        if let Some(repository) = error.repository() {
            return Self::fault(
                FaultClass::Environment,
                "artifact::repository-unavailable",
                repository.to_string(),
            );
        }
        let diagnostics = configuration_diagnostics(&error);
        if error
            .acquisition()
            .iter()
            .any(|issue| issue.io_error().is_some())
        {
            return Self::fault(
                FaultClass::Environment,
                "artifact::configuration-unreadable",
                diagnostics,
            );
        }
        Self::refusal(
            "artifact::configuration-refused",
            "one admitted tracked task-project policy and machine-local configuration",
            diagnostics,
            "repair the named configuration values before running the artifact command again",
        )
    }

    fn outcome(self) -> Outcome<ArtifactAnswer> {
        match self {
            Self::Refusal(refusal) => Outcome::Refusal(refusal),
            Self::Fault(fault) => Outcome::Fault(fault),
        }
    }
}

fn configuration_diagnostics(error: &LoadError) -> String {
    let mut diagnostics = error
        .acquisition()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if let Some(resolution) = error.resolution() {
        diagnostics.extend(resolution.issues().iter().map(ToString::to_string));
    }
    if diagnostics.is_empty() {
        error.to_string()
    } else {
        diagnostics.join(" | ")
    }
}
