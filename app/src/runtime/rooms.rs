//! Deterministic room, group, and preparation authority projected from facts.

use std::collections::BTreeMap;

use wire::rooms::{self as authority, RoomAuthorityStamp};
use wire::{
    CanonicalUuid, FactKind, Packed, Seq, StreamFact, TaskHandle, TaskPreparation, TaskStamp,
};

use super::BoardView;

/// Whether the durable fact snapshot can be rendered as current authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomProjectionStatus {
    /// The initial or continued fact snapshot has not reached its horizon.
    Loading,
    /// One complete fact snapshot and every later accepted fact were folded.
    Available,
    /// Stream retention or pagination continuity was lost; only a fresh snapshot can recover.
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
    /// Current rendered task handle for this durable anchor.
    Available(TaskHandle),
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
    /// Operator-authored or source-linked publication.
    Published(authority::RoomPublicationContent),
    /// Agent-authored source link and exact author/directed delivery identities.
    Messaged {
        /// Exact transcript-owned bytes.
        source: authority::TranscriptHunk,
        /// Occupant that authored the packet.
        author: CanonicalUuid,
        /// Directed messages opened atomically with the room publication.
        directed: Vec<CanonicalUuid>,
    },
    /// Scoped request linked to ordinary steering.
    Coordination {
        /// Human steering key.
        key: String,
        /// Exact occupant address asked to contribute.
        target: wire::OccupantAddress,
    },
    /// Put-aside action, including any claim-bound stop request.
    PutAside {
        /// Coordination keys canceled by this action.
        canceled: Vec<String>,
        /// Exact stop authority, absent when the anchor was idle.
        stop: Option<authority::RoomStopRequest>,
    },
}

/// One task-keyed room projection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoomState {
    events: BTreeMap<RoomAuthorityStamp, RoomEvent>,
}

impl RoomState {
    /// Stable events, keyed by their immutable publication identity.
    pub fn events(&self) -> &BTreeMap<RoomAuthorityStamp, RoomEvent> {
        &self.events
    }
}

/// Why a logical lane currently appears in a room.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ParticipantReason {
    /// The lane authored a transcript-backed publication.
    Contribution(RoomAuthorityStamp),
    /// The lane currently holds the anchor or explicitly related work.
    Work(TaskStamp),
    /// A live room coordination request addresses the lane.
    Coordination(String),
    /// A live stop request addresses the lane's exact claim.
    Stop(String),
}

/// One logical lane and its exact current reasons, never a team membership.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomParticipant {
    /// Logical lane identity.
    pub lane: CanonicalUuid,
    /// Sorted, deduplicated reasons derived from authority.
    pub reasons: Vec<ParticipantReason>,
}

/// Preparation authority retained under stable native identities.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreparationProjection {
    /// Retained candidates by stable candidate identity.
    pub candidates: BTreeMap<authority::CandidateRef, RoomPositioned<authority::RetainedCandidate>>,
    /// Critiques by stable critique identity.
    pub critiques: BTreeMap<authority::CritiqueRef, RoomPositioned<authority::CandidateCritique>>,
    /// Coverage by stable coverage identity.
    pub coverage: BTreeMap<authority::CoverageRef, RoomPositioned<authority::CandidateCoverage>>,
    /// Decisions by stable decision identity.
    pub decisions: BTreeMap<authority::DecisionRef, RoomPositioned<authority::PreparationDecision>>,
    /// Positive authorizations by their accepted decision identity.
    pub authorizations:
        BTreeMap<authority::DecisionRef, RoomPositioned<authority::ExecutionAuthorization>>,
    /// Current explicit preparation association for each controlled output.
    pub controls: BTreeMap<TaskStamp, RoomPositioned<TaskPreparation>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CoordinationState {
    room: TaskStamp,
    target: wire::OccupantAddress,
    live: bool,
}

/// Capability-local fold of authenticated, credential-free durable facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomProjection {
    status: RoomProjectionStatus,
    continuation: Option<Seq>,
    last_fact: Option<Seq>,
    groups: BTreeMap<RoomAuthorityStamp, RoomPositioned<authority::WorkingGroup>>,
    rooms: BTreeMap<TaskStamp, RoomState>,
    preparation: PreparationProjection,
    address_lanes: BTreeMap<wire::OccupantAddress, CanonicalUuid>,
    active_claims: BTreeMap<TaskStamp, (Seq, CanonicalUuid)>,
    coordination: BTreeMap<String, CoordinationState>,
    stops: BTreeMap<TaskStamp, RoomPositioned<authority::RoomStopRequest>>,
}

