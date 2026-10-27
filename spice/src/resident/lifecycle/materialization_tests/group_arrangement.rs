use super::*;

#[test]
fn arrangements_refresh_designs_removed_from_and_added_to_the_partition() {
    let (bus, mut incremental) = caught_up_fixture(0);
    committed(
        bus.commit(design_write(DESIGN_MOMENT, instant(5)))
            .expect("first open Design commits"),
    );
    committed(
        bus.commit(design_write(SECOND_DESIGN_MOMENT, instant(6)))
            .expect("second open Design commits"),
    );
    let removed = task(DESIGN_MOMENT);
    let added = task(SECOND_DESIGN_MOMENT);
    let old_group = wire::rooms::RoomAuthorityStamp::for_micros(1_900_000_000_000_001)
        .expect("old fixture group stamp");
    let new_group = wire::rooms::RoomAuthorityStamp::for_micros(1_900_000_000_000_002)
        .expect("new fixture group stamp");
    committed(
        bus.commit(move_write(
            &removed,
            None,
            Some(old_group.clone()),
            1,
            instant(7),
        ))
        .expect("first Design group move commits"),
    );
    let partition =
        |revision, group: &wire::rooms::RoomAuthorityStamp, design: &TaskStamp, stance| {
            wire::rooms::WorkingGroupPartition {
                revision,
                groups: vec![wire::rooms::WorkingGroup {
                    group: group.clone(),
                    anchors: vec![design.clone()],
                    stance,
                }],
                fast: Vec::new(),
            }
        };
    let running = partition(
        1,
        &old_group,
        &removed,
        wire::rooms::WorkingGroupStance::Running,
    );
    let paused = partition(
        2,
        &old_group,
        &removed,
        wire::rooms::WorkingGroupStance::Paused,
    );
    committed(
        bus.commit(arranged_write(running, paused.clone(), instant(8)))
            .expect("paused brake commits"),
    );
    assert!(!projected(&bus, &mut incremental, &removed).design_is_active());
    assert!(projected(&bus, &mut incremental, &added).design_is_active());

    let replaced = partition(
        3,
        &new_group,
        &added,
        wire::rooms::WorkingGroupStance::Paused,
    );
    committed(
        bus.commit(arranged_write(paused, replaced, instant(9)))
            .expect("replacement arrangement commits"),
    );
    assert!(
        projected(&bus, &mut incremental, &removed).design_is_active(),
        "a Design leaving every group becomes active"
    );
    assert!(!projected(&bus, &mut incremental, &added).design_is_active());
    bus.shutdown().expect("arrangement fixture bus stops");
}
