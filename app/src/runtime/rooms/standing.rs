//! Why each lane is in a room, where the room's coordination stands, and its
//! shadow relevance observations, each derived from admitted authority alone.

use std::collections::BTreeMap;

use wire::rooms as authority;
use wire::{CanonicalUuid, Disposition, Seq, TaskStamp};

use super::{
    BoardView, ParticipantReason, RoomEventKind, RoomParticipant, RoomPositioned, RoomProjection,
    associated,
};

/// One shadow relevance observation and the operator's assessment of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relevance {
    /// The resident's source-backed classification; never room authority.
    pub observation: authority::RoomRelevanceObservation,
    /// The operator's quality verdict, absent until assessed.
    pub verdict: Option<authority::RoomRelevanceVerdict>,
}

/// A room's latest coordination record and what it still allows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoordinationStanding<'a> {
    /// The room's latest operator coordination record.
    pub record: &'a RoomPositioned<authority::RoomCoordinationStarted>,
    /// The request's steering disposition, absent until its direction folds.
    pub request: Option<Disposition>,
    /// Whether a put-aside followed the record.
    pub put_aside: bool,
}

impl RoomProjection {
    /// Current logical-lane participants and the exact reasons for their
    /// presence. Publication history and native duties stay; authored room
    /// messages and typed contributions are participation, never attention,
    /// and do not end a leave. A Design request stands until the lane's next
    /// room publication, a leave included, answers or declines it.
    pub fn participants(
        &self,
        room: &TaskStamp,
        board: Option<&BoardView>,
    ) -> Vec<RoomParticipant> {
        let mut reasons = BTreeMap::<CanonicalUuid, Vec<ParticipantReason>>::new();
        let mut add = |lane: CanonicalUuid, reason| reasons.entry(lane).or_default().push(reason);
        if let Some(state) = self.rooms.get(room) {
            let mut leaves = BTreeMap::new();
            let mut published = BTreeMap::<CanonicalUuid, Seq>::new();
            for (publication, event) in &state.events {
                let lane = match &event.kind {
                    RoomEventKind::Messaged { source, .. } => {
                        let lane = source.head.lane;
                        add(lane, ParticipantReason::Publication(publication.clone()));
                        lane
                    }
                    RoomEventKind::Left { lane, clause } => {
                        leaves.insert(*lane, (publication, clause));
                        *lane
                    }
                    RoomEventKind::Contributed { lane, .. } => {
                        add(*lane, ParticipantReason::Publication(publication.clone()));
                        *lane
                    }
                    _ => continue,
                };
                let newest = published.entry(lane).or_insert(event.position);
                *newest = (*newest).max(event.position);
            }
            for (lane, (asked, evidence, questions)) in &state.asked {
                if published.get(lane).is_none_or(|answered| asked > answered) {
                    add(
                        *lane,
                        ParticipantReason::Asked {
                            evidence: evidence.clone(),
                            questions: questions.clone(),
                        },
                    );
                }
            }
            for (lane, (publication, clause)) in leaves {
                add(
                    lane,
                    ParticipantReason::Left {
                        publication: publication.clone(),
                        clause: clause.clone(),
                    },
                );
            }
        }
        self.native_reasons(room, board, &mut add);
        reasons
            .into_iter()
            .map(|(lane, mut reasons)| {
                reasons.sort();
                reasons.dedup();
                RoomParticipant { lane, reasons }
            })
            .collect()
    }

    /// Whether any lane currently has a reason to receive bounded catch-up.
    ///
    /// Participation and a leave never listen; Design requests and native
    /// duties do, matching the resident's attention.
    #[must_use]
    pub fn has_current_listener(&self, room: &TaskStamp, board: Option<&BoardView>) -> bool {
        self.participants(room, board)
            .iter()
            .flat_map(|participant| &participant.reasons)
            .any(|reason| match reason {
                ParticipantReason::Publication(_) | ParticipantReason::Left { .. } => false,
                ParticipantReason::Asked { .. }
                | ParticipantReason::Work(_)
                | ParticipantReason::Coordination(_)
                | ParticipantReason::Stop(_) => true,
            })
    }

