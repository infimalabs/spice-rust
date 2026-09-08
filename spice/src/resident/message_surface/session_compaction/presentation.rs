use std::path::Path;

use super::*;

pub(super) fn publish(
    record: CompactionRecord,
    lane: CanonicalUuid,
    author: CanonicalUuid,
    worktree: &Path,
    publisher: &impl AgentMessagePublisher,
    state: &mut MessageState,
) -> CommandResult<()> {
    publish_agent_event(
        publisher,
        AgentEvent {
            lane,
            author,
            source: None,
            worktree: Some(worktree),
            ordinal: record.ordinal,
            decoded: &record.decoded,
            authored: Some(&record.authored),
            observed: None,
            observations: &state.observations,
            unavailable_tool_terminals: &record.unavailable_tool_terminals,
        },
        &mut state.previews,
        None,
    )
}
