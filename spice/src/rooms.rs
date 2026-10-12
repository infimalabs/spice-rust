//! Derived task-room participation and listener attention.
//!
//! This module projects presentation state from authorities owned elsewhere. It
//! cannot create claims, lane or team membership, wake permission, task
//! visibility, or browser state. A room is still its anchor task; a lane joins
//! only by an admitted lane-authored publication. Current work, duties,
//! responsibilities, and recent substantive publications explain why a lane is
//! listening without creating another work lifecycle.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use wire::rooms::{RoomAuthorityStamp, RoomPublished};
use wire::{Actor, CanonicalUuid, Instant, TaskStamp};

/// Most authority signals one projection may inspect.
pub const MAX_ROOM_ATTENTION_SIGNALS: usize = 1_024;
/// Most lane rows one projection may return.
pub const MAX_ROOM_ATTENTION_LANES: usize = 64;

/// Positive repository-bounded work admitted for one room projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoomProjectionBudget {
    signals: usize,
    lanes: usize,
}

impl RoomProjectionBudget {
    /// Standard repository ceiling for one projection.
    pub const STANDARD: Self = Self {
        signals: MAX_ROOM_ATTENTION_SIGNALS,
        lanes: MAX_ROOM_ATTENTION_LANES,
    };

    /// Admit smaller positive limits beneath the repository ceilings.
    pub fn new(signals: usize, lanes: usize) -> Result<Self, RoomProjectionError> {
        if signals == 0 || signals > MAX_ROOM_ATTENTION_SIGNALS {
            return Err(RoomProjectionError::InvalidBudget {
                dimension: "signals",
                observed: signals,
                maximum: MAX_ROOM_ATTENTION_SIGNALS,
            });
        }
        if lanes == 0 || lanes > MAX_ROOM_ATTENTION_LANES {
            return Err(RoomProjectionError::InvalidBudget {
                dimension: "lanes",
                observed: lanes,
                maximum: MAX_ROOM_ATTENTION_LANES,
            });
        }
        Ok(Self { signals, lanes })
    }
}

/// One required projection source, either present or explicitly unavailable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Declared<T> {
    /// The caller obtained the complete bounded source view.
    Available(T),
    /// The owning authority could not supply a view.
    Unavailable { detail: String },
}

/// Current native work associated with one room and held by one lane.
///
/// Construction is derived from the current task/preparation and active-claim
/// projection. It is an input reference, not a claim or membership grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoomWork {
    room: TaskStamp,
    task: TaskStamp,
    lane: CanonicalUuid,
}

impl RoomWork {
    /// Derive relevant current work from one native task-head bundle.
    ///
    /// The anchor itself and tasks whose explicit preparation relationship
    /// names the anchor are relevant only while their claim remains active.
    #[must_use]
    pub fn from_task(room: &TaskStamp, task: &board::Entry) -> Option<Self> {
        let statement = task.statement();
        let related = task.born() == room
            || statement.preparation.as_ref().is_some_and(|preparation| {
                preparation.parent == *room
                    || preparation.controller == *room
                    || preparation.construction == *room
            });
        let claim = task
            .claim()
            .filter(|claim| claim.disposition == wire::ClaimDisposition::Active)?;
        related.then(|| Self {
            room: room.clone(),
            task: task.born().clone(),
            lane: claim.lane,
        })
    }
}

/// Current standing of a room-scoped delivery or result duty.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoomDutyStanding {
    /// The lane still owes this delivery or result.
    Owed,
    /// A terminal disposition ended the duty.
    Settled,
}

/// Kind and existing public key of one room-scoped duty.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoomDutyKind {
    /// An explicit request for involvement; it is not a contribution.
    Invitation { key: String },
    /// A directed delivery awaiting disposition.
    Delivery { key: String },
    /// A correlated substantive result awaiting the named lane.
    Result { key: String },
}

