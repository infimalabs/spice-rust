use super::*;

pub(super) fn publish(
    record: CompactionRecord,
    lane: CanonicalUuid,
    author: CanonicalUuid,
    publisher: &impl AgentMessagePublisher,
    state: &mut MessageState,
) -> CommandResult<()> {
    publish_agent_event(
        publisher,
        AgentEvent {
            lane,
            author,
            source: None,
            ordinal: record.ordinal,
            decoded: &record.decoded,
            authored: Some(&record.authored),
            observed: None,
            observations: &state.observations,
            unavailable_tool_terminals: &record.unavailable_tool_terminals,
            prose_origin: None,
            suppress_answer: false,
        },
        &mut state.previews,
        None,
    )
}
