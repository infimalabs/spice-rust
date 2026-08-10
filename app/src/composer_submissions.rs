//! Browser-owned steering submissions that survive socket replacement.
//!
//! A submission stops being a draft before the resident has necessarily seen
//! it. This queue owns that interval. Socket code may advance a record, but a
//! socket attempt never owns the record itself, so dropping a reader or writer
//! cannot drop operator prose with it.

use std::collections::{BTreeMap, VecDeque};

use wire::CanonicalUuid;
use wire::Disposition;
use wire::body::DirectiveAttachment;
use wire::intent::PendingDirective;

use crate::composer_drafts::ComposerDraft;

/// One target-specific document detached from its composer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Submission {
    pub(crate) id: CanonicalUuid,
    pub(crate) fanout: CanonicalUuid,
    pub(crate) lane: CanonicalUuid,
    pub(crate) message: String,
    pub(crate) attachments: Vec<DirectiveAttachment>,
    pub(crate) snapshot: ComposerDraft,
    stage: Stage,
}

impl Submission {
    pub(crate) fn new(
        id: CanonicalUuid,
        fanout: CanonicalUuid,
        lane: CanonicalUuid,
        residency: (CanonicalUuid, CanonicalUuid),
        message: String,
        attachments: Vec<DirectiveAttachment>,
        snapshot: ComposerDraft,
    ) -> Self {
        let (occupant, epoch) = residency;
        Self {
            id,
            fanout,
            lane,
            message,
            attachments,
            snapshot,
            stage: Stage::Queued { occupant, epoch },
        }
    }

    /// Detach a document for a bound seat that has no current occupant.
    ///
    /// No residency is guessed here. A successful wake and a causally later
    /// lane read must supply both values before this submission can become a
    /// steering command.
    pub(crate) fn wake(
        id: CanonicalUuid,
        fanout: CanonicalUuid,
        lane: CanonicalUuid,
        message: String,
        attachments: Vec<DirectiveAttachment>,
        snapshot: ComposerDraft,
    ) -> Self {
        Self {
            id,
            fanout,
            lane,
            message,
            attachments,
            snapshot,
            stage: Stage::WakeQueued,
        }
    }

    pub(crate) fn residency(&self) -> Option<(CanonicalUuid, CanonicalUuid)> {
        match &self.stage {
            Stage::Queued { occupant, epoch } => Some((*occupant, *epoch)),
            Stage::WakeQueued
            | Stage::WakeSent
            | Stage::AwaitingResidency
            | Stage::Sent { .. }
            | Stage::Accepted { .. } => None,
        }
    }
}

/// How far one submission has crossed the browser-to-resident boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Stage {
    /// The document needs a vacant bound lane woken before it can be steered.
    WakeQueued,
    /// The wake bytes crossed the socket and await their command response.
    WakeSent,
    /// Wake succeeded; only a causally subsequent lane read may bind residency.
    AwaitingResidency,
    Queued {
        occupant: CanonicalUuid,
        epoch: CanonicalUuid,
    },
    Sent {
        occupant: CanonicalUuid,
        epoch: CanonicalUuid,
    },
    Accepted {
        directive: CanonicalUuid,
        key: String,
        disposition: Disposition,
    },
}

/// Renderable projection of all submissions not yet covered by a fresh inbox.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Prediction {
    directive: CanonicalUuid,
    key: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SubmissionStatus {
    rows: BTreeMap<CanonicalUuid, Vec<Prediction>>,
    keyless: usize,
}

impl SubmissionStatus {
    pub(crate) fn is_unsafe(&self) -> bool {
        self.keyless != 0
    }

    pub(crate) fn len(&self) -> usize {
        self.rows.values().map(Vec::len).sum()
    }

    /// Backend truth plus predictions whose stable identity it does not carry.
    pub(crate) fn pending(&self, lane: CanonicalUuid, backend: &[PendingDirective]) -> usize {
        backend.len() + self.uncovered(lane, backend)
    }

    /// Every backend row plus predictions absent from their own lane's row.
    pub(crate) fn total_pending<'a>(
        &self,
        backend: impl Iterator<Item = (CanonicalUuid, &'a [PendingDirective])>,
    ) -> usize {
        let backend = backend.collect::<BTreeMap<_, _>>();
        backend.values().map(|pending| pending.len()).sum::<usize>()
            + self
                .rows
                .keys()
                .map(|lane| self.uncovered(*lane, backend.get(lane).copied().unwrap_or(&[])))
                .sum::<usize>()
    }