/// One current room-scoped duty from admitted delivery/result authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoomDuty {
    /// Anchor whose conversation carries the duty.
    pub room: TaskStamp,
    /// Logical lane that owes the disposition or result.
    pub lane: CanonicalUuid,
    /// Existing delivery/result identity and semantics.
    pub kind: RoomDutyKind,
    /// Whether the authority still requires action.
    pub standing: RoomDutyStanding,
}

/// One explicit continuing responsibility backed by a native task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoomResponsibility {
    /// Anchor whose conversation carries the responsibility.
    pub room: TaskStamp,
    /// Logical lane currently responsible.
    pub lane: CanonicalUuid,
    /// Native task that establishes the responsibility.
    pub task: TaskStamp,
}

/// Complete declared authority sources for a room projection.
///
/// These slices may cover several rooms; projection filters by anchor identity.
/// Unavailable authority is never treated as an empty slice.
pub struct RoomProjectionSources<'a> {
    /// Admitted immutable facts, including room publications.
    pub publications: Declared<&'a [wire::Envelope<wire::AnyFact>]>,
    /// Relevant current work derived from native task heads.
    pub work: Declared<&'a [RoomWork]>,
    /// Current room-scoped delivery and result duties.
    pub duties: Declared<&'a [RoomDuty]>,
    /// Explicit current responsibilities.
    pub responsibilities: Declared<&'a [RoomResponsibility]>,
    /// Current logical-lane topology and availability.
    pub residency: Declared<&'a wire::ResidencyHead>,
}

/// Required authority family that could not supply the projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum RoomProjectionSource {
    Publications,
    Work,
    Duties,
    Responsibilities,
    Residency,
}

/// Explicit unavailable source carried by a refused projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoomAuthorityUnavailable {
    pub source: RoomProjectionSource,
    pub detail: String,
}

/// Why a room projection could not be constructed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoomProjectionError {
    /// One or more required authority sources were unavailable.
    Unavailable(Vec<RoomAuthorityUnavailable>),
    /// A caller requested a zero or repository-exceeding limit.
    InvalidBudget {
        dimension: &'static str,
        observed: usize,
        maximum: usize,
    },
    /// Complete inputs exceed the admitted signal or lane work.
    BudgetExceeded {
        dimension: &'static str,
        observed: usize,
        maximum: usize,
    },
    /// Declared authority contradicts itself.
    InvalidAuthority(String),
}

impl fmt::Display for RoomProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(sources) => {
                write!(formatter, "room projection authority unavailable:")?;
                for source in sources {
                    write!(formatter, " {:?}: {}", source.source, source.detail)?;
                }
                Ok(())
            }
            Self::InvalidBudget {
                dimension,
                observed,
                maximum,
            } => write!(
                formatter,
                "room projection {dimension} budget must be within 1..={maximum}, observed {observed}"
            ),
            Self::BudgetExceeded {
                dimension,
                observed,
                maximum,
            } => write!(
                formatter,
                "room projection requires {observed} {dimension}, exceeding {maximum}"
            ),
            Self::InvalidAuthority(detail) => {
                write!(formatter, "room projection authority is invalid: {detail}")
            }
        }
    }
}

impl std::error::Error for RoomProjectionError {}

/// Durable explanation for a lane's participation in this room.
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum ParticipationReason {
    /// The authenticated lane authored this admitted publication.
    Contribution { publication: RoomAuthorityStamp },
}

/// Current reason a lane should receive bounded room catch-up.
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum AttentionReason {
    /// The lane has current relevant native work.
    Work { task: TaskStamp },
    /// The lane contributed substantively at or after the supplied cutoff.
    Activity {
        publication: RoomAuthorityStamp,
        at: Instant,
    },
    /// An explicit invitation remains owed; it does not make a participant.
    Invitation { key: String },
    /// A directed delivery remains owed.
    Delivery { key: String },
    /// A correlated substantive result remains owed.
    Result { key: String },
    /// Native work records an explicit continuing responsibility.
    Responsibility { task: TaskStamp },
}

