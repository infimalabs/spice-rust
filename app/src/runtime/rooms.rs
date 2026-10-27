//! Deterministic room, group, preparation, and conversation authority
//! projected from facts.

use std::collections::BTreeMap;

use wire::rooms::{self as authority, RoomAuthorityStamp};
use wire::{Actor, CanonicalUuid, FactKind, Packed, Seq, StreamFact, TaskHandle, TaskStamp};

use super::BoardView;

mod contributions;
mod conversation;
#[cfg(test)]
mod design_standing_tests;
mod standing;

pub use contributions::Contribution;
pub use conversation::{
    Conversation, ConversationProjection, Delivery, OperatorInboxItem, OperatorItem,
};
pub use standing::Relevance;

/// Whether the room snapshot can be rendered as current authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomProjectionStatus {
    /// This subscription's room snapshot has not reached its horizon.
    Loading,
    /// One complete room snapshot and every later live fact were folded.
    Available,
    /// Snapshot continuity was lost; only a fresh subscription can recover.
    RequiresSnapshot,
}

/// One authority value together with its immutable journal position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomPositioned<T> {
    /// Durable journal position that admitted the value.
    pub position: Seq,
    /// Validated authority carried by that fact.
    pub value: T,
}

/// Current resolution of a room anchor through the latest board snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoomAnchorAuthority {
    /// No board snapshot has established current task authority.
    Unavailable,
    /// The board snapshot proves that this durable anchor is absent.
    Missing,
    /// Current rendered task handle for this durable open anchor.
    Available(TaskHandle),
    /// Current rendered task handle and terminal standing for a retained anchor.
    Settled {
        /// Last rendered handle retained by task authority.
        handle: TaskHandle,
        /// Terminal standing that concluded the room's anchor.
        standing: RoomAnchorSettlement,
    },
}

/// Consumer-visible terminal standing of a retained room anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomAnchorSettlement {
    /// The task completed through its declared workflow.
    Completed,
    /// The operator closed the task with recorded evidence or a negative conclusion.
    Closed,
    /// The operator withdrew the task.
    Deleted,
}

impl RoomAnchorSettlement {
    /// Stable browser label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Closed => "closed",
            Self::Deleted => "deleted",
        }
    }
}

/// One Design's standing as its working group's light shows it: the board
/// row's lifecycle standing, refined while active by the resident's pass
/// cycle or a lane's live claim. Derived from the board on every read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesignStanding {
    /// No board snapshot, or the board judges no standing for the anchor.
    Unavailable,
    /// Open in a Paused group; no lane can acquire it.
    Passive,
    /// Open without a group or in a Running group, waiting where its cycle says.
    Active(wire::rooms::DesignCycle),
    /// Active, and a lane claims it for one pass.
    Claimed,
    /// The operator concluded it done.
    Done,
    /// The operator concluded it collapsed.
    Collapsed,
    /// The operator withdrew it.
    Deleted,
}

impl DesignStanding {
    /// Stable browser key.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Passive => "passive",
            Self::Active(cycle) => cycle.key(),
            Self::Claimed => "claimed",
            Self::Done => "done",
            Self::Collapsed => "collapsed",
            Self::Deleted => "deleted",
        }
    }

    /// Words naming the standing beside the Design's label.
    pub fn label(self) -> String {
        match self {
            Self::Unavailable => "unavailable".to_owned(),
            Self::Passive => "open, passive".to_owned(),
            Self::Active(cycle) => format!("open, active, {}", cycle.label()),
            Self::Claimed => "open, active, claimed".to_owned(),
            Self::Done => "concluded done".to_owned(),
            Self::Collapsed => "concluded collapsed".to_owned(),
            Self::Deleted => "deleted".to_owned(),
        }
    }
}

/// One stable room-timeline card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomEvent {
    /// Immutable journal position of the event.
    pub position: Seq,
    /// Typed event authority.
    pub kind: RoomEventKind,
}

