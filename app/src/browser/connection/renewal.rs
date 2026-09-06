//! Typed command dispatch for one member's agent-renewal state machine.

use super::*;

pub(super) struct RenewalRequest {
    topology_revision: u64,
    lineage: CanonicalUuid,
    lane: CanonicalUuid,
    requested: bool,
}

impl RenewalRequest {
    pub(super) const fn new(
        topology_revision: u64,
        lineage: CanonicalUuid,
        lane: CanonicalUuid,
        requested: bool,
    ) -> Self {
        Self {
            topology_revision,
            lineage,
            lane,
            requested,
        }
    }
}

pub(super) async fn send_lane_renewal(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    request: RenewalRequest,
) -> Result<(), String> {
    send_lane_renewal_as(principal, writer, pending, random_uuid()?, request).await
}

pub(super) async fn send_lane_renewal_as(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    id: CanonicalUuid,
    request: RenewalRequest,
) -> Result<(), String> {
    let RenewalRequest {
        topology_revision,
        lineage,
        lane,
        requested,
    } = request;
    send_typed_as(
        principal,
        writer,
        pending,
        id,
        PendingKind::LaneRenewal {
            lineage,
            lane,
            requested,
        },
        CommandKind::LaneRenewal,
        vec![],
        LaneRenewal {
            topology_revision,
            lineage_id: lineage,
            lane,
            requested,
        },
    )
    .await
}
