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
pub(crate) type RoomCoordinationResponse =
    std::rc::Rc<dyn Fn(Result<wire::intent::RoomCoordinationView, String>)>;
pub(crate) type PreparationResponse = std::rc::Rc<dyn Fn(Result<wire::intent::TextAnswer, String>)>;
pub(crate) type TaskResponse = std::rc::Rc<dyn Fn(Result<wire::intent::TaskAnswer, String>)>;
pub(crate) type ContributeResponse =
    std::rc::Rc<dyn Fn(Result<wire::intent::RoomContributeAnswer, String>)>;

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

/// One room's coordination: a Start or Resume asking one occupied seat for a
/// bounded contribution, or putting the room aside.
#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
pub(crate) enum RoomCoordinationRequest {
    /// The request, and the anchor and target heads it was drawn from.
    Coordinate {
        request: wire::intent::RoomCoordinate,
        expected: Vec<wire::Witness>,
    },
    /// Put the room aside at the anchor heads it was drawn from.
    PutAside {
        request: wire::intent::RoomPutAside,
        expected: Vec<wire::Witness>,
    },
}

/// One review-evidence record over a retained candidate, with the heads the
/// browser drew it from.
#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
pub(crate) enum PreparationRequest {
    Cover {
        request: wire::rooms::CandidateCoverageRecord,
        expected: Vec<wire::Witness>,
    },
}

/// One operator Draft creation, revision, or filing, with the heads it was
/// drawn from.
#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
pub(crate) enum DraftRequest {
    Create {
        request: wire::intent::TaskAdd,
        expected: Vec<wire::Witness>,
    },
    Edit {
        request: wire::intent::TaskEdit,
        expected: Vec<wire::Witness>,
    },
    File {
        request: wire::intent::TaskFile,
        expected: Vec<wire::Witness>,
    },
}

/// What one working-group change asks the authority for.
#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
pub(crate) enum GroupChange {
    /// Move one design into a group, founding it under a fresh identity, or
    /// out of every group.
    Move(wire::rooms::WorkingGroupMove),
    /// Restate every group and its designs in order.
    Arrange(wire::rooms::WorkingGroupArrange),
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
    /// One working-group move or arrangement at the observed partition
    /// revision.
    WorkingGroupChange {
        change: GroupChange,
        revision: u64,
        respond: GroupResponse,
    },
    /// Publish operator-authored text or one typed OOPS disposition to a task room.
    RoomPost(RoomPostAction),
    /// Settle the operator's own room receipts and results, or close a question.
    RoomControl {
        request: RoomControlRequest,
        respond: RoomControlResponse,
    },
    /// Start, resume, or put aside one room's coordination.
    RoomCoordination {
        request: RoomCoordinationRequest,
        respond: RoomCoordinationResponse,
    },
    /// Decide a Design's retained candidate or record review evidence over it.
    Preparation {
        request: PreparationRequest,
        respond: PreparationResponse,
    },
    /// Create, revise, or file one operator Draft.
    Draft {
        request: DraftRequest,
        respond: TaskResponse,
    },
    /// Conclude one open Design done or collapsed at the heads its view was
    /// drawn from, so a claim or transition landing since refuses it.
    Conclude {
        request: wire::intent::TaskConclude,
        expected: Vec<wire::Witness>,
        respond: TaskResponse,
    },
    /// Contribute one typed record to an open Design as this lane, at the
    /// Design and source heads its preview was drawn from.
    Contribute {
        request: wire::intent::RoomContribute,
        expected: Vec<wire::Witness>,
        respond: ContributeResponse,
    },
    /// Wake the connection vendor after a composer synchronously queues work.
    DrainSubmissions,
    GlobalSettings(GlobalSettingsSet),
    /// Wake the connection to send task detail reads the runtime owes.
    TaskDetails,
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