    fn uncovered(&self, lane: CanonicalUuid, backend: &[PendingDirective]) -> usize {
        let durable = backend
            .iter()
            .map(|directive| (directive.directive, directive.key.as_str()))
            .collect::<BTreeMap<_, _>>();
        self.rows
            .get(&lane)
            .into_iter()
            .flatten()
            .filter(|prediction| match prediction.key.as_deref() {
                Some(key) => durable
                    .get(&prediction.directive)
                    .is_none_or(|backend_key| *backend_key != key),
                None => !durable.contains_key(&prediction.directive),
            })
            .count()
    }
}

/// Per-lane submission queues shared across every connection attempt.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SubmissionQueue(
    BTreeMap<CanonicalUuid, VecDeque<Submission>>,
    BTreeMap<CanonicalUuid, String>,
);

impl SubmissionQueue {
    pub(crate) fn enqueue(&mut self, submission: Submission) -> Result<(), String> {
        if self
            .0
            .values()
            .flatten()
            .any(|candidate| candidate.id == submission.id)
        {
            return Err(format!(
                "composer submission identity {} is already queued",
                submission.id
            ));
        }
        self.0
            .entry(submission.lane)
            .or_default()
            .push_back(submission);
        Ok(())
    }

    /// Remove an enqueue whose wake could not reach the connection driver.
    pub(crate) fn rollback(&mut self, id: CanonicalUuid) -> Option<Submission> {
        let lane = self.lane_for(id)?;
        let rows = self.0.get_mut(&lane)?;
        let index = rows.iter().position(|submission| {
            submission.id == id
                && matches!(&submission.stage, Stage::WakeQueued | Stage::Queued { .. })
        })?;
        let removed = rows.remove(index);
        if rows.is_empty() {
            self.0.remove(&lane);
        }
        removed
    }

    /// At most one ready submission from each concrete lane.
    pub(crate) fn ready(&self) -> Vec<Submission> {
        self.0
            .values()
            .filter(|rows| {
                !rows
                    .iter()
                    .any(|submission| matches!(&submission.stage, Stage::Sent { .. }))
            })
            .filter_map(|rows| {
                rows.iter()
                    .find(|submission| !matches!(&submission.stage, Stage::Accepted { .. }))
                    .filter(|submission| matches!(&submission.stage, Stage::Queued { .. }))
                    .cloned()
            })
            .collect()
    }

    /// At most one wake that is the first unsettled document in its lane.
    pub(crate) fn ready_wakes(&self) -> Vec<Submission> {
        self.0
            .values()
            .filter(|rows| {
                !rows.iter().any(|submission| {
                    matches!(
                        &submission.stage,
                        Stage::WakeSent | Stage::AwaitingResidency | Stage::Sent { .. }
                    )
                })
            })
            .filter_map(|rows| {
                rows.iter()
                    .find(|submission| !matches!(&submission.stage, Stage::Accepted { .. }))
                    .filter(|submission| submission.stage == Stage::WakeQueued)
                    .cloned()
            })
            .collect()
    }

    pub(crate) fn wake_sent(&mut self, id: CanonicalUuid) -> Result<(), String> {
        let submission = self.submission_mut(id)?;
        if submission.stage != Stage::WakeQueued {
            return Err(format!(
                "composer submission {id} did not need a wake when its bytes were sent"
            ));
        }
        submission.stage = Stage::WakeSent;
        Ok(())
    }

    pub(crate) fn sent(&mut self, id: CanonicalUuid) -> Result<(), String> {
        let submission = self.submission_mut(id)?;
        let Stage::Queued { occupant, epoch } = submission.stage.clone() else {
            return Err(format!(
                "composer submission {id} was not queued when its bytes were sent"
            ));
        };
        submission.stage = Stage::Sent { occupant, epoch };
        Ok(())
    }

    /// Make every result-less write eligible for replay with the same identity.
    pub(crate) fn reconnect(&mut self) {
        for submission in self.0.values_mut().flatten() {
            submission.stage = match submission.stage.clone() {
                Stage::WakeSent => Stage::WakeQueued,
                Stage::Sent { occupant, epoch } => Stage::Queued { occupant, epoch },
                other => other,
            };
        }
    }

