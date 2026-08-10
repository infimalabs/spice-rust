use super::*;

#[test]
fn transaction_guard_retries_after_both_key_and_directive_command_races() {
    let fixture = Fixture::new("guard-races");
    let bag = bag(&[("polling", &["poll*"], "Wait.")]);
    let record = prose_record(fixture.lane, fixture.occupant, "polling now");
    let attempts = Cell::new(0_u8);

    recognize_with(
        &fixture.bus,
        &fixture.paths,
        fixture.epoch,
        &bag,
        &steer::Config::default(),
        None,
        &record,
        |bus, openings, proposal| {
            let attempt = attempts.get();
            attempts.set(attempt + 1);
            if attempt < 2 {
                let opening = &openings[0];
                let (directive, key) = if attempt == 0 {
                    (uuid(80), opening.key.to_string())
                } else {
                    (opening.directive, "00000000".to_owned())
                };
                append_send(
                    bus,
                    fixture.epoch,
                    fixture.lane,
                    directive,
                    &key,
                    fixture.anchor,
                );
            }
            commit(bus, proposal)
        },
        steering::publish_inbox,
    )
    .expect("guard retries from current authority");

    assert_eq!(attempts.get(), 3, "both collision guards forced a rebuild");
    assert_eq!(fixture.reminders().len(), 1);
    let snapshot = fixture.bus.fact_snapshot().expect("snapshot");
    assert_eq!(
        snapshot
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::SteerSent)
            .count(),
        2
    );
    steer::Ledger::from_snapshot(&snapshot).expect("all three directives replay distinctly");
    fixture.finish();
}

#[test]
fn transaction_guard_rebuilds_when_compaction_moves_after_recognition() {
    let fixture = Fixture::new("epoch-guard");
    let bag = bag(&[("polling", &["poll*"], "Wait.")]);
    let record = prose_record(fixture.lane, fixture.occupant, "polling now");
    let attempts = Cell::new(0_u8);
    let moved = Cell::new(0_u64);

    recognize_with(
        &fixture.bus,
        &fixture.paths,
        fixture.epoch,
        &bag,
        &steer::Config::default(),
        None,
        &record,
        |bus, _, proposal| {
            let attempt = attempts.get();
            attempts.set(attempt + 1);
            if attempt == 0 {
                moved.set(
                    append_compaction(
                        bus,
                        fixture.epoch,
                        fixture.lane,
                        fixture.occupant,
                        fixture.anchor,
                    )
                    .get(),
                );
            }
            commit(bus, proposal)
        },
        steering::publish_inbox,
    )
    .expect("epoch movement rebuilds");

    assert_eq!(attempts.get(), 2, "the stale epoch proposal was refused");
    assert_eq!(fixture.reminders()[0].compaction, moved.get());
    fixture.finish();
}

#[test]
fn transaction_guard_treats_a_concurrent_same_content_winner_as_success() {
    let fixture = Fixture::new("content-guard");
    let bag = bag(&[("polling", &["poll*"], "Wait.")]);
    let record = prose_record(fixture.lane, fixture.occupant, "polling now");
    let attempts = Cell::new(0_u8);

    recognize_with(
        &fixture.bus,
        &fixture.paths,
        fixture.epoch,
        &bag,
        &steer::Config::default(),
        None,
        &record,
        |bus, openings, proposal| {
            attempts.set(attempts.get() + 1);
            if attempts.get() == 1 {
                append_reminder(
                    bus,
                    fixture.epoch,
                    fixture.lane,
                    fixture.occupant,
                    fixture.anchor,
                    &openings[0].candidate,
                );
            }
            commit(bus, proposal)
        },
        steering::publish_inbox,
    )
    .expect("the concurrent winner suppresses retry work");

    assert_eq!(attempts.get(), 1);
    assert_eq!(
        fixture.reminders().len(),
        1,
        "same content and epoch has one durable winner"
    );
    fixture.finish();
}

