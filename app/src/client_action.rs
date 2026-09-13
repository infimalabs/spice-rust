use wire::body::DirectiveAttachment;
use wire::intent::{LineageArrangement, LineageLife};
use wire::{CanonicalUuid, TaskHandle};

#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
pub(crate) enum ClientAction {
    /// Wake the connection vendor after a composer synchronously queues work.
    DrainSubmissions,
    Refresh,
    FastMode(bool),
    /// Fetch the next retained page behind one member's oldest visible card.
    MessageHistory {
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
    },
    ArrangeLane {
        topology_revision: u64,
        lane: CanonicalUuid,
        before: CanonicalUuid,
    },
    RenewLane {
        topology_revision: u64,
        lineage: CanonicalUuid,
        lane: CanonicalUuid,
        requested: bool,
    },
    ConfigureLineage {
        topology_revision: u64,
        lineage: CanonicalUuid,
        member: Option<CanonicalUuid>,
        life: Option<LineageLife>,
    },
    /// Where every organization and every agent stands after one organization drag.
    ///
    /// The whole topology travels because that is the only thing the resident
    /// admits: a drop states an arrangement, not a move, so the surface that
    /// drew the organizations is the one that says where they all now stand.
    ArrangeLineages {
        topology_revision: u64,
        lineages: Vec<LineageArrangement>,
    },
    Note {
        id: CanonicalUuid,
        task: TaskHandle,
        note: String,
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
