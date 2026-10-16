//! Decode retired task priority and taste-review tags for the statement-tag
//! schema crossing.

use wire::{TaskPriority, TastePlan};

/// Retired tag prefix that declared a priority.
const PRIORITY_PREFIX: &str = "priority:";
/// Retired tag that held a statement for taste review.
const TASTE_HELD: &str = "taste-review:required";
/// Retired tag marking a held statement whose plan phase the hold prepended.
const TASTE_PREPENDED: &str = "taste-review:plan-prepended";

/// Remove every retired standing tag from `tags` and return what it declared:
/// the priority, and the plan of any taste hold.
pub(super) fn lift(
    tags: &mut Vec<String>,
) -> Result<(Option<TaskPriority>, Option<TastePlan>), String> {
    let mut priority = None;
    let mut held = false;
    let mut prepended = false;
    let mut refusal = None;
    tags.retain(|tag| {
        if let Some(word) = tag.strip_prefix(PRIORITY_PREFIX) {
            match (priority, TaskPriority::from_wire(word)) {
                (None, Some(declared)) => priority = Some(declared),
                (Some(_), _) => {
                    refusal.get_or_insert_with(|| "more than one priority tag".to_owned());
                }
                (None, None) => {
                    refusal.get_or_insert_with(|| format!("priority tag {tag} names no priority"));
                }
            }
            false
        } else if tag == TASTE_HELD {
            held = true;
            false
        } else if tag == TASTE_PREPENDED {
            prepended = true;
            false
        } else {
            true
        }
    });
    if let Some(refusal) = refusal {
        return Err(refusal);
    }
    let taste = match (held, prepended) {
        (false, false) => None,
        (true, false) => Some(TastePlan::Authored),
        (true, true) => Some(TastePlan::Prepended),
        (false, true) => return Err("a prepended plan marker without its taste hold".to_owned()),
    };
    Ok((priority, taste))
}