#[test]
fn concurrent_adjudication_replay_records_one_judgment_and_one_directive() {
    let fixture = Fixture::new("judgment-content-guard");
    let bag = bag(&[("polling", &["poll*"], "Wait.")]);
    let record = prose_record(fixture.lane, fixture.occupant, "polling now");
    let judge_calls = Cell::new(0_u8);
    let commit_attempts = Cell::new(0_u8);
    let mut judge = FunctionAdjudicator(|_: &Candidate| {
        judge_calls.set(judge_calls.get() + 1);
        Ok(judgment(
            true,
            vec![attempt(0, "YES", wire::body::TasteJudgeReply::Yes)],
        ))
    });

    recognize_with(
        &fixture.bus,
        &fixture.paths,
        fixture.epoch,
        &bag,
        &steer::Config::default(),
        Some(&mut judge),
        &record,
        |bus, openings, proposal| {
            commit_attempts.set(commit_attempts.get() + 1);
            if commit_attempts.get() == 1 {
                append_judgment(
                    bus,
                    fixture.epoch,
                    fixture.lane,
                    fixture.occupant,
                    fixture.anchor,
                    &openings[0],
                );
            }
            commit(bus, proposal)
        },
        steering::publish_inbox,
    )
    .expect("the concurrent judgment winner suppresses replay work");

    assert_eq!(judge_calls.get(), 1);
    assert_eq!(commit_attempts.get(), 1);
    assert_eq!(fixture.judgments().len(), 1);
    let snapshot = fixture.bus.fact_snapshot().expect("snapshot");
    assert_eq!(
        steer::Ledger::from_snapshot(&snapshot)
            .expect("winner judgment replays")
            .pending(fixture.lane)
            .len(),
        1
    );
    fixture.finish();
}

#[test]
fn a_commit_failure_delivers_nothing_while_a_rendering_failure_parks_for_repair() {
    // ST-05 + ID-10 + CON-05: taste reminders use the same journal-derived
    // fold progress and restart repair authority as operator directives.
    let fixture = Fixture::new("failure-order");
    let bag = bag(&[("polling", &["poll*"], "Wait.")]);
    let record = prose_record(fixture.lane, fixture.occupant, "polling now");
    let published = Cell::new(false);

    let failed = recognize_with(
        &fixture.bus,
        &fixture.paths,
        fixture.epoch,
        &bag,
        &steer::Config::default(),
        None,
        &record,
        |_, _, _| Err(store_fault("taste::forced-commit", "fixture refusal")),
        |_, _, _, _| {
            published.set(true);
            Ok(())
        },
    );
    assert!(failed.is_err());
    assert!(!published.get(), "delivery ran before durable commit");
    assert!(fixture.reminders().is_empty());

    let blocked_rendering = steer::inbox::path(&fixture.paths.inbox, fixture.lane);
    fs::create_dir_all(&blocked_rendering).expect("fixture blocks inbox rendering");
    super::recognize(
        &fixture.bus,
        &fixture.paths,
        fixture.epoch,
        &bag,
        &steer::Config::default(),
        None,
        &record,
    )
    .expect("rendering failure becomes durable parked progress");
    assert_eq!(fixture.reminders().len(), 1);
    let parked = steer::fold::read(&fixture.paths.data, fixture.lane)
        .expect("parked progress reads")
        .expect("parked progress exists");
    let DeliveryStanding::Parked {
        keys,
        failure,
        repair,
        ..
    } = parked.delivery()
    else {
        panic!("failed reminder rendering did not park: {parked:?}");
    };
    assert_eq!(keys, &[fixture.reminders()[0].key.clone()]);
    assert!(failure.contains("steer::inbox-unwritable"));
    assert_eq!(repair, &format!("spice steer repair {}", fixture.lane));

    let fixture = fixture.restart();
    let residency = current_residency(&fixture.bus, fixture.epoch).expect("restart residency");
    steering::publish_inboxes(&fixture.bus, &fixture.paths, residency.body())
        .expect("startup replay retains a durable parked answer");
    let replayed = steer::fold::read(&fixture.paths.data, fixture.lane)
        .expect("replayed progress reads")
        .expect("replayed progress exists");
    assert!(matches!(
        replayed.delivery(),
        DeliveryStanding::Parked { .. }
    ));

    fs::remove_dir_all(&blocked_rendering).expect("fixture unblocks inbox rendering");
    steering::publish_inbox(
        &fixture.bus,
        &fixture.paths,
        current_residency(&fixture.bus, fixture.epoch)
            .expect("residency")
            .body(),
        fixture.lane,
    )
    .expect("repair republishes current rendering");
    let repaired = steer::fold::read(&fixture.paths.data, fixture.lane)
        .expect("repaired progress reads")
        .expect("repaired progress exists");
    assert!(matches!(
        repaired.delivery(),
        DeliveryStanding::Published { .. }
    ));
    let rendered = fs::read_to_string(steer::inbox::path(&fixture.paths.inbox, fixture.lane))
        .expect("repaired inbox exists");
    assert!(rendered.contains(&fixture.reminders()[0].key));
    assert_eq!(rendered.matches(&fixture.reminders()[0].key).count(), 1);
    fixture.recognize(&bag, &record);
    assert_eq!(
        fixture.reminders().len(),
        1,
        "replaying after a failed rendering does not duplicate authority"
    );
    fixture.finish();
}

