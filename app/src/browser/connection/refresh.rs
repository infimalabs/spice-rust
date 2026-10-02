//! Demand-driven app current reads and duplicate-request coalescing.

use super::*;
use crate::action_mailbox::CurrentRead;

pub(super) async fn send_after_frame(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    submissions: &SubmissionHandle,
    full: bool,
    stale: crate::Stale,
) -> Result<(), String> {
    send_stale(principal, writer, actions, submissions, stale).await?;
    if full {
        send_refresh(principal, writer, actions, submissions).await?;
    }

    // A lane read already in flight can coalesce a causal refresh, but it
    // cannot prove a wake's residency or activity observed by a newer inbox.
    // Keep the requirement live until a post-observation lane read starts.
    let causal_lane_read = {
        let queue = submissions.queue.borrow();
        !queue.awaiting_residency().is_empty() || queue.activity_read_requirement().is_some()
    };
    if causal_lane_read {
        send_lanes(principal, writer, actions, submissions).await?;
    }
    Ok(())
}

pub(super) async fn send_refresh(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    submissions: &SubmissionHandle,
) -> Result<(), String> {
    send_board(principal, writer, actions).await?;
    send_lanes(principal, writer, actions, submissions).await?;
    send_inbox(principal, writer, actions, submissions).await?;
    send_consent(principal, writer, actions).await?;
    send_settings(principal, writer, actions).await?;
    send_vendors(principal, writer, actions).await
}

/// Remember each invalidated current read until every older request for it has
/// settled, then issue exactly one replacement from the websocket callback.
pub(super) async fn send_stale(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    submissions: &SubmissionHandle,
    stale: crate::Stale,
) -> Result<(), String> {
    actions.defer_stale_current_reads(&stale);

    if actions.take_ready_current_read(CurrentRead::Board) {
        send_board(principal, writer, actions).await?;
    }
    if actions.take_ready_current_read(CurrentRead::Lanes) {
        send_lanes(principal, writer, actions, submissions).await?;
    }
    if actions.take_ready_current_read(CurrentRead::Inbox) {
        send_inbox(principal, writer, actions, submissions).await?;
    }
    if actions.take_ready_current_read(CurrentRead::Consent) {
        send_consent(principal, writer, actions).await?;
    }
    for task in stale.task_details() {
        let _ = send_task_detail(principal, writer, actions, task).await?;
    }
    Ok(())
}

/// Re-read every retained task detail after a reconnect; facts published
/// while the socket was down were never streamed to invalidate them.
pub(super) async fn send_task_details(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    data: &SurfaceData,
) -> Result<(), String> {
    let tasks: Vec<wire::TaskStamp> = data
        .read()
        .as_ref()
        .map(|runtime| runtime.task_detail_reads().cloned().collect())
        .unwrap_or_default();
    for task in &tasks {
        let _ = send_task_detail(principal, writer, actions, task).await?;
    }
    Ok(())
}

async fn send_board(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
) -> Result<(), String> {
    send_if_offered(
        principal,
        writer,
        actions,
        PendingKind::Board,
        CommandKind::BoardRead,
        BoardRead::Open,
    )
    .await
}
pub(super) async fn send_direction_history(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    request: &crate::runtime::DirectionHistoryRequest,
) -> Result<Option<CanonicalUuid>, String> {
    send_typed(
        principal,
        writer,
        actions,
        PendingKind::DirectionHistory {
            request: request.clone(),
        },
        CommandKind::SteerHistory,
        vec![],
        SteerHistory {
            window: request.window.clone(),
            query: request.query.clone(),
            lanes: request.lanes.clone(),
            dispositions: request.dispositions.clone(),
            authored_before: request.authored_before,
            limit: crate::runtime::DIRECTION_HISTORY_PAGE_SIZE,
        },
    )
    .await
    .map(Some)
}

