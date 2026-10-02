use base64::Engine as _;

use super::*;

/// Read one task and its notes from one immutable journal barrier.
pub(in crate::board_command) fn read_task_detail(
    bus: &Bus,
    repository: Option<&Repository>,
    detail: &wire::intent::TaskDetail,
    principal: wire::Actor,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let observed = match bus.task_snapshot() {
        Ok(observed) => observed,
        Err(error) => return store_fault("board::heads-unreadable", &error.to_string()),
    };
    let selected = std::collections::BTreeSet::from([detail.task.clone()]);
    let snapshot = match validated_snapshot_from_with_occupant(
        &observed,
        principal,
        projects,
        allocation,
        None,
        Some(&selected),
        false,
    ) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(task) = snapshot.answer.tasks.into_iter().next() else {
        return refusal(
            "task::detail-unknown",
            Some(detail.task.to_string()),
            "read the board and request one current durable task identity",
        );
    };
    let media = match task_media(repository, &detail.task) {
        Ok(media) => media,
        Err(outcome) => return outcome,
    };
    let mut record = task_record(observed.facts(), &selected);
    let notes = record.notes.remove(&detail.task).unwrap_or_default();
    let closure = record.closures.remove(&detail.task);
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::TaskDetail(Box::new(wire::intent::TaskDetailAnswer {
            task,
            media,
            notes,
            closure,
        })),
    )
}

/// Durable history on the named tasks, each task's rows in ascending journal
/// position.
#[derive(Default)]
pub(super) struct TaskRecord {
    /// Every durable note.
    pub(super) notes:
        std::collections::BTreeMap<wire::TaskStamp, Vec<wire::intent::TaskDetailNote>>,
    /// Every task or claim fact whose origin is an acknowledged steering key,
    /// and every operator directive a holder acknowledged while holding it.
    pub(super) acknowledgments:
        std::collections::BTreeMap<wire::TaskStamp, Vec<wire::intent::TaskAcknowledgment>>,
    /// The operator's closure of each closed task; a settled task closes once.
    pub(super) closures:
        std::collections::BTreeMap<wire::TaskStamp, wire::intent::TaskDetailClosure>,
}

/// Read the named tasks' notes, closures, and acknowledgments from the
/// snapshot's facts.
pub(super) fn task_record(
    facts: &bus::TaskFacts,
    tasks: &std::collections::BTreeSet<wire::TaskStamp>,
) -> TaskRecord {
    let mut record = TaskRecord::default();
    for envelope in facts.iter() {
        if let Some(noted) = envelope.fact.read::<body::TaskNoted>()
            && tasks.contains(&noted.task)
        {
            record
                .notes
                .entry(noted.task)
                .or_default()
                .push(wire::intent::TaskDetailNote {
                    position: envelope.seq,
                    at: envelope.at,
                    author: envelope.actor.clone(),
                    text: noted.note,
                });
        }
        if let Some(closed) = envelope.fact.read::<body::TaskClosed>()
            && tasks.contains(&closed.task)
        {
            record.closures.insert(
                closed.task,
                wire::intent::TaskDetailClosure {
                    position: envelope.seq,
                    at: envelope.at,
                    author: envelope.actor.clone(),
                    validation: closed.validation,
                    phase: closed.phase,
                    claimed: closed.claimed,
                },
            );
        }
        let Some(key) = super::task_steering_origin_token(&envelope.origin) else {
            continue;
        };
        let kind = envelope.fact.kind();
        let Some(("task" | "claim", _)) = kind.as_str().split_once("::") else {
            continue;
        };
        let Some(task) = super::fact_task(envelope) else {
            continue;
        };
        if tasks.contains(&task) {
            record.acknowledgments.entry(task).or_default().push(
                wire::intent::TaskAcknowledgment {
                    position: envelope.seq,
                    kind,
                    key: key.to_owned(),
                },
            );
        }
    }
    for (task, steered) in super::steering_during_claims(facts, tasks) {
        let rows = record.acknowledgments.entry(task).or_default();
        rows.extend(steered);
        rows.sort_by_key(|row| row.position);
    }
    record
}

fn task_media(
    repository: Option<&Repository>,
    task: &wire::TaskStamp,
) -> Result<Vec<wire::Media>, Outcome<CommandAnswer>> {
    let Some(repository) = repository else {
        return Ok(Vec::new());
    };
    let state = repository
        .admit_state()
        .map_err(|error| repository_state_failure(&error))?;
    let store = artifact::Store::new(state.artifacts(), state.locks().join("task-artifacts.lock"));
    let mut encoded_bytes = 0_usize;
    let payloads = store
        .show_matching(task, |artifact| {
            let Ok(bytes) = usize::try_from(artifact.bytes) else {
                return false;
            };
            if !artifact.media_type.is_image() || bytes == 0 || bytes > wire::media::MAX_MEDIA_BYTES
            {
                return false;
            }
            let data_url_bytes =
                format!("data:{};base64,", artifact.media_type).len() + bytes.div_ceil(3) * 4;
            let Some(total) = encoded_bytes.checked_add(data_url_bytes) else {
                return false;
            };
            if total > wire::media::MAX_MEDIA_DATA_URL_BYTES {
                return false;
            }
            encoded_bytes = total;
            true
        })
        .map_err(artifact_operation_failure)?;
    let media = payloads
        .into_iter()
        .filter_map(|(artifact, bytes)| {
            wire::Media::from_base64(
                artifact.name,
                artifact.media_type.to_string(),
                &base64::engine::general_purpose::STANDARD.encode(bytes),
            )
            .ok()
        })
        .take(wire::media::MAX_MEDIA_ITEMS)
        .collect::<Vec<_>>();
    debug_assert!(wire::media::admit_media(&media).is_ok());
    Ok(media)
}
