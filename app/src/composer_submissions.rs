//! Browser-owned steering submissions that survive socket replacement.
//!
//! A submission stops being a draft before the resident has necessarily seen
//! it. This queue owns that interval. Socket code may advance a record, but a
//! socket attempt never owns the record itself, so dropping a reader or writer
//! cannot drop operator prose with it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

use wire::CanonicalUuid;
use wire::Disposition;
use wire::body::DirectiveAttachment;
use wire::intent::PendingDirective;

use crate::composer_drafts::{ComposerDraft, DraftImage, QuoteDraft};

const SUBMISSION_LIMIT: usize = 64;
const LANE_SUBMISSION_LIMIT: usize = 16;
const MEDIA_DATA_URL_BYTES: usize = wire::media::MAX_MEDIA_BYTES.div_ceil(3) * 4 + 64;
const MEDIA_RETAINED_BYTES: usize = MEDIA_DATA_URL_BYTES + 512;
const MESSAGE_BYTES: usize = wire::media::MAX_MEDIA_BYTES;
const PAYLOAD_METADATA_BYTES: usize =
    std::mem::size_of::<SubmissionPayload>() + 2 * std::mem::size_of::<usize>();
const MESSAGE_PAYLOAD_BYTES: usize =
    wire::media::MAX_MEDIA_ITEMS * MEDIA_RETAINED_BYTES + MESSAGE_BYTES + PAYLOAD_METADATA_BYTES;
const SUBMISSION_METADATA_BYTES: usize = std::mem::size_of::<Submission>();
const RETAINED_BYTES_LIMIT: usize = MESSAGE_PAYLOAD_BYTES * 2 + SUBMISSION_METADATA_BYTES;

/// One authored payload shared by every target-specific fanout record.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SubmissionPayload {
    id: CanonicalUuid,
    authored_millis: i64,
    pub(crate) message: String,
    pub(crate) attachments: Vec<DirectiveAttachment>,
}

impl SubmissionPayload {
    fn retained_bytes(&self) -> usize {
        PAYLOAD_METADATA_BYTES
            .saturating_add(self.message.capacity())
            .saturating_add(attachments_bytes(&self.attachments))
    }
}

/// One target-specific document detached from its composer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Submission {
    pub(crate) id: CanonicalUuid,
    pub(crate) fanout: CanonicalUuid,
    pub(crate) lane: CanonicalUuid,
    payload: Rc<SubmissionPayload>,
    pub(crate) snapshot: ComposerDraft,
    restore_lane: CanonicalUuid,
    stage: Stage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SubmissionRestore {
    pub(crate) lane: CanonicalUuid,
    pub(crate) draft: ComposerDraft,
}

impl Submission {
    pub(crate) fn new(
        id: CanonicalUuid,
        fanout: CanonicalUuid,
        lane: CanonicalUuid,
        residency: (CanonicalUuid, CanonicalUuid, i64),
        message: String,
        attachments: Vec<DirectiveAttachment>,
        snapshot: ComposerDraft,
    ) -> Self {
        let (occupant, epoch, authored_millis) = residency;
        Self::shared(
            id,
            fanout,
            lane,
            Self::payload(id, authored_millis, message, attachments),
            snapshot,
            lane,
            Stage::Queued {
                occupant: Some(occupant),
                epoch,
            },
        )
    }

    /// Detach a document for a bound seat that has no current occupant.
    ///
    /// The vacant residency is a sufficient steering witness. Publishing the
    /// directive first lets the resident's pending-inbox lifecycle wake the
    /// lane with that directive already visible.
    pub(crate) fn vacant(
        id: CanonicalUuid,
        fanout: CanonicalUuid,
        lane: CanonicalUuid,
        residency: (CanonicalUuid, i64),
        message: String,
        attachments: Vec<DirectiveAttachment>,
        snapshot: ComposerDraft,
    ) -> Self {
        let (epoch, authored_millis) = residency;
        Self::shared(
            id,
            fanout,
            lane,
            Self::payload(id, authored_millis, message, attachments),
            snapshot,
            lane,
            Stage::Queued {
                occupant: None,
                epoch,
            },
        )
    }

    pub(crate) fn payload(
        id: CanonicalUuid,
        authored_millis: i64,
        message: String,
        attachments: Vec<DirectiveAttachment>,
    ) -> Rc<SubmissionPayload> {
        Rc::new(SubmissionPayload {
            id,
            authored_millis,
            message,
            attachments,
        })
    }

