//! Typed room and working-group mutations on the live operator socket.

use super::*;

pub(super) async fn send_group_put(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    group: &wire::rooms::WorkingGroup,
    head: Option<wire::Seq>,
    respond: &crate::client_action::GroupResponse,
) -> Result<CanonicalUuid, String> {
    let mut expected = vec![Witness::Group {
        group: group.group.clone(),
        head,
    }];
    expected.extend(
        group
            .anchors
            .iter()
            .cloned()
            .map(|task| Witness::Existence { task }),
    );
    send_typed(
        principal,
        writer,
        actions,
        PendingKind::WorkingGroup {
            respond: respond.clone(),
        },
        CommandKind::WorkingGroupPut,
        expected,
        wire::rooms::WorkingGroupPut {
            group: group.clone(),
        },
    )
    .await
}

pub(super) async fn send_group_remove(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    group: &wire::rooms::RoomAuthorityStamp,
    head: wire::Seq,
    respond: &crate::client_action::GroupResponse,
) -> Result<CanonicalUuid, String> {
    send_typed(
        principal,
        writer,
        actions,
        PendingKind::WorkingGroup {
            respond: respond.clone(),
        },
        CommandKind::WorkingGroupRemove,
        vec![Witness::Group {
            group: group.clone(),
            head: Some(head),
        }],
        wire::rooms::WorkingGroupRemove {
            group: group.clone(),
        },
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
