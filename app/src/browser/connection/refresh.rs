//! Demand-driven Canvas current reads and duplicate-request coalescing.

use super::*;

pub(super) async fn send_after_frame(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    submissions: &SubmissionHandle,
    full: bool,
    stale: crate::Stale,
) -> Result<(), String> {
    if full {
        send_refresh(principal, writer, pending, submissions).await?;
    } else {
        send_stale(principal, writer, pending, submissions, stale).await?;
    }

    // A lane read already in flight can coalesce the refresh caused by a wake
    // reply, but that older read cannot prove post-wake residency. Once its
    // response clears it, keep the causal read live until one nominated after
    // the wake is actually in flight.
    if !submissions.queue.borrow().awaiting_residency().is_empty() {
        send_lanes(principal, writer, pending, submissions).await?;
    }
    Ok(())
}

pub(super) async fn send_refresh(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    submissions: &SubmissionHandle,
) -> Result<(), String> {
    send_board(principal, writer, pending).await?;
    send_lanes(principal, writer, pending, submissions).await?;
    send_inbox(principal, writer, pending, submissions).await?;
    send_consent(principal, writer, pending).await?;
    send_settings(principal, writer, pending).await?;
    send_policy(principal, writer, pending).await
}

/// Refresh only the current reads invalidated by one streamed fact batch.
///
pub(super) async fn send_stale(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    submissions: &SubmissionHandle,
    stale: crate::Stale,
) -> Result<(), String> {
    if stale.board() {
        send_board(principal, writer, pending).await?;
    }
    if stale.lanes() {
        send_lanes(principal, writer, pending, submissions).await?;
    }
    if stale.inbox() {
        send_inbox(principal, writer, pending, submissions).await?;
    }
    if stale.consent() {
        send_consent(principal, writer, pending).await?;
    }
    Ok(())
}

async fn send_board(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
) -> Result<(), String> {
    send_if_offered(
        principal,
        writer,
        pending,
        PendingKind::Board,
        CommandKind::BoardRead,
        BoardRead { family: None },
    )
    .await
}

async fn send_lanes(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    submissions: &SubmissionHandle,
) -> Result<(), String> {
    let wakes = submissions.queue.borrow().awaiting_residency();
    send_if_offered(
        principal,
        writer,
        pending,
        PendingKind::Lanes { wakes },
        CommandKind::LaneRead,
        LaneRead {},
    )
    .await
}

async fn send_inbox(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    submissions: &SubmissionHandle,
) -> Result<(), String> {
    let covered = submissions.queue.borrow().accepted();
    send_if_offered(
        principal,
        writer,
        pending,
        PendingKind::Inbox { covered },
        CommandKind::SteerInbox,
        SteerInbox { lane: None },
    )
    .await
}

async fn send_consent(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
) -> Result<(), String> {
    send_if_offered(
        principal,
        writer,
        pending,
        PendingKind::Consent,
        CommandKind::ConsentRead,
        ConsentRead {},
    )
    .await
}

async fn send_policy(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
) -> Result<(), String> {
    send_if_offered(
        principal,
        writer,
        pending,
        PendingKind::Policy,
        CommandKind::ConsentPolicy,
        ConsentPolicy {},
    )
    .await
}

async fn send_settings(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
) -> Result<(), String> {
    send_if_offered(
        principal,
        writer,
        pending,
        PendingKind::Settings,
        CommandKind::GlobalSettingsRead,
        GlobalSettingsRead {},
    )
    .await
}

async fn send_if_offered<B: Serialize>(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    pending_kind: PendingKind,
    kind: CommandKind,
    body: B,
) -> Result<(), String> {
    if !SteerPane::over(principal).offers(kind) {
        return Ok(());
    }
    if let Some(read) = pending_kind.current_read()
        && pending
            .values()
            .any(|pending| pending.current_read() == Some(read))
    {
        return Ok(());
    }
    send_typed(principal, writer, pending, pending_kind, kind, vec![], body).await
}
