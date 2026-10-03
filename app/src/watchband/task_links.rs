//! Task identities and retained message cards reachable from Work surfaces.

use std::collections::{BTreeMap, BTreeSet};

use wire::{CanonicalUuid, TaskHandle};

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

/// Retained authenticated direction replies reachable from global Directions.
///
/// The projection avoids rescanning message history for every recipient badge.
pub struct DirectionMessageLinks<'a> {
    directives: BTreeMap<String, &'a wire::AgentMessage>,
}

impl<'a> DirectionMessageLinks<'a> {
    /// Retained acknowledgement or decline for one recipient key.
    #[must_use]
    pub fn directive_key(&self, key: &str) -> Option<&'a wire::AgentMessage> {
        self.directives.get(key).copied()
    }
}

/// Index reachable direction replies for messages retained by these lanes.
#[must_use]
pub fn direction_message_links<'a>(
    member_lanes: &BTreeSet<CanonicalUuid>,
    messages: impl IntoIterator<Item = &'a wire::AgentMessage>,
) -> DirectionMessageLinks<'a> {
    let mut links = DirectionMessageLinks {
        directives: BTreeMap::new(),
    };
    for message in messages {
        if !member_lanes.contains(&message.lane) {
            continue;
        }
        let is_reply = message.classifications.iter().any(|classification| {
            matches!(
                classification.kind,
                wire::MessageClassificationKind::Acknowledgement
                    | wire::MessageClassificationKind::Decline
            )
        });
        if !is_reply {
            continue;
        }
        for context in &message.directive_contexts {
            if let wire::QuoteTarget::Steering { key, .. } = &context.quote_target {
                replace_with_newer(&mut links.directives, key.clone(), message);
            }
        }
    }
    links
}

fn replace_with_newer<'a, K: Ord>(
    messages: &mut BTreeMap<K, &'a wire::AgentMessage>,
    key: K,
    candidate: &'a wire::AgentMessage,
) {
    if messages
        .get(&key)
        .is_none_or(|current| current.chronology_key() <= candidate.chronology_key())
    {
        messages.insert(key, candidate);
    }
}
