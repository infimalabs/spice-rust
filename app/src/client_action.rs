use wire::body::DirectiveAttachment;
use wire::intent::{GlobalSettingsSet, TeamArrangement, TeamLife};
use wire::{CanonicalUuid, TaskHandle, TaskStamp};

pub(crate) type JobResponse = std::rc::Rc<dyn Fn(Result<wire::intent::LaneJobAnswer, String>)>;
pub(crate) type JobsResponse = std::rc::Rc<dyn Fn(Result<wire::intent::LaneJobsAnswer, String>)>;
pub(crate) type GroupResponse =
    std::rc::Rc<dyn Fn(Result<wire::intent::WorkingGroupChange, String>)>;
pub(crate) type ArrangementResponse = std::rc::Rc<dyn Fn(Result<(), String>)>;
pub(crate) type RoomPostResponse = std::rc::Rc<dyn Fn(Result<wire::rooms::RoomPostAnswer, String>)>;
pub(crate) type RoomControlResponse =
    std::rc::Rc<dyn Fn(Result<wire::intent::RoomControlAnswer, String>)>;
pub(crate) type RoomCoordinationResponse =
    std::rc::Rc<dyn Fn(Result<wire::intent::RoomCoordinationView, String>)>;
pub(crate) type PreparationResponse = std::rc::Rc<dyn Fn(Result<wire::intent::TextAnswer, String>)>;
pub(crate) type TaskResponse = std::rc::Rc<dyn Fn(Result<wire::intent::TaskAnswer, String>)>;
pub(crate) type PrincipalResponse =
    std::rc::Rc<dyn Fn(Result<wire::principal::PrincipalStanding, String>)>;

/// One complete room publication request retained by the bounded action mailbox.
#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
pub(crate) struct RoomPostAction {
    pub(crate) room: TaskHandle,
    pub(crate) head: wire::Seq,
    pub(crate) text: String,
    pub(crate) respond: RoomPostResponse,
}

impl RoomPostAction {
    /// Publish `text` in `room` at statement `head`, and run `then` only once
    /// the room records it; a refused post reports its reason to `refused`
    /// and nothing follows.
    pub(crate) fn then(
        room: TaskHandle,
        head: wire::Seq,
        text: String,
        then: impl FnOnce() + 'static,
        refused: impl Fn(String) + 'static,
    ) -> Self {
        let then = std::cell::Cell::new(Some(then));
        Self {
            room,
            head,
            text,
            respond: std::rc::Rc::new(move |result| match result {
                Ok(_) => {
                    if let Some(then) = then.take() {
                        then();
                    }
                }
                Err(issue) => refused(issue),
            }),
        }
    }
}

/// One operator item disposition, reply, recovery, or relevance assessment.
#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
pub(crate) enum RoomControlRequest {
    Control(wire::intent::RoomControl),
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

/// One exact proposal decision with the heads its preview observed.
#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
pub(crate) enum PreparationRequest {
    Decide {
        request: wire::rooms::PreparationDecide,
        expected: Vec<wire::Witness>,
    },
}

/// One principal write, the same command `spice principal` sends.
#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
pub(crate) enum PrincipalWrite {
    Admit(wire::principal::PrincipalAdmit),
    Edit(wire::principal::PrincipalEdit),
    Grant(wire::principal::PrincipalRoleRef),
    Revoke(wire::principal::PrincipalRoleRef),
    Deauthorize(wire::principal::PrincipalDeauthorize),
}

#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
impl PrincipalWrite {
    pub(crate) const fn kind(&self) -> wire::CommandKind {
        match self {
            Self::Admit(_) => wire::CommandKind::PrincipalAdmit,
            Self::Edit(_) => wire::CommandKind::PrincipalEdit,
            Self::Grant(_) => wire::CommandKind::PrincipalGrant,
            Self::Revoke(_) => wire::CommandKind::PrincipalRevoke,
            Self::Deauthorize(_) => wire::CommandKind::PrincipalDeauthorize,
        }
    }

    /// Bytes of operator-supplied text the request carries.
    pub(crate) fn text_bytes(&self) -> usize {
        let fields = |name: &Option<String>, email: &Option<String>| {
            name.as_ref()
                .map_or(0, String::len)
                .saturating_add(email.as_ref().map_or(0, String::len))
        };
        match self {
            Self::Admit(admit) => fields(&admit.name, &admit.email),
            Self::Edit(edit) => fields(&edit.name, &edit.email),
            Self::Grant(_) | Self::Revoke(_) | Self::Deauthorize(_) => 0,
        }
    }
}

/// One typed task creation, statement revision, or filing, with the heads it
/// was drawn from.
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

/// Wake one deferred filed task now, or set the instant its deferral wakes at.
#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
pub(crate) enum DeferralRequest {
    Wake(wire::intent::TaskWake),
    Defer(wire::intent::TaskDefer),
}

/// What one working-group change asks the authority for.
#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
pub(crate) enum GroupChange {
    /// Move one design into a group, founding it under a fresh identity, or
    /// out of every group.
    Move(wire::rooms::WorkingGroupMove),
    /// Restate every group and its designs in order.
    Arrange(wire::rooms::WorkingGroupArrange),
    /// End one group and leave all of its Designs ungrouped.
    Disband(wire::rooms::WorkingGroupDisband),
    /// Turn one group's fast-mode flag on or off.
    Fast(wire::rooms::WorkingGroupFast),
    /// Set one group's shared activity stance.
    Stance(wire::rooms::WorkingGroupStanceSet),
}

#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
impl GroupChange {
    /// The identity bytes this change names, which the action mailbox bounds.
    pub(crate) fn bytes(&self) -> usize {
        match self {
            Self::Move(request) => request.design.as_str().len(),
            Self::Arrange(request) => request
                .groups
                .iter()
                .map(|group| {
                    group.group.as_str().len()
                        + group
                            .anchors
                            .iter()
                            .map(|design| design.as_str().len())
                            .sum::<usize>()
                })
                .sum(),
            Self::Disband(request) => request.group.as_str().len(),
            Self::Fast(request) => request.group.as_str().len(),
            Self::Stance(request) => request.group.as_str().len(),
        }
    }
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
    /// Publish operator-authored text to a task room.
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
    /// Decide a Design pass proposal or record review evidence over it.
    Preparation {
        request: PreparationRequest,
        respond: PreparationResponse,
    },
    /// Admit a principal, edit a profile, change a role, or retire a
    /// capability.
    Principal {
        request: PrincipalWrite,
        respond: PrincipalResponse,
    },
    /// Create or file an operator Draft, or revise a task statement.
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
    /// Wake or reschedule one pending filed task at the workflow and deferral
    /// heads its view was drawn from.
    Deferral {
        request: DeferralRequest,
        expected: Vec<wire::Witness>,
        respond: TaskResponse,
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
        /// Optional result reporter for an explicit menu lifecycle action.
        respond: Option<ArrangementResponse>,
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
    /// Withdraw one task with the composed text as its reason.
    Delete {
        id: CanonicalUuid,
        task: TaskHandle,
        reason: String,
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

#[cfg(test)]
mod tests;