    pub(crate) fn broadcast(
        id: CanonicalUuid,
        fanout: CanonicalUuid,
        lane: CanonicalUuid,
        residency: (CanonicalUuid, CanonicalUuid),
        payload: Rc<SubmissionPayload>,
        snapshot: ComposerDraft,
        restore_lane: CanonicalUuid,
    ) -> Self {
        let (occupant, epoch) = residency;
        Self::shared(
            id,
            fanout,
            lane,
            payload,
            snapshot,
            restore_lane,
            Stage::Queued {
                occupant: Some(occupant),
                epoch,
            },
        )
    }

    fn shared(
        id: CanonicalUuid,
        fanout: CanonicalUuid,
        lane: CanonicalUuid,
        payload: Rc<SubmissionPayload>,
        snapshot: ComposerDraft,
        restore_lane: CanonicalUuid,
        stage: Stage,
    ) -> Self {
        Self {
            id,
            fanout,
            lane,
            payload,
            snapshot,
            restore_lane,
            stage,
        }
    }

    pub(crate) fn message(&self) -> &str {
        &self.payload.message
    }

    pub(crate) fn attachments(&self) -> &[DirectiveAttachment] {
        &self.payload.attachments
    }

    fn retained_bytes(&self) -> usize {
        SUBMISSION_METADATA_BYTES.saturating_add(draft_bytes(&self.snapshot))
    }

    pub(crate) fn residency(&self) -> Option<(Option<CanonicalUuid>, CanonicalUuid)> {
        match &self.stage {
            Stage::Queued { occupant, epoch } => Some((*occupant, *epoch)),
            Stage::AwaitingResidency | Stage::Sent { .. } | Stage::Accepted { .. } => None,
        }
    }
}

fn attachments_bytes(attachments: &Vec<DirectiveAttachment>) -> usize {
    attachments.iter().fold(
        attachments
            .capacity()
            .saturating_mul(std::mem::size_of::<DirectiveAttachment>()),
        |bytes, attachment| {
            bytes
                .saturating_add(attachment.name.capacity())
                .saturating_add(attachment.content_type.capacity())
                .saturating_add(attachment.data_url.capacity())
        },
    )
}

fn draft_images_bytes(images: &Vec<DraftImage>) -> usize {
    images.iter().fold(
        images
            .capacity()
            .saturating_mul(std::mem::size_of::<DraftImage>()),
        |bytes, image| {
            let media_bytes = |media: &DirectiveAttachment| {
                media
                    .name
                    .capacity()
                    .saturating_add(media.content_type.capacity())
                    .saturating_add(media.data_url.capacity())
                    .saturating_add(std::mem::size_of::<DirectiveAttachment>())
            };
            bytes
                .saturating_add(media_bytes(&image.media))
                .saturating_add(if Rc::ptr_eq(&image.media, &image.original) {
                    0
                } else {
                    media_bytes(&image.original)
                })
                .saturating_add(
                    image
                        .shapes
                        .capacity()
                        .saturating_mul(std::mem::size_of::<crate::image_annotation::Shape>()),
                )
                .saturating_add(image.shapes.iter().map(|shape| shape.text.len()).sum())
        },
    )
}

fn draft_bytes(draft: &ComposerDraft) -> usize {
    draft.quotes.iter().fold(
        draft
            .body
            .capacity()
            .saturating_add(draft_images_bytes(&draft.attachments))
            .saturating_add(
                draft
                    .quotes
                    .capacity()
                    .saturating_mul(std::mem::size_of::<QuoteDraft>()),
            ),
        |bytes, quote| {
            bytes
                .saturating_add(quote.identity.capacity())
                .saturating_add(quote.preview.capacity())
                .saturating_add(quote.body.capacity())
                .saturating_add(draft_images_bytes(&quote.attachments))
        },
    )
}

fn admit_payload(payload: &SubmissionPayload) -> Result<(), String> {
    if payload.attachments.len() > wire::media::MAX_MEDIA_ITEMS {
        return Err(format!(
            "Submissions admit at most {} attachments.",
            wire::media::MAX_MEDIA_ITEMS
        ));
    }
    if payload.message.len() > MESSAGE_BYTES
        || payload
            .attachments
            .iter()
            .any(|attachment| attachment.data_url.len() > MEDIA_DATA_URL_BYTES)
        || payload.retained_bytes() > MESSAGE_PAYLOAD_BYTES
    {
        return Err(format!(
            "Submission payload exceeds the {MESSAGE_PAYLOAD_BYTES}-byte limit."
        ));
    }
    Ok(())
}

/// How far one submission has crossed the browser-to-resident boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Stage {
    /// A direct vacant-seat send committed; a fresh lane read must rebase its tail.
    AwaitingResidency,
    Queued {
        occupant: Option<CanonicalUuid>,
        epoch: CanonicalUuid,
    },
    Sent {
        occupant: Option<CanonicalUuid>,
        epoch: CanonicalUuid,
    },
    Accepted {
        directive: CanonicalUuid,
        key: String,
        disposition: Disposition,
    },
}

