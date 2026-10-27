//! Typed room and working-group mutations on the live operator socket.

use super::*;

pub(super) async fn send_group_change(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    change: &crate::client_action::GroupChange,
    revision: u64,
    respond: &crate::client_action::GroupResponse,
) -> Result<CanonicalUuid, String> {
    let pending = PendingKind::WorkingGroup {
        respond: respond.clone(),
    };
    let partition = Witness::Group { revision };
    match change {
        crate::client_action::GroupChange::Move(request) => {
            send_typed(
                principal,
                writer,
                actions,
                pending,
                CommandKind::WorkingGroupMove,
                vec![
                    partition,
                    Witness::Existence {
                        task: request.design.clone(),
                    },
                ],
                request.clone(),
            )
            .await
        }
        crate::client_action::GroupChange::Arrange(request) => {
            send_typed(
                principal,
                writer,
                actions,
                pending,
                CommandKind::WorkingGroupArrange,
                vec![partition],
                request.clone(),
            )
            .await
        }
        crate::client_action::GroupChange::Disband(request) => {
            send_typed(
                principal,
                writer,
                actions,
                pending,
                CommandKind::WorkingGroupDisband,
                vec![partition],
                request.clone(),
            )
            .await
        }
        crate::client_action::GroupChange::Fast(request) => {
            send_typed(
                principal,
                writer,
                actions,
                pending,
                CommandKind::WorkingGroupFast,
                vec![partition],
                request.clone(),
            )
            .await
        }
    }
}

/// Post operator text to one room under the anchor head its composer read.
pub(super) async fn send_room_post(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    post: &crate::client_action::RoomPostAction,
) -> Result<CanonicalUuid, String> {
    let crate::client_action::RoomPostAction {
        room,
        head,
        text,
        respond,
    } = post;
    let expected = vec![Witness::Task {
        task: room
            .born()
            .map_err(|error| format!("room anchor is invalid: {error}"))?,
        head: *head,
    }];
    send_typed(
        principal,
        writer,
        actions,
        PendingKind::RoomPost {
            respond: respond.clone(),
        },
        CommandKind::RoomPost,
        expected,
        wire::rooms::RoomPost {
            room: room.clone(),
            text: text.to_owned(),
        },
    )
    .await
}

/// The resident guards every item head it reads, so a control proposes no
/// witness.
pub(super) async fn send_room_control(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    request: &crate::client_action::RoomControlRequest,
    respond: &crate::client_action::RoomControlResponse,
) -> Result<CanonicalUuid, String> {
    let pending = PendingKind::RoomControl {
        respond: respond.clone(),
    };
    match request {
        crate::client_action::RoomControlRequest::Control(control) => {
            send_typed(
                principal,
                writer,
                actions,
                pending,
                CommandKind::RoomControl,
                Vec::new(),
                control.clone(),
            )
            .await
        }
    }
}

/// Submit one preparation write under the heads its surface observed.
pub(super) async fn send_preparation(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    request: &crate::client_action::PreparationRequest,
    respond: &crate::client_action::PreparationResponse,
) -> Result<CanonicalUuid, String> {
    let pending = PendingKind::Preparation {
        respond: respond.clone(),
    };
    match request {
        crate::client_action::PreparationRequest::Decide { request, expected } => {
            send_typed(
                principal,
                writer,
                actions,
                pending,
                CommandKind::PreparationDecide,
                expected.clone(),
                (**request).clone(),
            )
            .await
        }
        crate::client_action::PreparationRequest::Nudge { request, expected } => {
            send_typed(
                principal,
                writer,
                actions,
                pending,
                CommandKind::DesignNudge,
                expected.clone(),
                request.clone(),
            )
            .await
        }
    }
}