impl Default for RoomProjection {
    fn default() -> Self {
        Self {
            status: RoomProjectionStatus::Loading,
            continuation: None,
            last_fact: None,
            groups: BTreeMap::new(),
            rooms: BTreeMap::new(),
            preparation: PreparationProjection::default(),
            address_lanes: BTreeMap::new(),
            active_claims: BTreeMap::new(),
            coordination: BTreeMap::new(),
            stops: BTreeMap::new(),
        }
    }
}

impl RoomProjection {
    /// Current snapshot standing.
    pub const fn status(&self) -> RoomProjectionStatus {
        self.status
    }

    /// Repository-shared groups only; no browser view state is represented here.
    pub fn groups(&self) -> &BTreeMap<RoomAuthorityStamp, RoomPositioned<authority::WorkingGroup>> {
        &self.groups
    }

    /// One durable task room, when any timeline authority exists for it.
    pub fn room(&self, room: &TaskStamp) -> Option<&RoomState> {
        self.rooms.get(room)
    }

    /// Preparation, decision, control, and authorization authority.
    pub const fn preparation(&self) -> &PreparationProjection {
        &self.preparation
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
            .find(|task| task.statement.body.born == *room)
            .map_or(RoomAnchorAuthority::Missing, |task| {
                RoomAnchorAuthority::Available(task.statement.body.handle.clone())
            })
    }

    /// A stop remains current only while the exact claim win it names is active.
    pub fn current_stop(
        &self,
        room: &TaskStamp,
    ) -> Option<&RoomPositioned<authority::RoomStopRequest>> {
        let stop = self.stops.get(room)?;
        self.active_claims
            .get(room)
            .is_some_and(|(claim, _)| claim.get() == stop.value.claim.get())
            .then_some(stop)
    }

    /// Current logical-lane participants and the exact reasons for their presence.
    pub fn participants(
        &self,
        room: &TaskStamp,
        board: Option<&BoardView>,
    ) -> Vec<RoomParticipant> {
        let mut reasons = BTreeMap::<CanonicalUuid, Vec<ParticipantReason>>::new();
        if let Some(state) = self.rooms.get(room) {
            for (publication, event) in &state.events {
                if let RoomEventKind::Messaged { source, .. } = &event.kind {
                    reasons
                        .entry(source.head.lane)
                        .or_default()
                        .push(ParticipantReason::Contribution(publication.clone()));
                }
            }
        }
        if let Some(board) = board {
            for task in &board.tasks {
                let statement = &task.statement.body;
                let related = statement.born == *room
                    || statement.preparation.as_ref().is_some_and(|preparation| {
                        preparation.parent == *room
                            || preparation.controller == *room
                            || preparation.construction == *room
                    });
                if related
                    && let Some(claim) = task
                        .claim
                        .as_ref()
                        .filter(|claim| claim.body.disposition == wire::ClaimDisposition::Active)
                {
                    reasons
                        .entry(claim.body.lane)
                        .or_default()
                        .push(ParticipantReason::Work(statement.born.clone()));
                }
            }
        }
        for (key, request) in &self.coordination {
            if request.live
                && request.room == *room
                && let Some(lane) = self.address_lanes.get(&request.target)
            {
                reasons
                    .entry(*lane)
                    .or_default()
                    .push(ParticipantReason::Coordination(key.clone()));
            }
        }
        if let Some(stop) = self.current_stop(room)
            && let Some(lane) = self.address_lanes.get(&stop.value.target)
        {
            reasons
                .entry(*lane)
                .or_default()
                .push(ParticipantReason::Stop(stop.value.key.clone()));
        }
        reasons
            .into_iter()
            .map(|(lane, mut reasons)| {
                reasons.sort();
                reasons.dedup();
                RoomParticipant { lane, reasons }
            })
            .collect()
    }

    pub(super) fn requires_snapshot(&self) -> bool {
        self.status == RoomProjectionStatus::RequiresSnapshot
    }

    pub(super) fn lose_continuity(&mut self) {
        self.status = RoomProjectionStatus::RequiresSnapshot;
        self.continuation = None;
    }