/// Authority carried by one room-timeline event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoomEventKind {
    /// Human-authored or source-linked publication.
    Published {
        /// The actor that published it.
        author: wire::Actor,
        /// What it published.
        content: authority::RoomPublicationContent,
    },
    /// Agent-authored source link and exact author/directed delivery identities.
    Messaged {
        /// Exact transcript-owned bytes.
        source: authority::TranscriptHunk,
        /// Occupant that authored the packet.
        author: CanonicalUuid,
        /// Directed messages opened atomically with the room publication.
        directed: Vec<CanonicalUuid>,
        /// The MSG, ASK, or ANS conversation this packet admitted, if any.
        conversation: Option<RoomAuthorityStamp>,
    },
    /// A human's MSG, ASK, or ANS; its publication is its conversation.
    Posted {
        /// The human that posted it.
        author: wire::Actor,
    },
    /// A lane stopped listening for this reason; native duties still hold it.
    Left {
        /// Logical lane that left.
        lane: CanonicalUuid,
        /// The lane's stated reason.
        clause: String,
    },
    /// A lane's typed problem-space contribution to an open Design. Its text
    /// is the fact's own; each source stays a link the resident resolves.
    Contributed {
        /// Logical lane that contributed.
        lane: CanonicalUuid,
        /// What the contribution offers: a charge or a question.
        contribution: authority::RoomContributionKind,
        /// The Design question item a charge weighs, absent on a question
        /// or a retained Design-level post.
        question: Option<authority::RoomQuestion>,
        /// The complete contributed text.
        text: String,
        /// Cited sources, in the author's order.
        sources: Vec<authority::RoomSource>,
    },
}

impl RoomEventKind {
    /// Whether this event divides a working-group plane as one full-width
    /// boundary, as a compaction divides a team's: only a lane's departure.
    #[must_use]
    pub const fn divides_plane(&self) -> bool {
        matches!(self, Self::Left { .. })
    }

    /// Whether this event carries a lane-authored room act.
    #[must_use]
    pub(crate) const fn lane_attributed(&self) -> bool {
        matches!(
            self,
            Self::Messaged { .. } | Self::Left { .. } | Self::Contributed { .. }
        )
    }
}

/// One task-keyed room projection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoomState {
    events: BTreeMap<RoomAuthorityStamp, RoomEvent>,
    /// Latest lane-authored event, used to prove earlier posts were heard
    /// without rescanning the room for every rendered card.
    latest_lane_event: Option<Seq>,
    /// Each nudge that messaged a lane: its position and the publication that
    /// carried the room's unheard operator posts.
    nudges: BTreeMap<Seq, RoomAuthorityStamp>,
    /// Each asked lane's newest recorded Design request: its position, exact
    /// retained evidence, and asked question items in asked order.
    asked: BTreeMap<
        CanonicalUuid,
        (
            Seq,
            authority::DesignNoticeEvidence,
            Vec<authority::RoomQuestion>,
        ),
    >,
}

impl RoomState {
    /// Every event in journal order, so typed contributions and ordinary
    /// conversation interleave exactly as the journal admitted them.
    #[must_use]
    pub fn in_journal_order(&self) -> Vec<(&RoomAuthorityStamp, &RoomEvent)> {
        let mut ordered = self.events.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|(_, event)| event.position);
        ordered
    }

    /// Stable events, keyed by their immutable publication identity.
    pub fn events(&self) -> &BTreeMap<RoomAuthorityStamp, RoomEvent> {
        &self.events
    }
}

/// Why a logical lane currently appears in a room. Each reason names the
/// admitted source it traces to; nothing ages, a Design request stands until
/// the lane's next room publication, and native duties end only through their
/// own dispositions.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ParticipantReason {
    /// The lane authored a room message or typed contribution; history never ages.
    Publication(RoomAuthorityStamp),
    /// A recorded Design request asks the lane from this exact retained
    /// evidence about these question items, or the whole Design when none;
    /// it never ages, and the lane's next room publication answers or
    /// declines it.
    Asked {
        evidence: authority::DesignNoticeEvidence,
        questions: Vec<authority::RoomQuestion>,
    },
    /// The lane currently holds the anchor or explicitly related work.
    Work(TaskStamp),
    /// A live stop request addresses the lane's exact claim.
    Stop(String),
    /// The lane recorded a leave; only a later Design request and native
    /// duties can make it listen.
    Left {
        /// The leave's publication identity.
        publication: RoomAuthorityStamp,
        /// The lane's stated reason.
        clause: String,
    },
}

/// One logical lane and its exact current reasons, never a team membership.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomParticipant {
    /// Logical lane identity.
    pub lane: CanonicalUuid,
    /// Sorted, deduplicated reasons derived from authority.
    pub reasons: Vec<ParticipantReason>,
}

