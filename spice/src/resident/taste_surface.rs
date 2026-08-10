//! Durable taste recognition over the sole admitted lane-capture path.

use std::collections::{BTreeMap, BTreeSet};

use bus::{Bus, JournalCommit};
use journal::{FactCause, GuardError, JournalGuard, JournalWrite};
use wire::body::TranscriptLocus;
use wire::body::{TasteDecision, TasteJudgeAttempt, TasteJudgeReply, TasteJudged};
use wire::{Actor, CanonicalUuid, Digest, Instant, Outcome, Producer, ResidencyHead, Seq};

use crate::message_shape::classify;

use super::{
    CommandResult, RuntimePaths, commit, current_residency, environment_fault, new_uuid, steering,
    store_fault,
};

const COMMIT_RETRIES: usize = 4;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Candidate {
    taste: String,
    content: Digest,
    bag: Digest,
    trigger: String,
    vendor: String,
    source: String,
    locus: TranscriptLocus,
    text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Judgment {
    pub(super) confirmed: bool,
    pub(super) attempts: Vec<TasteJudgeAttempt>,
}

#[derive(Clone, Debug)]
struct JudgedCandidate {
    candidate: Candidate,
    judgment: Judgment,
}

#[derive(Clone, Debug)]
struct Opening {
    candidate: Candidate,
    directive: CanonicalUuid,
    key: steer::Key,
}

pub(super) trait Adjudicator {
    fn adjudicate(&mut self, candidate: &Candidate) -> Result<Judgment, crate::judge::Error>;
}

#[derive(Debug, Default)]
pub(super) struct ProcessAdjudicator {
    process: crate::judge::Process,
}

impl Adjudicator for ProcessAdjudicator {
    fn adjudicate(&mut self, candidate: &Candidate) -> Result<Judgment, crate::judge::Error> {
        let decision = self.process.adjudicate(&judge_prompts(candidate))?;
        Ok(Judgment {
            confirmed: decision.confirmed(),
            attempts: decision
                .attempts()
                .iter()
                .map(|attempt| TasteJudgeAttempt {
                    framing: attempt.framing(),
                    reply: attempt.reply().to_owned(),
                    verdict: match attempt.answer() {
                        Some(true) => TasteJudgeReply::Yes,
                        Some(false) => TasteJudgeReply::No,
                        None => TasteJudgeReply::Ambiguous,
                    },
                })
                .collect(),
        })
    }
}

#[cfg(test)]
pub(super) struct FunctionAdjudicator<F>(pub(super) F);

#[cfg(test)]
impl<F> Adjudicator for FunctionAdjudicator<F>
where
    F: FnMut(&Candidate) -> Result<Judgment, crate::judge::Error>,
{
    fn adjudicate(&mut self, candidate: &Candidate) -> Result<Judgment, crate::judge::Error> {
        (self.0)(candidate)
    }
}

/// Recognize and durably raise every unsuppressed reminder in one admitted
/// capture record.
///
/// The caller has already proved the record belongs to the current supervised
/// occupant and committed any compaction boundary it carries. An empty bag or
/// a record without eligible prose performs no journal read at all.
pub(super) fn recognize(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    bag: &taste::Bag,
    steering_config: &steer::Config,
    judge: Option<&mut dyn Adjudicator>,
    record: &lane::capture::CaptureRecord,
) -> CommandResult<()> {
    recognize_with(
        bus,
        paths,
        epoch,
        bag,
        steering_config,
        judge,
        record,
        |bus, _, proposal| commit(bus, proposal),
        steering::publish_inbox,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "the test seam names both durable commit and disposable publication"
)]
fn recognize_with<C, P>(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    bag: &taste::Bag,
    steering_config: &steer::Config,
    mut judge: Option<&mut dyn Adjudicator>,
    record: &lane::capture::CaptureRecord,
    mut commit_write: C,
    mut publish: P,
) -> CommandResult<()>
where
    C: FnMut(&Bus, &[Opening], JournalWrite) -> CommandResult<JournalCommit>,
    P: FnMut(&Bus, &RuntimePaths, &ResidencyHead, CanonicalUuid) -> CommandResult<()>,
{
    let candidates = candidates(bag, record)?;
    if candidates.is_empty() {
        return Ok(());
    }
    let mut decision_cache = BTreeMap::<(Digest, Digest, u64), Option<Judgment>>::new();

    for attempt in 0..COMMIT_RETRIES {
        let observed = current_residency(bus, epoch)?;
        require_current_occupant(observed.body(), record.lane, record.occupant)?;
        let snapshot = bus
            .fact_snapshot()
            .map_err(|error| store_fault("taste::journal-unreadable", &error.to_string()))?;
        let anchor = context_anchor(&snapshot, record.lane, record.occupant)
            .map_err(|detail| store_fault("taste::epoch-missing", &detail))?;
        let unsuppressed = unsuppressed(&candidates, &snapshot, anchor);
        if unsuppressed.is_empty() {
            return Ok(());
        }

        let minted_millis = bus
            .read(|read| read.steering_minted_millis())
            .map_err(|error| store_fault("taste::steering-index-unreadable", &error.to_string()))?;
        let at = current_instant()?;
        let expires = expiry(at, steering_config.expiry().get())?;
        let (openings, proposal) = if let Some(adjudicator) = judge.as_deref_mut() {
            let decisions =
                adjudicate_candidates(unsuppressed, anchor, &mut decision_cache, adjudicator);
            if decisions.is_empty() {
                return Ok(());
            }
            judged_proposal(
                epoch,
                record.lane,
                record.occupant,
                anchor,
                decisions,
                minted_millis,
                at,
                expires,
            )?
        } else {
            reminder_proposal(
                epoch,
                record.lane,
                record.occupant,
                anchor,
                unsuppressed,
                minted_millis,
                at,
                expires,
            )?
        };

        match commit_write(bus, &openings, proposal) {
            Ok(_) => {
                // The shared fold authority turns an inbox failure into durable
                // parked progress. Only failure to record either publication
                // or parking remains an error here.
                if !openings.is_empty() {
                    publish(bus, paths, observed.body(), record.lane)?;
                }
                return Ok(());
            }
            Err(Outcome::Refusal(_)) if attempt + 1 < COMMIT_RETRIES => {
                // A command or duplicate capture may have won between the
                // optimistic read and the guarded commit. Rebuild from the
                // new authority: a duplicate disappears, while a key collision
                // receives a fresh key.
            }
            Err(outcome) => return Err(outcome),
        }
    }
    unreachable!("the final commit attempt returns instead of retrying")
}

fn unsuppressed(
    candidates: &[Candidate],
    snapshot: &journal::FactSnapshot,
    anchor: Seq,
) -> Vec<Candidate> {
    candidates
        .iter()
        .filter(|candidate| {
            !was_judged(snapshot, &candidate.content, &candidate.bag, anchor)
                && !was_reminded(snapshot, &candidate.content, anchor)
        })
        .cloned()
        .collect()
}

#[allow(
    clippy::too_many_arguments,
    reason = "the proposal binds one judged capture epoch and its indexed key clock"
)]
fn judged_proposal(
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    anchor: Seq,
    decisions: Vec<JudgedCandidate>,
    minted_millis: Option<i64>,
    at: Instant,
    expires: Instant,
) -> CommandResult<(Vec<Opening>, JournalWrite)> {
    let openings = openings(
        decisions
            .iter()
            .filter(|decision| decision.judgment.confirmed)
            .map(|decision| decision.candidate.clone())
            .collect(),
        minted_millis,
        at,
    )?;
    let origin = resident_origin(epoch)?;
    let mut proposal = JournalWrite::new(Actor::Spice, origin.clone(), at, Producer::Judge);
    proposal.guard(judgment_guard(
        epoch,
        lane,
        occupant,
        anchor,
        decisions.clone(),
        openings.clone(),
    ));
    for decided in &decisions {
        let decision = if decided.judgment.confirmed {
            let opening = openings
                .iter()
                .find(|opening| opening.candidate.content == decided.candidate.content)
                .expect("every confirmed decision owns one opening");
            TasteDecision::Confirmed {
                directive: opening.directive,
                key: opening.key.to_string(),
                text: opening.candidate.text.clone(),
                expires,
            }
        } else {
            TasteDecision::Cleared
        };
        let judged = TasteJudged {
            lane,
            occupant,
            compaction: anchor.get(),
            taste: decided.candidate.taste.clone(),
            content: decided.candidate.content.clone(),
            bag: decided.candidate.bag.clone(),
            trigger: decided.candidate.trigger.clone(),
            vendor: decided.candidate.vendor.clone(),
            source: decided.candidate.source.clone(),
            locus: decided.candidate.locus,
            attempts: decided.judgment.attempts.clone(),
            decision,
        };
        let pending = proposal
            .fact(judged.clone(), FactCause::Existing(anchor))
            .map_err(|error| store_fault("taste::judgment-invalid", &error.to_string()))?;
        if decided.judgment.confirmed {
            let head_origin = origin.clone();
            proposal.steering_head(pending, move |seq| {
                steer::Ledger::judged_head(&judged, head_origin, at, seq)
                    .expect("the admitted judgment builds its authority head")
            });
        }
    }
    Ok((openings, proposal))
}

