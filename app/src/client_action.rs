use wire::body::DirectiveAttachment;
use wire::intent::{GlobalSettingsSet, TeamArrangement, TeamLife};
use wire::{CanonicalUuid, TaskHandle, TaskStamp};

pub(crate) type JobResponse = std::rc::Rc<dyn Fn(Result<wire::intent::LaneJobAnswer, String>)>;
pub(crate) type JobsResponse = std::rc::Rc<dyn Fn(Result<wire::intent::LaneJobsAnswer, String>)>;
pub(crate) type GroupResponse =
    std::rc::Rc<dyn Fn(Result<wire::intent::WorkingGroupChange, String>)>;
pub(crate) type RoomPostResponse = std::rc::Rc<dyn Fn(Result<wire::rooms::RoomPostAnswer, String>)>;
pub(crate) type RoomControlResponse =
    std::rc::Rc<dyn Fn(Result<wire::intent::RoomControlAnswer, String>)>;

/// One complete room publication request retained by the bounded action mailbox.
#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
pub(crate) struct RoomPostAction {
    pub(crate) room: TaskHandle,
    pub(crate) head: wire::Seq,
    pub(crate) text: String,
    pub(crate) grooming: Option<wire::rooms::OopsGrooming>,
    /// Current statement heads aligned with `grooming.reports`.
    pub(crate) report_heads: Vec<wire::Seq>,
    pub(crate) respond: RoomPostResponse,
}

/// One operator receipt, answer, or recovery, or one question closure.
#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
pub(crate) enum RoomControlRequest {
    Control(wire::intent::RoomControl),
    Close(wire::intent::RoomClose),
}

#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
pub(crate) enum ClientAction {
    Jobs {
        respond: JobsResponse,
    },
    Job {
        target: wire::intent::LaneJobRef,
        stop: bool,
        respond: JobResponse,
    },
    /// Store one repository-shared working group's design order; an empty
    /// order ends it, and each named design leaves any other group.
    WorkingGroupPut {
        group: wire::rooms::WorkingGroup,
        head: Option<wire::Seq>,
        respond: GroupResponse,
    },
    /// Publish operator-authored text or one typed OOPS disposition to a task room.
    RoomPost(RoomPostAction),
    /// Settle the operator's own room receipts and results, or close a question.
    RoomControl {
        request: RoomControlRequest,
        respond: RoomControlResponse,
    },
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
