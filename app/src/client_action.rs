use wire::body::DirectiveAttachment;
use wire::intent::{GlobalSettingsSet, TeamArrangement, TeamLife};
use wire::{CanonicalUuid, TaskHandle, TaskStamp};

#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
pub(crate) enum ClientAction {
    /// Wake the connection vendor after a composer synchronously queues work.
    DrainSubmissions,
    GlobalSettings(GlobalSettingsSet),
    TaskDetail {
        task: TaskStamp,
    },
    TaskRelations {
        key: crate::TaskRelationsKey,
    },
    /// Allow or deny one queued consent request. The witnesses are derived
    /// from the queue current when the ruling is sent, not when it was chosen.
    ConsentRuling {
        request: wire::CanonicalUuid,
        answer: wire::Answer,
    },
    /// Approve one exact executable digest required by a current task
    /// statement. The statement identity is revalidated before dispatch.
    TrustGrant {
        task: TaskStamp,
        revision: wire::Seq,
        digest: wire::Digest,
    },
    /// Fetch the next retained page behind one member's oldest visible card.
    MessageHistory {
        generation: u64,
        lane: CanonicalUuid,
        before: Option<wire::MessageLocus>,
    },
    RenewLane {
        topology_revision: u64,
        team: CanonicalUuid,
        lane: CanonicalUuid,
        requested: bool,
        /// The member reservation this renewal settles, when one issued it.
        member: Option<crate::runtime::MemberRequest>,
    },
    /// Wake, drain, or remove one resident member.
    LaneState {
        request: crate::runtime::MemberRequest,
    },
    /// Restate the model one seat's next occupant launches with.
    RetuneLane {
        topology_revision: u64,
        request: crate::runtime::MemberRequest,
    },
    ConfigureTeam {
        topology_revision: u64,
        team: CanonicalUuid,
        member: Option<CanonicalUuid>,
        life: Option<TeamLife>,
        /// Replacement team fast-mode flag, or absent to preserve it.
        fast_mode: Option<bool>,
    },
    /// Where every team and every agent stands after one team drag.
    ///
    /// The whole topology travels because that is the only thing the resident
    /// admits: a drop states an arrangement, not a move, so the surface that
    /// drew the teams is the one that says where they all now stand.
    ArrangeTeams {
        topology_revision: u64,
        teams: Vec<TeamArrangement>,
    },
    Note {
        id: CanonicalUuid,
        task: TaskHandle,
        note: String,
        after: Option<wire::Seq>,
    },
    /// Close one task as completed with the composed text as its validation.
    ///
    /// The witnesses are the heads of the detail the operator closed from, so
    /// a transition or claim landing after that render refuses the closure
    /// instead of settling work the operator never saw.
    Close {
        id: CanonicalUuid,
        task: TaskHandle,
        validation: String,
        expected: Vec<wire::Witness>,
        force_claimed: bool,
    },
    /// Direction the operator aimed at one seated lane from its tile.
    ///
    /// The occupant and epoch travel with the message because the residency
    /// witness the resident re-proves is built from the same inbox snapshot the
    /// tile was drawn from. Recomputing them at the socket would let the tile
    /// name one lane while the command named another.
    Steer {
        id: CanonicalUuid,
        fanout: CanonicalUuid,
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        epoch: CanonicalUuid,
        message: String,
        attachments: Vec<DirectiveAttachment>,
    },
}