#[allow(
    clippy::too_many_arguments,
    reason = "the proposal binds one reminder capture epoch and its indexed key clock"
)]
fn reminder_proposal(
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    anchor: Seq,
    candidates: Vec<Candidate>,
    minted_millis: Option<i64>,
    at: Instant,
    expires: Instant,
) -> CommandResult<(Vec<Opening>, JournalWrite)> {
    let openings = openings(candidates, minted_millis, at)?;
    let origin = resident_origin(epoch)?;
    let mut proposal = JournalWrite::new(
        Actor::Spice,
        origin.clone(),
        at,
        Producer::ResidentRecognition,
    );
    proposal.guard(reminder_guard(
        epoch,
        lane,
        occupant,
        anchor,
        openings.clone(),
    ));
    for opening in &openings {
        let reminded = wire::body::TasteReminded {
            directive: opening.directive,
            key: opening.key.to_string(),
            lane,
            taste: opening.candidate.taste.clone(),
            content: opening.candidate.content.clone(),
            occupant,
            compaction: anchor.get(),
            locus: opening.candidate.locus,
            text: opening.candidate.text.clone(),
            expires,
        };
        let pending = proposal
            .fact(reminded.clone(), FactCause::Existing(anchor))
            .map_err(|error| store_fault("taste::reminder-invalid", &error.to_string()))?;
        let head_origin = origin.clone();
        proposal.steering_head(pending, move |seq| {
            steer::Ledger::reminded_head(&reminded, head_origin, at, seq)
                .expect("the admitted reminder builds its authority head")
        });
    }
    Ok((openings, proposal))
}