/// Authenticated journal provenance of one recorded Design proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalRecord {
    /// Principal that recorded the proposal.
    pub author: wire::Actor,
    /// Commit instant of the proposal fact.
    pub at: wire::Instant,
}

/// Preparation provenance recorded under stable native identities.
///
/// Current output control lives only in board dependency edges.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreparationProjection {
    /// Design pass proposals by stable candidate identity.
    pub candidates:
        BTreeMap<authority::CandidateRef, RoomPositioned<authority::DesignPassProposal>>,
    /// Authenticated journal provenance of each proposal.
    pub proposal_records: BTreeMap<authority::CandidateRef, ProposalRecord>,
    /// Critiques by stable critique identity.
    pub critiques: BTreeMap<authority::CritiqueRef, RoomPositioned<authority::CandidateCritique>>,
    /// Coverage by stable coverage identity.
    pub coverage: BTreeMap<authority::CoverageRef, RoomPositioned<authority::CandidateCoverage>>,
    /// Decisions by stable decision identity.
    pub decisions: BTreeMap<authority::DecisionRef, RoomPositioned<authority::PreparationDecision>>,
    /// Historical positive authorizations by their selected decision identity.
    pub authorizations:
        BTreeMap<authority::DecisionRef, RoomPositioned<authority::ExecutionAuthorization>>,
    /// Author withdrawals by the exact candidate each withdraws.
    pub withdrawals: BTreeMap<authority::CandidateRef, RoomPositioned<CandidateWithdrawalRecord>>,
}

/// One author's withdrawal of a Design pass proposal, with who and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateWithdrawalRecord {
    /// The withdrawn proposal identity and the author's reason.
    pub withdrawal: authority::CandidateWithdrawal,
    /// The authenticated author that withdrew it.
    pub author: wire::Actor,
    /// When the journal admitted the withdrawal.
    pub at: wire::Instant,
}

/// One stop request sent against a room's active claim, as the steering
/// control `room-stop:ROOM:CLAIM` names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopRequest {
    /// Human steering key.
    pub key: String,
    /// Logical lane the request addresses.
    pub lane: CanonicalUuid,
    /// The exact claim win the stop applies to.
    pub claim: wire::SequenceCode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StopState {
    request: StopRequest,
    disposition: wire::Disposition,
}

/// Capability-local fold of one subscription's room snapshot and the live
/// facts strictly after its horizon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomProjection {
    status: RoomProjectionStatus,
    /// Horizon the snapshot pages declare, fixed by the first page.
    horizon: Option<Seq>,
    continuation: Option<Seq>,
    /// Last folded position; the horizon once the snapshot completes.
    last_fact: Option<Seq>,
    /// Greatest live position seen before the snapshot completed.
    early: Option<Seq>,
    /// Earliest-born human principal, used only to present historical
    /// operator-authored room authority under its durable human identity.
    bootstrap: Option<CanonicalUuid>,
    partition: authority::WorkingGroupPartition,
    rooms: BTreeMap<TaskStamp, RoomState>,
    preparation: PreparationProjection,
    conversation: ConversationProjection,
    /// The run of room packet records directly before the current position,
    /// which a conversation record ends when the packet admitted one.
    messaged: Vec<(Seq, TaskStamp, RoomAuthorityStamp)>,
    /// Each room's latest stop request, whatever its disposition.
    stops: BTreeMap<TaskStamp, RoomPositioned<StopState>>,
    relevance: BTreeMap<RoomAuthorityStamp, RoomPositioned<Relevance>>,
    /// Bounded maturity evidence incrementally folded per Design.
    maturity: BTreeMap<TaskStamp, authority::DesignMaturity>,
}

impl Default for RoomProjection {
    fn default() -> Self {
        Self {
            status: RoomProjectionStatus::Loading,
            horizon: None,
            continuation: None,
            last_fact: None,
            early: None,
            bootstrap: None,
            partition: authority::WorkingGroupPartition::default(),
            rooms: BTreeMap::new(),
            preparation: PreparationProjection::default(),
            conversation: ConversationProjection::default(),
            messaged: Vec::new(),
            stops: BTreeMap::new(),
            relevance: BTreeMap::new(),
            maturity: BTreeMap::new(),
        }
    }
}

impl RoomProjection {
    /// Current snapshot standing.
    pub const fn status(&self) -> RoomProjectionStatus {
        self.status
    }

    /// The repository-shared working-group partition: group order, design
    /// order, and the revision a change witnesses. No browser view state is
    /// represented here.
    pub const fn partition(&self) -> &authority::WorkingGroupPartition {
        &self.partition
    }