    /// Fold a packed snapshot page. A continuation may skip hidden facts but
    /// can never move backward or precede the requested producer position.
    pub(super) fn apply_page(&mut self, packed: &Packed<StreamFact>) -> Result<bool, ()> {
        if self.requires_snapshot() {
            return Err(());
        }
        let first = packed.rows.first().map(StreamFact::seq);
        let last = packed.rows.last().map(StreamFact::seq);
        let expected_regressed = self.continuation.is_some_and(|expected| {
            packed.horizon < expected || first.is_some_and(|first| first < expected)
        });
        let history_regressed = self
            .last_fact
            .is_some_and(|previous| first.is_some_and(|first| first <= previous));
        let rows_regressed = packed
            .rows
            .windows(2)
            .any(|rows| rows[0].seq() >= rows[1].seq());
        let continuation_regressed = packed
            .overflow
            .is_some_and(|overflow| last.is_some_and(|last| overflow.continuation <= last));
        if expected_regressed || history_regressed || rows_regressed || continuation_regressed {
            self.lose_continuity();
            return Err(());
        }
        let mut changed = false;
        for fact in &packed.rows {
            changed |= self.apply_fact(fact);
        }
        if let Some(last) = last {
            self.last_fact = Some(last);
        }
        self.continuation = packed.overflow.map(|overflow| overflow.continuation);
        let status = if self.continuation.is_some() {
            RoomProjectionStatus::Loading
        } else {
            RoomProjectionStatus::Available
        };
        changed |= self.status != status;
        self.status = status;
        Ok(changed)
    }

    /// Fold one later accepted fact after a complete snapshot.
    pub(super) fn apply_live_fact(&mut self, fact: &StreamFact) -> Result<bool, ()> {
        if self.status == RoomProjectionStatus::Loading
            && self.continuation.is_none()
            && self.last_fact.is_none()
        {
            // Other runtime surfaces accept standalone live frames in tests and
            // during connection setup. Room authority remains explicitly
            // loading until its own complete fact snapshot arrives.
            return Ok(false);
        }
        if self.status != RoomProjectionStatus::Available
            || self
                .last_fact
                .is_some_and(|previous| fact.seq() <= previous)
        {
            self.lose_continuity();
            return Err(());
        }
        self.last_fact = Some(fact.seq());
        Ok(self.apply_fact(fact))
    }

    fn apply_fact(&mut self, fact: &StreamFact) -> bool {
        self.apply_room_fact(fact)
            .or_else(|| self.apply_preparation_fact(fact))
            .or_else(|| self.apply_participation_fact(fact))
            .unwrap_or(false)
    }

    fn apply_room_fact(&mut self, fact: &StreamFact) -> Option<bool> {
        match fact.kind() {
            FactKind::WorkingGroupStored => Some(
                fact.read::<authority::WorkingGroupStored>()
                    .is_some_and(|body| {
                        replace(
                            &mut self.groups,
                            body.group.group.clone(),
                            fact.seq(),
                            body.group,
                        )
                    }),
            ),
            FactKind::WorkingGroupRemoved => Some(
                fact.read::<authority::WorkingGroupRemoved>()
                    .is_some_and(|body| self.groups.remove(&body.group).is_some()),
            ),
            FactKind::RoomPublished => {
                Some(fact.read::<authority::RoomPublished>().is_some_and(|body| {
                    self.insert_event(
                        body.room,
                        body.publication,
                        RoomEvent {
                            position: fact.seq(),
                            kind: RoomEventKind::Published(body.content),
                        },
                    )
                }))
            }
            FactKind::RoomMessaged => {
                Some(fact.read::<authority::RoomMessaged>().is_some_and(|body| {
                    self.insert_event(
                        body.room,
                        body.publication,
                        RoomEvent {
                            position: fact.seq(),
                            kind: RoomEventKind::Messaged {
                                source: body.hunk,
                                author: body.author,
                                directed: body.directed,
                            },
                        },
                    )
                }))
            }
            FactKind::RoomCoordinationStarted => Some(
                fact.read::<authority::RoomCoordinationStarted>()
                    .is_some_and(|body| {
                        let changed = self.insert_event(
                            body.room.clone(),
                            body.publication,
                            RoomEvent {
                                position: fact.seq(),
                                kind: RoomEventKind::Coordination {
                                    key: body.key.clone(),
                                    target: body.target,
                                },
                            },
                        );
                        let current = CoordinationState {
                            room: body.room,
                            target: body.target,
                            live: true,
                        };
                        changed | replace_plain(&mut self.coordination, body.key, current)
                    }),
            ),
            FactKind::RoomPutAside => Some(
                fact.read::<authority::RoomPutAside>()
                    .is_some_and(|body| self.apply_put_aside(fact.seq(), body)),
            ),
            _ => None,
        }
    }

