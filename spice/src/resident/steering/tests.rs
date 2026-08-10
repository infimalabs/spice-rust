use super::{due_stage, missing_stages};

#[test]
fn successful_forward_publication_removes_only_the_obsolete_rendering() {
    let scratch = std::env::temp_dir().join(format!(
        "spice-inbox-forward-cleanup-{}",
        std::process::id()
    ));
    let legacy = scratch.join("common");
    let current = scratch.join("worktree");
    let lane: wire::CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce913116302"
        .parse()
        .expect("fixture lane");
    std::fs::create_dir_all(&legacy).expect("create legacy inbox");
    std::fs::create_dir_all(&current).expect("create current inbox");
    std::fs::write(steer::inbox::path(&legacy, lane), b"obsolete")
        .expect("seed obsolete rendering");
    std::fs::write(steer::inbox::path(&current, lane), b"current").expect("seed current rendering");

    super::remove_legacy_inbox(&legacy, &current, lane);

    assert!(!steer::inbox::path(&legacy, lane).exists());
    assert_eq!(
        std::fs::read(steer::inbox::path(&current, lane)).expect("read current rendering"),
        b"current"
    );

    super::remove_legacy_inbox(&current, &current, lane);
    assert!(steer::inbox::path(&current, lane).exists());
    std::fs::remove_dir_all(&scratch).expect("clean fixture");
}

#[test]
fn elapsed_time_from_publication_selects_each_named_stage_at_its_exact_horizon() {
    let published: wire::Instant = "2026-08-01T06:00:00.000Z"
        .parse()
        .expect("publication instant");
    let config = steer::Config::default();
    for (at, expected) in [
        (
            "2026-08-01T06:00:14.000Z",
            wire::body::SteeringStage::Published,
        ),
        (
            "2026-08-01T06:00:15.000Z",
            wire::body::SteeringStage::Reminded,
        ),
        (
            "2026-08-01T06:00:59.000Z",
            wire::body::SteeringStage::Reminded,
        ),
        (
            "2026-08-01T06:01:00.000Z",
            wire::body::SteeringStage::Escalated,
        ),
        (
            "2026-08-01T06:04:59.000Z",
            wire::body::SteeringStage::Escalated,
        ),
        (
            "2026-08-01T06:05:00.000Z",
            wire::body::SteeringStage::Overdue,
        ),
    ] {
        let at = at.parse().expect("fixture instant");
        assert_eq!(
            due_stage(published, at, &config).expect("configured horizon is representable"),
            expected,
            "stage due at {at}"
        );
    }
}

#[test]
fn repeated_delivery_appends_only_the_missing_ordered_stages() {
    use wire::body::SteeringStage::{Escalated, Overdue, Published, Reminded};

    assert_eq!(missing_stages(Published, Published), []);
    assert_eq!(missing_stages(Published, Reminded), [Reminded]);
    assert_eq!(missing_stages(Published, Escalated), [Reminded, Escalated]);
    assert_eq!(
        missing_stages(Published, Overdue),
        [Reminded, Escalated, Overdue]
    );
    assert_eq!(missing_stages(Reminded, Reminded), []);
    assert_eq!(missing_stages(Reminded, Escalated), [Escalated]);
    assert_eq!(missing_stages(Reminded, Overdue), [Escalated, Overdue]);
    assert_eq!(missing_stages(Escalated, Escalated), []);
    assert_eq!(missing_stages(Escalated, Overdue), [Overdue]);
    assert_eq!(missing_stages(Overdue, Overdue), []);
}
