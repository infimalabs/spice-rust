//! Decode retired document-slug and taste-proposal tags for the statement-tag
//! schema crossing.

use wire::{Digest, TasteProposal};

/// Retired tag prefix naming a task's document node slug.
const DOCUMENT_SLUG_PREFIX: &str = "document-slug:";
/// Retired tag marking a resident-filed taste proposal.
const PROPOSAL: &str = "taste-proposal";
/// Retired tag prefix naming a proposal's finding identity.
const FINDING_PREFIX: &str = "taste-finding:";
/// Retired tag prefix naming a proposal's occurrence identity.
const OCCURRENCE_PREFIX: &str = "taste-occurrence:";
/// Retired tag prefix naming a proposal's recurrence count.
const RECURRENCE_PREFIX: &str = "taste-recurrence:";

/// Remove every retired marker tag from `tags` and return what it declared.
pub(super) fn lift(
    tags: &mut Vec<String>,
) -> Result<(Option<String>, Option<TasteProposal>), String> {
    let mut slug = None;
    let mut marked = false;
    let mut finding = None;
    let mut occurrence = None;
    let mut recurrence = None;
    for tag in tags.drain(..) {
        if let Some(value) = tag.strip_prefix(DOCUMENT_SLUG_PREFIX) {
            if value.is_empty() {
                return Err("the statement declares an empty document slug".to_owned());
            }
            once(&mut slug, value.to_owned(), "document slug")?;
        } else if tag == PROPOSAL {
            if std::mem::replace(&mut marked, true) {
                return Err("the statement repeats the taste-proposal marker".to_owned());
            }
        } else if let Some(value) = tag.strip_prefix(FINDING_PREFIX) {
            once(&mut finding, digest(value, "finding")?, "proposal finding")?;
        } else if let Some(value) = tag.strip_prefix(OCCURRENCE_PREFIX) {
            once(
                &mut occurrence,
                digest(value, "occurrence")?,
                "proposal occurrence",
            )?;
        } else if let Some(value) = tag.strip_prefix(RECURRENCE_PREFIX) {
            let count = value
                .parse::<usize>()
                .ok()
                .filter(|count| *count >= 2)
                .ok_or_else(|| format!("the proposal recurrence {value:?} is not at least two"))?;
            once(&mut recurrence, count, "proposal recurrence")?;
        }
    }
    let proposal = match (marked, finding, occurrence, recurrence) {
        (false, None, None, None) => None,
        (true, Some(finding), Some(occurrence), Some(recurrence)) => Some(TasteProposal {
            finding,
            occurrence,
            recurrence,
        }),
        _ => {
            return Err("the statement carries an incomplete taste-proposal marker set".to_owned());
        }
    };
    Ok((slug, proposal))
}

fn once<T>(slot: &mut Option<T>, value: T, what: &str) -> Result<(), String> {
    match slot.replace(value) {
        Some(_) => Err(format!("the statement declares more than one {what}")),
        None => Ok(()),
    }
}

fn digest(value: &str, what: &str) -> Result<Digest, String> {
    value
        .parse()
        .map_err(|_| format!("the proposal {what} {value:?} is not a digest"))
}