/// Current availability of a lane named by the projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LaneAvailability {
    /// A current occupant is seated under an occupied residency row.
    Occupied,
    /// The known logical lane is between occupants.
    Vacant,
    /// The topology carries the lane but its row cannot currently receive.
    Unavailable { disposition: String },
    /// Current topology does not contain this referenced logical lane.
    Unknown,
}

/// One lane's participation, attention reasons, and delivery availability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoomLaneAttention {
    pub lane: CanonicalUuid,
    pub availability: LaneAvailability,
    pub participation: Vec<ParticipationReason>,
    pub attention: Vec<AttentionReason>,
}

impl RoomLaneAttention {
    /// Whether admitted authored evidence makes this lane a participant.
    #[must_use]
    pub fn participates(&self) -> bool {
        !self.participation.is_empty()
    }

    /// Whether at least one current reason requests bounded catch-up.
    #[must_use]
    pub fn listens(&self) -> bool {
        !self.attention.is_empty()
    }
}

/// Deterministic participation and listener view for one anchor task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoomAttention {
    pub room: TaskStamp,
    /// Rows ordered by canonical logical-lane identity.
    pub lanes: Vec<RoomLaneAttention>,
}

#[derive(Default)]
struct LaneFold {
    participation: BTreeSet<ParticipationReason>,
    attention: BTreeSet<AttentionReason>,
    latest_activity: Option<(Instant, RoomAuthorityStamp)>,
}

/// Derive one room's complete bounded attention view.
///
/// `activity_since` is supplied by the caller, keeping wall-clock policy out of
/// replay. Only admitted lane-authored room publications can create
/// participation or activity; replaying them is idempotent. Current duties and
/// work survive vacancy and activity expiry.
pub fn derive_room_attention(
    room: &TaskStamp,
    activity_since: Instant,
    sources: &RoomProjectionSources<'_>,
    budget: RoomProjectionBudget,
) -> Result<RoomAttention, RoomProjectionError> {
    let sources = available_sources(sources)?;
    require_signal_budget(&sources, budget)?;
    let topology = lane_topology(sources.residency)?;
    let mut lanes = BTreeMap::<CanonicalUuid, LaneFold>::new();
    fold_publications(room, sources.publications, &mut lanes, budget.lanes)?;
    fold_attention(room, &sources, &mut lanes, budget.lanes)?;
    Ok(RoomAttention {
        room: room.clone(),
        lanes: finish_lanes(lanes, &topology, activity_since),
    })
}

struct AvailableSources<'a> {
    publications: &'a [wire::Envelope<wire::AnyFact>],
    work: &'a [RoomWork],
    duties: &'a [RoomDuty],
    responsibilities: &'a [RoomResponsibility],
    residency: &'a wire::ResidencyHead,
}

fn available_sources<'a>(
    sources: &'a RoomProjectionSources<'a>,
) -> Result<AvailableSources<'a>, RoomProjectionError> {
    let mut unavailable = Vec::new();
    let publications = declared(
        &sources.publications,
        RoomProjectionSource::Publications,
        &mut unavailable,
    )
    .copied();
    let work = declared(&sources.work, RoomProjectionSource::Work, &mut unavailable).copied();
    let duties = declared(
        &sources.duties,
        RoomProjectionSource::Duties,
        &mut unavailable,
    )
    .copied();
    let responsibilities = declared(
        &sources.responsibilities,
        RoomProjectionSource::Responsibilities,
        &mut unavailable,
    )
    .copied();
    let residency = declared(
        &sources.residency,
        RoomProjectionSource::Residency,
        &mut unavailable,
    )
    .copied();
    if !unavailable.is_empty() {
        return Err(RoomProjectionError::Unavailable(unavailable));
    }
    let (Some(publications), Some(work), Some(duties), Some(responsibilities), Some(residency)) =
        (publications, work, duties, responsibilities, residency)
    else {
        unreachable!("every unavailable source was returned above")
    };
    Ok(AvailableSources {
        publications,
        work,
        duties,
        responsibilities,
        residency,
    })
}

