//! Task identities reported by authored message text.

use wire::TaskHandle;

use super::canonical_task_handle;

/// Canonical task handles an authored message reports, in first-report order.
///
/// Markdown punctuation is a boundary rather than part of a handle, so prose
/// may name one in backticks, parentheses, or at the end of a sentence. The
/// wire handle parser admits the outer stem/key shape, then `born` admits the
/// fixed-width base-52 task stamp. Repeated mentions produce one reported
/// identity even though the browser renders every authored occurrence.
#[must_use]
pub fn reported_tasks(text: &str) -> Vec<TaskHandle> {
    let mut tasks = Vec::new();
    for (_, task) in task_mentions(text) {
        if !tasks.contains(&task) {
            tasks.push(task);
        }
    }
    tasks
}

/// Canonical task handles and their byte ranges in authored text.
///
/// The browser uses the ranges to replace the handle where it was written,
/// while [`reported_tasks`] retains the de-duplicated identity view used by
/// non-browser surfaces.
#[must_use]
pub fn task_mentions(text: &str) -> Vec<(std::ops::Range<usize>, TaskHandle)> {
    let mut mentions = Vec::new();
    let mut start = None;
    for (index, character) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        if character.is_ascii_alphanumeric() || character == '-' {
            start.get_or_insert(index);
        } else if let Some(start) = start.take()
            && let Some(task) = canonical_task_handle(&text[start..index])
        {
            mentions.push((start..index, task));
        }
    }
    mentions
}