impl Stage {
    fn acceptance(&self) -> Option<(CanonicalUuid, &str)> {
        match self {
            Self::Accepted { directive, key, .. } => Some((*directive, key)),
            _ => None,
        }
    }
}

/// Renderable fold of all submissions not yet covered by a fresh inbox.
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
    /// Submissions the resident has not yet accepted, queued or in flight.
    pub(crate) fn awaiting(&self) -> usize {
        self.keyless
    }

    pub(crate) fn len(&self) -> usize {
        self.rows.values().map(Vec::len).sum()
    }

    /// Backend truth plus predictions whose stable identity it does not carry.
    pub(crate) fn pending(&self, lane: CanonicalUuid, backend: &[PendingDirective]) -> usize {
        backend.len() + self.uncovered(lane, backend)
    }

    /// Backend direction that can wake the lane plus uncovered predictions,
    /// which are the operator's own sends and so always wake it.
    pub(crate) fn waking(&self, lane: CanonicalUuid, backend: &[PendingDirective]) -> usize {
        backend.iter().filter(|directive| directive.wakes).count() + self.uncovered(lane, backend)
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct RetainedPayload {
    payload: Rc<SubmissionPayload>,
    records: usize,
}

/// Bounded per-lane submission queues shared across every connection attempt.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SubmissionQueue {
    rows: BTreeMap<CanonicalUuid, VecDeque<Submission>>,
    identities: BTreeMap<CanonicalUuid, CanonicalUuid>,
    backend: BTreeMap<CanonicalUuid, String>,
    activity_read_generation: u64,
    activity_read_required: Option<u64>,
    payloads: BTreeMap<CanonicalUuid, RetainedPayload>,
    retained_bytes: usize,
}

impl SubmissionQueue {
    pub(crate) fn enqueue(&mut self, submission: Submission) -> Result<(), String> {
        self.enqueue_all(vec![submission])
    }