    /// Admit the wake result without inventing the residency it did not carry.
    pub(crate) fn wake_accepted(&mut self, id: CanonicalUuid) -> Result<(), String> {
        let submission = self.submission_mut(id)?;
        if submission.stage != Stage::WakeSent {
            return Err(format!(
                "composer submission {id} received a wake result before wake was sent"
            ));
        }
        submission.stage = Stage::AwaitingResidency;
        Ok(())
    }

    /// Wake submissions awaiting a lane read issued after their wake replies.
    pub(crate) fn awaiting_residency(&self) -> Vec<CanonicalUuid> {
        self.0
            .values()
            .flatten()
            .filter(|submission| submission.stage == Stage::AwaitingResidency)
            .map(|submission| submission.id)
            .collect()
    }

    /// Lanes still awaiting residency among the identities one read nominated.
    ///
    /// Multiple refreshes may nominate the same identity. A later reply simply
    /// finds that the first one already advanced it and has nothing to do.
    pub(crate) fn awaiting_lanes(
        &self,
        ids: &[CanonicalUuid],
    ) -> std::collections::BTreeSet<CanonicalUuid> {
        ids.iter()
            .filter_map(|id| {
                let submission = self
                    .0
                    .values()
                    .flatten()
                    .find(|submission| submission.id == *id)?;
                (submission.stage == Stage::AwaitingResidency).then_some(submission.lane)
            })
            .collect()
    }

    /// Bind every unsent wake document in one lane to one fresh lane snapshot.
    pub(crate) fn bind_woken_lane(
        &mut self,
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        epoch: CanonicalUuid,
    ) {
        let Some(rows) = self.0.get_mut(&lane) else {
            return;
        };
        for submission in rows {
            if matches!(
                submission.stage,
                Stage::WakeQueued | Stage::AwaitingResidency
            ) {
                submission.stage = Stage::Queued { occupant, epoch };
            }
        }
    }

    pub(crate) fn accept(
        &mut self,
        id: CanonicalUuid,
        directive: CanonicalUuid,
        key: String,
        disposition: Disposition,
    ) -> Result<(), String> {
        if directive != id {
            return Err(format!(
                "composer submission {id} received mismatched directive {directive}"
            ));
        }
        if !matches!(&self.submission_mut(id)?.stage, Stage::Sent { .. }) {
            return Err(format!(
                "composer submission {id} received a result before it was sent"
            ));
        }
        let observed = self.1.get(&directive);
        if let Some(backend_key) = observed
            && backend_key != &key
        {
            return Err(format!(
                "composer submission {id} received key {key} after backend key {backend_key}"
            ));
        }
        if disposition != Disposition::Live || observed.is_some() {
            self.remove(id);
        } else {
            self.submission_mut(id)?.stage = Stage::Accepted {
                directive,
                key,
                disposition,
            };
        }
        Ok(())
    }

    /// Restore the refused write and every later write that never became durable.
    pub(crate) fn refuse(&mut self, id: CanonicalUuid) -> Result<Vec<ComposerDraft>, String> {
        self.refuse_when(id, |stage| matches!(stage, Stage::Sent { .. }))
    }

    pub(crate) fn refuse_wake(&mut self, id: CanonicalUuid) -> Result<Vec<ComposerDraft>, String> {
        self.refuse_when(id, |stage| *stage == Stage::WakeSent)
    }

    /// Restore a successful wake whose fresh lane read could not prove residency.
    pub(crate) fn refuse_woken_lane(
        &mut self,
        lane: CanonicalUuid,
    ) -> Result<Vec<ComposerDraft>, String> {
        let id = self
            .0
            .get(&lane)
            .into_iter()
            .flatten()
            .find(|submission| submission.stage == Stage::AwaitingResidency)
            .map(|submission| submission.id)
            .ok_or_else(|| format!("lane {lane} has no wake awaiting residency"))?;
        self.refuse_when(id, |stage| *stage == Stage::AwaitingResidency)
    }