    fn apply_put_aside(&mut self, position: Seq, body: authority::RoomPutAside) -> bool {
        let changed = self.insert_event(
            body.room.clone(),
            body.publication,
            RoomEvent {
                position,
                kind: RoomEventKind::PutAside {
                    canceled: body.canceled.clone(),
                    stop: body.stop.clone(),
                },
            },
        );
        let mut authority_changed = false;
        for key in body.canceled {
            if let Some(request) = self.coordination.get_mut(&key)
                && request.live
            {
                request.live = false;
                authority_changed = true;
            }
        }
        authority_changed |= match body.stop {
            Some(stop) => replace(&mut self.stops, body.room, position, stop),
            None => self.stops.remove(&body.room).is_some(),
        };
        changed | authority_changed
    }

    fn apply_preparation_fact(&mut self, fact: &StreamFact) -> Option<bool> {
        match fact.kind() {
            FactKind::CandidateRetained => Some(
                fact.read::<authority::CandidateRetained>()
                    .is_some_and(|body| {
                        replace(
                            &mut self.preparation.candidates,
                            body.candidate.candidate.clone(),
                            fact.seq(),
                            body.candidate,
                        )
                    }),
            ),
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
            FactKind::OutputControlled => Some(
                fact.read::<authority::OutputControlled>()
                    .is_some_and(|body| {
                        replace(
                            &mut self.preparation.controls,
                            body.output,
                            fact.seq(),
                            body.preparation,
                        )
                    }),
            ),
            _ => None,
        }
    }

    fn apply_participation_fact(&mut self, fact: &StreamFact) -> Option<bool> {
        match fact.kind() {
            FactKind::LaneStarted => {
                Some(fact.read::<wire::body::LaneStarted>().is_some_and(|body| {
                    let address = wire::OccupantAddress::new(fact.seq());
                    self.address_lanes.insert(address, body.lane) != Some(body.lane)
                }))
            }
            FactKind::ClaimWon => Some(fact.read::<wire::body::ClaimWon>().is_some_and(|body| {
                self.active_claims
                    .insert(body.task, (fact.seq(), body.lane))
                    != Some((fact.seq(), body.lane))
            })),
            FactKind::ClaimFreed
            | FactKind::ClaimLapsed
            | FactKind::TaskDone
            | FactKind::TaskReviewed
            | FactKind::TaskDeleted
            | FactKind::TaskClosed => Some(
                fact.task()
                    .is_some_and(|task| self.active_claims.remove(&task).is_some()),
            ),
            FactKind::SteerAcked => Some(
                fact.read::<wire::body::SteerAcked>()
                    .is_some_and(|body| self.retire_coordination(&body.key)),
            ),
            FactKind::SteerDeclined => Some(
                fact.read::<wire::body::SteerDeclined>()
                    .is_some_and(|body| self.retire_coordination(&body.key)),
            ),
            FactKind::SteerLapsed => Some(
                fact.read::<wire::body::SteerLapsed>()
                    .is_some_and(|body| self.retire_coordination(&body.key)),
            ),
            FactKind::SteerDeadLettered => Some(
                fact.read::<wire::steering::SteerDeadLettered>()
                    .is_some_and(|body| self.retire_coordination(&body.key)),
            ),
            FactKind::SteerCanceled => Some(
                fact.read::<wire::body::SteerCanceled>()
                    .is_some_and(|body| self.retire_coordination(&body.key)),
            ),
            FactKind::SteerRecovered => Some(
                fact.read::<wire::body::SteerRecovered>()
                    .is_some_and(|body| self.recover_coordination(&body.key)),
            ),
            _ => None,
        }
    }

    fn insert_event(&mut self, room: TaskStamp, id: RoomAuthorityStamp, event: RoomEvent) -> bool {
        replace_plain(&mut self.rooms.entry(room).or_default().events, id, event)
    }

    fn retire_coordination(&mut self, key: &str) -> bool {
        self.coordination.get_mut(key).is_some_and(|state| {
            let changed = state.live;
            state.live = false;
            changed
        })
    }

    fn recover_coordination(&mut self, key: &str) -> bool {
        self.coordination.get_mut(key).is_some_and(|state| {
            let changed = !state.live;
            state.live = true;
            changed
        })
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
