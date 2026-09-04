use std::collections::BTreeSet;

use super::*;

pub(super) const TASTE_REVIEW_TAG: &str = "taste-review:required";

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
            "taste-review audit policy={} outcome={outcome} matches=[{}]",
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