    fn refuse_when(
        &mut self,
        id: CanonicalUuid,
        expected: impl FnOnce(&Stage) -> bool,
    ) -> Result<Vec<ComposerDraft>, String> {
        let lane = self
            .lane_for(id)
            .ok_or_else(|| format!("composer result named unknown submission {id}"))?;
        let rows = self
            .0
            .get_mut(&lane)
            .expect("a located submission retains its lane queue");
        let index = rows
            .iter()
            .position(|submission| submission.id == id)
            .expect("a located submission remains in its lane queue");
        if !expected(&rows[index].stage) {
            return Err(format!(
                "composer submission {id} was not awaiting a result"
            ));
        }
        let restored = rows
            .drain(index..)
            .map(|submission| submission.snapshot)
            .collect::<Vec<_>>();
        if rows.is_empty() {
            self.0.remove(&lane);
        }
        Ok(restored)
    }

    pub(crate) fn accepted(&self) -> BTreeMap<CanonicalUuid, String> {
        self.0
            .values()
            .flatten()
            .filter_map(|submission| match &submission.stage {
                Stage::Accepted { directive, key, .. } => Some((*directive, key.clone())),
                Stage::WakeQueued
                | Stage::WakeSent
                | Stage::AwaitingResidency
                | Stage::Queued { .. }
                | Stage::Sent { .. } => None,
            })
            .collect()
    }

