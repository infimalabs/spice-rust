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
    }
}

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
        grooming,
        report_heads,
        respond,
    } = post;
    let mut expected = vec![Witness::Task {
        task: room
            .born()
            .map_err(|error| format!("room anchor is invalid: {error}"))?,
        head: *head,
    }];
    if let Some(grooming) = grooming {
        if grooming.reports.len() != report_heads.len() {
            return Err("OOPS report witnesses do not match the selected sources".into());
        }
        expected.extend(
            grooming
                .reports
                .iter()
                .cloned()
                .zip(report_heads.iter().copied())
                .map(|(task, head)| Witness::Task { task, head }),
        );
    }
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
            grooming: grooming.clone(),
        },
    )
    .await
}

/// The resident guards every item and result head it reads, so a control
/// proposes no witness.
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
        crate::client_action::RoomControlRequest::Close(close) => {
            send_typed(
                principal,
                writer,
                actions,
                pending,
                CommandKind::RoomClose,
                Vec::new(),
                close.clone(),
            )
            .await
        }
    }
}

/// Start, resume, or put aside one room's coordination under the anchor and
/// target heads it was drawn from.
pub(super) async fn send_room_coordination(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    request: &crate::client_action::RoomCoordinationRequest,
    respond: &crate::client_action::RoomCoordinationResponse,
) -> Result<CanonicalUuid, String> {
    let pending = PendingKind::RoomCoordination {
        respond: respond.clone(),
    };
    match request {
        crate::client_action::RoomCoordinationRequest::Coordinate { request, expected } => {
            send_typed(
                principal,
                writer,
                actions,
                pending,
                CommandKind::RoomCoordinate,
                expected.clone(),
                request.clone(),
            )
            .await
        }
        crate::client_action::RoomCoordinationRequest::PutAside { request, expected } => {
            send_typed(
                principal,
                writer,
                actions,
                pending,
                CommandKind::RoomPutAside,
                expected.clone(),
                request.clone(),
            )
            .await
        }
    }
}

/// Submit one exact proposal decision under the heads its preview observed.
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
                request.clone(),
            )
            .await
        }
    }
}