fn resident_origin(epoch: CanonicalUuid) -> CommandResult<wire::Origin> {
    format!("resident:{epoch}")
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("resident::origin-invalid", &error.to_string()))
}

fn candidates(
    bag: &taste::Bag,
    record: &lane::capture::CaptureRecord,
) -> CommandResult<Vec<Candidate>> {
    if bag.tastes().is_empty() {
        return Ok(Vec::new());
    }
    let bag_identity = taste::bag_identity(bag);
    let mut candidates = Vec::new();
    for (event, decoded) in record.decoded.events().iter().enumerate() {
        let listen::Event::Prose(prose) = decoded else {
            continue;
        };
        let classified = classify(&prose.text);
        let Some(authored) = classified.authored else {
            continue;
        };
        // Echo classification precedes bag matching. No trigger can make a
        // resident-authored reminder become a candidate again.
        if taste::is_reminder_echo(&authored) {
            continue;
        }
        let authored = listen::Event::Prose(listen::Prose {
            role: prose.role,
            text: authored,
            phase: prose.phase.clone(),
            // Taste derivation rewrites text, not the durable media record.
            media: prose.media.clone(),
            locus: prose.locus,
        });
        let event = checked_locus(event)?;
        for hit in bag.hits(&authored) {
            let content = taste::reminder_identity(hit.taste(), hit.context());
            if candidates
                .iter()
                .any(|candidate: &Candidate| candidate.content == content)
            {
                continue;
            }
            candidates.push(Candidate {
                taste: hit.taste().to_string(),
                content,
                bag: bag_identity.clone(),
                trigger: hit.trigger().to_owned(),
                vendor: hit.message().to_owned(),
                source: hit.context().to_owned(),
                locus: TranscriptLocus {
                    member: hit.locus().member,
                    offset: hit.locus().offset,
                    length: hit.locus().length,
                    event,
                    line: checked_locus(hit.line())?,
                    column: checked_locus(hit.column())?,
                },
                text: taste::reminder_text(hit.taste(), hit.message()),
            });
        }
    }
    Ok(candidates)
}