    /// Retire by stable backend key or a read issued after acceptance.
    pub(crate) fn observe_backend<'a>(
        &mut self,
        pending: impl Iterator<Item = &'a PendingDirective>,
        covered: &BTreeMap<CanonicalUuid, String>,
    ) -> Result<(), String> {
        let backend = pending
            .map(|directive| (directive.directive, directive.key.clone()))
            .collect::<BTreeMap<_, _>>();
        for submission in self.0.values().flatten() {
            let Stage::Accepted { directive, key, .. } = &submission.stage else {
                continue;
            };
            for (source, observed) in [
                ("backend", backend.get(directive)),
                ("captured", covered.get(directive)),
            ] {
                if let Some(observed) = observed
                    && observed != key
                {
                    return Err(format!(
                        concat!("composer submission {} has {} key {} ", "instead of {}"),
                        directive, source, observed, key,
                    ));
                }
            }
        }
        self.1 = backend;
        self.0.retain(|_, rows| {
            rows.retain(|submission| match &submission.stage {
                Stage::Accepted { directive, key, .. } => {
                    self.1
                        .get(directive)
                        .is_none_or(|backend_key| backend_key != key)
                        && covered
                            .get(directive)
                            .is_none_or(|captured_key| captured_key != key)
                }
                Stage::WakeQueued
                | Stage::WakeSent
                | Stage::AwaitingResidency
                | Stage::Queued { .. }
                | Stage::Sent { .. } => true,
            });
            !rows.is_empty()
        });
        Ok(())
    }

    pub(crate) fn status(&self) -> SubmissionStatus {
        SubmissionStatus {
            rows: self
                .0
                .iter()
                .map(|(lane, rows)| {
                    (
                        *lane,
                        rows.iter()
                            .map(|submission| Prediction {
                                directive: submission.id,
                                key: match &submission.stage {
                                    Stage::Accepted { key, .. } => Some(key.clone()),
                                    Stage::WakeQueued
                                    | Stage::WakeSent
                                    | Stage::AwaitingResidency
                                    | Stage::Queued { .. }
                                    | Stage::Sent { .. } => None,
                                },
                            })
                            .collect(),
                    )
                })
                .collect(),
            keyless: self
                .0
                .values()
                .flatten()
                .filter(|submission| !matches!(&submission.stage, Stage::Accepted { .. }))
                .count(),
        }
    }

    fn lane_for(&self, id: CanonicalUuid) -> Option<CanonicalUuid> {
        self.0.iter().find_map(|(lane, rows)| {
            rows.iter()
                .any(|submission| submission.id == id)
                .then_some(*lane)
        })
    }

    fn submission_mut(&mut self, id: CanonicalUuid) -> Result<&mut Submission, String> {
        self.0
            .values_mut()
            .flatten()
            .find(|submission| submission.id == id)
            .ok_or_else(|| format!("composer result named unknown submission {id}"))
    }

    fn remove(&mut self, id: CanonicalUuid) {
        self.0.retain(|_, rows| {
            rows.retain(|submission| submission.id != id);
            !rows.is_empty()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use wire::Instant;

    const LANE_A: &str = "11111111-1111-4111-8111-111111111111";
    const LANE_B: &str = "22222222-2222-4222-8222-222222222222";
    const OCCUPANT: &str = "33333333-3333-4333-8333-333333333333";
    const EPOCH: &str = "44444444-4444-4444-8444-444444444444";
    const ID_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const ID_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    const ID_C: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    const FANOUT: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";

    fn uuid(value: &str) -> CanonicalUuid {
        value.parse().expect("fixture UUID is canonical")
    }

    fn draft(body: &str) -> ComposerDraft {
        ComposerDraft {
            body: body.to_owned(),
            quotes: Vec::new(),
            attachments: Vec::new(),
            issue: None,
        }
    }

    fn submission(id: &str, lane: &str, body: &str) -> Submission {
        Submission::new(
            uuid(id),
            uuid(id),
            uuid(lane),
            (uuid(OCCUPANT), uuid(EPOCH)),
            body.to_owned(),
            Vec::new(),
            draft(body),
        )
    }

    fn wake_submission(id: &str, lane: &str, body: &str) -> Submission {
        Submission::wake(
            uuid(id),
            uuid(id),
            uuid(lane),
            body.to_owned(),
            Vec::new(),
            draft(body),
        )
    }

    fn pending(id: &str) -> PendingDirective {
        PendingDirective {
            directive: uuid(id),
            key: "1kKey".to_owned(),
            text: "durable".to_owned(),
            preview: Some("durable".to_owned()),
            control: None,
            attachments: Vec::new(),
            authored: "2030-01-01T00:00:00.000Z".parse::<Instant>().unwrap(),
            stage: wire::body::SteeringStage::Published,
            escalations: Vec::new(),
            expires: "2030-01-01T00:05:00.000Z".parse::<Instant>().unwrap(),
        }
    }

    #[test]
    fn one_lineage_submit_keeps_one_fanout_across_distinct_target_commands() {
        let mut queue = SubmissionQueue::default();
        for (id, lane) in [(ID_A, LANE_A), (ID_B, LANE_B)] {
            queue
                .enqueue(Submission::new(
                    uuid(id),
                    uuid(FANOUT),
                    uuid(lane),
                    (uuid(OCCUPANT), uuid(EPOCH)),
                    "prefer rg for repository searches".to_owned(),
                    Vec::new(),
                    draft("prefer rg for repository searches"),
                ))
                .unwrap();
        }

        let ready = queue.ready();
        assert_eq!(ready.len(), 2);
        assert_ne!(ready[0].id, ready[1].id);
        assert!(
            ready
                .iter()
                .all(|submission| submission.fanout == uuid(FANOUT))
        );

        for id in [ID_A, ID_B] {
            queue.sent(uuid(id)).unwrap();
        }
        queue.reconnect();
        assert!(
            queue
                .ready()
                .iter()
                .all(|submission| submission.fanout == uuid(FANOUT))
        );
    }

    #[test]
    fn lanes_drain_independently_and_replay_the_same_identity() {
        let mut queue = SubmissionQueue::default();
        queue.enqueue(submission(ID_A, LANE_A, "first")).unwrap();
        queue.enqueue(submission(ID_B, LANE_A, "second")).unwrap();
        queue.enqueue(submission(ID_C, LANE_B, "sibling")).unwrap();
        assert_eq!(queue.status().len(), 3);

        assert_eq!(
            queue
                .ready()
                .iter()
                .map(|submission| submission.id)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([uuid(ID_A), uuid(ID_C)])
        );
        queue.sent(uuid(ID_A)).unwrap();
        queue.sent(uuid(ID_C)).unwrap();
        assert!(queue.ready().is_empty());

        queue.reconnect();
        assert_eq!(
            queue
                .ready()
                .iter()
                .map(|submission| submission.id)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([uuid(ID_A), uuid(ID_C)]),
            "connection replacement replays the original command identities"
        );
    }

    #[test]
    fn an_acceptance_opens_only_its_lane_and_a_refusal_restores_the_tail() {
        let mut queue = SubmissionQueue::default();
        queue.enqueue(submission(ID_A, LANE_A, "first")).unwrap();
        queue.enqueue(submission(ID_B, LANE_A, "second")).unwrap();
        queue.enqueue(submission(ID_C, LANE_A, "third")).unwrap();
        queue.sent(uuid(ID_A)).unwrap();
        queue
            .accept(
                uuid(ID_A),
                uuid(ID_A),
                "1kFirst".to_owned(),
                Disposition::Live,
            )
            .unwrap();
        assert_eq!(queue.ready()[0].id, uuid(ID_B));
        queue.sent(uuid(ID_B)).unwrap();

        let restored = queue.refuse(uuid(ID_B)).unwrap();
        assert_eq!(
            restored
                .iter()
                .map(|draft| draft.body.as_str())
                .collect::<Vec<_>>(),
            ["second", "third"]
        );
        assert!(queue.accepted().contains_key(&uuid(ID_A)));
    }

    #[test]
    fn pending_is_key_aware_and_a_post_acceptance_read_retires_the_floor() {
        let mut queue = SubmissionQueue::default();
        queue.enqueue(submission(ID_A, LANE_A, "first")).unwrap();
        queue.enqueue(submission(ID_B, LANE_A, "second")).unwrap();
        assert_eq!(queue.status().pending(uuid(LANE_A), &[]), 2);
        assert_eq!(
            queue.status().total_pending(
                [
                    (uuid(LANE_A), [pending(ID_A)].as_slice()),
                    (uuid(LANE_B), [pending(ID_C)].as_slice()),
                ]
                .into_iter()
            ),
            3,
            "the global total retains unmatched backend lanes and uncovered predictions"
        );
        assert_eq!(
            queue.status().pending(uuid(LANE_A), &[pending(ID_A)]),
            2,
            "a durable identity is counted once, never as backend plus prediction"
        );

        queue.sent(uuid(ID_A)).unwrap();
        queue
            .accept(
                uuid(ID_A),
                uuid(ID_A),
                "1kKey".to_owned(),
                Disposition::Live,
            )
            .unwrap();
        queue
            .observe_backend([pending(ID_A)].iter(), &BTreeMap::new())
            .unwrap();
        assert_eq!(queue.status().pending(uuid(LANE_A), &[pending(ID_A)]), 2);
        assert!(
            queue.status().is_unsafe(),
            "the second draft remains queued"
        );
    }

    #[test]
    fn accepted_stage_retains_the_backend_key_until_reconciliation() {
        let mut queue = SubmissionQueue::default();
        queue.enqueue(submission(ID_A, LANE_A, "first")).unwrap();
        queue.sent(uuid(ID_A)).unwrap();
        queue
            .accept(
                uuid(ID_A),
                uuid(ID_A),
                "1kFirst".to_owned(),
                Disposition::Live,
            )
            .unwrap();
        let row = &queue.0[&uuid(LANE_A)][0];
        assert_eq!(
            row.stage,
            Stage::Accepted {
                directive: uuid(ID_A),
                key: "1kFirst".to_owned(),
                disposition: Disposition::Live,
            }
        );
        assert!(
            !queue.status().is_unsafe(),
            "a keyed accepted record no longer risks operator prose"
        );
    }

    #[test]
    fn backend_key_or_reconstructed_retirement_settles_a_late_response() {
        let mut observed = SubmissionQueue::default();
        observed.enqueue(submission(ID_A, LANE_A, "first")).unwrap();
        observed.sent(uuid(ID_A)).unwrap();
        observed
            .observe_backend([pending(ID_A)].iter(), &BTreeMap::new())
            .unwrap();
        observed
            .accept(
                uuid(ID_A),
                uuid(ID_A),
                "1kKey".to_owned(),
                Disposition::Live,
            )
            .unwrap();
        assert_eq!(observed.status().len(), 0);

        let mut retired = SubmissionQueue::default();
        retired.enqueue(submission(ID_B, LANE_A, "second")).unwrap();
        retired.sent(uuid(ID_B)).unwrap();
        retired
            .accept(
                uuid(ID_B),
                uuid(ID_B),
                "1kRetired".to_owned(),
                Disposition::Acked,
            )
            .unwrap();
        assert_eq!(retired.status().len(), 0);

        let mut reconstructed = SubmissionQueue::default();
        reconstructed
            .enqueue(submission(ID_C, LANE_A, "third"))
            .unwrap();
        reconstructed.sent(uuid(ID_C)).unwrap();
        reconstructed
            .accept(
                uuid(ID_C),
                uuid(ID_C),
                "1kConsumed".to_owned(),
                Disposition::Live,
            )
            .unwrap();
        let covered = reconstructed.accepted();
        reconstructed.observe_backend([].iter(), &covered).unwrap();
        assert_eq!(
            reconstructed.status().len(),
            0,
            "a later inbox absence reconstructs the live receipt as consumed"
        );
    }

    #[test]
    fn a_failed_connection_wake_rolls_back_only_its_newest_queued_identity() {
        let mut queue = SubmissionQueue::default();
        queue.enqueue(submission(ID_A, LANE_A, "first")).unwrap();
        queue.enqueue(submission(ID_B, LANE_A, "second")).unwrap();
        assert_eq!(queue.rollback(uuid(ID_B)).unwrap().snapshot.body, "second");
        assert_eq!(queue.ready()[0].id, uuid(ID_A));
        assert!(queue.rollback(uuid(ID_C)).is_none());
    }

    #[test]
    fn wake_requires_its_reply_and_a_nominated_fresh_residency_before_steer() {
        let mut queue = SubmissionQueue::default();
        queue
            .enqueue(wake_submission(ID_A, LANE_A, "wake first"))
            .unwrap();
        queue
            .enqueue(wake_submission(ID_B, LANE_A, "wake second"))
            .unwrap();

        assert_eq!(queue.ready_wakes()[0].id, uuid(ID_A));
        queue.wake_sent(uuid(ID_A)).unwrap();
        assert!(queue.ready().is_empty());
        assert!(queue.ready_wakes().is_empty());

        queue.reconnect();
        assert_eq!(queue.ready_wakes()[0].id, uuid(ID_A));
        queue.wake_sent(uuid(ID_A)).unwrap();
        queue.wake_accepted(uuid(ID_A)).unwrap();
        let nominated = queue.awaiting_residency();
        assert_eq!(nominated, [uuid(ID_A)]);
        assert_eq!(
            queue.awaiting_lanes(&nominated),
            BTreeSet::from([uuid(LANE_A)])
        );
        assert!(queue.ready().is_empty());

        queue.bind_woken_lane(uuid(LANE_A), uuid(OCCUPANT), uuid(EPOCH));
        assert!(queue.awaiting_residency().is_empty());
        assert_eq!(queue.ready()[0].id, uuid(ID_A));
        assert_eq!(
            queue.ready()[0].residency(),
            Some((uuid(OCCUPANT), uuid(EPOCH)))
        );
        queue.sent(uuid(ID_A)).unwrap();
        queue
            .accept(
                uuid(ID_A),
                uuid(ID_A),
                "1kWake".to_owned(),
                Disposition::Live,
            )
            .unwrap();
        assert_eq!(queue.ready()[0].id, uuid(ID_B));
        assert_eq!(
            queue.ready()[0].residency(),
            Some((uuid(OCCUPANT), uuid(EPOCH))),
            "the same causal lane read binds later vacant-lane drafts without a redundant wake"
        );
    }

    #[test]
    fn wake_refusal_and_missing_residency_restore_every_unsent_lane_draft() {
        let mut refused = SubmissionQueue::default();
        refused
            .enqueue(wake_submission(ID_A, LANE_A, "refused first"))
            .unwrap();
        refused
            .enqueue(wake_submission(ID_B, LANE_A, "refused second"))
            .unwrap();
        refused.wake_sent(uuid(ID_A)).unwrap();
        assert_eq!(
            refused
                .refuse_wake(uuid(ID_A))
                .unwrap()
                .into_iter()
                .map(|draft| draft.body)
                .collect::<Vec<_>>(),
            ["refused first", "refused second"]
        );

        let mut unproven = SubmissionQueue::default();
        unproven
            .enqueue(wake_submission(ID_C, LANE_B, "unproven"))
            .unwrap();
        unproven.wake_sent(uuid(ID_C)).unwrap();
        unproven.wake_accepted(uuid(ID_C)).unwrap();
        assert_eq!(
            unproven.refuse_woken_lane(uuid(LANE_B)).unwrap()[0].body,
            "unproven"
        );
    }
}
