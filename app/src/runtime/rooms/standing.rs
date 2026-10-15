//! Why each lane is in a room, where the room's coordination and automated
//! resumes stand, and its shadow relevance observations, each derived from
//! admitted authority alone.

use std::collections::BTreeMap;

use wire::rooms::{
    self as authority, QuestionResultState, ROOM_ACTIVITY_SECONDS, RoomDestination,
    RoomPublicationKind,
};
use wire::{CanonicalUuid, Disposition, Seq, TaskStamp};

use super::{
    BoardView, ParticipantReason, RoomEventKind, RoomParticipant, RoomPositioned, RoomProjection,
    associated,
};

const MICROS_PER_SECOND: i64 = 1_000_000;

/// One shadow relevance observation and the operator's assessment of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relevance {
    /// The resident's source-backed classification; never room authority.
    pub observation: authority::RoomRelevanceObservation,
    /// The operator's quality verdict, absent until assessed.
    pub verdict: Option<authority::RoomRelevanceVerdict>,
}

/// Where a room's operator-authorized automated resumes stand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResumeAutomation {
    /// The latest record carries no rule; only a manual Start or Resume
    /// continues the room.
    Off,
    /// Resumes remain. The next waits for the target to acknowledge the
    /// latest request and for another party to publish new input.
    Armed {
        /// Automated resumes the rule authorizes.
        limit: u8,
        /// Automated resumes already issued under it.
        spent: u8,
    },
    /// Every authorized resume was issued.
    Spent {
        /// Automated resumes the rule authorized.
        limit: u8,
    },
    /// A put-aside after the ruled record ended the rule.
    PutAside {
        /// Automated resumes the rule authorized.
        limit: u8,
        /// Automated resumes issued before the put-aside.
        spent: u8,
    },
    /// The target retired the latest request without acknowledging it.
    Ended {
        /// Automated resumes the rule authorized.
        limit: u8,
        /// Automated resumes issued before the request ended.
        spent: u8,
        /// How the request ended.
        disposition: Disposition,
    },
}

/// A room's latest coordination record and what it still allows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoordinationStanding<'a> {
    /// The room's latest operator or automated coordination record.
    pub record: &'a RoomPositioned<authority::RoomCoordinationStarted>,
    /// The request's steering disposition, absent until its direction folds.
    pub request: Option<Disposition>,
    /// Whether a put-aside followed the record.
    pub put_aside: bool,
    /// The record's automated-resume rule, if any, and what ended it.
    pub automation: ResumeAutomation,
}

impl RoomProjection {
    /// Current logical-lane participants and the exact reasons for their
    /// presence at `now_micros`. Contribution history and native duties stay;
    /// only activity and invitations leave once they age past
    /// [`ROOM_ACTIVITY_SECONDS`], and a leave later than the lane's latest
    /// contribution ends its activity and every invitation it postdates.
    pub fn participants(
        &self,
        room: &TaskStamp,
        board: Option<&BoardView>,
        now_micros: i64,
    ) -> Vec<RoomParticipant> {
        let since = now_micros.saturating_sub(ROOM_ACTIVITY_SECONDS * MICROS_PER_SECOND);
        let mut reasons = BTreeMap::<CanonicalUuid, Vec<ParticipantReason>>::new();
        let mut add = |lane: CanonicalUuid, reason| reasons.entry(lane).or_default().push(reason);
        if let Some(state) = self.rooms.get(room) {
            let mut latest = BTreeMap::new();
            let mut leaves = BTreeMap::new();
            let mut invitations = Vec::new();
            for (publication, event) in &state.events {
                let (directed, conversation) = match &event.kind {
                    RoomEventKind::Messaged {
                        source,
                        directed,
                        conversation,
                        ..
                    } => {
                        add(
                            source.head.lane,
                            ParticipantReason::Contribution(publication.clone()),
                        );
                        latest.insert(source.head.lane, publication);
                        (directed.as_slice(), conversation.as_ref())
                    }
                    RoomEventKind::Posted { .. } => (
                        self.conversation
                            .conversation(publication)
                            .map_or(&[][..], |posted| posted.directed.as_slice()),
                        Some(publication),
                    ),
                    RoomEventKind::Left { lane, clause } => {
                        leaves.insert(*lane, (publication, clause));
                        continue;
                    }
                    RoomEventKind::Contributed { lane, .. } => {
                        add(*lane, ParticipantReason::Contribution(publication.clone()));
                        latest.insert(*lane, publication);
                        continue;
                    }
                    _ => continue,
                };
                let answer = conversation
                    .and_then(|stamp| self.conversation.conversation(stamp))
                    .is_some_and(|conversation| {
                        matches!(
                            conversation.publication.kind(),
                            RoomPublicationKind::Ans { .. }
                        )
                    });
                let invites = !answer && publication.micros() >= since;
                for delivery in directed
                    .iter()
                    .filter_map(|directive| self.conversation.delivery(directive))
                {
                    let delivery = &delivery.value;
                    if invites {
                        invitations.push((delivery.lane, publication, delivery.key.clone()));
                    }
                    if delivery.disposition == Disposition::Live {
                        add(
                            delivery.lane,
                            ParticipantReason::Receipt(delivery.key.clone()),
                        );
                    }
                }
            }
            leaves.retain(|lane, (left, _)| latest.get(lane).is_none_or(|latest| *left > *latest));
            for (lane, publication, key) in invitations {
                if leaves
                    .get(&lane)
                    .is_none_or(|(left, _)| publication > *left)
                {
                    add(lane, ParticipantReason::Invitation(key));
                }
            }
            for (lane, publication) in latest {
                if publication.micros() >= since && !leaves.contains_key(&lane) {
                    add(lane, ParticipantReason::Activity(publication.clone()));
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

    /// Reasons a lane listens regardless of any leave: a pending result it
    /// owes, a claim on associated work, a live coordination request, and the
    /// stop it holds.
    fn native_reasons(
        &self,
        room: &TaskStamp,
        board: Option<&BoardView>,
        add: &mut impl FnMut(CanonicalUuid, ParticipantReason),
    ) {
        for result in self.conversation.results() {
            let record = &result.value.record;
            if let (RoomDestination::Lane { lane }, QuestionResultState::Pending {}) =
                (record.respondent(), &result.value.state)
                && record.context() == Some(room)
            {
                add(*lane, ParticipantReason::Result(record.result().clone()));
            }
        }
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

    /// The room's latest coordination record, its request's disposition, and
    /// where its automated-resume rule stands.
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
        let spent = record
            .value
            .automated
            .as_ref()
            .map_or(0, |automated| automated.step);
        let automation = match &record.value.rule {
            None => ResumeAutomation::Off,
            Some(rule) if put_aside => ResumeAutomation::PutAside {
                limit: rule.limit,
                spent,
            },
            Some(rule) if spent >= rule.limit => ResumeAutomation::Spent { limit: rule.limit },
            Some(rule) => match request {
                Some(
                    disposition @ (Disposition::Declined
                    | Disposition::Lapsed
                    | Disposition::DeadLettered
                    | Disposition::Canceled),
                ) => ResumeAutomation::Ended {
                    limit: rule.limit,
                    spent,
                    disposition,
                },
                _ => ResumeAutomation::Armed {
                    limit: rule.limit,
                    spent,
                },
            },
        };
        Some(CoordinationStanding {
            record,
            request,
            put_aside,
            automation,
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
