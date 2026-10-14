use std::collections::BTreeSet;

use board::TASTE_PLAN_TAG;

use super::*;

pub(super) const TASTE_REVIEW_TAG: &str = "taste-review:required";

/// Statement tags the board owns; authors never write or see them.
pub(super) const SYSTEM_TAGS: [&str; 2] = [TASTE_REVIEW_TAG, TASTE_PLAN_TAG];

/// Opening words of every durable taste-review audit note.
pub(super) const AUDIT_PREFIX: &str = "taste-review audit ";

/// What a lane reads where the operator reads a matched token or a taste's
/// own message.
const WITHHELD: &str = "withheld";

/// Audit fields that name the match itself rather than where it sits.
const WITHHELD_QUOTED: [&str; 2] = ["trigger=", "message="];

/// Audit fields that locate the match precisely enough to swap one word.
const WITHHELD_PLAIN: [&str; 1] = ["column="];

#[derive(Clone, Debug)]
pub(super) struct TasteReview {
    required: bool,
    audit: String,
}

impl TasteReview {
    pub(super) const fn required(&self) -> bool {
        self.required
    }

    pub(super) fn audit(&self) -> &str {
        &self.audit
    }
}

pub(super) fn has_taste_review(tags: &[String]) -> bool {
    tags.iter().any(|tag| tag == TASTE_REVIEW_TAG)
}

pub(super) fn visible_tags(tags: &[String]) -> impl Iterator<Item = &String> {
    tags.iter()
        .filter(|tag| !SYSTEM_TAGS.contains(&tag.as_str()))
}

pub(super) fn substantive_acceptance(acceptance: &[String]) -> bool {
    acceptance
        .iter()
        .any(|criterion| !criterion.trim().is_empty())
}

pub(super) fn validate_acceptance(acceptance: &[String]) -> Result<(), Outcome<CommandAnswer>> {
    if acceptance
        .iter()
        .any(|criterion| criterion.trim().is_empty())
    {
        return Err(refusal(
            "task::acceptance-empty",
            None,
            "remove blank acceptance values or replace each with a substantive completion criterion",
        ));
    }
    Ok(())
}

pub(super) fn require_taste_review(tags: &mut Vec<String>) {
    if !has_taste_review(tags) {
        tags.push(TASTE_REVIEW_TAG.to_owned());
    }
}

/// Hold a new task for taste review in a plan phase.
///
/// A flow that already opens in plan keeps that phase as authored; only a plan
/// phase this inserts is marked, so the hold's clearance can withdraw exactly
/// what the hold added.
pub(super) fn hold_for_taste(flow: &mut Vec<wire::Phase>, tags: &mut Vec<String>) {
    require_taste_review(tags);
    if flow.first().is_none_or(|phase| phase.as_str() != "plan") {
        flow.insert(0, phase("plan"));
        tags.push(TASTE_PLAN_TAG.to_owned());
    }
}

/// Clear a satisfied taste hold, withdrawing the plan phase it inserted.
///
/// Returns the workflow the task enters when the hold's own plan phase is still
/// current: the authored flow, positioned at its first phase, with every other
/// workflow field intact. A plan phase the author named stays.
pub(super) fn clear_taste_review(
    tags: &mut Vec<String>,
    workflow: &wire::WorkflowHead,
) -> Option<wire::WorkflowHead> {
    let prepended = tags.iter().any(|tag| tag == TASTE_PLAN_TAG);
    tags.retain(|tag| !SYSTEM_TAGS.contains(&tag.as_str()));
    let in_prepended_phase = workflow.phase.as_str() == "plan"
        && workflow.flow.first() == Some(&workflow.phase)
        && workflow.flow.len() > 1;
    if !prepended || !in_prepended_phase {
        return None;
    }
    let mut released = workflow.clone();
    released.flow.remove(0);
    released.phase = released.flow[0].clone();
    Some(released)
}

/// Refuse an authored document that writes a board-owned tag.
pub(super) fn refuse_system_tags(tags: &[String]) -> Result<(), Outcome<CommandAnswer>> {
    match tags.iter().find(|tag| SYSTEM_TAGS.contains(&tag.as_str())) {
        Some(tag) => Err(refusal(
            "doc::tag-reserved",
            Some(tag.clone()),
            "remove the system-owned taste-review marker from the authored document",
        )),
        None => Ok(()),
    }
}

/// Carry a document task's taste hold into the statement its document rewrites.
///
/// A new task is held like any other. An existing held task keeps the
/// board-owned tags while it still matches and, once it no longer does, returns
/// to the workflow its hold displaced.
pub(super) fn resync_taste_hold(
    snapshot: &ValidatedSnapshot,
    existing: Option<&Entry>,
    required: bool,
    flow: &mut Vec<wire::Phase>,
    tags: &mut Vec<String>,
) -> Result<Option<ReleasedPlan>, Outcome<CommandAnswer>> {
    let Some(entry) = existing else {
        if required {
            hold_for_taste(flow, tags);
        }
        return Ok(None);
    };
    let mut held = entry.statement().tags.clone();
    if !has_taste_review(&held) {
        if required {
            require_taste_review(tags);
        }
        return Ok(None);
    }
    if required {
        held.retain(|tag| SYSTEM_TAGS.contains(&tag.as_str()));
        tags.extend(held);
        return Ok(None);
    }
    clear_taste_review(&mut held, entry.workflow())
        .map(|workflow| ReleasedPlan::observe(snapshot, entry.born(), workflow))
        .transpose()
}