    /// Whether an operator-authored post is still unproved as heard.
    ///
    /// Current listening authority suppresses the mark immediately. A later
    /// lane-authored room event suppresses it permanently, including after
    /// that lane leaves and no current listening reason remains, as does a
    /// later nudge whose message, which carried the room's unheard posts, its
    /// lane acknowledged or declined. Callers rendering several cards compute
    /// `has_current_listener` once.
    #[must_use]
    pub fn unheard_post(
        &self,
        room: &TaskStamp,
        publication: &authority::RoomAuthorityStamp,
        has_current_listener: bool,
    ) -> bool {
        let Some(state) = self.rooms.get(room) else {
            return false;
        };
        let Some(post) = state.events.get(publication) else {
            return false;
        };
        !has_current_listener
            && matches!(
                &post.kind,
                RoomEventKind::Published {
                    author: wire::Actor::Operator | wire::Actor::Human(_),
                    content: authority::RoomPublicationContent::Authored { .. },
                }
            )
            && state
                .latest_lane_event
                .is_none_or(|position| position <= post.position)
            && !state
                .nudges
                .range(post.position..)
                .any(|(_, message)| self.message_heard(message))
    }

    /// Whether the lane a nudge's `message` was directed to acknowledged or
    /// declined it.
    fn message_heard(&self, message: &authority::RoomAuthorityStamp) -> bool {
        self.conversation
            .conversation(message)
            .is_some_and(|conversation| {
                conversation.directed.iter().any(|directive| {
                    self.conversation
                        .delivery(directive)
                        .is_some_and(|delivery| {
                            matches!(
                                delivery.value.disposition,
                                Disposition::Acked | Disposition::Declined
                            )
                        })
                })
            })
    }

    /// Reasons a lane listens regardless of any leave: a claim on associated
    /// work, a live coordination request, and the stop it holds.
    fn native_reasons(
        &self,
        room: &TaskStamp,
        board: Option<&BoardView>,
        add: &mut impl FnMut(CanonicalUuid, ParticipantReason),
    ) {
        for task in board.into_iter().flat_map(|board| &board.tasks) {
            let statement = &task.statement.body;
            if associated(statement, room)
                && let Some(claim) = task
                    .claim
                    .as_ref()
                    .filter(|claim| claim.body.disposition == wire::ClaimDisposition::Active)
            {
                add(
                    claim.body.lane,
                    ParticipantReason::Work(statement.born.clone()),
                );
            }
        }
        for (key, request) in &self.coordination {
            if request.disposition == Disposition::Live && request.room == *room {
                add(request.lane, ParticipantReason::Coordination(key.clone()));
            }
        }
        if let Some((stop, lane)) = self.stop_holder(room, board) {
            add(lane, ParticipantReason::Stop(stop.value.key.clone()));
        }
    }

    /// The room's latest coordination record and its request's disposition.
    pub fn coordination_standing(&self, room: &TaskStamp) -> Option<CoordinationStanding<'_>> {
        let record = self.directed.get(room)?;
        let request = self
            .coordination
            .get(&record.value.key)
            .map(|state| state.disposition);
        let put_aside = self
            .shelved
            .get(room)
            .is_some_and(|shelf| *shelf > record.position);
        Some(CoordinationStanding {
            record,
            request,
            put_aside,
        })
    }

    /// The room's shadow relevance observations in identity order.
    pub fn relevance(
        &self,
        room: &TaskStamp,
    ) -> impl Iterator<Item = &RoomPositioned<Relevance>> + '_ {
        let room = room.clone();
        self.relevance
            .values()
            .filter(move |relevance| relevance.value.observation.room == room)
    }

    pub(super) fn apply_observed(
        &mut self,
        position: Seq,
        observation: authority::RoomRelevanceObservation,
    ) -> bool {
        let key = observation.observation.clone();
        let verdict = self
            .relevance
            .get(&key)
            .and_then(|relevance| relevance.value.verdict);
        super::replace(
            &mut self.relevance,
            key,
            position,
            Relevance {
                observation,
                verdict,
            },
        )
    }

    pub(super) fn apply_assessed(
        &mut self,
        assessment: &authority::RoomRelevanceAssessment,
    ) -> bool {
        self.relevance
            .get_mut(&assessment.observation)
            .is_some_and(|relevance| {
                relevance.value.verdict.replace(assessment.verdict) != Some(assessment.verdict)
            })
    }
}
