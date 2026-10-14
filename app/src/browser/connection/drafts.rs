//! Typed Draft creation, revision, and filing on the live operator socket.

use super::*;

/// Create, revise, or file one Draft under the heads its control was drawn
/// from.
pub(super) async fn send_draft(
    principal: &Principal,
    writer: &mut SocketWriter,
    actions: &ActionQueue,
    request: &crate::client_action::DraftRequest,
    respond: &crate::client_action::DraftResponse,
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