#[test]
fn an_older_repair_replays_when_new_authority_lands_during_publication() {
    // CON-05 + ID-10: an older attempt must not publish stale progress across
    // a concurrent journal advance; it retries from the new authority horizon.
    let fixture = Fixture::new("fold-movement");
    let bag = bag(&[("polling", &["poll*"], "Wait.")]);
    let record = prose_record(fixture.lane, fixture.occupant, "polling now");
    fixture.recognize(&bag, &record);
    let residency = current_residency(&fixture.bus, fixture.epoch).expect("residency");
    let lane = residency.body().lanes[0].clone();
    let injected = Cell::new(false);

    let delivery = steering::fold_lane_inbox_with(&fixture.bus, &fixture.paths, &lane, |attempt| {
        if attempt == 0 && !injected.replace(true) {
            append_send(
                &fixture.bus,
                fixture.epoch,
                fixture.lane,
                uuid(80),
                "00000000",
                fixture.anchor,
            );
        }
    })
    .expect("moved authority is replayed before progress commits");

    let DeliveryStanding::Published { source } = delivery else {
        panic!("moving repair did not settle published");
    };
    assert_eq!(
        source,
        fixture
            .bus
            .fact_snapshot()
            .expect("current snapshot")
            .horizon()
            .expect("current horizon")
    );
    let rendered = fs::read_to_string(steer::inbox::path(&fixture.paths.inbox, fixture.lane))
        .expect("current rendering exists");
    assert!(rendered.contains("[TASTE] polling: Wait."));
    assert!(rendered.contains("concurrent command"));
    fixture.finish();
}

#[test]
fn unrelated_journal_movement_does_not_starve_a_lane_fold() {
    let fixture = Fixture::new("fold-unrelated-movement");
    let bag = bag(&[("polling", &["poll*"], "Wait.")]);
    let record = prose_record(fixture.lane, fixture.occupant, "polling now");
    fixture.recognize(&bag, &record);
    let residency = current_residency(&fixture.bus, fixture.epoch).expect("residency");
    let lane = residency.body().lanes[0].clone();
    let attempts = Cell::new(0);

    let delivery = steering::fold_lane_inbox_with(&fixture.bus, &fixture.paths, &lane, |attempt| {
        attempts.set(attempts.get() + 1);
        if attempt == 0 {
            append_send(
                &fixture.bus,
                fixture.epoch,
                uuid(81),
                uuid(82),
                "11111111",
                fixture.anchor,
            );
        }
    })
    .expect("another lane does not move this lane's fold authority");

    let DeliveryStanding::Published { source } = delivery else {
        panic!("unrelated movement did not settle published");
    };
    assert_eq!(attempts.get(), 1, "unrelated authority caused no retry");
    assert!(
        source
            < fixture
                .bus
                .fact_snapshot()
                .expect("current snapshot")
                .horizon()
                .expect("current horizon"),
        "the fold cursor truthfully names the older equivalent lane snapshot"
    );
    let rendered = fs::read_to_string(steer::inbox::path(&fixture.paths.inbox, fixture.lane))
        .expect("selected lane rendering exists");
    assert!(rendered.contains("[TASTE] polling: Wait."));
    assert!(!rendered.contains("concurrent command"));
    fixture.finish();
}