    /// Admit a complete fanout batch before publishing any of its destinations.
    pub(crate) fn enqueue_all(&mut self, submissions: Vec<Submission>) -> Result<(), String> {
        if submissions.len() > SUBMISSION_LIMIT.saturating_sub(self.identities.len()) {
            return Err(format!(
                "{} submissions are already retained; wait for delivery before retrying.",
                self.identities.len()
            ));
        }
        let mut identities = BTreeSet::new();
        let mut lane_counts = BTreeMap::<CanonicalUuid, usize>::new();
        let mut payloads = BTreeMap::<CanonicalUuid, Rc<SubmissionPayload>>::new();
        let mut additional_bytes = 0usize;
        for submission in &submissions {
            if self.identities.contains_key(&submission.id) || !identities.insert(submission.id) {
                return Err(format!(
                    "composer submission identity {} is already queued",
                    submission.id
                ));
            }
            let lane_count = self.rows.get(&submission.lane).map_or(0, VecDeque::len)
                + *lane_counts.entry(submission.lane).or_default();
            if lane_count >= LANE_SUBMISSION_LIMIT {
                return Err(format!(
                    "{LANE_SUBMISSION_LIMIT} submissions are already retained for lane {}; wait for delivery before retrying.",
                    submission.lane
                ));
            }
            *lane_counts
                .get_mut(&submission.lane)
                .expect("the batch lane count was initialized") += 1;
            admit_payload(&submission.payload)?;
            additional_bytes = additional_bytes.saturating_add(submission.retained_bytes());
            let retained = self
                .payloads
                .get(&submission.payload.id)
                .map(|retained| &retained.payload)
                .or_else(|| payloads.get(&submission.payload.id));
            if retained.is_some_and(|payload| !Rc::ptr_eq(payload, &submission.payload)) {
                return Err(format!(
                    "composer payload identity {} was reused",
                    submission.payload.id
                ));
            }
            if retained.is_none() {
                additional_bytes =
                    additional_bytes.saturating_add(submission.payload.retained_bytes());
                payloads.insert(submission.payload.id, Rc::clone(&submission.payload));
            }
        }
        if additional_bytes > RETAINED_BYTES_LIMIT.saturating_sub(self.retained_bytes) {
            return Err(format!(
                "Submission retention exceeds the {RETAINED_BYTES_LIMIT}-byte limit; wait for delivery before retrying."
            ));
        }
        for mut submission in submissions {
            if matches!(&submission.stage, Stage::Queued { occupant: None, .. })
                && self.rows.get(&submission.lane).is_some_and(|rows| {
                    rows.iter().any(|row| row.stage == Stage::AwaitingResidency)
                })
            {
                submission.stage = Stage::AwaitingResidency;
            }
            self.retained_bytes = self
                .retained_bytes
                .saturating_add(submission.retained_bytes());
            self.identities.insert(submission.id, submission.lane);
            match self.payloads.entry(submission.payload.id) {
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().records += 1;
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    let bytes = submission.payload.retained_bytes();
                    self.retained_bytes = self.retained_bytes.saturating_add(bytes);
                    entry.insert(RetainedPayload {
                        payload: Rc::clone(&submission.payload),
                        records: 1,
                    });
                }
            }
            self.rows
                .entry(submission.lane)
                .or_default()
                .push_back(submission);
        }
        Ok(())
    }

    pub(crate) fn restore_lane(&self, id: CanonicalUuid) -> Result<CanonicalUuid, String> {
        self.submission(id)
            .map(|submission| submission.restore_lane)
    }

    /// Remove an enqueue whose command could not reach the connection vendor.
    pub(crate) fn rollback(&mut self, id: CanonicalUuid) -> Option<Submission> {
        let lane = self.lane_for(id)?;
        let rows = self.rows.get_mut(&lane)?;
        let index = rows.iter().position(|submission| {
            submission.id == id
                && matches!(
                    &submission.stage,
                    Stage::Queued { .. } | Stage::AwaitingResidency
                )
        })?;
        let removed = rows.remove(index);
        if rows.is_empty() {
            self.rows.remove(&lane);
        }
        if let Some(submission) = &removed {
            self.release(submission);
        }
        removed
    }

    /// At most one ready submission from each concrete lane.
    pub(crate) fn ready(&self) -> Vec<Submission> {
        self.rows
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
        for submission in self.rows.values_mut().flatten() {
            submission.stage = match submission.stage.clone() {
                Stage::Sent { occupant, epoch } => Stage::Queued { occupant, epoch },
                other => other,
            };
        }
    }

    /// Submissions whose vacant witness must be refreshed after an earlier send.
    pub(crate) fn awaiting_residency(&self) -> Vec<CanonicalUuid> {
        self.rows
            .values()
            .flatten()
            .filter(|submission| submission.stage == Stage::AwaitingResidency)
            .map(|submission| submission.id)
            .collect()
    }

    /// Lanes still awaiting residency among the identities one read nominated.
    #[cfg_attr(
        all(test, not(target_arch = "wasm32")),
        expect(
            dead_code,
            reason = "only the wasm32 browser connection's lane read checks awaiting lanes"
        )
    )]
    pub(crate) fn awaiting_lanes(&self, ids: &[CanonicalUuid]) -> BTreeSet<CanonicalUuid> {
        ids.iter()
            .filter_map(|id| {
                let submission = self.submission(*id).ok()?;
                (submission.stage == Stage::AwaitingResidency).then_some(submission.lane)
            })
            .collect()
    }

    /// Rebase only submissions nominated when this lane read started.
    pub(crate) fn rebase_awaiting_lane(
        &mut self,
        ids: &[CanonicalUuid],
        lane: CanonicalUuid,
        occupant: Option<CanonicalUuid>,
        epoch: CanonicalUuid,
    ) {
        for id in ids {
            if self.lane_for(*id) != Some(lane) {
                continue;
            }
            let submission = self
                .submission_mut(*id)
                .expect("a nominated submission retains its lane queue");
            if submission.stage == Stage::AwaitingResidency {
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
    ) -> Result<Option<Rc<SubmissionPayload>>, String> {
        if directive != id {
            return Err(format!(
                "composer submission {id} received mismatched directive {directive}"
            ));
        }
        let vacant = match &self.submission(id)?.stage {
            Stage::Sent { occupant, .. } => occupant.is_none(),
            _ => {
                return Err(format!(
                    "composer submission {id} received a result before it was sent"
                ));
            }
        };
        let lane = self
            .lane_for(id)
            .expect("an admitted submission retains its lane identity");
        let observed = self.backend.get(&directive).cloned();
        if let Some(backend_key) = &observed
            && backend_key != &key
        {
            return Err(format!(
                "composer submission {id} received key {key} after backend key {backend_key}"
            ));
        }
        let accepted = if disposition == Disposition::Live {
            Some(Rc::clone(&self.submission(id)?.payload))
        } else {
            None
        };
        if disposition != Disposition::Live || observed.is_some() {
            self.remove(id);
        } else {
            self.submission_mut(id)?.stage = Stage::Accepted {
                directive,
                key,
                disposition,
            };
        }
        if vacant && let Some(rows) = self.rows.get_mut(&lane) {
            for submission in rows {
                if matches!(&submission.stage, Stage::Queued { occupant: None, .. }) {
                    submission.stage = Stage::AwaitingResidency;
                }
            }
        }
        Ok(accepted)
    }

    /// Restore the refused write and every later write that never became durable.
    pub(crate) fn refuse(&mut self, id: CanonicalUuid) -> Result<Vec<SubmissionRestore>, String> {
        self.refuse_when(id, |stage| matches!(stage, Stage::Sent { .. }))
    }

    /// Restore the first nominated tail whose fresh lane read found no target.
    #[cfg_attr(
        all(test, not(target_arch = "wasm32")),
        expect(
            dead_code,
            reason = "only the wasm32 browser connection's lane read refuses a nominated tail"
        )
    )]
    pub(crate) fn refuse_awaiting_lane(
        &mut self,
        lane: CanonicalUuid,
        ids: &[CanonicalUuid],
    ) -> Result<Vec<SubmissionRestore>, String> {
        let id = ids
            .iter()
            .copied()
            .find(|id| {
                self.lane_for(*id) == Some(lane)
                    && self
                        .submission(*id)
                        .is_ok_and(|submission| submission.stage == Stage::AwaitingResidency)
            })
            .ok_or_else(|| format!("lane {lane} has no submission awaiting residency"))?;
        self.refuse_when(id, |stage| *stage == Stage::AwaitingResidency)
    }

    fn refuse_when(
        &mut self,
        id: CanonicalUuid,
        expected: impl FnOnce(&Stage) -> bool,
    ) -> Result<Vec<SubmissionRestore>, String> {
        let lane = self
            .lane_for(id)
            .ok_or_else(|| format!("composer result named unknown submission {id}"))?;
        let (removed, empty) = {
            let rows = self
                .rows
                .get_mut(&lane)
                .expect("an indexed submission retains its lane queue");
            let index = rows
                .iter()
                .position(|submission| submission.id == id)
                .expect("an indexed submission remains in its lane queue");
            if !expected(&rows[index].stage) {
                return Err(format!(
                    "composer submission {id} was not awaiting a result"
                ));
            }
            let removed = rows.drain(index..).collect::<Vec<_>>();
            (removed, rows.is_empty())
        };
        if empty {
            self.rows.remove(&lane);
        }
        for submission in &removed {
            self.release(submission);
        }
        Ok(removed
            .into_iter()
            .map(|submission| SubmissionRestore {
                lane: submission.restore_lane,
                draft: submission.snapshot,
            })
            .collect())
    }

    pub(crate) fn accepted(&self) -> BTreeMap<CanonicalUuid, String> {
        self.rows
            .values()
            .flatten()
            .filter_map(|submission| {
                submission
                    .stage
                    .acceptance()
                    .map(|(directive, key)| (directive, key.to_owned()))
            })
            .collect()
    }

    /// Causal generation that still requires a lane read to start.
    pub(crate) const fn activity_read_requirement(&self) -> Option<u64> {
        self.activity_read_required
    }

    /// Observe that one lane read carrying this causal generation started.
    pub(crate) fn activity_read_started(&mut self, generation: Option<u64>) {
        if generation.is_some() && generation == self.activity_read_required {
            self.activity_read_required = None;
        }
    }

    /// Retire accepted predictions after one causally fresh inbox observation.
    ///
    /// A consumed directive may already be running, so this records a lane-read
    /// generation that an older concurrent read cannot satisfy.
    pub(crate) fn observe_backend<'a>(
        &mut self,
        pending: impl Iterator<Item = &'a PendingDirective>,
        covered: &BTreeMap<CanonicalUuid, String>,
    ) -> Result<(), String> {
        let backend = pending
            .filter(|directive| self.identities.contains_key(&directive.directive))
            .map(|directive| (directive.directive, directive.key.clone()))
            .collect::<BTreeMap<_, _>>();
        for submission in self.rows.values().flatten() {
            let Some((directive, key)) = submission.stage.acceptance() else {
                continue;
            };
            for (source, observed) in [
                ("backend", backend.get(&directive)),
                ("captured", covered.get(&directive)),
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
        let consumed = self.rows.values().flatten().any(|submission| {
            submission
                .stage
                .acceptance()
                .is_some_and(|(directive, key)| {
                    !backend.contains_key(&directive)
                        && covered
                            .get(&directive)
                            .is_some_and(|observed| observed == key)
                })
        });
        let retired = self
            .rows
            .values()
            .flatten()
            .filter_map(|submission| {
                let (directive, key) = submission.stage.acceptance()?;
                [backend.get(&directive), covered.get(&directive)]
                    .into_iter()
                    .flatten()
                    .any(|observed| observed == key)
                    .then_some(submission.id)
            })
            .collect::<Vec<_>>();
        self.backend = backend;
        for id in retired {
            self.remove(id);
        }
        if consumed {
            self.activity_read_generation = self
                .activity_read_generation
                .checked_add(1)
                .expect("composer activity-read generation is exhausted");
            self.activity_read_required = Some(self.activity_read_generation);
        }
        Ok(())
    }

    pub(crate) fn status(&self) -> SubmissionStatus {
        SubmissionStatus {
            rows: self
                .rows
                .iter()
                .map(|(lane, rows)| {
                    (
                        *lane,
                        rows.iter()
                            .map(|submission| Prediction {
                                directive: submission.id,
                                key: submission.stage.acceptance().map(|(_, key)| key.to_owned()),
                            })
                            .collect(),
                    )
                })
                .collect(),
            keyless: self
                .rows
                .values()
                .flatten()
                .filter(|submission| !matches!(&submission.stage, Stage::Accepted { .. }))
                .count(),
        }
    }

    fn lane_for(&self, id: CanonicalUuid) -> Option<CanonicalUuid> {
        self.identities.get(&id).copied()
    }

    fn submission(&self, id: CanonicalUuid) -> Result<&Submission, String> {
        let lane = self
            .lane_for(id)
            .ok_or_else(|| format!("composer result named unknown submission {id}"))?;
        self.rows
            .get(&lane)
            .expect("an indexed submission retains its lane queue")
            .iter()
            .find(|submission| submission.id == id)
            .ok_or_else(|| format!("composer result named unknown submission {id}"))
    }

    fn submission_mut(&mut self, id: CanonicalUuid) -> Result<&mut Submission, String> {
        let lane = self
            .identities
            .get(&id)
            .copied()
            .ok_or_else(|| format!("composer result named unknown submission {id}"))?;
        self.rows
            .get_mut(&lane)
            .expect("an indexed submission retains its lane queue")
            .iter_mut()
            .find(|submission| submission.id == id)
            .ok_or_else(|| format!("composer result named unknown submission {id}"))
    }

    fn remove(&mut self, id: CanonicalUuid) {
        let _ = self.remove_submission(id);
    }

    fn remove_submission(&mut self, id: CanonicalUuid) -> Option<Submission> {
        let lane = self.lane_for(id)?;
        let (removed, empty) = {
            let rows = self
                .rows
                .get_mut(&lane)
                .expect("an indexed submission retains its lane queue");
            let index = rows.iter().position(|submission| submission.id == id)?;
            let removed = rows.remove(index);
            (removed, rows.is_empty())
        };
        if empty {
            self.rows.remove(&lane);
        }
        if let Some(submission) = &removed {
            self.release(submission);
        }
        removed
    }

    fn release(&mut self, submission: &Submission) {
        assert_eq!(
            self.identities.remove(&submission.id),
            Some(submission.lane),
            "released submission identity remains indexed to its lane"
        );
        self.backend.remove(&submission.id);
        self.retained_bytes = self
            .retained_bytes
            .saturating_sub(submission.retained_bytes());
        let remove_payload = {
            let payload = self
                .payloads
                .get_mut(&submission.payload.id)
                .expect("released submission retains its payload accounting");
            payload.records -= 1;
            payload.records == 0
        };
        if remove_payload {
            let payload = self
                .payloads
                .remove(&submission.payload.id)
                .expect("empty payload accounting remains present");
            self.retained_bytes = self
                .retained_bytes
                .saturating_sub(payload.payload.retained_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn numbered(value: usize) -> CanonicalUuid {
        format!("00000000-0000-4000-8000-{value:012x}")
            .parse()
            .expect("numbered fixture UUID is canonical")
    }

    fn draft(body: &str) -> ComposerDraft {
        ComposerDraft {
            body: body.to_owned(),
            ..ComposerDraft::default()
        }
    }

    fn submission(id: &str, lane: &str, body: &str) -> Submission {
        Submission::new(
            uuid(id),
            uuid(id),
            uuid(lane),
            (uuid(OCCUPANT), uuid(EPOCH), 1_000),
            body.to_owned(),
            Vec::new(),
            draft(body),
        )
    }

    fn vacant_submission(id: &str, lane: &str, body: &str) -> Submission {
        Submission::vacant(
            uuid(id),
            uuid(id),
            uuid(lane),
            (uuid(EPOCH), 1_000),
            body.to_owned(),
            Vec::new(),
            draft(body),
        )
    }

    fn broadcast(index: usize, lane: CanonicalUuid, payload: &Rc<SubmissionPayload>) -> Submission {
        Submission::broadcast(
            numbered(index),
            uuid(FANOUT),
            lane,
            (uuid(OCCUPANT), uuid(EPOCH)),
            Rc::clone(payload),
            ComposerDraft::default(),
            uuid(LANE_A),
        )
    }

    fn accept(queue: &mut SubmissionQueue, id: &str, key: &str, disposition: Disposition) {
        queue.sent(uuid(id)).unwrap();
        queue
            .accept(uuid(id), uuid(id), key.to_owned(), disposition)
            .unwrap();
    }

    fn pending(id: &str, key: &str) -> PendingDirective {
        PendingDirective {
            from: None,
            sender: None,
            room: None,
            addressed: None,
            wakes: true,
            fanout: None,
            directive: uuid(id),
            key: key.to_owned(),
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
    fn admission_bounds_batches_atomically_and_counts_one_shared_fanout_payload() {
        let payload = Submission::payload(
            uuid(FANOUT),
            1_000,
            "[BROADCAST] bounded".to_owned(),
            Vec::new(),
        );
        let batch = (0..SUBMISSION_LIMIT)
            .map(|index| broadcast(1_000 + index, numbered(2_000 + index), &payload))
            .collect();
        let mut queue = SubmissionQueue::default();
        queue.enqueue_all(batch).unwrap();
        assert_eq!(
            (queue.status().len(), queue.identities.len()),
            (SUBMISSION_LIMIT, SUBMISSION_LIMIT)
        );
        assert_eq!(queue.payloads[&uuid(FANOUT)].records, SUBMISSION_LIMIT);
        assert!(Rc::ptr_eq(&queue.payloads[&uuid(FANOUT)].payload, &payload));
        assert_eq!(queue.ready()[0].message(), "[BROADCAST] bounded");
        assert!(queue.ready()[0].attachments().is_empty());
        assert_eq!(queue.restore_lane(numbered(1_017)).unwrap(), uuid(LANE_A));
        let retained = queue.retained_bytes;
        assert!(
            queue
                .enqueue(broadcast(9_000, numbered(9_001), &payload))
                .is_err()
        );
        assert_eq!(
            (queue.status().len(), queue.retained_bytes),
            (SUBMISSION_LIMIT, retained)
        );

        let mut lane = SubmissionQueue::default();
        for index in 0..LANE_SUBMISSION_LIMIT {
            lane.enqueue(broadcast(10_000 + index, uuid(LANE_A), &payload))
                .unwrap();
        }
        assert!(lane.enqueue(submission(ID_A, LANE_A, "overflow")).is_err());

        let mut bytes = SubmissionQueue {
            retained_bytes: RETAINED_BYTES_LIMIT,
            ..SubmissionQueue::default()
        };
        assert!(bytes.enqueue(submission(ID_A, LANE_A, "overflow")).is_err());
    }

    #[test]
    fn indexed_fifo_replays_exact_commands_and_refusal_restores_the_lane_tail() {
        let mut queue = SubmissionQueue::default();
        queue.enqueue(submission(ID_A, LANE_A, "first")).unwrap();
        let mut second = submission(ID_B, LANE_A, "second");
        second.fanout = uuid(ID_A);
        queue.enqueue(second).unwrap();
        queue.enqueue(submission(ID_C, LANE_B, "sibling")).unwrap();
        queue.sent(uuid(ID_A)).unwrap();
        queue.sent(uuid(ID_C)).unwrap();
        assert!(queue.ready().is_empty());
        queue.reconnect();
        assert_eq!(
            queue
                .ready()
                .iter()
                .map(|row| row.id)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([uuid(ID_A), uuid(ID_C)])
        );

        accept(&mut queue, ID_A, "1kFirst", Disposition::Live);
        assert_eq!(queue.ready()[0].id, uuid(ID_B));
        queue.sent(uuid(ID_B)).unwrap();
        assert_eq!(queue.refuse(uuid(ID_B)).unwrap()[0].draft.body, "second");
        assert_eq!(queue.rollback(uuid(ID_C)).unwrap().snapshot.body, "sibling");
        assert_eq!(queue.identities.len(), 1);
    }

    #[test]
    fn peer_direction_counts_as_pending_but_only_operator_sends_wake_the_lane() {
        let mut queue = SubmissionQueue::default();
        queue.enqueue(submission(ID_A, LANE_A, "first")).unwrap();
        queue.enqueue(submission(ID_B, LANE_A, "second")).unwrap();
        accept(&mut queue, ID_A, "1kKey", Disposition::Live);
        let status = queue.status();
        let mut peer = pending(ID_C, "1kPeer");
        peer.wakes = false;
        let backend = [peer, pending(ID_A, "1kKey")];

        // The backend row covers the accepted send; the queued one is still
        // the operator's own prediction and wakes the lane like its row will.
        assert_eq!(status.pending(uuid(LANE_A), &backend), 3);
        assert_eq!(status.waking(uuid(LANE_A), &backend), 2);
        // A lane holding only a peer reply is pending but not queued.
        let idle = SubmissionQueue::default().status();
        assert_eq!(idle.pending(uuid(LANE_A), &backend[..1]), 1);
        assert_eq!(idle.waking(uuid(LANE_A), &backend[..1]), 0);
    }

    #[test]
    fn backend_keys_reconcile_accepted_predictions_without_losing_queued_work() {
        let mut queue = SubmissionQueue::default();
        queue.enqueue(submission(ID_A, LANE_A, "first")).unwrap();
        queue.enqueue(submission(ID_B, LANE_A, "second")).unwrap();
        accept(&mut queue, ID_A, "1kKey", Disposition::Live);
        assert_eq!(
            queue.status().awaiting(),
            1,
            "only the queued send awaits acceptance"
        );
        assert_eq!(queue.accepted()[&uuid(ID_A)], "1kKey");
        queue
            .observe_backend([pending(ID_A, "1kKey")].iter(), &BTreeMap::new())
            .unwrap();
        assert_eq!(
            queue.activity_read_requirement(),
            None,
            "a directive still pending does not imply lane activity changed"
        );
        assert_eq!(queue.status().len(), 1);
        assert_eq!(queue.ready()[0].id, uuid(ID_B));

        accept(&mut queue, ID_B, "1kDone", Disposition::Acked);
        assert!(queue.identities.is_empty());
    }

    #[test]
    fn a_consumed_accepted_submission_requires_newer_lane_activity() {
        let mut queue = SubmissionQueue::default();
        queue.enqueue(submission(ID_A, LANE_A, "start")).unwrap();
        accept(&mut queue, ID_A, "1kStarted", Disposition::Live);
        let coalesced = queue.activity_read_requirement();

        queue
            .observe_backend(
                std::iter::empty(),
                &BTreeMap::from([(uuid(ID_A), "1kStarted".to_owned())]),
            )
            .unwrap();

        let causal = queue.activity_read_requirement();
        assert!(causal.is_some());
        queue.activity_read_started(coalesced);
        assert_eq!(
            queue.activity_read_requirement(),
            causal,
            "a lane read started before inbox acceptance cannot satisfy the causal refresh"
        );
        queue.activity_read_started(causal);
        assert_eq!(queue.activity_read_requirement(), None);
        assert!(queue.identities.is_empty());
    }

    #[test]
    fn vacant_submission_sends_directly_replays_and_rebases_its_fifo_tail() {
        let mut queue = SubmissionQueue::default();
        queue
            .enqueue(vacant_submission(ID_A, LANE_A, "first"))
            .unwrap();
        queue
            .enqueue(vacant_submission(ID_B, LANE_A, "second"))
            .unwrap();
        assert_eq!(queue.ready()[0].residency(), Some((None, uuid(EPOCH))));

        queue.sent(uuid(ID_A)).unwrap();
        assert!(queue.ready().is_empty());
        queue.reconnect();
        assert_eq!(queue.ready()[0].id, uuid(ID_A));

        accept(&mut queue, ID_A, "1kFirst", Disposition::Live);
        assert!(queue.ready().is_empty());
        let first_read = queue.awaiting_residency();
        assert_eq!(first_read, [uuid(ID_B)]);
        queue
            .enqueue(vacant_submission(ID_C, LANE_A, "third"))
            .unwrap();
        queue.rebase_awaiting_lane(&first_read, uuid(LANE_A), None, uuid(EPOCH));
        assert_eq!(queue.ready()[0].id, uuid(ID_B));

        accept(&mut queue, ID_B, "1kSecond", Disposition::Live);
        assert!(queue.ready().is_empty());
        let second_read = queue.awaiting_residency();
        assert_eq!(second_read, [uuid(ID_C)]);
        queue.rebase_awaiting_lane(
            &second_read,
            uuid(LANE_A),
            Some(uuid(OCCUPANT)),
            uuid(EPOCH),
        );
        assert_eq!(
            queue.ready()[0].residency(),
            Some((Some(uuid(OCCUPANT)), uuid(EPOCH)))
        );
        queue.sent(uuid(ID_C)).unwrap();
        assert_eq!(
            queue
                .refuse(uuid(ID_C))
                .unwrap()
                .into_iter()
                .map(|restored| restored.draft.body)
                .collect::<Vec<_>>(),
            ["third"]
        );
    }
}