    /// One durable task room, when any timeline authority exists for it.
    pub fn room(&self, room: &TaskStamp) -> Option<&RoomState> {
        self.rooms.get(room)
    }
    /// One Design's bounded advisory maturity evidence.
    #[must_use]
    pub fn maturity(&self, design: &TaskStamp) -> Option<&authority::DesignMaturity> {
        self.maturity.get(design)
    }

    /// Preparation, decision, and authorization authority.
    pub const fn preparation(&self) -> &PreparationProjection {
        &self.preparation
    }

    /// Conversation, operator item, result, and lane delivery authority.
    pub const fn conversation(&self) -> &ConversationProjection {
        &self.conversation
    }

    /// The author a room read presents for one recorded fact actor.
    #[must_use]
    pub fn author(&self, actor: &Actor) -> Actor {
        match (actor, self.bootstrap) {
            (Actor::Operator, Some(bootstrap)) => Actor::Human(bootstrap),
            _ => actor.clone(),
        }
    }

    /// Resolve the durable room identity only through the latest board snapshot.
    pub fn anchor_authority(
        &self,
        room: &TaskStamp,
        board: Option<&BoardView>,
    ) -> RoomAnchorAuthority {
        let Some(board) = board else {
            return RoomAnchorAuthority::Unavailable;
        };
        board
            .tasks
            .iter()
            .chain(board.room_anchors.iter())
            .find(|task| task.statement.body.born == *room)
            .map_or(RoomAnchorAuthority::Missing, |task| {
                let workflow = &task.workflow.body;
                let handle = task.statement.body.handle.clone();
                let standing = match workflow.status.conclusion() {
                    Some(wire::WorkflowConclusion::Positive { .. }) => {
                        Some(RoomAnchorSettlement::Completed)
                    }
                    Some(wire::WorkflowConclusion::Negative(wire::WorkflowNegative::Closed)) => {
                        Some(RoomAnchorSettlement::Closed)
                    }
                    Some(wire::WorkflowConclusion::Negative(wire::WorkflowNegative::Deleted)) => {
                        Some(RoomAnchorSettlement::Deleted)
                    }
                    None => None,
                };
                match standing {
                    Some(standing) => RoomAnchorAuthority::Settled { handle, standing },
                    None => RoomAnchorAuthority::Available(handle),
                }
            })
    }

    /// One Design's standing. The board row's lifecycle standing decides
    /// terminal and passive Designs; an active one shows a lane's live claim,
    /// else the cycle the resident judged.
    pub fn design_standing(&self, room: &TaskStamp, board: Option<&BoardView>) -> DesignStanding {
        let Some(board) = board else {
            return DesignStanding::Unavailable;
        };
        let Some(design) = board.row(room) else {
            return DesignStanding::Unavailable;
        };
        match design.design_standing() {
            None => DesignStanding::Unavailable,
            Some(wire::intent::DesignStanding::Passive) => DesignStanding::Passive,
            Some(wire::intent::DesignStanding::Done) => DesignStanding::Done,
            Some(wire::intent::DesignStanding::Collapsed { .. }) => DesignStanding::Collapsed,
            Some(wire::intent::DesignStanding::Deleted) => DesignStanding::Deleted,
            Some(wire::intent::DesignStanding::Active) => {
                if design.design_availability() == wire::rooms::DesignAvailability::Claimed {
                    DesignStanding::Claimed
                } else {
                    board
                        .design_cycles
                        .get(room)
                        .map_or(DesignStanding::Unavailable, |cycle| {
                            DesignStanding::Active(*cycle)
                        })
                }
            }
        }
    }

    /// A stop remains current only while it is live and the board shows the
    /// exact claim win it names still active.
    pub fn current_stop(
        &self,
        room: &TaskStamp,
        board: Option<&BoardView>,
    ) -> Option<RoomPositioned<&StopRequest>> {
        self.stop_holder(room, board).map(|(stop, _)| stop)
    }

    fn stop_holder(
        &self,
        room: &TaskStamp,
        board: Option<&BoardView>,
    ) -> Option<(RoomPositioned<&StopRequest>, CanonicalUuid)> {
        let stop = self.stops.get(room)?;
        if stop.value.disposition != wire::Disposition::Live {
            return None;
        }
        let claim = board?
            .tasks
            .iter()
            .find(|task| task.statement.body.born == *room)?
            .claim
            .as_ref()
            .filter(|claim| {
                claim.body.disposition == wire::ClaimDisposition::Active
                    && claim.body.claimed.get() == stop.value.request.claim.get()
            })?;
        Some((
            RoomPositioned {
                position: stop.position,
                value: &stop.value.request,
            },
            claim.body.lane,
        ))
    }