fn adjudicate_candidates(
    candidates: Vec<Candidate>,
    anchor: Seq,
    cache: &mut BTreeMap<(Digest, Digest, u64), Option<Judgment>>,
    judge: &mut dyn Adjudicator,
) -> Vec<JudgedCandidate> {
    candidates
        .into_iter()
        .filter_map(|candidate| {
            let key = (
                candidate.content.clone(),
                candidate.bag.clone(),
                anchor.get(),
            );
            let judgment = if let Some(cached) = cache.get(&key) {
                cached.clone()
            } else {
                let decided = judge.adjudicate(&candidate).ok();
                cache.insert(key, decided.clone());
                decided
            }?;
            Some(JudgedCandidate {
                candidate,
                judgment,
            })
        })
        .collect()
}

fn judge_prompts(candidate: &Candidate) -> Vec<crate::judge::Prompt> {
    let subject = format!(
        "taste={}; trigger={}; vendor={}; authored_source={}",
        serde_json::to_string(&candidate.taste).expect("taste text encodes"),
        serde_json::to_string(&candidate.trigger).expect("trigger text encodes"),
        serde_json::to_string(&candidate.vendor).expect("vendor text encodes"),
        serde_json::to_string(&candidate.source).expect("source text encodes"),
    );
    let mut prompts = vec![
        crate::judge::Prompt::new(
            0,
            format!(
                "PROPOSITION: the authored source genuinely exhibits the configured taste behavior, rather than merely mentioning, quoting, denying, or discussing it.\n{subject}\nANSWER ONLY YES iff the proposition is true; otherwise ANSWER ONLY NO.\n"
            ),
        ),
        crate::judge::Prompt::new(
            1,
            format!(
                "Decide whether this configured taste actually applies to the authored source. A mere reference, quotation, negation, or meta-discussion does not apply.\n{subject}\nReturn exactly YES when it applies and exactly NO when it does not.\n"
            ),
        ),
        crate::judge::Prompt::new(
            2,
            format!(
                "Classify the same proposition: the source demonstrates the behavior selected by the taste trigger and warrants its vendor. Mentions without demonstrated behavior are false.\n{subject}\nOutput one token only: YES for true, NO for false.\n"
            ),
        ),
    ];
    let seed = candidate.content.as_str().as_bytes();
    let count = prompts.len();
    prompts.rotate_left(usize::from(seed[0]) % count);
    if seed[1] % 2 == 1 {
        prompts.reverse();
    }
    prompts
}

fn checked_locus(value: usize) -> CommandResult<u32> {
    u32::try_from(value)
        .map_err(|error| store_fault("taste::transcript-locus-invalid", &error.to_string()))
}

fn require_current_occupant(
    residency: &ResidencyHead,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> CommandResult<()> {
    if residency
        .lanes
        .iter()
        .any(|row| row.lane == lane && row.occupant == Some(occupant))
    {
        Ok(())
    } else {
        Err(store_fault(
            "taste::occupant-moved",
            "the reminder capture no longer belongs to the current occupant",
        ))
    }
}

fn openings(
    candidates: Vec<Candidate>,
    previous: Option<i64>,
    at: Instant,
) -> CommandResult<Vec<Opening>> {
    let now = at.get().as_millisecond();
    let mut millis = previous.map_or(Ok(now), |previous| {
        previous
            .checked_add(1)
            .map(|next| next.max(now))
            .ok_or_else(|| {
                environment_fault(
                    "taste::steering-key-invalid",
                    "epoch millisecond overflowed while advancing the indexed key clock",
                )
            })
    })?;
    let mut keys = BTreeSet::new();
    let mut opened = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let directive = new_uuid();
        let key = steer::Key::for_millis(millis).map_err(|error| {
            environment_fault("taste::steering-key-invalid", &error.to_string())
        })?;
        let inserted = keys.insert(key.clone());
        debug_assert!(inserted, "monotone moments mint distinct keys");
        millis = millis.checked_add(1).ok_or_else(|| {
            environment_fault(
                "taste::steering-key-invalid",
                "epoch millisecond overflowed while allocating one reminder batch",
            )
        })?;
        opened.push(Opening {
            candidate,
            directive,
            key,
        });
    }
    Ok(opened)
}

fn current_instant() -> CommandResult<Instant> {
    Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("resident::clock-invalid", &error.to_string()))
}

fn expiry(at: Instant, duration: std::time::Duration) -> CommandResult<Instant> {
    let duration = jiff::SignedDuration::try_from(duration)
        .map_err(|error| environment_fault("taste::expiry-invalid", &error.to_string()))?;
    let expires = at
        .get()
        .checked_add(duration)
        .map_err(|error| environment_fault("taste::expiry-invalid", &error.to_string()))?;
    Instant::new(expires)
        .map_err(|error| environment_fault("taste::expiry-invalid", &error.to_string()))
}

