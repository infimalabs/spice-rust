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
    let partition = |revision, group: &wire::rooms::RoomAuthorityStamp, design: &TaskStamp| {
        wire::rooms::WorkingGroupPartition {
            revision,
            groups: vec![wire::rooms::WorkingGroup {
                group: group.clone(),
                anchors: vec![design.clone()],
            }],
            fast: Vec::new(),
        }
    };
    assert!(standing(&bus, &mut incremental, &removed));
    assert!(!standing(&bus, &mut incremental, &added));

    committed(
        bus.commit(arranged_write(
            partition(1, &old_group, &removed),
            partition(2, &new_group, &added),
            instant(9),
        ))
        .expect("replacement arrangement commits"),
    );
    assert!(
        !standing(&bus, &mut incremental, &removed),
        "a Design an arrangement removes leaves every group"
    );
    assert!(
        standing(&bus, &mut incremental, &added),
        "a Design an arrangement adds stands grouped"
    );
    bus.shutdown().expect("arrangement fixture bus stops");
}
