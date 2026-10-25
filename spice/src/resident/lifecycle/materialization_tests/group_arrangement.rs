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
    assert!(projected(&bus, &mut incremental, &removed).design_is_active());
    assert!(!projected(&bus, &mut incremental, &added).design_is_active());

    let from = wire::rooms::WorkingGroupPartition {
        revision: 1,
        groups: vec![wire::rooms::WorkingGroup {
            group: old_group,
            anchors: vec![removed.clone()],
            stance: wire::rooms::WorkingGroupStance::Explore,
        }],
        fast: Vec::new(),
    };
    let partition = wire::rooms::WorkingGroupPartition {
        revision: 2,
        groups: vec![wire::rooms::WorkingGroup {
            group: new_group,
            anchors: vec![added.clone()],
            stance: wire::rooms::WorkingGroupStance::Explore,
        }],
        fast: Vec::new(),
    };
    committed(
        bus.commit(arranged_write(from, partition, instant(8)))
            .expect("replacement arrangement commits"),
    );
    assert!(!projected(&bus, &mut incremental, &removed).design_is_active());
    assert!(projected(&bus, &mut incremental, &added).design_is_active());
    bus.shutdown().expect("arrangement fixture bus stops");
}