fn require_signal_budget(
    sources: &AvailableSources<'_>,
    budget: RoomProjectionBudget,
) -> Result<(), RoomProjectionError> {
    let observed = sources
        .publications
        .len()
        .checked_add(sources.work.len())
        .and_then(|count| count.checked_add(sources.duties.len()))
        .and_then(|count| count.checked_add(sources.responsibilities.len()))
        .and_then(|count| count.checked_add(sources.residency.lanes.len()))
        .unwrap_or(usize::MAX);
    if observed > budget.signals {
        return Err(RoomProjectionError::BudgetExceeded {
            dimension: "signals",
            observed,
            maximum: budget.signals,
        });
    }
    Ok(())
}

fn fold_publications(
    room: &TaskStamp,
    publications: &[wire::Envelope<wire::AnyFact>],
    lanes: &mut BTreeMap<CanonicalUuid, LaneFold>,
    maximum: usize,
) -> Result<(), RoomProjectionError> {
    let mut seen = BTreeMap::<RoomAuthorityStamp, (CanonicalUuid, Instant)>::new();
    for envelope in publications {
        let Some(publication) = envelope.fact.read::<RoomPublished>() else {
            continue;
        };
        if publication.room != *room {
            continue;
        }
        let Actor::Lane(lane) = &envelope.actor else {
            continue;
        };
        let lane = *lane;
        if let Some((known_lane, known_at)) = seen.get(&publication.publication) {
            if *known_lane != lane || *known_at != envelope.at {
                return Err(RoomProjectionError::InvalidAuthority(format!(
                    "publication {} has conflicting authorship or time",
                    publication.publication
                )));
            }
            continue;
        }
        seen.insert(publication.publication.clone(), (lane, envelope.at));
        let folded = lane_fold(lanes, lane, maximum)?;
        folded
            .participation
            .insert(ParticipationReason::Contribution {
                publication: publication.publication.clone(),
            });
        let observed = (envelope.at, publication.publication);
        if folded
            .latest_activity
            .as_ref()
            .is_none_or(|known| observed > *known)
        {
            folded.latest_activity = Some(observed);
        }
    }
    Ok(())
}

fn fold_attention(
    room: &TaskStamp,
    sources: &AvailableSources<'_>,
    lanes: &mut BTreeMap<CanonicalUuid, LaneFold>,
    maximum: usize,
) -> Result<(), RoomProjectionError> {
    for current in sources.work.iter().filter(|current| current.room == *room) {
        lane_fold(lanes, current.lane, maximum)?
            .attention
            .insert(AttentionReason::Work {
                task: current.task.clone(),
            });
    }
    for duty in sources
        .duties
        .iter()
        .filter(|duty| duty.room == *room && duty.standing == RoomDutyStanding::Owed)
    {
        let reason = match &duty.kind {
            RoomDutyKind::Invitation { key } => AttentionReason::Invitation { key: key.clone() },
            RoomDutyKind::Delivery { key } => AttentionReason::Delivery { key: key.clone() },
            RoomDutyKind::Result { key } => AttentionReason::Result { key: key.clone() },
        };
        lane_fold(lanes, duty.lane, maximum)?
            .attention
            .insert(reason);
    }
    for responsibility in sources
        .responsibilities
        .iter()
        .filter(|responsibility| responsibility.room == *room)
    {
        lane_fold(lanes, responsibility.lane, maximum)?
            .attention
            .insert(AttentionReason::Responsibility {
                task: responsibility.task.clone(),
            });
    }
    Ok(())
}

fn finish_lanes(
    lanes: BTreeMap<CanonicalUuid, LaneFold>,
    topology: &BTreeMap<CanonicalUuid, LaneAvailability>,
    activity_since: Instant,
) -> Vec<RoomLaneAttention> {
    lanes
        .into_iter()
        .map(|(lane, mut folded)| {
            if let Some((at, publication)) = folded.latest_activity
                && at >= activity_since
            {
                folded
                    .attention
                    .insert(AttentionReason::Activity { publication, at });
            }
            RoomLaneAttention {
                lane,
                availability: topology
                    .get(&lane)
                    .cloned()
                    .unwrap_or(LaneAvailability::Unknown),
                participation: folded.participation.into_iter().collect(),
                attention: folded.attention.into_iter().collect(),
            }
        })
        .collect()
}

