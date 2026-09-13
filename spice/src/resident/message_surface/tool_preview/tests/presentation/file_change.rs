use super::*;

fn session_worktree(residency: &wire::ResidencyHead, expectation: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(
        &residency.lanes[0]
            .session
            .as_ref()
            .expect(expectation)
            .worktree,
    )
}

#[test]
fn file_change_summary_keeps_the_session_worktree_when_the_lane_row_moves() {
    let bytes = br#"{"type":"item.completed","item":{"id":"file-change","type":"file_change","changes":[{"path":"/workspace/wire/src/lib.rs","kind":"add"},{"path":"/workspace/spice/src/lib.rs","kind":"update"}],"status":"completed"}}"#;
    let record = capture_record(bytes, 0);
    let mut residency = wire::ResidencyHead {
        epoch: lane(),
        state: wire::ResidentState::Seated,
        topology_revision: 0,
        lanes: vec![wire::LaneResidency {
            lane: record.lane,
            lineage_id: record.lane,
            name: None,
            worktree: "/workspace".to_owned(),
            life: "drive".to_owned(),
            vendor: "codex".parse().expect("fixture vendor"),
            occupant: Some(record.occupant),
            lineage: vec![record.occupant],
            capabilities: Vec::new(),
            session: Some(wire::LaneSession {
                lane: record.lane,
                occupant: record.occupant,
                worktree: "/workspace".to_owned(),
                vendor: "codex".parse().expect("fixture vendor"),
                launch: None,
                epoch: lane(),
                stream: "stdout".to_owned(),
                identity: "session".to_owned(),
                locus: wire::TapeLocus {
                    member: 1,
                    offset: 0,
                    length: 1,
                },
            }),
            disposition: "occupied".to_owned(),
        }],
        subscriptions: Vec::new(),
        renewals: Vec::new(),
    };
    let live_worktree = session_worktree(&residency, "live session owns a worktree");
    let live = Messages::default();
    publish_agent_messages_in(
        &live,
        &record,
        &mut MessageState::default(),
        None,
        Some(&live_worktree),
    )
    .expect("live file change publishes");

    residency.lanes[0].worktree = "/workspace-imported".to_owned();
    let replay_worktree = session_worktree(&residency, "moved lane retains its session worktree");
    let context = record.decoded.context().expect("record carries context");
    let events = record
        .decoded
        .events()
        .iter()
        .enumerate()
        .map(|(ordinal, event)| (ordinal as u32, event))
        .collect::<Vec<_>>();
    let replay = Messages::default();
    publish_agent_record(
        &replay,
        AgentRecord {
            lane: record.lane,
            author: record.occupant,
            source: Some(lane()),
            worktree: Some(&replay_worktree),
            locus: record.locus,
            context,
            events: &events,
            observed: None,
            unavailable_tool_terminals: &[],
        },
        &mut MessageState::default(),
        None,
    )
    .expect("replayed file change publishes");

    let live = live.0.lock().expect("live messages");
    let replay = replay.0.lock().expect("replayed messages");
    assert_eq!(*live, *replay);
    assert_eq!(
        live[0].text,
        "file_change: spice/src/lib.rs +1 paths [add,update]"
    );
    assert_eq!(
        live[1].text,
        "file_change: spice/src/lib.rs +1 paths [add,update] succeeded"
    );
}
