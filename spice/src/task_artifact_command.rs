use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use artifact::{AddRequest, ArtifactAnswer, ArtifactContent, ErrorClass, Retention, Store};
use spice::cli::Contract;
use spice::config::LoadError;
use spice::repository::{Repository, StateError, StatePaths};
use wire::{
    Fault, FaultClass, Instant, Outcome, Refusal, Reply, ReplyPosition, Seq, TaskHandle, TaskStamp,
};

pub(crate) fn run(
    matches: &clap::ArgMatches,
    contract: &Contract,
    configuration: Option<&spice::config::RepositoryCapture>,
) -> Outcome<ArtifactAnswer> {
    match run_inner(matches, contract, configuration) {
        Ok((position, body)) => Outcome::Reply(Reply { position, body }),
        Err(failure) => failure.outcome(),
    }
}

fn run_inner(
    matches: &clap::ArgMatches,
    contract: &Contract,
    configuration: Option<&spice::config::RepositoryCapture>,
) -> Result<(ReplyPosition, ArtifactAnswer), Failure> {
    let snapshot = snapshot(configuration)?;
    let store = Store::new(
        snapshot.state.artifacts(),
        snapshot.state.locks().join("task-artifacts.lock"),
    );
    let position = ReplyPosition::Snapshot(snapshot.cursor);
    match contract.path {
        ["task", "artifact", "add"] => {
            let selected = selected_task(matches, &snapshot)?;
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
                    task: &selected.born,
                    source: Path::new(source),
                    name: matches.get_one::<String>("name").map(String::as_str),
                    media_type: matches.get_one::<String>("type").map(String::as_str),
                    retention,
                    created_at,
                })
                .map_err(artifact_failure)?;
            Ok((
                position,
                ArtifactAnswer::Added {
                    task: selected.handle,
                    artifact,
                },
            ))
        }
        ["task", "artifact", "list"] => {
            let selected = selected_task(matches, &snapshot)?;
            let artifacts = store.list(&selected.born).map_err(artifact_failure)?;
            Ok((
                position,
                ArtifactAnswer::Listed {
                    task: selected.handle,
                    artifacts,
                },
            ))
        }
        ["task", "artifact", "show"] => {
            let selected = selected_task(matches, &snapshot)?;
            let artifact_id = matches
                .get_one::<String>("ARTIFACT")
                .expect("generated artifact show parser requires ARTIFACT");
            let (artifact, bytes) = store
                .show(&selected.born, artifact_id)
                .map_err(artifact_failure)?;
            let content = if artifact.media_type.is_text() {
                ArtifactContent::Text {
                    text: String::from_utf8(bytes).expect("store proved text artifact UTF-8"),
                }
            } else {
                ArtifactContent::Binary {
                    path: payload_path(store.root(), &selected.born, &artifact.digest)
                        .display()
                        .to_string(),
                }
            };
            Ok((
                position,
                ArtifactAnswer::Shown {
                    task: selected.handle,
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
    statuses: BTreeMap<TaskStamp, (TaskHandle, String)>,
}

fn snapshot(configuration: Option<&spice::config::RepositoryCapture>) -> Result<Snapshot, Failure> {
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
    let projects = task_project_policy(&repository, configuration)?;
    let state = repository.admit_state().map_err(state_failure)?;
    let journal = journal::JournalReader::open(state.journal()).map_err(|error| {
        Failure::fault(
            FaultClass::Store,
            "artifact::journal-unreadable",
            error.to_string(),
        )
    })?;
    // Keep every bounded page and the reply cursor in one read-only snapshot.
    // The transaction ends before artifact payload I/O or store locking.
    let (cursor, statuses) = journal
        .read_with(|read| {
            let cursor = read.cursor().map_err(task_heads_failure)?.ok_or_else(|| {
                Failure::fault(
                    FaultClass::Store,
                    "artifact::cursor-missing",
                    "journal has no current task snapshot",
                )
            })?;
            let mut statuses = BTreeMap::new();
            let mut after = None;
            loop {
                let page = read
                    .task_head_page(after.as_ref(), journal::FactReadBudget::RETAINED)
                    .map_err(task_heads_failure)?;
                if page.witness.cursor != Some(cursor) {
                    return Err(task_heads_failure(journal::GuardError::Invalid(
                        "task snapshot authority moved between bounded pages".to_owned(),
                    )));
                }
                for heads in &page.rows {
                    let entry = board::Entry::new(
                        heads.statement().body().clone(),
                        heads.workflow().body().clone(),
                        heads.claim().map(|head| head.body().clone()),
                        heads.lease().map(|head| head.body().clone()),
                        &projects,
                    )
                    .map_err(|error| {
                        Failure::fault(
                            FaultClass::Store,
                            "artifact::task-head-invalid",
                            error.to_string(),
                        )
                    })?;
                    statuses.insert(
                        entry.born().clone(),
                        (entry.handle().clone(), entry.workflow().status.clone()),
                    );
                }
                after = page.continuation;
                if after.is_none() {
                    break;
                }
            }
            Ok((cursor, statuses))
        })
        .map_err(|error| match error {
            journal::ReadError::Journal(error) => task_heads_failure(error),
            journal::ReadError::Answer(failure) => failure,
        })?;
    Ok(Snapshot {
        state,
        cursor,
        statuses,
    })
}

fn task_heads_failure(error: journal::GuardError) -> Failure {
    Failure::fault(
        FaultClass::Store,
        "artifact::task-heads-unreadable",
        error.to_string(),
    )
}

fn task_project_policy(
    repository: &Repository,
    configuration: Option<&spice::config::RepositoryCapture>,
) -> Result<board::ProjectPolicy, Failure> {
    let loaded = spice::config::capture_repository_scoped(repository, &[], configuration)
        .map_err(Failure::configuration)?;
    Ok(loaded.resolved().root().tasks().project_policy())
}

struct SelectedTask {
    handle: TaskHandle,
    born: TaskStamp,
}

fn selected_task(matches: &clap::ArgMatches, snapshot: &Snapshot) -> Result<SelectedTask, Failure> {
    let text = matches
        .get_one::<String>("HANDLE")
        .expect("generated artifact parser requires HANDLE");
    let task = text.parse::<TaskHandle>().map_err(|error| {
        Failure::refusal(
            "artifact::task-invalid",
            "one canonical task handle",
            error.to_string(),
            "read board and pass the task handle",
        )
    })?;
    let born = task.born().map_err(|error| {
        Failure::refusal(
            "artifact::task-invalid",
            "one handle carrying a canonical birth stamp",
            error.to_string(),
            "read board and pass the task handle",
        )
    })?;
    match snapshot.statuses.get(&born) {
        Some((current, _)) => {
            if let Some(notice) = crate::board_command::rename_notice(&task, current) {
                eprintln!("{notice}");
            }
            Ok(SelectedTask {
                handle: current.clone(),
                born,
            })
        }
        None => Err(Failure::refusal(
            "artifact::task-unknown",
            "one existing task",
            task.to_string(),
            "read board and pass an existing task handle",
        )),
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

fn payload_path(root: &Path, task: &TaskStamp, digest: &wire::Digest) -> PathBuf {
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
        | StateError::NotRegularFile { .. } => Failure::refusal(
            "repository::state-path-refused",
            "real fixed components beneath the canonical Git-common .spice-rs directory",
            error.to_string(),
            format!(
                "repair the malformed state component at {}",
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
        detail: impl Into<String>,
    ) -> Self {
        Self::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                reason.into(),
                Some("task-artifact".to_owned()),
                Some(expected.into()),
                Some(observed.into()),
            )
            .with_detail(detail),
        ))
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
