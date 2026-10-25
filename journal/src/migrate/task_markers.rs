//! Decode the retired document-slug tag for the statement-tag schema crossing.

/// Retired tag prefix naming a task's document node slug.
const DOCUMENT_SLUG_PREFIX: &str = "document-slug:";

/// Remove every retired marker tag from `tags` and return the document slug it
/// declared.
pub(super) fn lift(tags: &mut Vec<String>) -> Result<Option<String>, String> {
    let mut slug = None;
    for tag in tags.drain(..) {
        let Some(value) = tag.strip_prefix(DOCUMENT_SLUG_PREFIX) else {
            continue;
        };
        if value.is_empty() {
            return Err("the statement declares an empty document slug".to_owned());
        }
        if slug.replace(value.to_owned()).is_some() {
            return Err("the statement declares more than one document slug".to_owned());
        }
    }
    Ok(slug)
}