/// The workflow a cleared hold returns its task to, guarded on the workflow
/// and claim heads the clearance read.
pub(super) struct ReleasedPlan {
    workflow_witness: Witness,
    claim_witness: Witness,
    workflow: wire::WorkflowHead,
    claim: Option<ClaimHead>,
    release_actor: Option<Actor>,
}

impl ReleasedPlan {
    pub(super) fn observe(
        snapshot: &ValidatedSnapshot,
        task: &wire::TaskStamp,
        workflow: wire::WorkflowHead,
    ) -> Result<Self, Outcome<CommandAnswer>> {
        let observed = snapshot
            .answer
            .tasks
            .iter()
            .find(|heads| heads.statement.body.born == *task)
            .ok_or_else(|| {
                protocol_fault(
                    "task::snapshot-incoherent",
                    "validated board snapshot lost a task entry",
                )
            })?;
        let mut claim = observed.claim.as_ref().map(|claim| claim.body.clone());
        let release_actor = claim
            .as_ref()
            .filter(|claim| claim.disposition == ClaimDisposition::Active)
            .map(|claim| claim.actor.clone());
        if let Some(claim) = claim.as_mut()
            && release_actor.is_some()
        {
            claim.disposition = ClaimDisposition::Freed;
        }
        Ok(Self {
            workflow_witness: workflow_witness(observed),
            claim_witness: claim_witness(observed),
            workflow,
            claim,
            release_actor,
        })
    }

    pub(super) const fn workflow(&self) -> &wire::WorkflowHead {
        &self.workflow
    }

    pub(super) fn claim(&self) -> Option<ClaimHead> {
        self.claim.clone()
    }

    /// Record the phase change and any exact active-claim release in one write,
    /// so replay never exposes authored execution under the synthetic-plan claim.
    pub(super) fn write(
        self,
        write: &mut JournalWrite,
        task: &wire::TaskStamp,
        edited: PendingFact,
    ) -> Result<PendingFact, Outcome<CommandAnswer>> {
        guard(write, vec![self.workflow_witness, self.claim_witness]);
        if let Some(actor) = self.release_actor {
            let freed = write
                .fact(
                    body::ClaimFreed {
                        task: task.clone(),
                        actor,
                        parked: None,
                    },
                    FactCause::Pending(edited),
                )
                .map_err(|error| store_fault("claim::free-build", &error.to_string()))?;
            let claim = self
                .claim
                .expect("an active synthetic-plan claim has a projected release");
            write.claim_head(task.clone(), freed, move |fact| ClaimHead { fact, ..claim });
        }
        let phased = write
            .fact(
                body::TaskPhased {
                    task: task.clone(),
                    phase: self.workflow.phase.clone(),
                },
                FactCause::Pending(edited),
            )
            .map_err(|error| store_fault("task::phase-build", &error.to_string()))?;
        let workflow = self.workflow;
        write.workflow_head(task.clone(), phased, move |_| workflow);
        Ok(phased)
    }
}

