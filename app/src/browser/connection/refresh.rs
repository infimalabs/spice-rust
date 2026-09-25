//! Demand-driven Canvas current reads and duplicate-request coalescing.

use super::*;

pub(super) async fn send_after_frame(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    submissions: &SubmissionHandle,
    full: bool,
    stale: crate::Stale,
) -> Result<(), String> {
    if full {
        send_refresh(principal, writer, actions, submissions).await?;
    } else {
        send_stale(principal, writer, actions, submissions, stale).await?;
    }

    // A lane read already in flight can coalesce the refresh caused by a wake
    // reply, but that older read cannot prove post-wake residency. Once its
    // response clears it, keep the causal read live until one nominated after
    // the wake is actually in flight.
    if !submissions.queue.borrow().awaiting_residency().is_empty() {
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
    send_settings(principal, writer, actions).await?;
    send_vendors(principal, writer, actions).await
}

/// Refresh only the current reads invalidated by one streamed fact batch.
///
pub(super) async fn send_stale(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    submissions: &SubmissionHandle,
    stale: crate::Stale,
) -> Result<(), String> {
    if stale.board() {
        send_board(principal, writer, actions).await?;
    }
    if stale.lanes() {
        send_lanes(principal, writer, actions, submissions).await?;
    }
    if stale.inbox() {
        send_inbox(principal, writer, actions, submissions).await?;
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
            limit: 50,
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
    let wakes = submissions.queue.borrow().awaiting_residency();
    send_if_offered(
        principal,
        writer,
        actions,
        PendingKind::Lanes { wakes },
        CommandKind::LaneRead,
        LaneRead {},
    )
    .await
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
        SteerInbox { lane: None },
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