    pub(super) fn requires_snapshot(&self) -> bool {
        self.status == RoomProjectionStatus::RequiresSnapshot
    }

    pub(super) fn lose_continuity(&mut self) {
        self.status = RoomProjectionStatus::RequiresSnapshot;
        self.continuation = None;
    }

    /// Fold one room snapshot page. Every page declares the same horizon,
    /// rows rise strictly through it, and a continuation never moves back.
    /// Completion makes the horizon the live boundary.
    pub(super) fn apply_page(&mut self, packed: &Packed<StreamFact>) -> Result<bool, ()> {
        if self.status != RoomProjectionStatus::Loading {
            self.lose_continuity();
            return Err(());
        }
        let first = packed.rows.first().map(StreamFact::seq);
        let last = packed.rows.last().map(StreamFact::seq);
        let horizon_moved = self
            .horizon
            .is_some_and(|horizon| horizon != packed.horizon);
        let continuation_regressed = self
            .continuation
            .is_some_and(|expected| first.is_some_and(|first| first < expected))
            || self
                .last_fact
                .is_some_and(|previous| first.is_some_and(|first| first <= previous));
        let rows_regressed = packed
            .rows
            .windows(2)
            .any(|rows| rows[0].seq() >= rows[1].seq())
            || last.is_some_and(|last| last > packed.horizon);
        let overflow_regressed = packed.overflow.is_some_and(|overflow| {
            last.is_some_and(|last| overflow.continuation <= last)
                || overflow.continuation > packed.horizon
        });
        if horizon_moved || continuation_regressed || rows_regressed || overflow_regressed {
            self.lose_continuity();
            return Err(());
        }
        self.horizon = Some(packed.horizon);
        for fact in &packed.rows {
            self.apply_fact(fact);
        }
        if last.is_some() {
            self.last_fact = last;
        }
        self.continuation = packed.overflow.map(|overflow| overflow.continuation);
        if self.continuation.is_none() {
            if self.early.is_some_and(|early| early > packed.horizon) {
                self.lose_continuity();
                return Err(());
            }
            self.last_fact = Some(packed.horizon);
            self.status = RoomProjectionStatus::Available;
        }
        Ok(true)
    }

    /// Fold one live fact strictly after the completed snapshot's horizon.
    pub(super) fn apply_live_fact(&mut self, fact: &StreamFact) -> Result<bool, ()> {
        match self.status {
            RoomProjectionStatus::Loading => {
                self.early = self.early.max(Some(fact.seq()));
                Ok(false)
            }
            RoomProjectionStatus::RequiresSnapshot => Ok(false),
            RoomProjectionStatus::Available => {
                if self.last_fact.is_some_and(|last| fact.seq() <= last) {
                    self.lose_continuity();
                    return Err(());
                }
                self.last_fact = Some(fact.seq());
                Ok(self.apply_fact(fact))
            }
        }
    }

    fn apply_fact(&mut self, fact: &StreamFact) -> bool {
        let principal = self.apply_principal_fact(fact).unwrap_or(false);
        let delivered = self.conversation.apply_delivery(fact);
        let changed = self
            .apply_room_fact(fact)
            .or_else(|| self.conversation.apply(fact))
            .or_else(|| self.apply_preparation_fact(fact))
            .or_else(|| self.apply_steering_fact(fact))
            .unwrap_or(false);
        if fact.kind() != FactKind::RoomMessaged {
            self.messaged.clear();
        }
        principal | delivered | changed
    }

    fn apply_principal_fact(&mut self, fact: &StreamFact) -> Option<bool> {
        match fact.kind() {
            FactKind::PrincipalAdmitted => Some(
                fact.read::<wire::principal::PrincipalAdmitted>()
                    .is_some_and(|body| {
                        if self.bootstrap.is_some() {
                            false
                        } else {
                            self.bootstrap = Some(body.principal);
                            true
                        }
                    }),
            ),
            _ => None,
        }
    }