pub(super) fn task_taste_review(
    repository: Option<&Repository>,
    snapshot: &ValidatedSnapshot,
    actor: &Actor,
    title: &str,
    description: Option<&str>,
    acceptance: &[String],
) -> Result<Option<TasteReview>, Outcome<CommandAnswer>> {
    let Some(repository) = repository else {
        return Ok(None);
    };
    let loaded = spice::config::load_repository(repository, &[])
        .map_err(|error| environment_fault("task::taste-policy-unreadable", &error.to_string()))?;
    let bag = loaded.root().taste().bag();
    let seat = match actor {
        Actor::Lane(lane) => snapshot
            .answer
            .residency
            .body
            .lanes
            .iter()
            .find(|seat| seat.lane == *lane),
        Actor::Operator | Actor::Spice => None,
    };
    let vendor = seat.map(|seat| seat.vendor.as_str());
    let model = seat
        .and_then(|seat| seat.session.as_ref())
        .map(|session| session.launch.model.as_str());
    let role = if matches!(actor, Actor::Operator) {
        listen::ProseRole::Operator
    } else {
        listen::ProseRole::Assistant
    };
    let fields = std::iter::once(("title", title))
        .chain(description.map(|text| ("description", text)))
        .chain(acceptance.iter().map(|text| ("acceptance", text.as_str())))
        .collect::<Vec<_>>();
    let mut evidence = BTreeSet::new();
    // Task prose has no repository path, extension, or gate phase. A taste
    // constrained to one of those axes is inapplicable here, not unresolved.
    let applicable = bag
        .tastes()
        .iter()
        .filter(|taste| {
            let scopes = taste.scopes();
            scopes.paths().is_empty()
                && scopes.extensions().is_empty()
                && scopes.phases().is_empty()
                && (scopes.vendors().is_empty() || vendor.is_some())
                && (scopes.models().is_empty() || model.is_some())
        })
        .cloned()
        .collect::<Vec<_>>();
    let applicable = taste::Bag::new(applicable).map_err(|error| {
        protocol_fault(
            "task::taste-policy-invalid",
            &format!("resolved taste bag could not be reconstructed: {error}"),
        )
    })?;
    let mut context = gate::PolicyContext::new();
    if let Some(vendor) = vendor {
        context = context.with_vendor(vendor);
    }
    if let Some(model) = model {
        context = context.with_model(model);
    }
    for (field, text) in &fields {
        let event = listen::Event::Prose(listen::Prose {
            role,
            text: (*text).to_owned(),
            phase: None,
            media: Vec::new(),
            external_media: Vec::new(),
            locus: listen::Locus::new(0, 0, text.len() as u64),
        });
        let hits = applicable.hits(&event, &context).map_err(|error| {
            protocol_fault(
                "task::taste-policy-invalid",
                &format!("resolved taste policy refused task prose: {error}"),
            )
        })?;
        for hit in hits {
            evidence.insert(format!(
                "field={field:?} taste={:?} trigger={:?} message={:?} line={} column={}",
                hit.taste().as_str(),
                hit.trigger(),
                hit.message(),
                hit.line(),
                hit.column(),
            ));
        }
    }
    let required = !evidence.is_empty();
    let outcome = if required { "required" } else { "clear" };
    Ok(Some(TasteReview {
        required,
        audit: format!(
            "{AUDIT_PREFIX}policy={} outcome={outcome} matches=[{}]",
            taste::bag_identity(bag),
            evidence.into_iter().collect::<Vec<_>>().join("; "),
        ),
    }))
}

pub(super) fn append_taste_review_audit(
    write: &mut JournalWrite,
    task: &wire::TaskStamp,
    review: &TasteReview,
    cause: FactCause,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    write
        .fact(
            body::TaskNoted {
                task: task.clone(),
                note: review.audit().to_owned(),
            },
            cause,
        )
        .map_err(|error| store_fault("task::taste-audit-build", &error.to_string()))
}

/// How many times the current policy has read one task's statement.
///
/// Every recheck appends its own audit, so the task's own notes count them
/// without a second policy run.
pub(super) fn taste_rechecks(facts: &bus::TaskFacts, task: &wire::TaskStamp) -> usize {
    facts
        .iter()
        .filter_map(|envelope| envelope.fact.read::<body::TaskNoted>())
        .filter(|noted| &noted.task == task && noted.note.starts_with(AUDIT_PREFIX))
        .count()
}

/// Project one durable audit into what a lane may read of it.
///
/// `spice task show` and the plan-completion refusal already withhold the
/// matched token and the taste's own message, because an author who is told
/// the word swaps the word instead of rewriting the statement. The durable
/// note keeps both for the operator who owns the policy; a lane reads the
/// same note with the match itself, and the column that pinpoints it,
/// withheld. The field and line remain: they name the passage to rewrite.
///
/// Returns `None` for a note that is not a taste-review audit.
pub(super) fn lane_audit(note: &str) -> Option<String> {
    if !note.starts_with(AUDIT_PREFIX) {
        return None;
    }
    let mut lane = String::with_capacity(note.len());
    let mut rest = note;
    while let Some((before, key, after)) = next_withheld_field(rest) {
        lane.push_str(before);
        lane.push_str(key);
        lane.push_str(WITHHELD);
        rest = after;
    }
    lane.push_str(rest);
    Some(lane)
}

/// Split the audit at its next withheld field: text before it, the field's
/// key, and the text after its value.
///
/// An unmeasurable value withholds the rest of the note rather than releasing
/// it: this projection exists to keep a match from reaching its author, so a
/// note it cannot read must end there.
fn next_withheld_field(audit: &str) -> Option<(&str, &str, &str)> {
    let mut earliest: Option<(usize, &str)> = None;
    for key in WITHHELD_QUOTED.iter().chain(WITHHELD_PLAIN.iter()) {
        if let Some(at) = audit.find(key)
            && earliest.is_none_or(|(found, _)| at < found)
        {
            earliest = Some((at, key));
        }
    }
    let (at, key) = earliest?;
    let value = &audit[at + key.len()..];
    let end = if WITHHELD_QUOTED.contains(&key) {
        quoted_width(value).unwrap_or(value.len())
    } else {
        value
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(value.len())
    };
    Some((&audit[..at], key, &value[end..]))
}

/// Width of one `{:?}` rendered value, counting the escapes that rendering
/// wrote into it.
fn quoted_width(value: &str) -> Option<usize> {
    let bytes = value.as_bytes();
    if bytes.first() != Some(&b'"') {
        return None;
    }
    let mut index = 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return Some(index + 1),
            _ => index += 1,
        }
    }
    None
}
