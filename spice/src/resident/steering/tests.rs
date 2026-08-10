use super::{due_stage, missing_stages};

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