    fn apply_room_fact(&mut self, fact: &StreamFact) -> Option<bool> {
        match fact.kind() {
            FactKind::WorkingGroupMoved => Some(
                fact.read::<authority::WorkingGroupMoved>()
                    .is_some_and(|body| self.replace_partition(body.partition)),
            ),
            FactKind::WorkingGroupsArranged => Some(
                fact.read::<authority::WorkingGroupsArranged>()
                    .is_some_and(|body| self.replace_partition(body.partition)),
            ),
            FactKind::RoomPublished => {
                Some(fact.read::<authority::RoomPublished>().is_some_and(|body| {
                    self.insert_event(
                        body.room,
                        body.publication,
                        RoomEvent {
                            position: fact.seq(),
                            kind: RoomEventKind::Published {
                                author: fact.actor().clone(),
                                content: body.content,
                            },
                        },
                    )
                }))
            }
            FactKind::RoomMessaged => Some(
                fact.read::<authority::RoomMessaged>()
                    .is_some_and(|body| self.apply_messaged(fact.seq(), body)),
            ),
            FactKind::RoomConversed => Some(
                fact.read::<authority::RoomConversed>()
                    .is_some_and(|body| self.apply_conversed(fact, body)),
            ),
            FactKind::RoomPosted => Some(
                fact.read::<authority::RoomPosted>()
                    .is_some_and(|body| self.apply_posted(fact, body)),
            ),
            FactKind::RoomLeft => Some(fact.read::<authority::RoomLeft>().is_some_and(|body| {
                if let Some(maturity) = self.maturity.get_mut(&body.room) {
                    maturity.observe_leave(&body);
                }
                self.insert_event(
                    body.room,
                    body.publication,
                    RoomEvent {
                        position: fact.seq(),
                        kind: RoomEventKind::Left {
                            lane: body.lane,
                            clause: body.clause,
                        },
                    },
                )
            })),
            FactKind::RoomContributed => Some(
                fact.read::<authority::RoomContributed>()
                    .is_some_and(|body| {
                        self.maturity
                            .entry(body.room.clone())
                            .or_default()
                            .observe_contribution(&body);
                        self.insert_event(
                            body.room,
                            body.publication,
                            RoomEvent {
                                position: fact.seq(),
                                kind: RoomEventKind::Contributed {
                                    lane: body.lane,
                                    contribution: body.contribution,
                                    question: body.question,
                                    text: body.text,
                                    sources: body.sources,
                                },
                            },
                        )
                    }),
            ),
            FactKind::DesignNoticed => Some(
                fact.read::<authority::DesignNoticed>()
                    .is_some_and(|body| self.apply_noticed(fact.seq(), &body)),
            ),
            FactKind::DesignNudged => Some(
                fact.read::<authority::DesignNudged>()
                    .is_some_and(|body| self.apply_nudged(fact.seq(), body)),
            ),
            FactKind::RoomRelevanceObserved => Some(
                fact.read::<authority::RoomRelevanceObserved>()
                    .is_some_and(|body| self.apply_observed(fact.seq(), body.observation)),
            ),
            FactKind::RoomRelevanceAssessed => Some(
                fact.read::<authority::RoomRelevanceAssessed>()
                    .is_some_and(|body| self.apply_assessed(&body.assessment)),
            ),
            _ => None,
        }
    }

    fn apply_noticed(&mut self, position: Seq, body: &authority::DesignNoticed) -> bool {
        self.rooms
            .entry(body.design.clone())
            .or_default()
            .asked
            .insert(
                body.lane,
                (
                    position,
                    body.evidence.clone(),
                    body.asks.iter().map(|ask| ask.question).collect(),
                ),
            );
        self.maturity
            .entry(body.design.clone())
            .or_default()
            .observe_request(body);
        true
    }

    /// Retain a nudge's message, whose acknowledgement hears every earlier
    /// operator post in its Design's room.
    fn apply_nudged(&mut self, position: Seq, body: authority::DesignNudged) -> bool {
        body.message.is_some_and(|message| {
            self.rooms
                .entry(body.design)
                .or_default()
                .nudges
                .insert(position, message)
                .is_none()
        })
    }

    fn apply_messaged(&mut self, position: Seq, body: authority::RoomMessaged) -> bool {
        self.conversation.name(&body.directed);
        self.messaged
            .push((position, body.room.clone(), body.publication.clone()));
        self.insert_event(
            body.room,
            body.publication,
            RoomEvent {
                position,
                kind: RoomEventKind::Messaged {
                    source: body.hunk,
                    author: body.author,
                    directed: body.directed,
                    conversation: None,
                },
            },
        )
    }

