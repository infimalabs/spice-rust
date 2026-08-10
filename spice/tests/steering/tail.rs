#[test]
fn resend_before_the_first_horizon_redelivers_one_unchanged_stable_directive() {
    let (fixture, resident, lanes) = setup();
    let lane = lanes.target.to_string();
    let sent = text_directive(&invoke(
        &fixture,
        &["steer", &lane, "deliver this stable direction again"],
        false,
    ));
    let resent = text_directive(&invoke(
        &fixture,
        &["steer", "resend", &sent.directive.to_string()],
        false,
    ));
    assert_eq!(resent, sent);

    let inbox = text_inbox(&invoke(&fixture, &["inbox", &lane], false));
    let pending = inbox.lanes[0]
        .pending
        .iter()
        .find(|pending| pending.directive == sent.directive)
        .expect("redelivered directive remains pending");
    assert_eq!(pending.key, sent.key);
    assert_eq!(pending.stage, wire::body::SteeringStage::Published);
    assert!(pending.escalations.is_empty());

    let journal = journal::Journal::open(fixture.journal()).expect("steering journal opens");
    let snapshot = journal.fact_snapshot().expect("steering facts read");
    assert!(snapshot.facts().iter().all(|envelope| {
        envelope
            .fact
            .read::<wire::body::SteerEscalated>()
            .is_none_or(|body| body.directive != sent.directive)
    }));

    let canceled = text_directive(&invoke(
        &fixture,
        &["steer", "cancel", &sent.directive.to_string()],
        false,
    ));
    assert_eq!(canceled.directive, sent.directive);
    assert_eq!(canceled.key, sent.key);
    assert_eq!(canceled.disposition, wire::Disposition::Canceled);
    stop(&fixture, resident);
}

#[test]
fn steering_commands_preserve_principals_races_and_restart_state() {
    let (fixture, mut resident, lanes) = setup();
    let (acknowledged, historical_key) = acknowledgment_paths(&fixture, &mut resident, lanes);
    let declined = decline_path(&fixture, lanes.target);
    let (race, raced) = cancellation_race(&fixture, lanes.target);
    assert!(matches!(
        race,
        wire::Disposition::Canceled | wire::Disposition::Acked
    ));
    cancellation_path(&fixture, lanes.target);
    direct_input_cannot_ack(&fixture, lanes.target);
    rendering_failure_path(&fixture, resident, lanes.target, &historical_key);
    let mut expected = vec![acknowledged, declined];
    expected.extend(raced);
    verify_authority(&fixture, lanes.target, race, &expected);
}
