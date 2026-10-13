//! Typed room and working-group mutations on the live operator socket.

use super::*;

pub(super) async fn send_group_move(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    request: &wire::rooms::WorkingGroupMove,
    revision: u64,
    respond: &crate::client_action::GroupResponse,
) -> Result<CanonicalUuid, String> {
    send_typed(
        principal,
        writer,
        actions,
        PendingKind::WorkingGroup {
            respond: respond.clone(),
        },
        CommandKind::WorkingGroupMove,
        vec![
            Witness::Group { revision },
            Witness::Existence {
                task: request.design.clone(),
            },
        ],
        request.clone(),
    )
    .await
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