fn declared<'a, T>(
    source: &'a Declared<T>,
    kind: RoomProjectionSource,
    unavailable: &mut Vec<RoomAuthorityUnavailable>,
) -> Option<&'a T> {
    match source {
        Declared::Available(value) => Some(value),
        Declared::Unavailable { detail } => {
            unavailable.push(RoomAuthorityUnavailable {
                source: kind,
                detail: detail.clone(),
            });
            None
        }
    }
}

fn lane_fold(
    lanes: &mut BTreeMap<CanonicalUuid, LaneFold>,
    lane: CanonicalUuid,
    maximum: usize,
) -> Result<&mut LaneFold, RoomProjectionError> {
    if !lanes.contains_key(&lane) && lanes.len() == maximum {
        return Err(RoomProjectionError::BudgetExceeded {
            dimension: "lanes",
            observed: lanes.len().saturating_add(1),
            maximum,
        });
    }
    Ok(lanes.entry(lane).or_default())
}

fn lane_topology(
    residency: &wire::ResidencyHead,
) -> Result<BTreeMap<CanonicalUuid, LaneAvailability>, RoomProjectionError> {
    let mut topology = BTreeMap::new();
    for lane in &residency.lanes {
        let availability = match (lane.disposition.as_str(), lane.occupant) {
            ("occupied", Some(_)) => LaneAvailability::Occupied,
            ("vacant", None) => LaneAvailability::Vacant,
            (disposition, _) => LaneAvailability::Unavailable {
                disposition: disposition.to_owned(),
            },
        };
        if topology.insert(lane.lane, availability).is_some() {
            return Err(RoomProjectionError::InvalidAuthority(format!(
                "residency contains lane {} more than once",
                lane.lane
            )));
        }
    }
    Ok(topology)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wire::rooms::RoomPublicationContent;
    use wire::{
        ClaimCell, ClaimDisposition, ClaimHead, Fact, LaneResidency, LeaseHead, Phase,
        ResidentState, Seq, TaskHead, TaskPreparation, WireVersion, WorkflowHead,
    };

    fn uuid(last: &str) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last}")
            .parse()
            .expect("canonical fixture uuid")
    }

    fn task(text: &str) -> TaskStamp {
        text.parse().expect("fixture task stamp")
    }

    fn at(minute: u8) -> Instant {
        format!("2026-10-12T07:{minute:02}:00.000Z")
            .parse()
            .expect("fixture instant")
    }

    fn publication(
        room: &TaskStamp,
        identity: &str,
        lane: CanonicalUuid,
        at: Instant,
        seq: u64,
    ) -> wire::Envelope<wire::AnyFact> {
        let fact = Fact {
            body: RoomPublished {
                room: room.clone(),
                publication: identity.parse().expect("publication identity"),
                content: RoomPublicationContent::Authored {
                    text: "substantive contribution".to_owned(),
                },
            },
        };
        wire::Envelope {
            v: WireVersion::CURRENT,
            seq: Seq::new(seq).expect("positive sequence"),
            at,
            actor: Actor::Lane(lane),
            origin: "fixture:room-attention".parse().expect("fixture origin"),
            cause: None,
            fact: wire::decode(&wire::encode(&fact).expect("encode publication"))
                .expect("decode opaque publication"),
        }
    }

    fn lane(lane: CanonicalUuid, occupant: Option<CanonicalUuid>) -> LaneResidency {
        LaneResidency {
            lane,
            team: lane,
            name: Some("Lane".to_owned()),
            worktree: "/worktree".to_owned(),
            life: "steer".to_owned(),
            vendor: "omp".parse().expect("vendor name"),
            model: None,
            effort: None,
            fast_mode: false,
            occupant,
            lineage: Vec::new(),
            capabilities: Vec::new(),
            session: None,
            disposition: if occupant.is_some() {
                "occupied".to_owned()
            } else {
                "vacant".to_owned()
            },
        }
    }

    fn residency(lanes: Vec<LaneResidency>) -> wire::ResidencyHead {
        wire::ResidencyHead {
            epoch: uuid("00"),
            state: ResidentState::Seated,
            topology_revision: 0,
            lanes,
            subscriptions: Vec::new(),
            renewals: Vec::new(),
        }
    }
    fn phase() -> Phase {
        "todo".parse().expect("fixture phase")
    }

    fn current_task(
        room: &TaskStamp,
        task: TaskStamp,
        lane: CanonicalUuid,
        disposition: ClaimDisposition,
    ) -> board::Entry {
        let statement = TaskHead {
            born: task.clone(),
            queue_origin: Seq::new(1).expect("positive sequence"),
            title: "Relevant room work".to_owned(),
            description: None,
            acceptance: Vec::new(),
            requirements: wire::EnvironmentRequirements::default(),
            project: "spice.rooms".to_owned(),
            tags: Vec::new(),
            dependencies: Vec::new(),
            intent: None,
            preparation: Some(TaskPreparation {
                parent: room.clone(),
                controller: room.clone(),
                construction: room.clone(),
            }),
            authorization: None,
            provenance: format!("task:{room}").parse().expect("fixture provenance"),
            creation: wire::body::TaskCreationContext::unavailable(
                wire::body::TaskCreationUnavailable::SpiceActor,
            ),
            revision: Seq::new(1).expect("positive sequence"),
        };
        let workflow = WorkflowHead {
            flow: vec![phase()],
            phase: phase(),
            status: "pending".to_owned(),
            review: "unassigned".to_owned(),
            assignment: None,
            deferral: None,
            completion: None,
            generation: 0,
        };
        let claim = ClaimHead {
            actor: Actor::Lane(lane),
            fact: Seq::new(2).expect("positive sequence"),
            claimed: Seq::new(2).expect("positive sequence"),
            cell: ClaimCell {
                project: "spice.rooms".to_owned(),
                phase: phase(),
            },
            lane,
            occupant: uuid("03"),
            epoch: uuid("04"),
            recovery: None,
            disposition,
            acknowledged: 0,
        };
        let lease = (disposition == ClaimDisposition::Active).then(|| LeaseHead {
            start: at(1),
            renewal: at(2),
            expiry: at(59),
            maximum: at(59),
            epoch: uuid("04"),
        });
        let projects = board::ProjectPolicy::new(["spice".to_owned()], Vec::new(), 2, 3)
            .expect("fixture project policy");
        board::Entry::new(statement, workflow, Some(claim), lease, &projects)
            .expect("fixture task heads agree")
    }

    fn sources<'a>(
        publications: &'a [wire::Envelope<wire::AnyFact>],
        work: &'a [RoomWork],
        duties: &'a [RoomDuty],
        responsibilities: &'a [RoomResponsibility],
        residency: &'a wire::ResidencyHead,
    ) -> RoomProjectionSources<'a> {
        RoomProjectionSources {
            publications: Declared::Available(publications),
            work: Declared::Available(work),
            duties: Declared::Available(duties),
            responsibilities: Declared::Available(responsibilities),
            residency: Declared::Available(residency),
        }
    }

    #[test]
    fn overlapping_authorities_explain_participation_and_attention() {
        let room = task("1kKRRsDR0");
        let output = task("1kKRRsDR2");
        let lane_id = uuid("01");
        let publications = vec![publication(&room, "1kKRRsDR1", lane_id, at(40), 1)];
        let work = vec![
            RoomWork::from_task(
                &room,
                &current_task(&room, output.clone(), lane_id, ClaimDisposition::Active),
            )
            .expect("active associated task is relevant"),
        ];
        let duties = vec![
            RoomDuty {
                room: room.clone(),
                lane: lane_id,
                kind: RoomDutyKind::Delivery {
                    key: "1kKRRsDS0".to_owned(),
                },
                standing: RoomDutyStanding::Owed,
            },
            RoomDuty {
                room: room.clone(),
                lane: lane_id,
                kind: RoomDutyKind::Result {
                    key: "1kKRRsDS1".to_owned(),
                },
                standing: RoomDutyStanding::Owed,
            },
        ];
        let responsibilities = vec![RoomResponsibility {
            room: room.clone(),
            lane: lane_id,
            task: room.clone(),
        }];
        let topology = residency(vec![lane(lane_id, Some(uuid("02")))]);

        let projected = derive_room_attention(
            &room,
            at(30),
            &sources(&publications, &work, &duties, &responsibilities, &topology),
            RoomProjectionBudget::STANDARD,
        )
        .expect("complete authority projects");

        let row = projected.lanes.first().expect("lane row");
        assert!(row.participates());
        assert!(row.listens());
        assert_eq!(row.availability, LaneAvailability::Occupied);
        assert_eq!(row.participation.len(), 1);
        assert_eq!(row.attention.len(), 5);
    }

    #[test]
    fn native_work_requires_an_explicit_relationship_and_active_claim() {
        let room = task("1kKRRsDR0");
        let other = task("1kKRRsDR4");
        let output = task("1kKRRsDR2");
        let lane_id = uuid("01");
        let active = current_task(&room, output.clone(), lane_id, ClaimDisposition::Active);
        assert!(RoomWork::from_task(&room, &active).is_some());
        assert!(RoomWork::from_task(&other, &active).is_none());

        let freed = current_task(&room, output, lane_id, ClaimDisposition::Freed);
        assert!(RoomWork::from_task(&room, &freed).is_none());
    }
    #[test]
    fn expiry_and_handoff_preserve_native_duties() {
        let room = task("1kKRRsDR0");
        let output = task("1kKRRsDR2");
        let old_lane = uuid("01");
        let new_lane = uuid("02");
        let publications = vec![publication(&room, "1kKRRsDR1", old_lane, at(10), 1)];
        let prior = RoomWork::from_task(
            &room,
            &current_task(&room, output.clone(), old_lane, ClaimDisposition::Active),
        )
        .expect("original holder is relevant");
        let current = RoomWork::from_task(
            &room,
            &current_task(&room, output, new_lane, ClaimDisposition::Active),
        )
        .expect("successor holder is relevant");
        assert_eq!(prior.lane, old_lane);
        assert_eq!(current.lane, new_lane);
        let work = vec![current];
        let duties = vec![RoomDuty {
            room: room.clone(),
            lane: old_lane,
            kind: RoomDutyKind::Result {
                key: "1kKRRsDS0".to_owned(),
            },
            standing: RoomDutyStanding::Owed,
        }];
        let responsibilities = vec![RoomResponsibility {
            room: room.clone(),
            lane: old_lane,
            task: room.clone(),
        }];
        let topology = residency(vec![lane(new_lane, Some(uuid("03")))]);

        let projected = derive_room_attention(
            &room,
            at(30),
            &sources(&publications, &work, &duties, &responsibilities, &topology),
            RoomProjectionBudget::STANDARD,
        )
        .expect("handoff projects");

        let old = projected
            .lanes
            .iter()
            .find(|row| row.lane == old_lane)
            .expect("old participant remains visible");
        assert!(old.participates());
        assert_eq!(old.availability, LaneAvailability::Unknown);
        assert_eq!(old.attention.len(), 2);
        assert!(
            !old.attention
                .iter()
                .any(|reason| matches!(reason, AttentionReason::Activity { .. }))
        );
        let new = projected
            .lanes
            .iter()
            .find(|row| row.lane == new_lane)
            .expect("new holder listens");
        assert!(!new.participates());
        assert_eq!(new.attention.len(), 1);
        assert!(matches!(new.attention[0], AttentionReason::Work { .. }));
    }

    #[test]
    fn invitation_only_and_unknown_lanes_stay_explicit() {
        let room = task("1kKRRsDR0");
        let invited = uuid("01");
        let duties = vec![RoomDuty {
            room: room.clone(),
            lane: invited,
            kind: RoomDutyKind::Invitation {
                key: "1kKRRsDS0".to_owned(),
            },
            standing: RoomDutyStanding::Owed,
        }];
        let topology = residency(Vec::new());
        let projected = derive_room_attention(
            &room,
            at(30),
            &sources(&[], &[], &duties, &[], &topology),
            RoomProjectionBudget::STANDARD,
        )
        .expect("invitation projects");

        let row = projected.lanes.first().expect("invited lane row");
        assert!(!row.participates());
        assert!(row.listens());
        assert_eq!(row.availability, LaneAvailability::Unknown);
        assert!(matches!(
            row.attention[0],
            AttentionReason::Invitation { .. }
        ));
    }

    #[test]
    fn replay_renewal_and_settled_receipts_do_not_refresh_activity() {
        let room = task("1kKRRsDR0");
        let lane_id = uuid("01");
        let admitted = publication(&room, "1kKRRsDR1", lane_id, at(10), 1);
        let replayed = vec![admitted.clone(), admitted];
        let settled = vec![RoomDuty {
            room: room.clone(),
            lane: lane_id,
            kind: RoomDutyKind::Delivery {
                key: "1kKRRsDS0".to_owned(),
            },
            standing: RoomDutyStanding::Settled,
        }];
        let before = residency(vec![lane(lane_id, Some(uuid("02")))]);
        let after = residency(vec![lane(lane_id, Some(uuid("03")))]);

        let original = derive_room_attention(
            &room,
            at(30),
            &sources(&replayed[..1], &[], &settled, &[], &before),
            RoomProjectionBudget::STANDARD,
        )
        .expect("original projects");
        let rebuilt = derive_room_attention(
            &room,
            at(30),
            &sources(&replayed, &[], &settled, &[], &after),
            RoomProjectionBudget::STANDARD,
        )
        .expect("replay projects");

        assert_eq!(rebuilt, original);
        assert!(rebuilt.lanes[0].attention.is_empty());
    }

    #[test]
    fn one_lane_can_participate_in_several_rooms() {
        let first = task("1kKRRsDR0");
        let second = task("1kKRRsDR2");
        let lane_id = uuid("01");
        let publications = vec![
            publication(&first, "1kKRRsDR1", lane_id, at(40), 1),
            publication(&second, "1kKRRsDR3", lane_id, at(41), 2),
        ];
        let topology = residency(vec![lane(lane_id, Some(uuid("02")))]);
        let source = sources(&publications, &[], &[], &[], &topology);

        let first_view =
            derive_room_attention(&first, at(30), &source, RoomProjectionBudget::STANDARD)
                .expect("first room projects");
        let second_view =
            derive_room_attention(&second, at(30), &source, RoomProjectionBudget::STANDARD)
                .expect("second room projects");

        assert!(first_view.lanes[0].participates());
        assert!(second_view.lanes[0].participates());
    }

    #[test]
    fn unavailable_and_over_budget_authority_never_becomes_empty_success() {
        let room = task("1kKRRsDR0");
        let topology = residency(Vec::new());
        let unavailable = RoomProjectionSources {
            publications: Declared::Unavailable {
                detail: "room publication index unavailable".to_owned(),
            },
            work: Declared::Available(&[]),
            duties: Declared::Available(&[]),
            responsibilities: Declared::Available(&[]),
            residency: Declared::Available(&topology),
        };
        assert!(matches!(
            derive_room_attention(&room, at(30), &unavailable, RoomProjectionBudget::STANDARD),
            Err(RoomProjectionError::Unavailable(_))
        ));

        let lane_id = uuid("01");
        let publications = vec![
            publication(&room, "1kKRRsDR1", lane_id, at(40), 1),
            publication(&room, "1kKRRsDR2", lane_id, at(41), 2),
        ];
        assert!(matches!(
            derive_room_attention(
                &room,
                at(30),
                &sources(&publications, &[], &[], &[], &topology),
                RoomProjectionBudget::new(1, 1).expect("small positive budget")
            ),
            Err(RoomProjectionError::BudgetExceeded {
                dimension: "signals",
                ..
            })
        ));
    }
}
