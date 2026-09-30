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
    TaskTimeline {
        request: crate::runtime::TaskTimelineRequest,
    },
    WorkHistory {
        request: crate::runtime::WorkHistoryRequest,
    },
    /// Read one page of the work one team recently settled.
    TeamSettled {
        request: crate::runtime::TeamSettledRequest,
    },
    /// Allow or deny one queued consent request. The witnesses are derived
    /// from the queue current when the ruling is sent, not when it was chosen.
    ConsentRuling {
        request: wire::CanonicalUuid,
        answer: wire::Answer,
    },
    /// Fetch the next retained page behind one member's oldest visible card.
    MessageHistory {
        generation: u64,
        lane: CanonicalUuid,
        before: Option<wire::MessageLocus>,
    },
    Seat {
        topology_revision: u64,
        lane: CanonicalUuid,
        name: Option<String>,
        worktree: String,
        life: String,
        vendor: wire::Name,
        model: Option<String>,
        effort: Option<String>,
    },
    RenewLane {
        topology_revision: u64,
        team: CanonicalUuid,
        lane: CanonicalUuid,
        requested: bool,
    },
    ConfigureTeam {
        topology_revision: u64,
        team: CanonicalUuid,
        member: Option<CanonicalUuid>,
        life: Option<TeamLife>,
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