    /// A lane conversation directly follows the room packet records of its
    /// own write, which own the same transcript hunk; each links to it.
    fn apply_conversed(&mut self, fact: &StreamFact, body: authority::RoomConversed) -> bool {
        let Ok(publication) = body.publication() else {
            return false;
        };
        let stamp = body.publication.clone();
        let admitted = self.conversation.admit(
            fact.seq(),
            *fact.at(),
            publication,
            body.directed,
            body.expires,
            None,
        );
        let mut expected = fact.seq().get();
        let mut linked = false;
        for (position, room, packet) in self.messaged.iter().rev() {
            expected -= 1;
            if position.get() != expected {
                break;
            }
            let Some(RoomEventKind::Messaged {
                source,
                conversation,
                ..
            }) = self
                .rooms
                .get_mut(room)
                .and_then(|state| state.events.get_mut(packet))
                .map(|event| &mut event.kind)
            else {
                break;
            };
            if *source != body.hunk {
                break;
            }
            linked |= conversation.replace(stamp.clone()).as_ref() != Some(&stamp);
        }
        admitted | linked
    }

    /// An operator post records in the one room its routes speak in.
    fn apply_posted(&mut self, fact: &StreamFact, body: authority::RoomPosted) -> bool {
        let Ok(publication) = body.publication(wire::SequenceCode::new(fact.seq().get())) else {
            return false;
        };
        let room = publication
            .routes()
            .iter()
            .find_map(|route| match &route.destination {
                authority::RoomDestination::Task { task } => Some(task.clone()),
                _ => route.context.clone(),
            });
        let stamp = body.publication.clone();
        let admitted = self.conversation.admit(
            fact.seq(),
            *fact.at(),
            publication,
            body.directed,
            body.expires,
            Some(body.text),
        );
        let recorded = room.is_some_and(|room| {
            self.insert_event(
                room,
                stamp,
                RoomEvent {
                    position: fact.seq(),
                    kind: RoomEventKind::Posted {
                        author: fact.actor().clone(),
                    },
                },
            )
        });
        admitted | recorded
    }

    fn apply_preparation_fact(&mut self, fact: &StreamFact) -> Option<bool> {
        match fact.kind() {
            FactKind::DesignProposed => Some(fact.read::<authority::DesignProposed>().is_some_and(
                |body| {
                    let candidate = body.candidate.candidate.clone();
                    let record = ProposalRecord {
                        author: fact.actor().clone(),
                        at: *fact.at(),
                    };
                    let provenance = self
                        .preparation
                        .proposal_records
                        .insert(candidate.clone(), record.clone())
                        .is_none_or(|previous| previous != record);
                    replace(
                        &mut self.preparation.candidates,
                        candidate,
                        fact.seq(),
                        body.candidate,
                    ) | provenance
                },
            )),
            FactKind::CandidateCritiqued => Some(
                fact.read::<authority::CandidateCritiqued>()
                    .is_some_and(|body| {
                        replace(
                            &mut self.preparation.critiques,
                            body.critique.critique.clone(),
                            fact.seq(),
                            body.critique,
                        )
                    }),
            ),
            FactKind::CandidateCovered => Some(
                fact.read::<authority::CandidateCovered>()
                    .is_some_and(|body| {
                        replace(
                            &mut self.preparation.coverage,
                            body.coverage.coverage.clone(),
                            fact.seq(),
                            body.coverage,
                        )
                    }),
            ),
            FactKind::PreparationDecided => Some(
                fact.read::<authority::PreparationDecided>()
                    .is_some_and(|body| {
                        replace(
                            &mut self.preparation.decisions,
                            body.decision.decision.clone(),
                            fact.seq(),
                            body.decision,
                        )
                    }),
            ),
            FactKind::ExecutionAuthorized => Some(
                fact.read::<authority::ExecutionAuthorized>()
                    .is_some_and(|body| {
                        replace(
                            &mut self.preparation.authorizations,
                            body.authorization.decision.clone(),
                            fact.seq(),
                            body.authorization,
                        )
                    }),
            ),
            FactKind::CandidateWithdrawn => Some(
                fact.read::<authority::CandidateWithdrawn>()
                    .is_some_and(|body| {
                        replace(
                            &mut self.preparation.withdrawals,
                            body.withdrawal.candidate.clone(),
                            fact.seq(),
                            CandidateWithdrawalRecord {
                                withdrawal: body.withdrawal,
                                author: fact.actor().clone(),
                                at: *fact.at(),
                            },
                        )
                    }),
            ),
            _ => None,
        }
    }