pub(super) async fn send_work_history(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    request: &crate::runtime::WorkHistoryRequest,
) -> Result<Option<CanonicalUuid>, String> {
    send_typed(
        principal,
        writer,
        actions,
        PendingKind::WorkHistory {
            request: request.clone(),
        },
        CommandKind::BoardRead,
        vec![],
        BoardRead::History {
            before: request.before.clone(),
            query: request.query.clone(),
            completed_before: request.completed_before,
            team: None,
            limit: 50,
        },
    )
    .await
    .map(Some)
}
/// The newest settled page for one team, or the page behind `request.before`.
pub(super) async fn send_team_settled(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    request: &crate::runtime::TeamSettledRequest,
) -> Result<Option<CanonicalUuid>, String> {
    send_typed(
        principal,
        writer,
        actions,
        PendingKind::TeamSettled {
            request: request.clone(),
        },
        CommandKind::BoardRead,
        vec![],
        BoardRead::History {
            query: request.query.clone(),
            completed_before: request.completed_before,
            before: request.before.clone(),
            team: Some(request.team),
            limit: 20,
        },
    )
    .await
    .map(Some)
}
pub(super) async fn send_task_detail(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    task: &wire::TaskStamp,
) -> Result<Option<CanonicalUuid>, String> {
    if !SteerPane::over(principal).offers(CommandKind::TaskDetail) || actions.has_task_detail(task)
    {
        return Ok(None);
    }
    send_typed(
        principal,
        writer,
        actions,
        PendingKind::TaskDetail { task: task.clone() },
        CommandKind::TaskDetail,
        vec![],
        wire::intent::TaskDetail { task: task.clone() },
    )
    .await
    .map(Some)
}

pub(super) async fn send_task_relations(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    key: &crate::TaskRelationsKey,
) -> Result<Option<CanonicalUuid>, String> {
    if !SteerPane::over(principal).offers(CommandKind::TaskRelations)
        || actions.has_task_relations(key)
    {
        return Ok(None);
    }
    send_typed(
        principal,
        writer,
        actions,
        PendingKind::TaskRelations { key: key.clone() },
        CommandKind::TaskRelations,
        vec![],
        key.request(),
    )
    .await
    .map(Some)
}

pub(super) async fn send_lanes(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    submissions: &SubmissionHandle,
) -> Result<(), String> {
    let (wakes, activity_generation) = {
        let queue = submissions.queue.borrow();
        (
            queue.awaiting_residency(),
            queue.activity_read_requirement(),
        )
    };
    if !SteerPane::over(principal).offers(CommandKind::LaneRead)
        || actions.has_current_read(CurrentRead::Lanes)
    {
        return Ok(());
    }
    send_typed(
        principal,
        writer,
        actions,
        PendingKind::Lanes { wakes },
        CommandKind::LaneRead,
        vec![],
        LaneRead {},
    )
    .await?;
    submissions
        .queue
        .borrow_mut()
        .activity_read_started(activity_generation);
    Ok(())
}

async fn send_inbox(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    submissions: &SubmissionHandle,
) -> Result<(), String> {
    let covered = submissions.queue.borrow().accepted();
    send_if_offered(
        principal,
        writer,
        actions,
        PendingKind::Inbox { covered },
        CommandKind::SteerInbox,
        SteerInbox {
            lane: None,
            retired: true,
        },
    )
    .await
}

/// The operator's consent queue; a lane principal is not offered it.
async fn send_consent(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
) -> Result<(), String> {
    send_if_offered(
        principal,
        writer,
        actions,
        PendingKind::Consent,
        CommandKind::ConsentRead,
        wire::intent::ConsentRead {},
    )
    .await
}

/// One operator ruling, carrying the witnesses its queue snapshot implied.
pub(super) async fn send_consent_ruling(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    ruling: &wire::intent::ConsentDecide,
    expected: &[wire::Witness],
) -> Result<CanonicalUuid, String> {
    send_typed(
        principal,
        writer,
        actions,
        PendingKind::ConsentRuling {
            request: ruling.request,
        },
        CommandKind::ConsentDecide,
        expected.to_vec(),
        ruling.clone(),
    )
    .await
}

async fn send_settings(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
) -> Result<(), String> {
    send_if_offered(
        principal,
        writer,
        actions,
        PendingKind::Settings,
        CommandKind::GlobalSettingsRead,
        GlobalSettingsRead {},
    )
    .await
}

async fn send_vendors(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
) -> Result<(), String> {
    send_if_offered(
        principal,
        writer,
        actions,
        PendingKind::Vendors,
        CommandKind::VendorCatalog,
        VendorCatalog {},
    )
    .await
}

async fn send_if_offered<B: Serialize>(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    pending_kind: PendingKind,
    kind: CommandKind,
    body: B,
) -> Result<(), String> {
    if !SteerPane::over(principal).offers(kind) {
        return Ok(());
    }
    if let Some(read) = pending_kind.current_read()
        && actions.has_current_read(read)
    {
        return Ok(());
    }
    send_typed(principal, writer, actions, pending_kind, kind, vec![], body)
        .await
        .map(|_| ())
}
