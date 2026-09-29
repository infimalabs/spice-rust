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
    let notes = task_notes(&observed, &selected)
        .remove(&detail.task)
        .unwrap_or_default();
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::TaskDetail(Box::new(wire::intent::TaskDetailAnswer {
            task,
            media,
            notes,
        })),
    )
}

/// Every durable note on the named tasks, each task's in ascending journal
/// position, read in one pass over the snapshot's facts.
pub(super) fn task_notes(
    observed: &bus::TaskSnapshot,
    tasks: &std::collections::BTreeSet<wire::TaskStamp>,
) -> std::collections::BTreeMap<wire::TaskStamp, Vec<wire::intent::TaskDetailNote>> {
    let mut notes = std::collections::BTreeMap::<_, Vec<_>>::new();
    for envelope in observed.facts().iter() {
        let Some(noted) = envelope.fact.read::<body::TaskNoted>() else {
            continue;
        };
        if tasks.contains(&noted.task) {
            notes
                .entry(noted.task)
                .or_default()
                .push(wire::intent::TaskDetailNote {
                    position: envelope.seq,
                    at: envelope.at,
                    author: envelope.actor.clone(),
                    text: noted.note,
                });
        }
    }
    notes
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