    fn apply_steering_fact(&mut self, fact: &StreamFact) -> Option<bool> {
        match fact.kind() {
            FactKind::SteerSent => Some(fact.read::<wire::body::SteerSent>().is_some_and(|body| {
                let Some(control) = body
                    .control
                    .as_deref()
                    .and_then(|control| control.parse::<authority::StopControl>().ok())
                else {
                    return false;
                };
                replace(
                    &mut self.stops,
                    control.room,
                    fact.seq(),
                    StopState {
                        request: StopRequest {
                            key: body.key,
                            lane: body.lane,
                            claim: control.claim,
                        },
                        disposition: wire::Disposition::Live,
                    },
                )
            })),
            FactKind::SteerAcked => Some(
                fact.read::<wire::body::SteerAcked>()
                    .is_some_and(|body| self.dispose_stop(&body.key, wire::Disposition::Acked)),
            ),
            FactKind::SteerDeclined => Some(
                fact.read::<wire::body::SteerDeclined>()
                    .is_some_and(|body| self.dispose_stop(&body.key, wire::Disposition::Declined)),
            ),
            FactKind::SteerLapsed => Some(
                fact.read::<wire::body::SteerLapsed>()
                    .is_some_and(|body| self.dispose_stop(&body.key, wire::Disposition::Lapsed)),
            ),
            FactKind::SteerDeadLettered => Some(
                fact.read::<wire::steering::SteerDeadLettered>()
                    .is_some_and(|body| {
                        self.dispose_stop(&body.key, wire::Disposition::DeadLettered)
                    }),
            ),
            FactKind::SteerCanceled => Some(
                fact.read::<wire::body::SteerCanceled>()
                    .is_some_and(|body| self.dispose_stop(&body.key, wire::Disposition::Canceled)),
            ),
            FactKind::SteerRecovered => Some(
                fact.read::<wire::body::SteerRecovered>()
                    .is_some_and(|body| self.dispose_stop(&body.key, wire::Disposition::Live)),
            ),
            _ => None,
        }
    }

    fn insert_event(&mut self, room: TaskStamp, id: RoomAuthorityStamp, event: RoomEvent) -> bool {
        let state = self.rooms.entry(room).or_default();
        if event.kind.lane_attributed() {
            state.latest_lane_event = state.latest_lane_event.max(Some(event.position));
        }
        replace_plain(&mut state.events, id, event)
    }

    /// Move the stop request keyed `key` to `disposition`; `Live` is a recovery.
    fn dispose_stop(&mut self, key: &str, disposition: wire::Disposition) -> bool {
        self.stops
            .values_mut()
            .find(|stop| stop.value.request.key == key)
            .is_some_and(|stop| {
                let changed = stop.value.disposition != disposition;
                stop.value.disposition = disposition;
                changed
            })
    }

    fn replace_partition(&mut self, partition: authority::WorkingGroupPartition) -> bool {
        if self.partition == partition {
            return false;
        }
        self.partition = partition;
        true
    }
}

fn replace<K: Ord, T: PartialEq>(
    values: &mut BTreeMap<K, RoomPositioned<T>>,
    key: K,
    position: Seq,
    value: T,
) -> bool {
    replace_plain(values, key, RoomPositioned { position, value })
}

fn replace_plain<K: Ord, T: PartialEq>(values: &mut BTreeMap<K, T>, key: K, value: T) -> bool {
    if values.get(&key) == Some(&value) {
        false
    } else {
        values.insert(key, value);
        true
    }
}

/// Whether one task is current work in a Design room: the Design itself or a
/// task with a direct dependency on it. Selection removes that dependency, so
/// immutable preparation provenance never keeps released work in the room.
#[must_use]
pub fn associated(statement: &wire::intent::BoardStatement, room: &TaskStamp) -> bool {
    statement.born == *room || statement.dependencies.contains(room)
}

/// Open tasks currently gated by this Design's dependency edge.
#[must_use]
pub fn design_dependent_count(board: &super::BoardView, design: &TaskStamp) -> usize {
    board
        .tasks
        .iter()
        .filter(|task| task.statement.body.dependencies.contains(design))
        .count()
}