fn reminder_guard(
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    anchor: Seq,
    openings: Vec<Opening>,
) -> JournalGuard {
    JournalGuard::new(move |read| {
        let residency = read
            .residency_head()?
            .ok_or_else(|| GuardError::refused("taste::residency-missing"))?;
        if residency.body().epoch != epoch
            || !residency
                .body()
                .lanes
                .iter()
                .any(|row| row.lane == lane && row.occupant == Some(occupant))
        {
            return Err(GuardError::refused("taste::occupant-moved"));
        }

        let snapshot = read
            .fact_snapshot()
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        let current = context_anchor(&snapshot, lane, occupant).map_err(GuardError::Invalid)?;
        if current != anchor {
            return Err(GuardError::refused("taste::epoch-moved"));
        }
        for opening in &openings {
            if was_reminded(&snapshot, &opening.candidate.content, anchor)
                || was_judged(
                    &snapshot,
                    &opening.candidate.content,
                    &opening.candidate.bag,
                    anchor,
                )
            {
                return Err(GuardError::refused("taste::reminder-recorded"));
            }
            if read.steering_head(opening.directive)?.is_some()
                || read.steering_key(opening.key.as_str())?.is_some()
            {
                return Err(GuardError::refused("taste::steering-identity-taken"));
            }
        }
        Ok(())
    })
}

fn judgment_guard(
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    anchor: Seq,
    decisions: Vec<JudgedCandidate>,
    openings: Vec<Opening>,
) -> JournalGuard {
    JournalGuard::new(move |read| {
        let residency = read
            .residency_head()?
            .ok_or_else(|| GuardError::refused("taste::residency-missing"))?;
        if residency.body().epoch != epoch
            || !residency
                .body()
                .lanes
                .iter()
                .any(|row| row.lane == lane && row.occupant == Some(occupant))
        {
            return Err(GuardError::refused("taste::occupant-moved"));
        }
        let snapshot = read
            .fact_snapshot()
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        let current = context_anchor(&snapshot, lane, occupant).map_err(GuardError::Invalid)?;
        if current != anchor {
            return Err(GuardError::refused("taste::epoch-moved"));
        }
        for decided in &decisions {
            if was_judged(
                &snapshot,
                &decided.candidate.content,
                &decided.candidate.bag,
                anchor,
            ) || was_reminded(&snapshot, &decided.candidate.content, anchor)
            {
                return Err(GuardError::refused("taste::judgment-recorded"));
            }
        }
        for opening in &openings {
            if read.steering_head(opening.directive)?.is_some()
                || read.steering_key(opening.key.as_str())?.is_some()
            {
                return Err(GuardError::refused("taste::steering-identity-taken"));
            }
        }
        Ok(())
    })
}

fn context_anchor(
    snapshot: &journal::FactSnapshot,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> Result<Seq, String> {
    for envelope in snapshot.facts().iter().rev() {
        let fact = &envelope.fact;
        let matches = if let Some(body) = fact.read::<wire::body::LaneCompacted>() {
            body.lane == lane && body.occupant == occupant
        } else if let Some(body) = fact.read::<wire::body::LaneRenewed>() {
            body.lane == lane && body.occupant == occupant
        } else if let Some(body) = fact.read::<wire::body::LaneStarted>() {
            body.lane == lane && body.occupant == occupant
        } else {
            false
        };
        if matches {
            return Ok(envelope.seq);
        }
    }
    Err(format!(
        "lane {lane} occupant {occupant} has no start, renewal, or compaction fact"
    ))
}

fn was_reminded(snapshot: &journal::FactSnapshot, content: &Digest, anchor: Seq) -> bool {
    snapshot.facts().iter().any(|envelope| {
        envelope
            .fact
            .read::<wire::body::TasteReminded>()
            .is_some_and(|body| &body.content == content && body.compaction == anchor.get())
    })
}

fn was_judged(
    snapshot: &journal::FactSnapshot,
    content: &Digest,
    bag: &Digest,
    anchor: Seq,
) -> bool {
    snapshot.facts().iter().any(|envelope| {
        envelope.fact.read::<TasteJudged>().is_some_and(|body| {
            &body.content == content && &body.bag == bag && body.compaction == anchor.get()
        })
    })
}

#[cfg(test)]
mod tests;
