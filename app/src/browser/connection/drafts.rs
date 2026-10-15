//! Typed task changes on the live socket: creation, statement revision,
//! filing, a Design's conclusion, and a lane's Design contribution.

use super::*;

/// Send one typed task change under the heads its control was drawn from:
/// task creation, statement revision or filing, a Design's conclusion, or a
/// lane's contribution to an open Design.
pub(super) async fn send_task_change(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    action: &ClientAction,
) -> Result<CanonicalUuid, String> {
    match action {
        ClientAction::Draft { request, respond } => {
            send_draft(principal, writer, actions, request, respond).await
        }
        ClientAction::Conclude {
            request,
            expected,
            respond,
        } => {
            send_typed(
                principal,
                writer,
                actions,
                PendingKind::Conclude {
                    respond: respond.clone(),
                },
                CommandKind::TaskConclude,
                expected.clone(),
                request,
            )
            .await
        }
        ClientAction::Contribute {
            request,
            expected,
            respond,
        } => {
            send_typed(
                principal,
                writer,
                actions,
                PendingKind::Contribute {
                    respond: respond.clone(),
                },
                CommandKind::RoomContribute,
                expected.clone(),
                request,
            )
            .await
        }
        _ => unreachable!("only task changes route here"),
    }
}

async fn send_draft(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    request: &crate::client_action::DraftRequest,
    respond: &crate::client_action::TaskResponse,
) -> Result<CanonicalUuid, String> {
    use crate::client_action::DraftRequest;
    let pending = PendingKind::Draft {
        respond: respond.clone(),
    };
    match request {
        DraftRequest::Create { request, expected } => {
            send_typed(
                principal,
                writer,
                actions,
                pending,
                CommandKind::TaskAdd,
                expected.clone(),
                request,
            )
            .await
        }
        DraftRequest::Edit { request, expected } => {
            send_typed(
                principal,
                writer,
                actions,
                pending,
                CommandKind::TaskEdit,
                expected.clone(),
                request,
            )
            .await
        }
        DraftRequest::File { request, expected } => {
            send_typed(
                principal,
                writer,
                actions,
                pending,
                CommandKind::TaskFile,
                expected.clone(),
                request,
            )
            .await
        }
    }
}
