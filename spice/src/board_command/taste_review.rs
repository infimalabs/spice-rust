use std::collections::BTreeSet;

use super::*;

pub(super) const TASTE_REVIEW_TAG: &str = "taste-review:required";

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
    tags.iter().filter(|tag| tag.as_str() != TASTE_REVIEW_TAG)
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

pub(super) fn clear_taste_review(tags: &mut Vec<String>) {
    tags.retain(|tag| tag != TASTE_REVIEW_TAG);
}

pub(super) fn prepend_plan(flow: &mut Vec<wire::Phase>) {
    if flow.first().is_none_or(|phase| phase.as_str() != "plan") {
        flow.insert(0, phase("plan"));
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
        .and_then(|session| session.launch.as_ref())
        .map(|launch| launch.model.as_str());
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
